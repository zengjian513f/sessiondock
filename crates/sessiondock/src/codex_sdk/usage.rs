//! Recover the latest context measurement without replaying the transcript.
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Default)]
pub(crate) struct Snapshot {
    pub usage: Value,
    pub limits: Value,
    pub captured_at: Value,
}

pub(crate) fn latest(path: &Path) -> Option<Snapshot> {
    let mut file = File::open(path).ok()?;
    let mut offset = file.metadata().ok()?.len();
    let mut carry = Vec::new();
    let mut snapshot = Snapshot::default();
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
                return Some(snapshot);
            }
            if row["type"] != "event_msg" || row["payload"]["type"] != "token_count" {
                continue;
            }
            let limits = &row["payload"]["rate_limits"];
            if snapshot.limits.is_null()
                && limits.is_object()
                && (limits["limit_id"].is_null() || limits["limit_id"] == "codex")
            {
                let window = |name: &str| {
                    let value = &limits[name];
                    if value["used_percent"].is_number() {
                        json!({"usedPercent":value["used_percent"],
                            "windowDurationMins":value["window_minutes"], "resetsAt":value["resets_at"]})
                    } else {
                        Value::Null
                    }
                };
                let primary = window("primary");
                let secondary = window("secondary");
                if !primary.is_null() || !secondary.is_null() || limits["credits"].is_object() {
                    snapshot.limits = json!({"capturedAt":row["timestamp"], "rateLimits":{
                        "limitId":"codex", "primary":primary, "secondary":secondary,
                        "credits": if limits["credits"].is_object() { json!({
                            "balance":limits["credits"]["balance"], "hasCredits":limits["credits"]["has_credits"],
                            "unlimited":limits["credits"]["unlimited"]}) } else {Value::Null}}});
                }
            }
            let info = &row["payload"]["info"];
            let used = info["last_token_usage"]["total_tokens"].as_u64();
            let window = info["model_context_window"].as_u64();
            if let (Some(used), Some(window)) = (used, window.filter(|v| *v > 0)) {
                let total = &info["total_token_usage"];
                snapshot.captured_at = row["timestamp"].clone();
                snapshot.usage = json!({"last":{"totalTokens":used}, "modelContextWindow":window,
                    "total":{"totalTokens":total["total_tokens"], "inputTokens":total["input_tokens"],
                        "outputTokens":total["output_tokens"], "cachedInputTokens":total["cached_input_tokens"],
                        "reasoningOutputTokens":total["reasoning_output_tokens"]}});
                return Some(snapshot);
            }
        }
        carry = bytes[..boundary].to_vec();
    }
    Some(snapshot)
}
