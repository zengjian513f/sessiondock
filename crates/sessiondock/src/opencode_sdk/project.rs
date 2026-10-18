//! Project native message/form shapes without rewriting native records.
use serde_json::{Value, json};

pub(super) fn messages(record: &Value) -> Vec<Value> {
    let id = record["id"].as_str().unwrap_or_default();
    let mut output = Vec::new();
    let mut emit = |suffix: &str, role: &str, text: Value, extra: Value| {
        let mut item = json!({"id":format!("{id}{suffix}"),"native_id":id,"role":role,
            "content":text,"text":text,"timestamp":record["time"]["created"]});
        if let Some(extra) = extra.as_object() {
            item.as_object_mut().unwrap().extend(extra.clone());
        }
        output.push(item);
    };
    match record["type"].as_str().unwrap_or_default() {
        "user" => emit(
            "",
            "user",
            record["text"].clone(),
            json!({"files":record["files"]}),
        ),
        "assistant" => {
            if let Some(parts) = record["content"].as_array() {
                let last_text = parts.iter().rposition(|p| p["type"] == "text");
                for (index, part) in parts.iter().enumerate() {
                    let suffix = format!(":{index}");
                    match part["type"].as_str().unwrap_or_default() {
                        "text" | "reasoning" => emit(
                            &suffix,
                            if part["type"] == "reasoning" {
                                "thinking"
                            } else {
                                "assistant"
                            },
                            part["text"].clone(),
                            json!({"phase":if record["finish"]=="stop" && Some(index)==last_text {"final"} else {"progress"},
                                "interrupted":record["error"]["type"]=="aborted"}),
                        ),
                        "tool" => {
                            let state = &part["state"];
                            let failed = state["status"] == "error";
                            let result = if state["status"] == "completed" || failed {
                                let text = if failed {
                                    display(&state["error"])
                                } else {
                                    state["content"]
                                        .as_array()
                                        .map(|items| {
                                            items
                                                .iter()
                                                .map(|p| {
                                                    p["text"]
                                                        .as_str()
                                                        .map(str::to_owned)
                                                        .unwrap_or_else(|| display(p))
                                                })
                                                .collect::<Vec<_>>()
                                                .join("\n")
                                        })
                                        .unwrap_or_else(|| display(&state["content"]))
                                };
                                json!({"role":"tool_result","text":text,"content":state["content"],"error":failed,"call_id":part["id"]})
                            } else {
                                Value::Null
                            };
                            emit(
                                &suffix,
                                "tool",
                                json!(display(&state["input"])),
                                json!({"name":part["name"],"tool_name":part["name"],"tool_use_id":part["id"],
                                    "call_id":part["id"],"summary":part["name"],
                                    "input":state["input"],"arguments":state["input"],"result":result,"output":state["content"],
                                    "error":state["error"],"is_error":state["status"]=="error",
                                    "status":state["status"],"native":part}),
                            );
                        }
                        _ => {}
                    }
                }
            }
            if record["finish"] == "error" && record["error"]["type"] != "aborted" {
                emit(
                    ":error",
                    "system",
                    record["error"]["message"].clone(),
                    json!({"error":record["error"]}),
                );
            }
        }
        "idle" if record["outcome"] == "failed" => emit(
            "",
            "system",
            json!("OpenCode 本轮失败"),
            json!({"error":true}),
        ),
        "system" => emit("", "system", record["text"].clone(), json!({})),
        _ => {}
    }
    output
}

fn display(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        if value.is_null() {
            String::new()
        } else {
            value.to_string()
        }
    })
}

pub(super) fn requests(permissions: &Value, forms: &Value) -> Vec<Value> {
    let mut output = Vec::new();
    for permission in permissions["data"].as_array().into_iter().flatten() {
        if let Some(id) = permission["id"].as_str() {
            output.push(json!({"key":format!("permission:{id}"),"type":"approval",
                "title":permission["action"],"description":permission["message"],
                "resources":permission["resources"],"native":permission,
                "options":[{"label":"允许一次","value":"once"},{"label":"始终允许","value":"always"},
                    {"label":"拒绝","value":"reject"}]}));
        }
    }
    for form in forms["data"].as_array().into_iter().flatten() {
        if let Some(id) = form["id"].as_str() {
            let fields = form["fields"].as_array();
            output.push(json!({"key":format!("form:{id}"),"type":"question","title":form["title"],
                "fields":form["fields"],"options":fields.and_then(|f|f.first()).map(|f|f["options"].clone()),
                "native":form,"answer_format":"object keyed by native field key"}));
        }
    }
    output
}
