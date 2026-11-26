//! Agy can persist input before emitting its headless user_input notification.
use super::{Live, Turn, is_control_input, project, transport::ProcessConfig};
use crate::sessions::agy::steps::bytes_field;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

fn unbound(item: &Value) -> bool {
    item["role"] == "user" && item["native_id"].is_null() && item["status"] != "unknown"
}

pub(super) fn pending(live: &Live, turn: Option<&Turn>) -> bool {
    live.data.lock().unwrap().items.iter().any(|item| {
        unbound(item)
            && (is_control_input(item)
                || turn.is_some_and(|turn| {
                    turn.first_native_step.is_some() && item["request_id"] == turn.id
                }))
    })
}

fn read_steps(path: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(Duration::ZERO)?;
    Ok(db)
}

pub(super) async fn first_native_step(process: &ProcessConfig, live: &Live) -> Option<u64> {
    let path = process.native_path(&live.sid);
    tokio::task::spawn_blocking(move || {
        read_steps(&path)?.query_row("SELECT COALESCE(MAX(idx) + 1, 0) FROM steps", [], |row| {
            row.get(0)
        })
    })
    .await
    .ok()?
    .ok()
}

pub(super) async fn compacted_since(process: &ProcessConfig, live: &Live, first: u64) -> bool {
    let path = process.native_path(&live.sid);
    // Agy's completed CONVERSATION_HISTORY step is a native compaction checkpoint.
    tokio::task::spawn_blocking(move || {
        read_steps(&path)?.query_row(
            "SELECT EXISTS(SELECT 1 FROM steps WHERE idx>=?1 AND step_type=23 AND status=3)",
            [first],
            |row| row.get(0),
        )
    })
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or(false)
}

// The native uncompressed CascadeStep stores UserInput at field 19, whose
// fields 2 and 19 are userResponse and clientMessageId (Agy 1.5.9).
pub(super) async fn refresh(process: &ProcessConfig, live: &Live, turn: &Turn) {
    let (requests, stdin) = {
        let data = live.data.lock().unwrap();
        let requests: Vec<String> = data
            .items
            .iter()
            .filter(|item| unbound(item) && is_control_input(item))
            .filter_map(|item| item["request_id"].as_str().map(str::to_owned))
            .collect();
        let stdin = turn.first_native_step.filter(|_| {
            data.items
                .iter()
                .any(|item| unbound(item) && item["request_id"] == turn.id)
        });
        (requests, stdin)
    };
    if requests.is_empty() && stdin.is_none() {
        return;
    }
    let path = process.native_path(&live.sid);
    let stdin = stdin.map(|first| (first, turn.id.clone(), turn.text.clone()));
    let matches = tokio::task::spawn_blocking(move || -> Option<Vec<(u64, String)>> {
        let db = read_steps(&path).ok()?;
        let first = if requests.is_empty() { stdin.as_ref()?.0 } else { 0 };
        let mut query = db.prepare("SELECT idx, step_payload FROM steps WHERE idx>=?1 AND step_type=14 AND status=3 AND step_format=0 AND step_payload IS NOT NULL").ok()?;
        let rows = query.query_map([first], |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Vec<u8>>(1)?))).ok()?;
        let mut found = Vec::new();
        let mut stdin_step = None;
        let mut ambiguous = false;
        for row in rows {
            let (index, payload) = row.ok()?;
            let Some(user) = bytes_field(&payload, 19) else { continue; };
            if let Some(id) = bytes_field(user, 19).filter(|id| !id.is_empty()) {
                if let Some(request) = requests.iter().find(|request| request.as_bytes() == id) {
                    found.push((index, request.clone()));
                }
            } else if let Some((first, _, text)) = &stdin
                && index >= *first
                && bytes_field(user, 2) == Some(text.as_bytes())
            {
                // stdin has no clientMessageId. Only a unique matching row
                // written after this serialized turn's pre-send checkpoint
                // can supply the missing echo, including after index reuse.
                ambiguous |= stdin_step.replace(index).is_some();
            }
        }
        if !ambiguous && let (Some(index), Some((_, request, _))) = (stdin_step, stdin) {
            found.push((index, request));
        }
        Some(found)
    }).await.ok().flatten().unwrap_or_default();
    for (index, request) in matches {
        project(
            live,
            turn,
            &json!({"step_index":index,"step_type":"user_input",
            "state":"DONE","client_message_id":request}),
            None,
        );
    }
}
