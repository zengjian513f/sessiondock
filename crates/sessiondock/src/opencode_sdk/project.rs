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
            json!({"files":record["files"],"delivery_state":"consumed"}),
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
        "shell" => emit(
            "",
            "tool",
            record["command"].clone(),
            json!({
            "name":"shell","tool_name":"shell","status":record["status"],
            "input":{"command":record["command"]},"native":record,
            "result":{"role":"tool_result","text":record["output"]["output"],
                "content":record["output"]["output"],"error":record["exit"].as_i64().is_some_and(|v| v != 0)},
            "output":record["output"]["output"]}),
        ),
        // Compaction belongs to the status bar. Persisted history remains
        // authoritative and is not changed by this live projection.
        "compaction" => {}
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

pub(super) fn queue(inbox: &Value) -> Value {
    json!(
        inbox["data"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|entry| {
                json!({"id":entry["id"],"native_id":entry["id"],"type":entry["type"],
            "delivery":entry["delivery"],"text":entry["payload"]["text"],
            "status":"queued","native":entry})
            })
            .collect::<Vec<_>>()
    )
}

/// Session totals come from native Session.Info, never by summing repeatedly
/// observed streaming snapshots. Context is explicitly a last-request estimate.
pub(super) fn usage(
    info: &Value,
    records: &std::collections::HashMap<String, Value>,
    catalog: &[Value],
) -> Value {
    let latest = records
        .values()
        .filter(|r| {
            (r["type"] == "assistant" && r["tokens"].is_object())
                || (r["type"] == "compaction" && r["status"] == "completed")
        })
        .max_by(|a, b| {
            let time = |r: &Value| r["time"]["created"].as_f64().unwrap_or_default();
            time(a)
                .total_cmp(&time(b))
                .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
        });
    let mut context = Value::Null;
    if let Some(record) = latest.filter(|r| r["type"] == "assistant") {
        let current_model = &info["model"];
        let native_model = &record["model"];
        if current_model["id"] == native_model["id"]
            && current_model["providerID"] == native_model["providerID"]
            && let Some(model) = catalog.iter().find(|m| {
                m["model"] == native_model["id"] && m["providerID"] == native_model["providerID"]
            })
            && let Some(max) = model["limit"]["context"].as_f64().filter(|v| *v > 0.0)
        {
            let tokens = &record["tokens"];
            let used = tokens["input"].as_f64().unwrap_or_default()
                + tokens["cache"]["read"].as_f64().unwrap_or_default()
                + tokens["cache"]["write"].as_f64().unwrap_or_default()
                + tokens["output"].as_f64().unwrap_or_default();
            context = json!({"used_tokens":used,"max_tokens":max,"percent":used/max*100.0,
                        "estimated":true,"basis":"最近一次模型请求估计（含缓存及输出）；压缩后重新计量",
                        "message_id":record["id"]});
        }
    }
    let mut tokens = info["tokens"].clone();
    if tokens.is_object() && tokens["input"].is_number() && tokens["output"].is_number() {
        tokens["total_tokens"] = json!(
            tokens["input"].as_f64().unwrap_or_default()
                + tokens["output"].as_f64().unwrap_or_default()
                + tokens["cache"]["read"].as_f64().unwrap_or_default()
                + tokens["cache"]["write"].as_f64().unwrap_or_default()
        );
    }
    json!({"tokens":tokens,"cost_usd":info["cost"],"context":context,
        "basis":"原生会话累计","quota":null})
}
