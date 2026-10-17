//! Recover the latest context measurement without replaying the transcript.
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub(super) fn latest(path: &Path) -> Option<Value> {
    let mut file = File::open(path).ok()?;
    let mut offset = file.metadata().ok()?.len();
    let mut carry = Vec::new();
    while offset > 0 {
        let size = offset.min(64 * 1024) as usize;
        offset -= size as u64;
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut bytes = vec![0; size];
        file.read_exact(&mut bytes).ok()?;
        bytes.extend_from_slice(&carry);
        let boundary = if offset == 0 {
            0
        } else {
            bytes
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |i| i + 1)
        };
        for line in bytes[boundary..].rsplit(|b| *b == b'\n') {
            let Ok(row) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            // A measurement from before compaction is no longer current.
            if row["type"] == "compacted" {
                return None;
            }
            if row["type"] != "event_msg" || row["payload"]["type"] != "token_count" {
                continue;
            }
            let info = &row["payload"]["info"];
            let used = info["last_token_usage"]["total_tokens"].as_u64();
            let window = info["model_context_window"].as_u64();
            if let (Some(used), Some(window)) = (used, window.filter(|v| *v > 0)) {
                return Some(json!({"last":{"totalTokens":used}, "modelContextWindow":window}));
            }
        }
        carry = bytes[..boundary].to_vec();
    }
    None
}
