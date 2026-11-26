//! OpenCode v2 background state and bounded, session-scoped output reads.
use std::{collections::HashMap, path::Path};

use chrono::{DateTime, SecondsFormat};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};

use super::{Bridge, encoded, string};

const OUTPUT_BYTES: usize = 64 * 1024;
const CLIPPED_NOTE: &str = "仅显示最近 64 KiB，较早的输出无法在此补读。";

impl Bridge {
    async fn background_jobs(&self, sid: &str, cwd: &Path) -> Result<Vec<Value>, String> {
        let response = self
            .transport
            .api(
                cwd,
                "GET",
                &format!(
                    "/api/experimental/job?sessionID={}&kind=shell",
                    encoded(sid)
                ),
                None,
            )
            .await?;
        Ok(response["data"]
            .as_array()
            .ok_or("OpenCode job.list 返回格式未知")?
            .iter()
            .filter(|job| job["sessionID"] == sid && job["kind"] == "shell")
            .cloned()
            .collect())
    }

    pub(super) async fn background_status(
        &self,
        sid: &str,
        cwd: &Path,
    ) -> (Option<Vec<Value>>, Option<Vec<Value>>) {
        let automation = format!("/api/experimental/automation?sessionID={}", encoded(sid));
        let (jobs, timers) = tokio::join!(
            self.background_jobs(sid, cwd),
            self.transport.api(cwd, "GET", &automation, None)
        );
        let jobs = jobs
            .ok()
            .map(|jobs| jobs.iter().filter_map(job_command).collect());
        let timers = timers.ok().and_then(|response| {
            Some(
                response["data"]
                    .as_array()?
                    .iter()
                    .filter(|timer| timer["sessionID"] == sid)
                    .filter_map(timer_row)
                    .collect(),
            )
        });
        (jobs, timers)
    }

    pub(super) async fn background_output(
        &self,
        target: &Value,
        input: &Value,
    ) -> Result<Value, String> {
        let sid = string(target, "session_id")?;
        let cwd = Path::new(string(target, "cwd")?);
        let task = string(input, "task")?;
        let uid = native_uid(target)?;

        // The optional SQLite projection supports cold ordinary views. Validate
        // its native session and project before using any matching task.
        let stored = if let Some(path) = &self.database {
            let (path, sid, cwd, uid, root, task) = (
                path.clone(),
                sid.to_owned(),
                cwd.to_owned(),
                uid.to_owned(),
                self.root.clone(),
                task.to_owned(),
            );
            tokio::task::spawn_blocking(move || stored_task(&path, &root, &sid, &cwd, &uid, &task))
                .await
                .map_err(|error| error.to_string())?
                .unwrap_or_default()
        } else {
            StoredTask::default()
        };
        if let Some(output) = stored.output {
            return Ok(json!({"raw": output}));
        }
        if let Some(shell) = stored.shell {
            // A native session message or retained Job row authorized this id.
            return self.shell_output(cwd, &shell).await;
        }

        let info = self
            .transport
            .api(cwd, "GET", &format!("/api/session/{}", encoded(sid)), None)
            .await?;
        let identity = self.identity(&info["data"])?;
        if identity["uid"] != uid
            || identity["session_id"] != target["session_id"]
            || identity["cwd"] != target["cwd"]
        {
            return Err("OpenCode SDK 原生 session/cwd/uid 与 inventory 不一致".into());
        }
        if let Some(job) = self
            .background_jobs(sid, cwd)
            .await?
            .into_iter()
            .find(|job| job["id"] == task)
            && let Some(shell) = job["shellID"].as_str()
        {
            return self.shell_output(cwd, shell).await;
        }
        Ok(json!({"raw": null}))
    }

