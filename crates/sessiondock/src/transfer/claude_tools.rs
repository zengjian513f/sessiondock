//! Native Claude agent destinations, scoped to calls in one transcript.
//! Free-form messages and teammate names do not establish missing file dependencies.
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(in crate::transfer) struct Links {
    pub requests: BTreeSet<String>,
    pub resumed: BTreeMap<String, String>,
}

fn resumed(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|v| resumed(&v));
    }
    if let Some(items) = value.as_array() {
        return items
            .iter()
            .filter(|v| v["type"] == "text")
            .find_map(|v| resumed(&v["text"]));
    }
    if value["success"] == true {
        return value["resumedAgentId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
    }
    None
}

pub(in crate::transfer) fn links(rows: &[Value]) -> Links {
    let mut result = Links::default();
    let mut sends = BTreeSet::new();
    for row in rows {
        for block in row["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] != "tool_use" {
                continue;
            }
            let field = match block["name"].as_str() {
                Some("SendMessage") => {
                    if let Some(id) = block["id"].as_str() {
                        sends.insert(id);
                    }
                    "to"
                }
                Some("Agent" | "Task") => "resume",
                _ => continue,
            };
            if let Some(id) = block["input"][field].as_str().filter(|id| !id.is_empty()) {
                result.requests.insert(id.into());
            }
        }
    }
    for row in rows {
        for block in row["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] != "tool_result" {
                continue;
            }
            let Some(call) = block["tool_use_id"]
                .as_str()
                .filter(|id| sends.contains(id))
            else {
                continue;
            };
            if let Some(id) = resumed(&block["content"]).or_else(|| resumed(&row["toolUseResult"]))
            {
                result.resumed.insert(call.into(), id);
            }
        }
    }
    result
}

/// A successful native reply resolves abbreviated destinations to a full ID.
/// Rewrite only that call's destination; never substitute in user text.
pub(in crate::transfer) fn resolve(row: &mut Value, links: &Links) {
    for block in row["message"]["content"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        if block["type"] == "tool_use" && block["name"] == "SendMessage" {
            if let Some(id) = block["id"]
                .as_str()
                .and_then(|call| links.resumed.get(call))
            {
                block["input"]["to"] = id.clone().into();
            }
        }
    }
}
