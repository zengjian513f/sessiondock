//! CodeBuddy row summary from the native `~/.codebuddy/projects/<cwd>/<sid>.jsonl`
//! head and tail. Records are OpenAI-Responses shaped (`message`, `reasoning`,
//! `function_call`, `function_call_result`) plus bookkeeping kinds; see
//! docs/codebuddy.md. The summary never reads sidecars and never guesses a
//! turn state the records do not prove.

use serde_json::Value;

use super::{
    CLAUDE_HEAD_LINES, Input, Records, RowSummary, clip, committed_end, cursor_head, flatten_text,
    is_injected, iso_seconds, latest_timestamp, native_identity, norm_ts, py_strip,
    skipped_warnings, text_if_truthy, title_from_text,
};
use crate::sessions::text::is_timeline_protocol;

/// A real user prompt: not CLI-internal, injected context or an interrupt marker.
pub(in crate::sessions) fn real_user_text(value: &Value) -> Result<Option<String>, String> {
    if value["type"] != "message" || value["role"] != "user" {
        return Ok(None);
    }
    let provider = &value["providerData"];
    if provider["isMeta"] == true
        || provider["skipRun"] == true
        || provider["isCompactInternal"] == true
        || provider["isCompacted"] == true
        || provider["isSummary"] == true
    {
        return Ok(None);
    }
    let text = flatten_text(&value["content"])?;
    let trimmed = py_strip(&text);
    Ok((!trimmed.is_empty()
        && !is_injected(&text)
        && !is_timeline_protocol(&text)
        && !trimmed.starts_with("<turn_aborted>"))
    .then(|| title_from_text(&text)))
}

/// The newest model the assistant side reported.
fn model_of(value: &Value) -> Option<&Value> {
    let model = &value["providerData"]["model"];
    (value["type"] != "message" || value["role"] == "assistant")
        .then_some(model)
        .filter(|model| crate::sessions::providers::valid_native_model(model))
}

pub(super) fn summarize(input: &Input<'_>) -> RowSummary {
    let stem = input
        .path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(data) = &input.data else {
        let mut row = RowSummary::blank(
            stem.clone(),
            clip(
                &format!("(无标题) {}", stem.chars().take(8).collect::<String>()),
                110,
            ),
            "(未知)".to_owned(),
            iso_seconds(0),
        );
        row.unsupported = Some("会话文件暂时不可读取".to_owned());
        return row;
    };
    let records = Records::parse(data, CLAUDE_HEAD_LINES);
    let mut hard_error: Option<String> = None;
    let mut first_user: Option<String> = None;
    for record in &records.head.records {
        if first_user.is_none() {
            match real_user_text(&record.value) {
                Ok(title) => first_user = title,
                Err(reason) => {
                    hard_error.get_or_insert(reason);
                }
            }
        }
    }
    let sid = records
        .all()
        .find_map(|record| text_if_truthy(&record.value["sessionId"]))
        .unwrap_or_else(|| stem.clone());
    let cwd = records
        .all()
        .find_map(|record| text_if_truthy(&record.value["cwd"]))
        .unwrap_or_else(|| "(未知)".to_owned());
    // Rename-style records are appended at the end, so the tail decides.
    let custom = records
        .tail
        .records
        .iter()
        .rev()
        .chain(records.head.records.iter().rev())
        .find_map(|record| match record.value["type"].as_str() {
            Some("custom-title") => text_if_truthy(&record.value["customTitle"]),
            _ => None,
        });
    let ai = records
        .tail
        .records
        .iter()
        .rev()
        .chain(records.head.records.iter().rev())
        .find_map(|record| match record.value["type"].as_str() {
            Some("ai-title") => text_if_truthy(&record.value["aiTitle"]),
            _ => None,
        });
    let title = custom
        .or(ai)
        .or(first_user)
        .unwrap_or_else(|| format!("(无标题) {}", stem.chars().take(8).collect::<String>()));
    let model = records
        .tail
        .records
        .iter()
        .rev()
        .chain(records.head.records.iter().rev())
        .find_map(|record| model_of(&record.value).cloned())
        .unwrap_or(Value::Null);
    let created = records
        .all()
        .find_map(|record| norm_ts(&record.value["timestamp"]))
        .unwrap_or_else(|| iso_seconds(data.stamp.mtime_ns));
    let updated = latest_timestamp(&records).unwrap_or_else(|| created.clone());
    let (native_id, declared_ids) = native_identity(
        records
            .all()
            .filter_map(|record| record.value.get("sessionId")),
    );
    let mut warnings = skipped_warnings("codebuddy", &records);
    if hard_error.is_some() {
        warnings.clear();
    }
    let committed = committed_end(data);
    RowSummary {
        forked_from_id: None,
        sid,
        title: clip(&title, 110),
        cwd,
        created,
        updated,
        size: data.stamp.size,
        model,
        branch: Value::Null,
        codex: None,
        agent: None,
        grok: None,
        continued_in_sid: None,
        native_id,
        declared_ids,
        unsupported: hard_error,
        warnings,
        committed,
        cursor_head: committed.and_then(|end| cursor_head(data, end)),
        background: 0,
        turn: None,
    }
}
