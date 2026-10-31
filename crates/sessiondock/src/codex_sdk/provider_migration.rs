//! Explicitly confirmed single-thread metadata migration. Never used by reads.
use super::RpcError;
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

fn failure(error: impl std::fmt::Display) -> RpcError {
    RpcError::Storage(format!("provider 迁移失败：{error}"))
}

#[cfg(unix)]
fn lock(path: &Path, create: bool) -> Result<fs::File, RpcError> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .mode(0o600)
        .open(path)
        .map_err(failure)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(RpcError::Storage(
            "该会话仍被原生 Codex 占用，请先停止会话并等待线程卸载后重试；尚未修改 provider".into(),
        ));
    }
    Ok(file)
}

fn replace(line: &[u8], field: &str, source: &str) -> Result<Vec<u8>, RpcError> {
    let pattern = format!(r#""{}"\s*:\s*("(?:[^"\\]|\\.)*")"#, field);
    let re = regex::bytes::Regex::new(&pattern).map_err(failure)?;
    let captures: Vec<_> = re.captures_iter(line).collect();
    if captures.len() != 1
        || serde_json::from_slice::<String>(&captures[0][1]).map_err(failure)? != source
    {
        return Err(failure(format!(
            "{field} 与确认的 provider 不一致，请刷新后重试"
        )));
    }
    let value = captures[0].get(1).expect("provider value");
    let mut result = line[..value.start()].to_vec();
    result.extend_from_slice(b"\"openai\"");
    result.extend_from_slice(&line[value.end()..]);
    Ok(result)
}

#[cfg(unix)]
pub(super) fn migrate(
    home: &Path,
    backup_root: &Path,
    id: &str,
    source: &str,
) -> Result<Value, RpcError> {
    use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let _coordination = lock(&home.join("thread-writer-locks/.coordination.lock"), true)?;
    let writer_path = home.join("thread-writer-locks").join(format!("{id}.lock"));
    let _writer = if writer_path.exists() {
        Some(lock(&writer_path, false)?)
    } else {
        None
    };
    let mut db = Connection::open_with_flags(
        home.join("state_5.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .map_err(failure)?;
    db.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(failure)?;
    let history = home.join("thread_history_1.sqlite");
    let has_history = history.exists();
    if has_history {
        db.execute(
            "ATTACH DATABASE ? AS history",
            [history.to_string_lossy().as_ref()],
        )
        .map_err(failure)?;
    }
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(failure)?;
    let (path, provider): (String, String) = tx
        .query_row(
            "SELECT rollout_path,model_provider FROM threads WHERE id=?",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(failure)?;
    if provider == "openai" {
        return Ok(json!({"provider":"openai","already_current":true}));
    }
    if provider != source {
        return Err(failure("会话 provider 已改变，请刷新后重新确认"));
    }
    let path = PathBuf::from(path);
    let original = fs::read(&path).map_err(failure)?;
    let mut lines: Vec<Vec<u8>> = original
        .split_inclusive(|byte| *byte == b'\n')
        .map(Vec::from)
        .collect();
    let meta: Value = serde_json::from_slice(lines.first().ok_or_else(|| failure("历史为空"))?)
        .map_err(failure)?;
    if meta["type"] != "session_meta" || meta["payload"]["id"] != id {
        return Err(failure("原生历史身份不一致"));
    }
    let mut latest = None;
    for (n, line) in lines.iter().enumerate() {
        let value: Value = serde_json::from_slice(line).map_err(failure)?;
        if value["type"] == "event_msg"
            && value["payload"]["type"] == "thread_settings_applied"
            && value["payload"]["thread_id"] == id
        {
            latest = Some(n);
        }
    }
    let mut offset = 0i64;
    let mut delta = 0i64;
    let mut boundaries = Vec::new();
    for (n, line) in lines.iter_mut().enumerate() {
        offset += line.len() as i64;
        let field = if n == 0 {
            Some("model_provider")
        } else if Some(n) == latest {
            Some("model_provider_id")
        } else {
            None
        };
        if let Some(field) = field {
            let changed = replace(line, field, source)?;
            delta += changed.len() as i64 - line.len() as i64;
            *line = changed;
            boundaries.push((offset, delta));
        }
    }
    let shifted = |value: Option<i64>| {
        value.map(|value| {
            value
                + boundaries
                    .iter()
                    .rev()
                    .find(|(at, _)| *at <= value)
                    .map_or(0, |(_, delta)| *delta)
        })
    };
    let mut turns: Vec<(String, Option<i64>, Option<i64>)> = Vec::new();
    let mut projection: Option<i64> = None;
    if has_history {
        let mut statement = tx.prepare("SELECT turn_id,rollout_byte_offset,rollout_end_byte_offset FROM history.thread_turns WHERE thread_id=?").map_err(failure)?;
        turns = statement
            .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(failure)?
            .collect::<Result<_, _>>()
            .map_err(failure)?;
        projection = tx.query_row("SELECT next_rollout_byte_offset FROM history.thread_history_projection_state WHERE thread_id=?", [id], |row| row.get(0)).optional().map_err(failure)?;
    }
    let stamp = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(failure)?
            .as_nanos()
    );
    let backup = backup_root.join(format!("codex-provider-{stamp}"));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&backup)
        .map_err(failure)?;
    fs::write(backup.join("rollout.jsonl"), &original).map_err(failure)?;
    fs::write(backup.join("metadata.json"), serde_json::to_vec_pretty(&json!({"thread_id":id,"provider":source,"rollout_path":path,"turn_offsets":turns,"projection":projection})).map_err(failure)?).map_err(failure)?;
    let temporary = path.with_extension(format!("provider-{stamp}.tmp"));
    let result = (|| -> Result<Value, RpcError> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(failure)?;
        file.set_permissions(fs::metadata(&path).map_err(failure)?.permissions())
            .map_err(failure)?;
        for line in &lines {
            file.write_all(line).map_err(failure)?;
        }
        file.sync_all().map_err(failure)?;
        tx.execute(
            "UPDATE threads SET model_provider='openai' WHERE id=?",
            [id],
        )
        .map_err(failure)?;
        for (turn, start, end) in &turns {
            tx.execute("UPDATE history.thread_turns SET rollout_byte_offset=?,rollout_end_byte_offset=? WHERE thread_id=? AND turn_id=?", params![shifted(*start),shifted(*end),id,turn]).map_err(failure)?;
        }
        if let Some(projection) = projection {
            tx.execute("UPDATE history.thread_history_projection_state SET next_rollout_byte_offset=? WHERE thread_id=?", params![shifted(Some(projection)),id]).map_err(failure)?;
        }
        fs::rename(&temporary, &path).map_err(failure)?;
        if let Err(error) = tx.commit() {
            fs::copy(backup.join("rollout.jsonl"), &path).map_err(failure)?;
            return Err(failure(error));
        }
        Ok(json!({"provider":"openai","migrated":true}))
    })();
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(failure(e)),
    }
    result
}

#[cfg(not(unix))]
pub(super) fn migrate(_: &Path, _: &Path, _: &str, _: &str) -> Result<Value, RpcError> {
    Err(failure("此平台不支持 Codex SDK"))
}
