//! Factory Droid 1.6 JSON-RPC projection. Native history remains read-only.
use super::*;

pub(super) fn request(id: &str, request: &Value) -> Value {
    json!({"jsonrpc":"2.0","type":"request","factoryApiVersion":"1.0.0",
        "id":id,"method":request["method"],"params":request["params"]})
}
pub(super) fn settings(live: &mut Live, settings: &Value) {
    if let Some(model) = settings["modelId"].as_str() {
        live.model = Some(model.into());
    }
    if let Some(effort) = settings["reasoningEffort"].as_str() {
        live.effort = Some(effort.into());
    }
    for key in ["interactionMode", "autonomyLevel"] {
        if let Some(value) = settings.get(key) {
            live.settings[key] = value.clone();
        }
    }
}
fn text(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.into();
    }
    if let Some(a) = v.as_array() {
        return a
            .iter()
            .map(|b| text(b.get("text").unwrap_or(b)))
            .collect::<Vec<_>>()
            .join("\n");
    }
    v.to_string()
}
fn upsert(live: &mut Live, mut row: Value) {
    live.message_time.stamp(&mut row);
    if let Some(old) = live.items.iter_mut().find(|r| r["id"] == row["id"]) {
        crate::sdk_message_time::preserve(&mut row, old);
        for key in ["request_id", "result"] {
            if let Some(v) = old.get(key) {
                row[key] = v.clone();
            }
        }
        *old = row;
    } else {
        live.items.push(row);
    }
}
fn message(live: &mut Live, n: &Value) {
    let m = &n["message"];
    let Some(id) = m["id"].as_str() else {
        return;
    };
    if m["content"].as_array().is_none_or(|a| a.is_empty()) {
        return;
    }
    let role = if m["visibility"] == "llm_only" {
        "system"
    } else {
        m["role"].as_str().unwrap_or("system")
    };
    if role == "user"
        && !m["content"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|b| b["type"] == "tool_result")
    {
        let request = n["requestId"].as_str().map(str::to_owned).or_else(|| {
            live.accepted
                .iter()
                .find(|(_, r)| r["message_id"] == id)
                .map(|(k, _)| k.clone())
        });
        let mut row = json!({"id":id,"native_id":id,"role":"user","text":text(&m["content"]),"content":text(&m["content"]),"native_echo":true,"timestamp":m["createdAt"]});
        if let Some(request) = request {
            row["request_id"] = json!(request);
        }
        upsert(live, row);
        return;
    }
    for (index, b) in m["content"].as_array().into_iter().flatten().enumerate() {
        let kind = b["type"].as_str().unwrap_or("unknown");
        if kind == "tool_result" {
            let tool = format!(
                "tool:{}",
                b["toolUseId"]
                    .as_str()
                    .or_else(|| b["tool_use_id"].as_str())
                    .unwrap_or("")
            );
            if let Some(row) = live.items.iter_mut().find(|r| r["id"] == tool) {
                row["result"] = json!({"text":text(&b["content"]),"error":b["isError"] == true || b["is_error"] == true});
            } else {
                upsert(
                    live,
                    json!({"id":format!("result:{id}:{index}"),"native_id":id,"role":"system","text":text(&b["content"])}),
                );
            }
            continue;
        }
        if kind == "redacted_thinking" {
            continue;
        }
        let (row_id, projected, content) = match kind {
            "text" => (format!("text:{id}:{index}"), role, text(&b["text"])),
            "thinking" => (
                format!("thinking:{id}:{index}"),
                "thinking",
                text(&b["thinking"]),
            ),
            "tool_use" => (
                format!("tool:{}", b["id"].as_str().unwrap_or(id)),
                "tool",
                text(&b["input"]),
            ),
            _ => (format!("other:{id}:{index}"), "system", b.to_string()),
        };
        let mut row = json!({"id":row_id,"native_id":id,"role":projected,"text":content,"content":content,"timestamp":m["createdAt"],"native_type":kind,"phase":"progress"});
        if kind == "tool_use" {
            row["name"] = b["name"].clone();
            row["input"] = b["input"].clone();
        }
        upsert(live, row);
    }
    if let Some(model) = m["modelId"].as_str() {
        live.model = Some(model.into());
    }
}
pub(super) fn observe(session: &Session, frame: Value) {
    observe_at(session, frame, None, Some(crate::sdk_message_time::now()));
}
pub(super) fn observe_at(
    session: &Session,
    frame: Value,
    offset: Option<u64>,
    received: Option<String>,
) {
    let mut live = lock(&session.live);
    if let Some(offset) = offset {
        live.stdout_offset = offset;
    }
    live.message_time = crate::sdk_message_time::MessageTime::new(&frame, received);
    if let Some(id) = frame["id"].as_str() {
        if frame.get("method").is_none() {
            let request = live.control_requests.remove(id);
            let result = if !frame["error"].is_null() {
                Err(text(&frame["error"]["message"]))
            } else {
                Ok(frame["result"].clone())
            };
            if let (Some(request), Ok(result)) = (request, result.as_ref()) {
                match request["method"].as_str().unwrap_or("") {
                    "droid.initialize_session" | "droid.load_session" | "droid.list_models" => {
                        settings(&mut live, &result["settings"]);
                        live.models = result
                            .get("models")
                            .or_else(|| result.get("availableModels"))
                            .cloned()
                            .unwrap_or(Value::Null);
                        normalize_models(&mut live.models);
                        if let Some(total) = result.get("tokenUsage").filter(|v| v.is_object()) {
                            live.usage = usage(total);
                        }
                        live.ready = true;
                    }
                    "droid.update_session_settings" => settings(&mut live, &request["params"]),
                    _ => {}
                }
            }
            if let Some(receipt) = live.accepted.get_mut(id) {
                receipt["native_accepted"] = json!(frame["error"].is_null());
            }
            if let Some(tx) = live.controls.remove(id) {
                let _ = tx.send(result);
            }
            if live.accepted.contains_key(id) && !frame["error"].is_null() {
                live.turns.retain(|t| t != id);
                live.native_running = false;
                live.error = Some(text(&frame["error"]["message"]));
                live.error_kind = Some("turn");
                live.last_turn = json!({"status":"failed","error":live.error});
            }
        } else {
            let method = frame["method"].as_str().unwrap_or("");
            let params = &frame["params"];
            let mut card = json!({"key":id,"method":method,"native":params});
            match method {
                "droid.ask_user" => {
                    card["type"] = json!("question");
                    card["questions"] = json!(params["questions"].as_array().into_iter().flatten().map(|q| json!({"header":q["topic"],"question":q["question"],"multiSelect":q["multiSelect"] == true,"options":q["options"].as_array().into_iter().flatten().map(|s|json!({"label":s,"description":""})).collect::<Vec<_>>()})).collect::<Vec<_>>());
                }
                "droid.request_permission" => {
                    card["type"] = json!("approval");
                    card["name"] = params["toolUses"][0]["toolUse"]["name"].clone();
                    card["input"] = params["toolUses"].clone();
                    card["options"] = json!(
                        params["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|o| matches!(
                                o["value"].as_str(),
                                Some("proceed_once" | "cancel")
                            ))
                            .cloned()
                            .collect::<Vec<_>>()
                    );
                }
                _ => {
                    card["type"] = json!("unsupported");
                }
            }
            live.requests.insert(id.into(), card);
        }
    } else if frame["method"] == "droid.session_notification" {
        let n = &frame["params"]["notification"];
        match n["type"].as_str().unwrap_or("") {
            "create_message" => message(&mut live, n),
            "tool_call" => {
                let tool = &n["toolUse"];
                upsert(
                    &mut live,
                    json!({"id":format!("tool:{}",tool["id"].as_str().unwrap_or("")),
                    "role":"tool","name":tool["name"],"input":tool["input"],"text":text(&tool["input"]),"native_type":"tool_call"}),
                );
            }
            "assistant_text_delta" | "thinking_text_delta" => {
                let thinking = n["type"] == "thinking_text_delta";
                let id = format!(
                    "{}:{}:{}",
                    if thinking { "thinking" } else { "text" },
                    n["messageId"].as_str().unwrap_or(""),
                    n["blockIndex"].as_u64().unwrap_or(0)
                );
                let old = live
                    .items
                    .iter()
                    .find(|r| r["id"] == id)
                    .map(|r| text(&r["text"]))
                    .unwrap_or_default();
                let content = old + n["textDelta"].as_str().unwrap_or("");
                upsert(
                    &mut live,
                    json!({"id":id,"native_id":n["messageId"],"role":if thinking {"thinking"} else {"assistant"},"text":content,"content":content,"phase":"progress"}),
                );
            }
            "settings_updated" => settings(&mut live, &n["settings"]),
            "permission_resolved" => {
                if let Some(id) = n["requestId"].as_str() {
                    live.requests.remove(id);
                }
            }
            "agent_turn_completed" => {
                live.turns.clear();
                live.native_running = false;
                let reason = n["reason"].as_str().unwrap_or("unknown");
                let status = match reason {
                    "completed" => "completed",
                    "cancelled" | "interrupted" => "interrupted",
                    "spec_handoff" => "incomplete",
                    _ => "failed",
                };
                live.last_turn = json!({"status":status,"reason":reason,"error":live.error});
                if let Some(row) = live
                    .items
                    .iter_mut()
                    .rev()
                    .find(|r| r["role"] == "assistant")
                {
                    row["phase"] = json!("final");
                }
                if let Some(total) = n.get("cumulativeTokenUsage").filter(|v| v.is_object()) {
                    live.usage = usage(total);
                }
            }
            "session_token_usage_changed" => live.usage = usage(&n["tokenUsage"]),
            "error" => {
                live.error = Some(text(&n["message"]));
                live.error_kind = Some("turn");
            }
            "droid_working_state_changed" => {
                live.status = n["newState"].clone();
            }
            "assistant_message_retracted" => {
                live.items.retain(|r| r["native_id"] != n["messageId"])
            }
            "session_compacted" => live.compact = n.clone(),
            "tool_result" => {
                let id = format!("tool:{}", n["toolUseId"].as_str().unwrap_or(""));
                if let Some(row) = live.items.iter_mut().find(|r| r["id"] == id) {
                    row["result"] =
                        json!({"text":text(&n["content"]),"error":n["isError"] == true});
                } else {
                    upsert(
                        &mut live,
                        json!({"id":format!("result:{}",n["messageId"].as_str().unwrap_or(&id)),
                        "native_id":n["messageId"],"role":"system","text":text(&n["content"]),"native_type":"tool_result"}),
                    );
                }
            }
            _ => {}
        }
    }
    if matches!(
        frame["params"]["notification"]["type"].as_str(),
        Some("assistant_text_delta" | "thinking_text_delta")
    ) {
        session.publish_throttled(&mut live);
    } else {
        session.publish(&mut live);
    }
}
fn usage(v: &Value) -> Value {
    // Factory reports uncached input separately from cache reads and writes.
    // Thinking is already represented in output, so do not count it twice.
    let total = [
        "inputTokens",
        "outputTokens",
        "cacheReadTokens",
        "cacheCreationTokens",
    ]
    .into_iter()
    .map(|key| v[key].as_u64())
    .collect::<Option<Vec<_>>>()
    .map(|parts| parts.into_iter().sum::<u64>());
    json!({"basis":"原生会话累计", "credits":v["factoryCredits"],
        "tokens":{"total_tokens":total,"input":v["inputTokens"],"output":v["outputTokens"],"reasoning":v["thinkingTokens"],
        "cache":{"read":v["cacheReadTokens"],"write":v["cacheCreationTokens"]}}})
}
pub(super) fn restore_initial_requests(_session: &Session) {}
pub(super) fn retire_dialog(_live: &mut Live, _card: &Value) {}
pub(super) fn permission_result(card: &Value, result: &Value) -> Result<Value, String> {
    if card["method"] == "droid.ask_user" {
        if result["behavior"] == "deny" {
            return Ok(json!({"cancelled":true,"answers":[]}));
        }
        let answers = card["native"]["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|q| {
                let answer = &result["answers"][q["question"].as_str().unwrap_or("")];
                json!({"index":q["index"],"question":q["question"],"answer":text(answer)})
            })
            .collect::<Vec<_>>();
        return Ok(json!({"answers":answers}));
    }
    let selected = result
        .as_str()
        .or_else(|| result["selectedOption"].as_str())
        .or_else(|| result["option"].as_str())
        .or_else(|| result["value"].as_str())
        .unwrap_or_else(|| {
            if result["behavior"] == "allow" {
                "proceed_once"
            } else {
                "cancel"
            }
        });
    if !card["native"]["options"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|o| o["value"] == selected)
    {
        return Err("droid_sdk: native permission option unavailable".into());
    }
    Ok(json!({"selectedOption":selected}))
}
