//! Native deletion through Agy's resume picker, or explicit empty-session cleanup.
//! Native SQLite contents are never rewritten.
use super::launcher::Launcher;
use ptyhost_client::{CaptureKind, ControlOp, ControlReply, HostClient, Limits};
use rusqlite::{Connection, OpenFlags};
use std::{path::Path, time::Duration};

pub(crate) static DELETE: std::sync::LazyLock<std::sync::Arc<tokio::sync::Mutex<()>>> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Mutex::new(())));
const NAME: &str = "agy-delete";
const FILTER_ECHO_WAIT: Duration = Duration::from_secs(1);

fn present(home: &Path, sid: &str) -> Result<bool, String> {
    let db = Connection::open_with_flags(
        home.join("conversation_summaries.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "无法读取 Agy 原生会话索引".to_owned())?;
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation_summaries WHERE conversation_id=?1)",
        [sid],
        |r| r.get(0),
    )
    .map_err(|_| "无法核对 Agy 原生会话索引".to_owned())
}

fn deleted(home: &Path, sid: &str) -> Result<bool, String> {
    Ok(!present(home, sid)? && payload_missing(home, sid)?)
}

fn payload_missing(home: &Path, sid: &str) -> Result<bool, String> {
    for path in [
        home.join("conversations").join(format!("{sid}.db")),
        home.join("brain").join(sid),
    ] {
        if path
            .try_exists()
            .map_err(|_| "无法核对 Agy 原生文件删除结果")?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn remove_empty_payload(home: &Path, sid: &str) -> Result<bool, String> {
    // Agy's picker omits zero-step sessions, even when --conversation opens
    // their database successfully. Resuming them cannot reach a delete action
    // and unnecessarily advertises the cleanup worker as a live session.
    // Missing transcripts alone are NOT evidence of an empty native session.
    let empty = || -> rusqlite::Result<bool> {
        let catalog = Connection::open_with_flags(
            home.join("conversation_summaries.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let empty_summary: bool = catalog.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversation_summaries \
             WHERE conversation_id=?1 AND step_count=0 AND parent_conversation_id='')",
            [sid],
            |row| row.get(0),
        )?;
        if !empty_summary {
            return Ok(false);
        }
        let db = Connection::open_with_flags(
            home.join("conversations").join(format!("{sid}.db")),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        db.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM steps) \
             AND NOT EXISTS(SELECT 1 FROM parent_references)",
            [],
            |row| row.get(0),
        )
    };
    if !empty().unwrap_or(false) {
        // Unknown schemas and unreadable databases keep the native CLI path.
        return Ok(false);
    }
    let brain = home.join("brain").join(sid);
    for name in ["transcript_full.jsonl", "transcript.jsonl"] {
        if brain
            .join(".system_generated/logs")
            .join(name)
            .try_exists()
            .map_err(|_| "无法核对 Agy 原生正文")?
        {
            return Ok(false);
        }
    }
    // This is reached only by an explicitly confirmed delete, after the fresh
    // running-state check and serialized deletion slot. Remove only this SID's
    // native carriers; leave the shared catalog untouched. The caller records
    // the existing durable deletion receipt before publishing success.
    match std::fs::remove_dir_all(&brain) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err("无法删除 Agy 空会话目录".into()),
    }
    // Keep the database until last so a partial cleanup remains retryable.
    for suffix in [".db-wal", ".db-shm", ".db"] {
        match std::fs::remove_file(home.join("conversations").join(format!("{sid}{suffix}"))) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err("无法删除 Agy 空会话数据库".into()),
        }
    }
    Ok(true)
}

async fn send(client: &HostClient, text: &str) -> Result<(), String> {
    client
        .request(NAME, ControlOp::Send { text: text.into() })
        .await
        .map(|_| ())
        .map_err(|_| "Agy 删除输入未确认，请刷新列表核对".into())
}

async fn screen(client: &HostClient) -> Option<crate::delivery::driver::ScreenCapture> {
    let ControlReply::Capture(c) = client
        .request(
            NAME,
            ControlOp::Capture {
                kind: CaptureKind::Screen,
                styled: false,
                join: false,
                lines: 0,
            },
        )
        .await
        .ok()?
    else {
        return None;
    };
    if c.lag != Some(0) || c.dropped != Some(0) || c.resets != Some(0) {
        return None;
    }
    Some(crate::delivery::driver::ScreenCapture {
        text: c.text,
        cursor: (c.cursor[0], c.cursor[1]),
        lag: c.lag,
        dropped: c.dropped,
        resets: c.resets,
        alt: c.alt,
    })
}

