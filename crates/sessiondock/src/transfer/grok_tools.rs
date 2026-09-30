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
    Ok(rows)
}

pub(in crate::transfer) fn references(rows: &[Value]) -> BTreeSet<String> {
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
    fn visit(row: &Value, names: &BTreeMap<String, String>, result: &mut BTreeSet<String>) {
        for call in row["tool_calls"].as_array().into_iter().flatten() {
            if call["name"]
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
            args(&row["rawInput"], result);
        }
        for key in ["params", "update"] {
            if row[key].is_object() {
                visit(&row[key], names, result);
            }
        }
    }
    for row in rows {
        visit(row, &names, &mut result);
    }
    result
}
