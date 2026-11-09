//! Shared Claude/CodeBuddy stream-json message and content-block projection.
use serde_json::{Value, json};

pub(crate) fn text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.into();
    }
    if let Some(blocks) = value.as_array() {
        return blocks
            .iter()
            .map(|block| {
                block["text"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| block.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    value.to_string()
}

pub(crate) fn upsert(items: &mut Vec<Value>, mut row: Value) {
    if let Some(existing) = items.iter_mut().find(|item| item["id"] == row["id"]) {
        // A late full assistant frame must not erase an observed tool result.
        // Claude can emit that full frame before content_block_stop; a later
        // stream bookkeeping event must not erase its native JSONL identity.
        if existing["native_id"].is_string() && row["native_id"].is_null() {
            return;
        }
        let result = existing.get("result").cloned();
        let request_id = existing.get("request_id").cloned();
        crate::sdk_message_time::preserve(&mut row, existing);
        *existing = row;
        if let Some(result) = result {
            existing["result"] = result;
        }
        if let Some(request_id) = request_id {
            existing["request_id"] = request_id;
        }
    } else {
        items.push(row);
    }
}

pub(crate) fn block_row(
    message: &str,
    index: u64,
    block: &Value,
    native_id: Option<&str>,
) -> Value {
    let kind = block["type"].as_str().unwrap_or("unknown");
    let (id, role, content) = match kind {
        "text" => (
            format!("assistant:{message}:{index}"),
            "assistant",
            text(&block["text"]),
        ),
        "thinking" => (
            format!("thinking:{message}:{index}"),
            "thinking",
            text(&block["thinking"]),
        ),
        "tool_use" => (
            format!("tool:{}", block["id"].as_str().unwrap_or(message)),
            "tool",
            text(&block["input"]),
        ),
        _ => (
            format!("block:{message}:{index}"),
            "system",
            block.to_string(),
        ),
    };
    let mut row = json!({"id":id,"role":role,"text":content,"content":content,"native_id":native_id,
        "message_id":message,"native_block":block});
    if kind == "tool_use" {
        row["call_id"] = block["id"].clone();
        row["name"] = block["name"].clone();
        row["input"] = block["input"].clone();
    }
    row
}
