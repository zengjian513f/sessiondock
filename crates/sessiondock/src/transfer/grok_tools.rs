//! Grok agent requests across the chat/update streams of one native session.
use super::TransferError;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

pub(in crate::transfer) fn rows(directory: &Path) -> Result<Vec<Value>, TransferError> {
    let mut rows = Vec::new();
    for name in ["chat_history.jsonl", "updates.jsonl"] {
        let path = directory.join(name);
        if !path.exists() {
            continue;
        }
        for line in fs::read(path)?
            .split(|b| *b == b'\n')
            .filter(|v| !v.is_empty())
        {
            rows.push(serde_json::from_slice(line)?);
        }
    }
    let checkpoints = directory.join("compaction_checkpoints");
    if checkpoints.is_dir() {
        for entry in fs::read_dir(checkpoints)? {
            let path = entry?.path();
            if path.extension().is_some_and(|s| s == "json") {
                let checkpoint: Value = serde_json::from_slice(&fs::read(path)?)?;
                rows.extend(
                    checkpoint["compacted_history"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .cloned(),
                );
            }
        }
    }
    Ok(rows)
}

pub(in crate::transfer) fn references(rows: &[Value]) -> BTreeSet<String> {
    collect(rows, false)
}

pub(in crate::transfer) fn required_references(rows: &[Value]) -> BTreeSet<String> {
    collect(rows, true)
}

fn collect(rows: &[Value], required: bool) -> BTreeSet<String> {
    let mut names = BTreeMap::new();
    for row in rows {
        super::super::files::collect_tools(row, "grok", &mut names);
    }
    let mut result = BTreeSet::new();
    fn args(value: &Value, result: &mut BTreeSet<String>) {
        for key in ["resume_from", "subagent_id", "task_id"] {
            if let Some(id) = value[key]
                .as_str()
                .filter(|v| !v.is_empty() && *v != "parent")
            {
                result.insert(id.into());
            }
        }
        for id in value["task_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            result.insert(id.into());
        }
    }
    fn visit(
        row: &Value,
        names: &BTreeMap<String, String>,
        result: &mut BTreeSet<String>,
        required: bool,
    ) {
        for call in row["tool_calls"].as_array().into_iter().flatten() {
            if !required
                && call["name"]
                    .as_str()
                    .is_some_and(super::super::files::agent_tool)
            {
                if let Some(raw) = call["arguments"].as_str() {
                    if let Ok(value) = serde_json::from_str::<Value>(raw) {
                        args(&value, result);
                    }
                } else {
                    args(&call["arguments"], result);
                }
            }
        }
        if row["toolCallId"]
            .as_str()
            .or_else(|| row["tool_call_id"].as_str())
            .and_then(|id| names.get(&format!("grok:{id}")))
            .is_some_and(|name| super::super::files::agent_tool(name))
        {
            if !required {
                args(&row["rawInput"], result);
            }
            for key in ["content", "rawOutput"] {
                result_ids(&row[key], result, required);
            }
        }
        for key in ["params", "update"] {
            if row[key].is_object() {
                visit(&row[key], names, result, required);
            }
        }
    }
    for row in rows {
        visit(row, &names, &mut result, required);
    }
    result
}

// Match the native envelopes also handled by files::agent_text. Do not scan
// user messages, arbitrary tool output, or unmarked answer prose for UUIDs.
fn result_ids(value: &Value, result: &mut BTreeSet<String>, required: bool) {
    if let Some(text) = value.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Value>(text)
            && (parsed.is_object() || parsed.is_array())
        {
            result_ids(&parsed, result, required);
            return;
        }
        for (prefix, suffix) in [
            ("subagent_id: ", "\n"),
            ("agentId: ", " "),
            ("=== Task ", " ==="),
            ("<subagent_meta>id=", ","),
            ("resume_from=\"", "\""),
            ("task_ids=[\"", "\"]"),
        ] {
            if required && matches!(prefix, "=== Task " | "resume_from=\"" | "task_ids=[\"") {
                continue;
            }
            for (start, _) in text.match_indices(prefix) {
                if let Some(id) = text[start + prefix.len()..].split_once(suffix).map(|v| v.0)
                    && !id.is_empty()
                {
                    result.insert(id.into());
                }
            }
        }
    } else if let Some(items) = value.as_array() {
        for item in items.iter().filter(|item| item["type"] == "text") {
            result_ids(&item["text"], result, required);
        }
    } else if value.is_object() {
        for key in ["subagent_id", "child_session_id", "task_id", "agentId"] {
            if required && key == "task_id" {
                continue;
            }
            if let Some(id) = value[key].as_str().filter(|id| !id.is_empty()) {
                result.insert(id.into());
            }
        }
        for key in ["text", "output", "Result"] {
            result_ids(&value[key], result, required);
        }
    }
}
