//! OpenCode row summary from the SessionDock mirror (`sessions::opencode`):
//! the session row in `summary.json`, plus the message file's stamp. Head
//! records only supply a title for a session OpenCode has not titled yet.

use serde_json::Value;

use super::{
    CLAUDE_HEAD_LINES, Input, Records, RowSummary, SidecarBytes, clip, committed_end, cursor_head,
    iso_seconds, native_identity, norm_ts, skipped_warnings, text_if_truthy,
};

pub const UNTITLED: &str = "OpenCode 会话";

pub(super) fn summarize(input: &Input<'_>) -> RowSummary {
    let directory = input
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut hard_error = None;
    let mut summary_stamp = None;
    let info = match &input.sidecar {
        Some(SidecarBytes::Bytes { bytes, stamp }) => {
            summary_stamp = Some(*stamp);
            match serde_json::from_slice::<Value>(bytes) {
                Ok(value) if value["format"] == crate::sessions::opencode::FORMAT => value,
                Ok(value) => {
                    hard_error = Some("OpenCode summary.json 格式未知".to_owned());
                    value
                }
                Err(_) => {
                    hard_error = Some("OpenCode summary.json 不是完整有效的 JSON".to_owned());
                    Value::Null
                }
            }
        }
        Some(SidecarBytes::Failed { stamp, reason }) => {
            summary_stamp = *stamp;
            hard_error = Some(reason.clone());
            Value::Null
        }
        None => {
            hard_error = Some("OpenCode summary.json 暂时不可读取".to_owned());
            Value::Null
        }
    };
    let session = &info["session"];
    let sid = text_if_truthy(&session["id"]).unwrap_or_else(|| directory.clone());
    let records = match &input.data {
        Some(data) => Records::parse(data, CLAUDE_HEAD_LINES),
        None => Records::empty(),
    };
    let title = text_if_truthy(&session["title"])
        .or_else(|| {
            records
                .all()
                .find(|record| record.value["type"] == "user")
                .and_then(|record| text_if_truthy(&record.value["data"]["text"]))
        })
        .unwrap_or_else(|| UNTITLED.to_owned());
    let fallback_mtime = input
        .data
        .as_ref()
        .map(|data| data.stamp.mtime_ns)
        .or(summary_stamp.map(|stamp| stamp.mtime_ns))
        .unwrap_or(0);
    let created = norm_ts(&session["time_created"]).unwrap_or_else(|| iso_seconds(fallback_mtime));
    let updated = norm_ts(&session["time_updated"]).unwrap_or_else(|| iso_seconds(fallback_mtime));
    let warnings = if hard_error.is_some() {
        Vec::new()
    } else {
        skipped_warnings("opencode", &records)
    };
    let (native_id, declared_ids) = if hard_error.is_none() {
        native_identity(session.get("id").into_iter())
    } else {
        native_identity(std::iter::empty::<&Value>())
    };
    let committed = input.data.as_ref().and_then(committed_end);
    RowSummary {
        sid,
        title: clip(title.trim(), 110),
        cwd: text_if_truthy(&session["directory"]).unwrap_or_default(),
        created,
        updated,
        size: input.data.as_ref().map_or(0, |data| data.stamp.size)
            + summary_stamp.map_or(0, |stamp| stamp.size),
        model: session["model"]["id"].clone(),
        branch: session["agent"].clone(),
        codex: None,
        agent: None,
        grok: None,
        continued_in_sid: None,
        native_id,
        declared_ids,
        unsupported: hard_error,
        warnings,
        committed,
        cursor_head: committed
            .and_then(|end| input.data.as_ref().and_then(|data| cursor_head(data, end))),
    }
}
