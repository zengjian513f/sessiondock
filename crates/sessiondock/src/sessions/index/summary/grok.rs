//! Grok row summary: `GrokAdapter.session_meta` from `summary.json` plus the
//! chat file's stamp. The chat head/tail is only inspected for corrupt lines
//! and unknown record kinds (today's row warnings); no row field comes from it.

use std::path::Path;

use serde_json::Value;

use super::{
    CLAUDE_HEAD_LINES, GrokMeta, Input, Records, RowSummary, SidecarBytes, clip, committed_end,
    cursor_head, flatten_text, iso_seconds, native_identity, norm_ts, skipped_warnings,
    text_if_truthy, truthy, unquote,
};

pub(super) fn summarize(input: &Input<'_>) -> RowSummary {
    let path = input.path;
    let directory = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let project = path
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut hard_error = None;
    let mut summary_stamp = None;
    let info = match &input.sidecar {
        Some(SidecarBytes::Bytes { bytes, stamp }) => {
            summary_stamp = Some(*stamp);
            match serde_json::from_slice::<Value>(bytes) {
                Ok(value) => match crate::sessions::providers::validate_grok_summary(&value) {
                    Ok(()) => value,
                    Err(reason) => {
                        hard_error = Some(reason);
                        value
                    }
                },
                Err(_) => {
                    hard_error = Some("Grok summary.json 不是完整有效的 JSON".to_owned());
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
            hard_error = Some("Grok summary.json 暂时不可读取".to_owned());
            Value::Null
        }
    };
    let base = if info["info"].is_object() {
        &info["info"]
    } else {
        &Value::Null
    };
    let sid = text_if_truthy(&base["id"]).unwrap_or_else(|| directory.clone());
    let title = text_if_truthy(&info["generated_title"])
        .or_else(|| text_if_truthy(&info["session_summary"]))
        .unwrap_or_else(|| crate::sessions::providers::grok_untitled_title().to_owned());
    let cwd = text_if_truthy(&base["cwd"]).unwrap_or_else(|| unquote(&project));
    // Chat mtime, or the summary's when the chat does not exist yet.
    let fallback_mtime = input
        .data
        .as_ref()
        .map(|data| data.stamp.mtime_ns)
        .or(summary_stamp.map(|stamp| stamp.mtime_ns))
        .unwrap_or(0);
    let created = norm_ts(&info["created_at"]).unwrap_or_else(|| iso_seconds(fallback_mtime));
    let selected = if truthy(&info["last_active_at"]) {
        &info["last_active_at"]
    } else {
        &info["updated_at"]
    };
    let updated = norm_ts(selected).unwrap_or_else(|| iso_seconds(fallback_mtime));
    let records = match &input.data {
        Some(data) => Records::parse(data, CLAUDE_HEAD_LINES),
        None => Records::empty(),
    };
    if hard_error.is_none() {
        for record in records.all() {
            let value = &record.value;
            if matches!(
                value["type"].as_str(),
                Some("user" | "assistant" | "system")
            ) && let Err(reason) = flatten_text(&value["content"])
            {
                hard_error = Some(reason);
                break;
            }
        }
    }
    let warnings = if hard_error.is_some() {
        Vec::new()
    } else {
        skipped_warnings("grok", &records)
    };
    // The session directory's `summary.json` declares its own id
    // (`info.id`, the value Grok's `--session-id`/`--resume` name), so a
    // Grok main session has a native scope for binding, stop and takeover.
    // A summary that failed validation declares nothing.
    let (native_id, declared_ids) = if hard_error.is_none() {
        native_identity(base.get("id").into_iter())
    } else {
        native_identity(std::iter::empty::<&Value>())
    };
    let committed = input.data.as_ref().and_then(committed_end);
    RowSummary {
        sid,
        title: clip(&title, 110),
        cwd,
        created,
        updated,
        size: input.data.as_ref().map_or(0, |data| data.stamp.size)
            + summary_stamp.map_or(0, |stamp| stamp.size),
        model: info["current_model_id"].clone(),
        branch: info["agent_name"].clone(),
        codex: None,
        agent: None,
        grok: Some(GrokMeta {
            chat_exists: input.data.is_some(),
        }),
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

/// Native turn activity excludes background summary/memory refreshes. Read the
/// same bounded, complete JSONL regions as other inventory summaries; absent or
/// unknown event formats retain the summary fallback.
pub(crate) fn activity_updated(data: &super::DataFile<'_>) -> Option<String> {
    let records = Records::parse(data, CLAUDE_HEAD_LINES);
    records
        .all()
        .filter(|record| {
            matches!(
                record.value["type"].as_str(),
                Some(
                    "turn_started"
                        | "turn_ended"
                        | "loop_started"
                        | "first_token"
                        | "phase_changed"
                        | "tool_started"
                        | "tool_completed"
                        | "permission_requested"
                        | "permission_resolved"
                )
            )
        })
        .filter_map(|record| norm_ts(&record.value["ts"]))
        .max()
}
