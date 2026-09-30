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
    let Some(text) = p[key].as_str() else {
        return Err(TransferError::new(
            "move_reference_unsupported",
            "子代理工具不是已适配的 JSON 字符串格式",
        ));
    };
    let mut value: Value = serde_json::from_str(text).map_err(|_| {
        TransferError::new(
            "move_reference_unsupported",
            "子代理工具参数或结果不是 JSON",
        )
    })?;
    if key == "arguments" {
        if matches!(tool.as_str(), "send_input" | "close_agent" | "resume_agent") {
            if let Some(id) = value.get_mut("id") {
                thread(id, map)?;
            }
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
    p[key] = Value::String(serde_json::to_string(&value)?);
    Ok(())
}

/// Report a concrete unsupported reference without copying potentially sensitive
/// tool input into diagnostics. The staging manifest is never a publish permit.
pub(super) fn audit(row: &Value, map: &IdentityMap) -> Option<String> {
    if row["type"] != "response_item" {
        return None;
    }
    let p = &row["payload"];
    if p["type"] == "custom_tool_call" && p["name"] == "exec" {
        return Some("code-mode exec 中的代码和工具引用尚未适配".into());
    }
    let tool = p["call_id"].as_str().and_then(|id| map.tool_calls.get(id));
    if tool.is_some_and(|name| {
        matches!(
            name.as_str(),
            "spawn_agent" | "send_input" | "wait" | "close_agent" | "resume_agent"
        )
    }) {
        return None;
    }
    if matches!(
        p["type"].as_str(),
        Some(
            "function_call"
                | "function_call_output"
                | "custom_tool_call"
                | "custom_tool_call_output"
        )
    ) {
        for key in ["arguments", "input", "output"] {
            if let Some(value) = p.get(key) {
                let text = value.to_string();
                if map.threads.keys().any(|id| text.contains(id)) {
                    return Some(format!("未适配的工具 {key} 含组内身份引用"));
                }
            }
        }
    }
    None
}
