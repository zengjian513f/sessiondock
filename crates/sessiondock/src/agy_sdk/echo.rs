//! Agy can persist steered input while headless output waits for an older tool.
//! Join by the native clientMessageId, never by body text or step position.
use super::{Live, Turn, project, transport::ProcessConfig};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;

pub(super) fn pending(live: &Live) -> bool {
    live.data.lock().unwrap().items.iter().any(|item| {
        item["steered"] == true && item["native_id"].is_null() && item["status"] != "unknown"
    })
}

// The native uncompressed CascadeStep stores UserInput at field 19, whose
// field 19 is clientMessageId (Agy 1.5.9). Unknown fields are skipped by wire
// type; unsupported or incomplete payloads leave the receipt unverified.
fn varint(bytes: &mut &[u8]) -> Option<u64> {
    let mut value = 0;
    for shift in (0..70).step_by(7) {
        let (&byte, rest) = bytes.split_first()?;
        *bytes = rest;
        if shift == 63 && byte > 1 {
            return None;
        }
        value |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Some(value);
        }
    }
    None
}

fn field(mut bytes: &[u8], wanted: u64) -> Option<&[u8]> {
    while !bytes.is_empty() {
        let tag = varint(&mut bytes)?;
        let length = match tag & 7 {
            0 => {
                varint(&mut bytes)?;
                continue;
            }
            1 => 8,
            2 => usize::try_from(varint(&mut bytes)?).ok()?,
            5 => 4,
            _ => return None,
        };
        let value = bytes.get(..length)?;
        bytes = bytes.get(length..)?;
        if tag >> 3 == wanted && tag & 7 == 2 {
            return Some(value);
        }
    }
    None
}

pub(super) async fn refresh(process: &ProcessConfig, live: &Live, turn: &Turn) {
    let requests: Vec<String> = live
        .data
        .lock()
        .unwrap()
        .items
        .iter()
        .filter(|item| {
            item["steered"] == true && item["native_id"].is_null() && item["status"] != "unknown"
        })
        .filter_map(|item| item["request_id"].as_str().map(str::to_owned))
        .collect();
    if requests.is_empty() {
        return;
    }
    let path = process.native_path(&live.sid);
    let matches = tokio::task::spawn_blocking(move || -> Option<Vec<(u64, String)>> {
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        db.busy_timeout(std::time::Duration::ZERO).ok()?;
        let mut query = db.prepare("SELECT idx, step_payload FROM steps WHERE step_type=14 AND status=3 AND step_format=0").ok()?;
        let rows = query.query_map([], |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Vec<u8>>(1)?))).ok()?;
        let mut found = Vec::new();
        for row in rows {
            let (index, payload) = row.ok()?;
            let Some(id) = field(&payload, 19).and_then(|user| field(user, 19))
                .and_then(|id| std::str::from_utf8(id).ok()) else { continue; };
            if requests.iter().any(|request| request == id) { found.push((index, id.to_owned())); }
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
