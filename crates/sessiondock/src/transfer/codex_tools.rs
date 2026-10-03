//! Native JSON agent tools only. Code-mode input and arbitrary output text need
//! their own adapter and are never rewritten by substring replacement.
use super::{TransferError, codex::IdentityMap};
use serde_json::Value;

fn thread(v: &mut Value, map: &IdentityMap) -> Result<(), TransferError> {
    if let Some(id) = v.as_str() {
        *v = Value::String(map.threads.get(id).cloned().ok_or_else(|| {
            TransferError::new(
                "move_group_incomplete",
                format!("工具引用未映射的子代理: {id}"),
            )
        })?);
    }
    Ok(())
}

pub(super) fn rewrite(row: &mut Value, map: &IdentityMap) -> Result<(), TransferError> {
    super::code_mode::rewrite(row, map)?;
    if row["type"] != "response_item" {
        return Ok(());
    }
    let p = &mut row["payload"];
    let Some(tool) = p["call_id"].as_str().and_then(|id| map.tool_calls.get(id)) else {
        return Ok(());
    };
    if !matches!(
        tool.as_str(),
        "spawn_agent" | "send_input" | "wait" | "close_agent" | "resume_agent"
    ) {
        return Ok(());
    }
    let key = match p["type"].as_str() {
        Some("function_call") => "arguments",
        Some("function_call_output") => "output",
        _ => return Ok(()),
    };
    if key == "output" {
        return output(&mut p[key], tool, map);
    }
    let text = p[key].as_str().ok_or_else(|| {
        TransferError::new(
            "move_reference_unsupported",
            "子代理工具参数不是 JSON 字符串",
        )
    })?;
    let mut value: Value = serde_json::from_str(text)
        .map_err(|_| TransferError::new("move_reference_unsupported", "子代理工具参数不是 JSON"))?;
    rewrite_value(&mut value, tool, true, map)?;
    p[key] = Value::String(serde_json::to_string(&value)?);
    Ok(())
}

/// Native function outputs are either text or typed content items. Keep the
/// wire envelope and non-text items intact; only parsed agent identity slots
/// may change. Plain error/status text is opaque, including quoted UUIDs.
fn output(value: &mut Value, tool: &str, map: &IdentityMap) -> Result<(), TransferError> {
    match value {
        Value::String(text) => {
            let mut parsed = match serde_json::from_str::<Value>(text) {
                Ok(v) => v,
                Err(_) => return Ok(()),
            };
            rewrite_value(&mut parsed, tool, false, map)?;
            *text = serde_json::to_string(&parsed)?;
        }
        Value::Array(items) => {
            for item in items {
                match item["type"].as_str() {
                    Some("input_text") if item["text"].is_string() => {
                        output(&mut item["text"], tool, map)?
                    }
                    Some("input_image" | "input_audio" | "encrypted_content") => {}
                    _ => {
                        return Err(TransferError::new(
                            "move_reference_unsupported",
                            "子代理工具结果包含无法识别的内容项",
                        ));
                    }
                }
            }
        }
        _ => {
            return Err(TransferError::new(
                "move_reference_unsupported",
                "子代理工具结果不是文本或原生内容数组",
            ));
        }
    }
    Ok(())
}

fn rewrite_value(
    value: &mut Value,
    tool: &str,
    arguments: bool,
    map: &IdentityMap,
) -> Result<(), TransferError> {
    if arguments {
        if matches!(tool, "send_input" | "close_agent" | "resume_agent")
            && let Some(id) = value.get_mut("id")
        {
            thread(id, map)?;
        }
        if tool == "wait"
            && let Some(ids) = value.get_mut("ids").and_then(Value::as_array_mut)
        {
            for id in ids {
                thread(id, map)?;
            }
        }
    } else {
        if tool == "spawn_agent"
            && let Some(id) = value.get_mut("agent_id")
        {
            thread(id, map)?;
        }
        if tool == "wait"
            && let Some(statuses) = value.get_mut("status").and_then(Value::as_object_mut)
        {
            let mut next = serde_json::Map::new();
            for (id, status) in std::mem::take(statuses) {
                let mut id = Value::String(id);
                thread(&mut id, map)?;
                next.insert(id.as_str().unwrap().to_owned(), status);
            }
            *statuses = next;
        }
    }
    Ok(())
}