async fn wait_screen<T>(
    client: &HostClient,
    accept: impl Fn(&crate::delivery::driver::ScreenCapture) -> Option<T>,
) -> Result<T, String> {
    loop {
        if let Some(frame) = screen(client).await {
            if crate::bridge::menus::agy::screen_prompt(&frame.text)
                .is_some_and(|prompt| prompt["menu_type"] == "folder_trust")
            {
                return Err("Agy 正在等待工作目录信任确认。请先启动此会话，自行选择是否信任目录；确认后停止会话，再重试删除。本次未提交删除确认。".into());
            }
            if let Some(result) = accept(&frame) {
                return Ok(result);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn picker_rows(text: &str) -> Option<impl Iterator<Item = &str>> {
    let (_, menu) = text.rsplit_once("(tab to cycle)")?;
    let mut rows = menu.lines().map(str::trim).filter(|row| !row.is_empty());
    (rows.next() == Some("Conversations")).then_some(rows)
}

fn delete_shortcut(row: &str) -> bool {
    row.starts_with("Keyboard:") && row.contains("f4 Delete")
}

fn unfiltered_picker(text: &str) -> bool {
    picker_rows(text).is_some_and(|mut rows| {
        rows.next()
            .is_some_and(|row| row.starts_with("Type to search conversations..."))
            && rows.any(delete_shortcut)
    })
}

fn picker_row<'a>(text: &'a str, sid: &str) -> Option<&'a str> {
    // Anchor in the picker below the last editor boundary, never in history.
    let mut rows = picker_rows(text)?;
    if rows.next()?.strip_prefix("Search: ") != Some(sid) {
        return None;
    }
    let row = rows.next()?;
    (row.starts_with("> [CURRENT] ") && rows.next().is_some_and(delete_shortcut)).then_some(row)
}

fn empty_editor(frame: &crate::delivery::driver::ScreenCapture) -> bool {
    crate::conversation::agy_editor(frame).as_deref() == Some("")
}

async fn open_picker(client: &HostClient) -> Result<(), String> {
    send(client, "/resume\r").await?;
    wait_screen(client, |frame| unfiltered_picker(&frame.text).then_some(())).await
}

async fn filter_picker(client: &HostClient, sid: &str) -> Result<(), String> {
    // The SID comes from native inventory and is sent as text.
    client
        .request(
            NAME,
            ControlOp::Paste {
                text: sid.into(),
                bracketed: true,
            },
        )
        .await
        .map(|_| ())
        .map_err(|_| "Agy 会话选择未确认".into())
}

enum Selection {
    Current,
    ReturnedToEditor,
    FilterCleared,
}

async fn select_current(client: &HostClient, sid: &str) -> Result<(), String> {
    filter_picker(client, sid).await?;
    let echo_deadline = tokio::time::Instant::now() + FILTER_ECHO_WAIT;
    let selection = wait_screen(client, |frame| {
        if picker_row(&frame.text, sid).is_some() {
            Some(Selection::Current)
        } else if picker_rows(&frame.text).is_none() && empty_editor(frame) {
            Some(Selection::ReturnedToEditor)
        } else if tokio::time::Instant::now() >= echo_deadline && unfiltered_picker(&frame.text) {
            Some(Selection::FilterCleared)
        } else {
            None
        }
    })
    .await?;
    match selection {
        Selection::Current => return Ok(()),
        Selection::ReturnedToEditor => open_picker(client).await?,
        Selection::FilterCleared => (),
    }
    // A redraw can drop the picker or its search. Retry only the search,
    // before any destructive input, and prove the full SID again.
    filter_picker(client, sid).await?;
    wait_screen(client, |frame| picker_row(&frame.text, sid).map(|_| ())).await
}

pub async fn remove(launcher: &Launcher, sid: &str, cwd: &Path, home: &Path) -> Result<(), String> {
    // An explicitly deleted index-only remnant has no native payload to remove.
    // Do not resume it: doing so may create a new empty conversation database.
    if payload_missing(home, sid)? {
        return Ok(());
    }
    if remove_empty_payload(home, sid)? {
        return Ok(());
    }
    let id = super::store::fresh_request_id().map_err(|_| "无法创建 Agy 删除任务")?;
    let directory = launcher.host_dir().join(format!(".agy-delete-{id}"));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&directory)
        .map_err(|_| "无法创建 Agy 删除工作目录")?;
    let result = async {
        let client = HostClient::new(
            &directory,
            Limits {
                operation_timeout: Duration::from_secs(2),
                ..Limits::default()
            },
        )
        .map_err(|_| "无法连接 Agy 删除终端")?;
        let mut command = launcher
            .agy_delete_command(&directory, sid, cwd)
            .map_err(|_| "Agy 启动项不可用")?;
        let mut child = command.spawn().map_err(|_| "Agy 删除终端启动失败")?;
        let mut stage = "等待原生编辑区";
        let operation = async {
            wait_screen(&client, |frame| empty_editor(frame).then_some(())).await?;
            stage = "打开原生会话选择器";
            open_picker(&client).await?;
            stage = "按完整会话 ID 核对选中行";
            select_current(&client, sid).await?;
            let before = screen(&client).await.ok_or("Agy 会话选择画面丢失")?;
            let selected = picker_row(&before.text, sid).ok_or("Agy 会话选择已变化")?;
            let markers = selected.matches("[Delete? (y/n)]").count();
            stage = "等待原生删除确认";
            send(&client, "\x1bOS").await?; // F4, advertised by the verified picker.
            wait_screen(&client, |c| {
                picker_row(&c.text, sid)
                    .filter(|s| s.matches("[Delete? (y/n)]").count() == markers + 1)
                    .map(|_| ())
            })
            .await?;
            stage = "核对原生删除结果";
            send(&client, "y").await?;
            while !deleted(home, sid)? {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Ok::<_, String>(())
        };
        let outcome = tokio::time::timeout(Duration::from_secs(25), operation)
            .await
            .unwrap_or_else(|_| Err(format!("Agy 原生删除超时（{stage}），请刷新列表核对后重试")));
        // Always stop the private worker, including partial/failed menu operations.
        let _ = client.request(NAME, ControlOp::Kill { force: true }).await;
        let _ = tokio::task::spawn_blocking(move || {
            let _ = child.kill();
            let _ = child.wait();
        })
        .await;
        // A lost terminal acknowledgement after confirmation is resolved by native state.
        if deleted(home, sid)? {
            Ok(())
        } else {
            outcome.and(Err("Agy 原生会话仍存在，删除未完成".into()))
        }
    }
    .await;
    let _ = std::fs::remove_dir_all(directory);
    result
}