    async fn shell_output(&self, cwd: &Path, shell: &str) -> Result<Value, String> {
        let directory = cwd.to_str().ok_or("OpenCode 原生目录无法编码")?;
        let path = format!(
            "/api/shell/{}/output?location%5Bdirectory%5D={}",
            encoded(shell),
            encoded(directory)
        );
        let Some(info) = self
            .transport
            .shell_output(cwd, &format!("{path}&limit=0"))
            .await?
        else {
            return Ok(json!({"raw": null}));
        };
        let start = |response: &Value| {
            response["data"]["size"]
                .as_u64()
                .map(|size| size.saturating_sub(OUTPUT_BYTES as u64))
                .ok_or("OpenCode shell.output 返回格式未知")
        };
        let read = |cursor| {
            let page = format!("{path}&cursor={cursor}&limit={OUTPUT_BYTES}");
            async move { self.transport.shell_output(cwd, &page).await }
        };
        let mut cursor = start(&info)?;
        let Some(mut page) = read(cursor).await? else {
            return Ok(json!({"raw": null}));
        };
        // If output grew since the size probe, move the window once to the
        // newly observed tail. A busy command can keep appending as we read.
        let newer_cursor = start(&page)?;
        if newer_cursor > cursor {
            cursor = newer_cursor;
            let Some(newer_page) = read(cursor).await? else {
                return Ok(json!({"raw": null}));
            };
            page = newer_page;
        }
        let text = page["data"]["output"]
            .as_str()
            .ok_or("OpenCode shell.output 返回格式未知")?;
        Ok(json!({"raw": output_window(text, cursor > 0 || page["data"]["truncated"] == true)}))
    }
}

pub(super) fn native_uid(target: &Value) -> Result<&str, String> {
    nonempty(&target["native_uid"]).map_or_else(|| string(target, "uid"), Ok)
}

fn job_command(job: &Value) -> Option<Value> {
    if job["status"] != "running" {
        return None;
    }
    nonempty(&job["shellID"])?;
    Some(json!({"task":nonempty(&job["id"])?,"command":job["command"],"output_available":true}))
}

pub(crate) fn message_commands<'a>(
    messages: impl Iterator<Item = (Option<&'a str>, &'a Value)>,
) -> Vec<Value> {
    messages
        .filter(|(kind, native)| {
            *kind == Some("shell")
                && native["metadata"]["background"] == true
                && native["status"] == "running"
        })
        .filter_map(|(_, native)| {
            let task = nonempty(&native["shellID"])?;
            Some(json!({"task":task,"command":native["command"],"output_available":true}))
        })
        .collect()
}

pub(crate) fn merge_commands(jobs: &[Value], messages: Vec<Value>) -> Vec<Value> {
    let mut rows = std::collections::BTreeMap::new();
    for row in jobs.iter().cloned().chain(messages) {
        if let Some(task) = row["task"].as_str() {
            rows.entry(task.to_owned()).or_insert(row);
        }
    }
    rows.into_values().collect()
}

fn timer_row(timer: &Value) -> Option<Value> {
    if timer["status"] != "active" || timer["action"]["type"] != "prompt" {
        return None;
    }
    let mut row =
        json!({"task":nonempty(&timer["id"])?,"prompt":nonempty(&timer["action"]["prompt"])?});
    let trigger = &timer["trigger"];
    match trigger["type"].as_str()? {
        "at" => {}
        "every" => {
            row["schedule"] = json!(format!("每 {}", nonempty(&trigger["every"])?));
        }
        "cron" => {
            let cron = nonempty(&trigger["cron"])?;
            row["schedule"] = match nonempty(&timer["tz"]) {
                Some(tz) => json!(format!("{cron} ({tz})")),
                None => json!(cron),
            };
        }
        _ => return None,
    }
    if let Some(created) = iso(&timer["time"]["created"]) {
        row["created_at"] = json!(created);
    }
    if let Some(due) = iso(&timer["nextDueAt"]) {
        row["due_at"] = json!(due);
    }
    Some(row)
}

