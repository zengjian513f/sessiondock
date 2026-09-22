//! Claude row summary: `ClaudeAdapter._meta` (main transcripts, including
//! the tail `continued-in` sid) and the sidecar item rules of its
//! `agent_items` loop (`_claude_agent_tail`: last record time and open
//! turn), from head/tail records only. Whether an open turn is still
//! running needs the owner's stop notices: `index/agent_stops.rs`.

use std::path::Path;

use serde_json::Value;

use super::{
    AgentMeta, CLAUDE_AGENT_CREATED_LINES, CLAUDE_HEAD_LINES, Input, Records, RowSummary,
    SidecarBytes, claude_bash_input, claude_bash_output_matches, committed_end, cursor_head,
    flatten_text, is_injected, iso_seconds, latest_timestamp, native_identity, norm_ts, py_str,
    py_strip, skipped_warnings, text_if_truthy, title_from_text, truthy,
};

/// `<project>/<sid>/subagents/agent-<id>.jsonl` → `<id>`.
pub(crate) fn agent_id(path: &Path) -> Option<&str> {
    let directory = path.parent()?;
    if directory.file_name()? != "subagents" {
        return None;
    }
    path.file_stem()?.to_str()?.strip_prefix("agent-")
}

/// The main transcript a sidecar belongs to (matched by exact path, never opened).
pub(crate) fn owner_path(path: &Path) -> Option<std::path::PathBuf> {
    agent_id(path)?;
    let session_directory = path.parent()?.parent()?;
    let stem = session_directory.file_name()?.to_str()?;
    Some(session_directory.parent()?.join(format!("{stem}.jsonl")))
}

/// Turn state: the last user/assistant record
/// closes the turn only when it is an assistant `end_turn`
/// (`_CLAUDE_TURN_CLOSED`; refusal/stop_sequence are followed by another
/// assistant record without a stop notice, and a streamed text record may
/// still carry `stop_reason: null`). No such record means closed.
fn open_turn(records: &Records) -> bool {
    records
        .tail
        .records
        .iter()
        .rev()
        .chain(records.head.records.iter().rev())
        .find(|record| matches!(record.value["type"].as_str(), Some("user" | "assistant")))
        .is_some_and(|record| {
            !(record.value["type"] == "assistant"
                && record.value["message"]["stop_reason"] == "end_turn")
        })
}

