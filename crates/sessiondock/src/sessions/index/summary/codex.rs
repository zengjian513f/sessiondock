//! Codex row summary: `CodexAdapter._raw_meta` (120 head pieces) plus the
//! subagent tail rules of `_codex_agent_tail` (last record time, open turn),
//! from head/tail records only. Fork inheritance and rename enrichment are
//! graph/index concerns.

use serde_json::Value;

use super::{
    AgentMeta, CODEX_HEAD_LINES, CodexMeta, Input, Records, RowSummary, clip, committed_end,
    cursor_head, flatten_text, is_codex_protocol_injection, is_injected, iso_seconds,
    latest_timestamp, native_identity, norm_ts, py_strip, skipped_warnings, text_if_truthy,
    title_from_text, truthy,
};
use crate::sessions::providers::Skipped;

fn first_truthy<'a>(values: impl IntoIterator<Item = &'a Value>) -> Option<String> {
    values.into_iter().find_map(text_if_truthy)
}

/// Python `_codex_agent_tail`'s turn state: the latest `event_msg` whose
/// kind is a turn boundary (`_CODEX_TURN_OPEN`; other kinds such as
/// `token_count` are passed over) says whether the turn is still open;
/// no such record means closed. Subagents share the parent process, so
/// this is the only liveness fact a rollout carries.
fn open_turn(records: &Records) -> bool {
    records
        .tail
        .records
        .iter()
        .rev()
        .chain(records.head.records.iter().rev())
        .filter(|record| record.value["type"] == "event_msg")
        .find_map(
            |record| match record.value["payload"]["type"].as_str().unwrap_or("") {
                "task_started" | "turn_started" => Some(true),
                "task_complete" | "turn_complete" | "turn_aborted" => Some(false),
                _ => None,
            },
        )
        .unwrap_or(false)
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
                &format!(
                    "(无标题) {}",
                    stem.replace("rollout-", "")
                        .chars()
                        .take(16)
                        .collect::<String>()
                ),
                110,
            ),
            "(未知)".to_owned(),
            iso_seconds(0),
        );
        row.unsupported = Some("会话文件暂时不可读取".to_owned());
        return row;
    };
    let records = Records::parse(data, CODEX_HEAD_LINES);
    let mut hard_error: Option<String> = None;

    // Head (Python `_raw_meta`): first session_meta, first model, first
    // qualifying user message.
    let mut meta: Option<&Value> = None;
    let mut model = Value::Null;
    let mut first_user: Option<String> = None;
    for record in &records.head.records {
        let value = &record.value;
        let payload = &value["payload"];
        let kind = value["type"].as_str().unwrap_or("");
        if kind == "session_meta" && meta.is_none() && truthy(payload) {
            meta = Some(payload);
        }
        if kind == "turn_context" && !truthy(&model) {
            model = payload["model"].clone();
        }
        if first_user.is_none()
            && kind == "response_item"
            && payload["type"] == "message"
            && payload["role"] == "user"
        {
            match flatten_text(&payload["content"]) {
                Ok(text) => {
                    let native_meta = &payload["internal_chat_message_metadata_passthrough"];
                    if !py_strip(&text).is_empty()
                        && !is_injected(&text)
                        && !is_codex_protocol_injection(&text, native_meta)
                    {
                        first_user = Some(text);
                    }
                }
                Err(reason) => {
                    hard_error.get_or_insert(reason);
                }
            }
        }
    }
    // Hard failures today's projection raises on the records seen, in file order.
    // Only the first session_meta is this file's identity (Python `and not
    // meta`); old-style forks and subagent rollouts copy their ancestors'
    // metas after it, which the projection skips and counts.
    let mut first_meta: Option<&Value> = None;
    let mut duplicate_meta = 0usize;
    for record in records.all() {
        let value = &record.value;
        let payload = &value["payload"];
        match value["type"].as_str().unwrap_or("") {
            "session_meta" => {
                if first_meta.is_some() {
                    duplicate_meta += 1;
                    continue;
                }
                first_meta = Some(payload);
                if !payload["history_base"].is_null() && !payload["history_base"].is_object() {
                    hard_error.get_or_insert_with(|| {
                        "Codex history_base 必须是对象或 null；拒绝忽略损坏的继承身份".to_owned()
                    });
                }
            }
            "response_item" if payload["type"] == "message" => {
                if let Err(reason) = flatten_text(&payload["content"]) {
                    hard_error.get_or_insert(reason);
                }
            }
            _ => {}
        }
    }

    let meta = meta.cloned().unwrap_or(Value::Null);
    let thread_source = text_if_truthy(&meta["thread_source"]).unwrap_or_default();
    let source_meta = &meta["source"];
    let is_subagent = thread_source == "subagent"
        || source_meta
            .as_object()
            .is_some_and(|source| source.contains_key("subagent"));
    let spawn = &source_meta["subagent"]["thread_spawn"];
    let spawn = if spawn.is_object() {
        spawn
    } else {
        &Value::Null
    };
    let sid = first_truthy([
        if is_subagent {
            &meta["id"]
        } else {
            &meta["session_id"]
        },
        &meta["id"],
    ])
    .unwrap_or_else(|| stem.clone());
    let base_title = match &first_user {
        Some(text) => title_from_text(text),
        None => format!(
            "(无标题) {}",
            stem.replace("rollout-", "")
                .chars()
                .take(16)
                .collect::<String>()
        ),
    };
    let mtime = iso_seconds(data.stamp.mtime_ns);
    let created = norm_ts(&meta["timestamp"]).unwrap_or_else(|| mtime.clone());
    let updated = if is_subagent {
        latest_timestamp(&records).unwrap_or_else(|| mtime.clone())
    } else {
        mtime
    };
    let mut warnings = skipped_warnings("codex", &records);
    if duplicate_meta > 0 {
        warnings.insert(0, Skipped::duplicate_codex_meta_warning(duplicate_meta));
    }
    if hard_error.is_some() {
        warnings.clear();
    }
    let (native_id, declared_ids) =
        native_identity(first_meta.and_then(|meta| meta.get("id")).into_iter());
    let agent = is_subagent.then(|| AgentMeta {
        id: sid.clone(),
        title: first_truthy([
            &meta["agent_path"],
            &spawn["agent_path"],
            &meta["agent_nickname"],
            &spawn["agent_nickname"],
        ])
        .unwrap_or_else(|| sid.clone()),
        kind: first_truthy([&meta["agent_role"], &spawn["agent_role"]])
            .unwrap_or_else(|| "subagent".to_owned()),
        open_turn: open_turn(&records),
    });
    let committed = committed_end(data);
    RowSummary {
        sid,
        title: clip(&base_title, 110),
        cwd: text_if_truthy(&meta["cwd"]).unwrap_or_else(|| "(未知)".to_owned()),
        created,
        updated,
        size: data.stamp.size,
        model,
        branch: Value::Null,
        codex: Some(CodexMeta {
            has_meta: truthy(&meta),
            forked_from_id: text_if_truthy(&meta["forked_from_id"]).unwrap_or_default(),
            history_base: if meta["history_base"].is_object() {
                meta["history_base"].clone()
            } else {
                Value::Null
            },
            parent_thread_id: if is_subagent {
                first_truthy([
                    &meta["parent_thread_id"],
                    &spawn["parent_thread_id"],
                    &meta["forked_from_id"],
                ])
                .unwrap_or_default()
            } else {
                String::new()
            },
        }),
        agent,
        grok: None,
        continued_in_sid: None,
        native_id,
        declared_ids,
        unsupported: hard_error,
        warnings,
        committed,
        cursor_head: committed.and_then(|end| cursor_head(data, end)),
    }
}