fn iso(value: &Value) -> Option<String> {
    Some(
        DateTime::from_timestamp_millis(value.as_i64()?)?
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}

fn nonempty(value: &Value) -> Option<&str> {
    value.as_str().filter(|value| !value.trim().is_empty())
}

fn output_window(text: &str, native_clipped: bool) -> Value {
    let mut start = text.len().saturating_sub(OUTPUT_BYTES);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let mut result = json!({"output": &text[start..]});
    if native_clipped || start > 0 {
        result["clipped"] = json!(true);
        result["note"] = json!(CLIPPED_NOTE);
    }
    result
}

#[derive(Default)]
pub(crate) struct MirrorStatus {
    pub commands: Vec<Value>,
    pub timers: Vec<Value>,
}

pub(crate) fn mirror_status(db: &Connection) -> rusqlite::Result<HashMap<String, MirrorStatus>> {
    let mut status: HashMap<String, MirrorStatus> = HashMap::new();
    if table(db, "job")? {
        let mut query = db.prepare(
            "SELECT session_id,id,recovery FROM job
             WHERE kind='shell' AND status='running' ORDER BY time_created DESC",
        )?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (sid, id, recovery) = row?;
            let Ok(recovery) = serde_json::from_str::<Value>(&recovery) else {
                continue;
            };
            if recovery["kind"] == "shell"
                && recovery["sessionID"] == sid
                && let Some(row) = job_command(&json!({"id":id,"status":"running",
                    "shellID":recovery["shellID"],"command":recovery["command"]}))
            {
                status.entry(sid).or_default().commands.push(row);
            }
        }
    }
    if table(db, "automation")? {
        let mut query = db.prepare(
            "SELECT session_id,id,trigger,action,tz,next_due_at,time_created
             FROM automation WHERE session_id IS NOT NULL AND status='active' ORDER BY next_due_at,id",
        )?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        for row in rows {
            let (sid, id, trigger, action, tz, due, created) = row?;
            let (Ok(trigger), Ok(action)) = (
                serde_json::from_str::<Value>(&trigger),
                serde_json::from_str::<Value>(&action),
            ) else {
                continue;
            };
            if let Some(timer) = timer_row(&json!({"id":id,"trigger":trigger,"action":action,
                "tz":tz,"nextDueAt":due,"time":{"created":created},"status":"active"}))
            {
                status.entry(sid).or_default().timers.push(timer);
            }
        }
    }
    Ok(status)
}

fn table(db: &Connection, name: &str) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |row| row.get(0),
    )
}

#[derive(Default)]
struct StoredTask {
    shell: Option<String>,
    output: Option<Value>,
}

fn stored_task(
    path: &Path,
    root: &Path,
    sid: &str,
    cwd: &Path,
    uid: &str,
    task: &str,
) -> rusqlite::Result<StoredTask> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let db = db.unchecked_transaction()?;
    let Some((project, directory)) = db
        .query_row(
            "SELECT project_id,directory FROM session_v2 WHERE id=?1",
            [sid],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
    else {
        return Ok(StoredTask::default());
    };
    if !crate::sessions::opencode::safe_id(&project)
        || !crate::sessions::opencode::safe_id(sid)
        || Path::new(&directory) != cwd
        || crate::sessions::uid_for("opencode", &root.join(project).join(sid)) != uid
    {
        return Ok(StoredTask::default());
    }
    let shell = db
        .query_row(
            "SELECT data FROM session_message WHERE session_id=?1 AND type='shell'
          AND json_extract(data,'$.shellID')=?2 AND json_type(data,'$.metadata.background')='true'
          ORDER BY seq DESC LIMIT 1",
            params![sid, task],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(shell) = shell {
        let Ok(shell) = serde_json::from_str::<Value>(&shell) else {
            return Ok(StoredTask::default());
        };
        let page = &shell["output"];
        let output = page["cursor"]
            .as_u64()
            .zip(page["size"].as_u64())
            .filter(|(cursor, size)| cursor == size)
            .and_then(|_| page["output"].as_str())
            .map(|text| output_window(text, page["truncated"] == true));
        return Ok(StoredTask {
            shell: Some(task.to_owned()),
            output,
        });
    }
    let shell = if table(&db, "job")? {
        db.query_row(
            "SELECT json_extract(recovery,'$.shellID') FROM job
             WHERE session_id=?1 AND id=?2 AND kind='shell'
             AND json_extract(recovery,'$.kind')='shell'
             AND json_extract(recovery,'$.sessionID')=session_id
             ORDER BY time_created DESC LIMIT 1",
            params![sid, task],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .filter(|shell| !shell.is_empty())
    } else {
        None
    };
    Ok(StoredTask {
        shell,
        output: None,
    })
}