/// Report a concrete unsupported reference without copying potentially sensitive
/// tool input into diagnostics. The staging manifest is never a publish permit.
pub(super) fn audit(row: &Value, map: &IdentityMap) -> Option<String> {
    if row["type"] != "response_item" {
        return None;
    }
    let p = &row["payload"];
    if (p["type"] == "custom_tool_call" && p["name"] == "exec")
        || p["type"] == "custom_tool_call_output"
    {
        return super::code_mode::rewrite(&mut row.clone(), map)
            .err()
            .map(|e| e.message);
    }
    // Shell commands, MCP payloads and other tool prose may quote a UUID.
    // Only typed native identity slots are relationships; do not reject or
    // rewrite an unrelated tool's text because it happens to contain one.
    None
}

/// Native agent references create graph edges even if the UI has no parent link.
pub(super) fn references(
    row: &Value,
    calls: &std::collections::BTreeMap<String, String>,
) -> Vec<String> {
    let mut refs = Vec::new();
    let _ = super::codex_ids::visit(&mut row.clone(), &mut |kind, id| {
        if matches!(kind, super::codex_ids::Identity::Thread) {
            refs.push(id.to_owned());
        }
        Ok(id.into())
    });
    if row["type"] != "response_item" {
        return refs;
    }
    let p = &row["payload"];
    if p["type"] == "custom_tool_call"
        && p["name"] == "exec"
        && let Some(code) = p["input"].as_str()
    {
        refs.extend(super::code_mode::references(code));
    }
    if matches!(
        p["type"].as_str(),
        Some("custom_tool_call_output" | "function_call_output")
    ) && p["call_id"]
        .as_str()
        .and_then(|id| calls.get(id))
        .is_some_and(|name| matches!(name.as_str(), "__code_agent" | "spawn_agent" | "wait"))
    {
        refs.extend(super::code_mode::result_references(&p["output"]));
    }
    if p["type"] == "function_call"
        && (p["namespace"].is_null() || p["namespace"] == "functions")
        && let Some(args) = p["arguments"]
            .as_str()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
    {
        match p["name"].as_str() {
            Some("send_input" | "close_agent" | "resume_agent") => {
                if let Some(id) = args["id"].as_str() {
                    refs.push(id.into());
                }
            }
            Some("wait") => {
                if let Some(ids) = args["ids"].as_array() {
                    refs.extend(ids.iter().filter_map(Value::as_str).map(str::to_owned));
                }
            }
            _ => {}
        }
    }
    refs
}

/// Associate outputs with native agent calls before deriving graph edges. An
/// external tool may legitimately return an unrelated field named agent_id.
pub(super) fn collect_call(row: &Value, calls: &mut std::collections::BTreeMap<String, String>) {
    if row["type"] != "response_item" {
        return;
    }
    let p = &row["payload"];
    let Some(id) = p["call_id"].as_str() else {
        return;
    };
    if p["type"] == "custom_tool_call"
        && p["name"] == "exec"
        && p["input"]
            .as_str()
            .is_some_and(super::code_mode::agent_call)
    {
        calls.insert(id.into(), "__code_agent".into());
    } else if p["type"] == "function_call"
        && (p["namespace"].is_null() || p["namespace"] == "functions")
        && let Some(name) = p["name"].as_str().filter(|name| {
            matches!(
                *name,
                "spawn_agent" | "wait" | "send_input" | "resume_agent" | "close_agent"
            )
        })
    {
        calls.insert(id.into(), name.into());
    }
}