fn project_name(path: &Path, agent: bool) -> String {
    let project = if agent {
        path.parent().and_then(Path::parent).and_then(Path::parent)
    } else {
        path.parent()
    };
    project
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(super) fn summarize(input: &Input<'_>) -> RowSummary {
    let path = input.path;
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let agent = agent_id(path).map(str::to_owned);
    let Some(data) = &input.data else {
        // Claude candidates always carry a data file; keep the row honest.
        let mut row = RowSummary::blank(
            stem.chars().take(8).collect(),
            stem.chars().take(8).collect(),
            format!(
                "/{}",
                project_name(path, agent.is_some())
                    .trim_start_matches('-')
                    .replace('-', "/")
            ),
            iso_seconds(0),
        );
        row.unsupported = Some("会话文件暂时不可读取".to_owned());
        return row;
    };
    let records = Records::parse(data, CLAUDE_HEAD_LINES);
    let mut hard_error: Option<String> = None;

    // Head: first values in file order.
    let mut generated_title = None;
    let mut cwd = None;
    let mut branch = Value::Null;
    let mut created = None;
    let mut sid = None;
    let mut first_user: Option<String> = None;
    for record in &records.head.records {
        let value = &record.value;
        let kind = value["type"].as_str().unwrap_or("");
        if kind == "ai-title" && generated_title.is_none() {
            generated_title = text_if_truthy(&value["aiTitle"]);
        }
        if cwd.is_none() {
            cwd = text_if_truthy(&value["cwd"]);
        }
        if branch.is_null() && truthy(&value["gitBranch"]) {
            branch = value["gitBranch"].clone();
        }
        if created.is_none() && truthy(&value["timestamp"]) {
            created = norm_ts(&value["timestamp"]);
        }
        if sid.is_none() {
            sid = text_if_truthy(&value["sessionId"]);
        }
        if kind == "user" && first_user.is_none() && !truthy(&value["isSidechain"]) {
            match flatten_text(&value["message"]["content"]) {
                Ok(text) => {
                    if let Some(command) = claude_bash_input(&text) {
                        first_user = Some(command);
                    } else if !claude_bash_output_matches(&text)
                        && !py_strip(&text).is_empty()
                        && !is_injected(&text)
                    {
                        first_user = Some(crate::sessions::providers::claude_pasted_text(&text));
                    }
                }
                Err(reason) => {
                    hard_error.get_or_insert(reason);
                }
            }
        }
    }

    // Tail: latest titles, the continuation sid and the cwd majority.
    let mut custom_title = None;
    let mut latest_ai_title = None;
    let mut continued_in_sid = None;
    let mut tail_cwds: Vec<(String, usize)> = Vec::new();
    for record in &records.tail.records {
        let value = &record.value;
        if let Some(seen) = text_if_truthy(&value["cwd"]) {
            match tail_cwds.iter_mut().find(|(known, _)| *known == seen) {
                Some((_, count)) => *count += 1,
                None => tail_cwds.push((seen, 1)),
            }
        }
        let kind = value["type"].as_str().unwrap_or("");
        if kind == "custom-title" && truthy(&value["customTitle"]) {
            custom_title = Some(py_str(&value["customTitle"]));
        } else if kind == "ai-title" && truthy(&value["aiTitle"]) {
            latest_ai_title = Some(py_str(&value["aiTitle"]));
        } else if kind == "continued-in" && truthy(&value["continuedInSessionId"]) {
            continued_in_sid = Some(py_str(&value["continuedInSessionId"]));
        }
    }
    // Shape errors of records the reference adapter would also choke on.
    for record in records.all() {
        let value = &record.value;
        if matches!(value["type"].as_str(), Some("user" | "assistant"))
            && let Err(reason) = flatten_text(&value["message"]["content"])
        {
            hard_error.get_or_insert(reason);
        }
    }

    let title = custom_title
        .or(latest_ai_title)
        .or(generated_title)
        .unwrap_or_else(|| match &first_user {
            Some(text) => title_from_text(text),
            None => stem.chars().take(8).collect(),
        });
    let cwd = cwd
        .or_else(|| {
            // The first of equal counts is kept.
            tail_cwds
                .iter()
                .rev()
                .max_by_key(|(_, count)| *count)
                .map(|(cwd, _)| cwd.clone())
        })
        .unwrap_or_else(|| {
            format!(
                "/{}",
                project_name(path, agent.is_some())
                    .trim_start_matches('-')
                    .replace('-', "/")
            )
        });
    let mtime = iso_seconds(data.stamp.mtime_ns);
    let mut warnings = skipped_warnings("claude", &records);
    let (native_id, declared_ids) = native_identity(
        records
            .all()
            .filter_map(|record| record.value.get("sessionId")),
    );

    let (created, updated, agent_meta) = if let Some(id) = &agent {
        // Sidecar item rules: created = first head timestamp (8 pieces) or the
        // meta.json mtime or the updated time; updated = last timestamp or mtime.
        let updated = latest_timestamp(&records).unwrap_or_else(|| mtime.clone());
        let (info, sidecar_mtime) = match &input.sidecar {
            Some(SidecarBytes::Bytes { bytes, stamp }) => {
                match serde_json::from_slice::<Value>(bytes) {
                    Ok(value) if value.is_object() => (value, Some(stamp.mtime_ns)),
                    _ => {
                        hard_error.get_or_insert_with(|| {
                            "子代理元数据 (meta.json) 不是完整有效的 JSON 对象".to_owned()
                        });
                        (Value::Null, Some(stamp.mtime_ns))
                    }
                }
            }
            Some(SidecarBytes::Failed { stamp, reason }) => {
                hard_error.get_or_insert_with(|| reason.clone());
                (Value::Null, stamp.map(|stamp| stamp.mtime_ns))
            }
            None => (Value::Null, None),
        };
        let created = records
            .head
            .records
            .iter()
            .filter(|record| record.line < CLAUDE_AGENT_CREATED_LINES)
            .find_map(|record| norm_ts(&record.value["timestamp"]))
            .or_else(|| sidecar_mtime.map(iso_seconds))
            .unwrap_or_else(|| updated.clone());
        let title = text_if_truthy(&info["description"])
            .unwrap_or_else(|| format!("子代理 {}", id.chars().take(8).collect::<String>()));
        let kind = text_if_truthy(&info["agentType"]).unwrap_or_else(|| "subagent".to_owned());
        (
            created,
            updated,
            Some(AgentMeta {
                id: id.clone(),
                title,
                kind,
                open_turn: open_turn(&records),
            }),
        )
    } else {
        let updated = latest_timestamp(&records)
            .or_else(|| created.clone())
            .unwrap_or_else(|| mtime.clone());
        (created.unwrap_or_else(|| mtime.clone()), updated, None)
    };
    if hard_error.is_some() {
        warnings.clear();
    }
    let committed = committed_end(data);
    RowSummary {
        // A sidecar row is keyed by its agent id, like today's `_is_subagent` meta.
        sid: agent.clone().or(sid).unwrap_or(stem),
        title: match &agent_meta {
            Some(meta) => meta.title.clone(),
            None => title,
        },
        cwd,
        created,
        updated,
        size: data.stamp.size,
        model: Value::Null,
        branch,
        codex: None,
        agent: agent_meta,
        grok: None,
        // `continued_in_sid` is kept on main rows only; a sidecar's
        // tail never carries one, and its row is folded away anyway.
        continued_in_sid: if agent.is_some() {
            None
        } else {
            continued_in_sid
        },
        native_id,
        declared_ids,
        unsupported: hard_error,
        warnings,
        committed,
        cursor_head: committed.and_then(|end| cursor_head(data, end)),
    }
}
