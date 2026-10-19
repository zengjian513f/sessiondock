//! Projection of observed native messages, not persisted history or an oracle.
use super::*;

fn text(value: &Value) -> String {
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

fn upsert(live: &mut Live, row: Value) {
    if let Some(existing) = live.items.iter_mut().find(|item| item["id"] == row["id"]) {
        // A late full assistant frame must not erase an observed tool result.
        // Claude can emit that full frame before content_block_stop; a later
        // stream bookkeeping event must not erase its native JSONL identity.
        if existing["native_id"].is_string() && row["native_id"].is_null() {
            return;
        }
        let result = existing.get("result").cloned();
        let request_id = existing.get("request_id").cloned();
        *existing = row;
        if let Some(result) = result {
            existing["result"] = result;
        }
        if let Some(request_id) = request_id {
            existing["request_id"] = request_id;
        }
    } else {
        live.items.push(row);
    }
}

fn block_row(message: &str, index: u64, block: &Value, native_id: Option<&str>) -> Value {
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

pub(super) fn observe(session: &Session, frame: Value) {
    let mut live = lock(&session.live);
    live.idle_since = None;
    // A process is tied to one exact SID; don't let a foreign frame's content
    // or approval cross into the selected native session.
    if frame["session_id"]
        .as_str()
        .is_some_and(|sid| !sid.is_empty() && sid != session.sid)
    {
        live.error =
            Some("claude_sdk: native stream session_id differs from selected session".into());
        live.error_kind = Some("transport");
        if let Some(stop) = &live.runtime_stop {
            stop.cancel();
        }
        session.publish(&mut live);
        return;
    }
    for field in ["fast_mode_state", "fast_mode_disabled_reason"] {
        if let Some(value) = frame.get(field) {
            live.settings[field] = value.clone();
        }
    }
    match frame["type"].as_str().unwrap_or("") {
        "control_response" => {
            let response = &frame["response"];
            if let Some(id) = response["request_id"].as_str()
                && let Some(waiter) = live.controls.remove(id)
            {
                let result = if response["subtype"] == "error" {
                    Err(format!(
                        "claude_sdk_rejected: {}",
                        response["error"]
                            .as_str()
                            .unwrap_or("native control rejected")
                    ))
                } else {
                    Ok(response
                        .get("response")
                        .cloned()
                        .unwrap_or_else(|| json!({})))
                };
                let _ = waiter.send(result);
            }
            return;
        }
        "control_request" => {
            let Some(key) = frame["request_id"].as_str() else {
                return;
            };
            let native = &frame["request"];
            if native["subtype"] == "can_use_tool" {
                let question = native["tool_name"] == "AskUserQuestion";
                let name = native["tool_name"].as_str().unwrap_or("tool");
                let title = native["title"].as_str().unwrap_or(name);
                let description = native["description"]
                    .as_str()
                    .or_else(|| native["decision_reason"].as_str());
                let mut card = json!({"key":key,"type":if question {"question"} else {"approval"},
                    "title":title,"description":description,"tool_name":name,"input":native["input"],
                    "tool_use_id":native["tool_use_id"],"native":native});
                if question {
                    card["questions"] = native["input"]["questions"].clone();
                } else {
                    card["options"] = json!([{"label":"允许本次","value":"allow"},{"label":"拒绝","value":"deny"}]);
                }
                live.requests.insert(key.into(), card);
            } else if native["subtype"] == "elicitation" {
                let card = json!({"key":key,"type":"elicitation",
                    "title":native.get("title").unwrap_or(&native["mcp_server_name"]),
                    "description":native["description"],"message":native["message"],
                    "mode":native.get("mode").cloned().unwrap_or(json!("form")),
                    "requested_schema":native["requested_schema"],"property_order":native["property_order"],
                    "url":native["url"],"elicitation_id":native["elicitation_id"],"native":native});
                live.requests.insert(key.into(), card);
            } else if native["subtype"] == "request_user_dialog" {
                if native["dialog_kind"] == "refusal_fallback_prompt" {
                    let payload = &native["payload"];
                    live.requests.insert(key.into(), json!({"key":key,"type":"user_dialog",
                        "dialog_kind":native["dialog_kind"],"title":"Claude 模型拒绝",
                        "description":payload["guidanceText"],"payload":payload,"native":native,
                        "options":[{"label":"使用备用模型重试","value":"retry_fallback"},
                            {"label":"修改输入","value":"edit_prompt"},{"label":"取消","value":"cancelled"}]}));
                } else {
                    // Exact per-kind renderer opt-in: never settle an unknown
                    // kind as a real user dismissal.
                    live.items.push(json!({"id":format!("unsupported:{key}"),"role":"system",
                        "content":format!("Unsupported Claude dialog kind: {}",native["dialog_kind"]),
                        "text":format!("Unsupported Claude dialog kind: {}",native["dialog_kind"]),"error":true}));
                }
            } else {
                if let Some(sender) = &live.outbound {
                    let _=sender.send(json!({"type":"control_response","response":{"subtype":"error","request_id":key,
                        "error":"SessionDock does not implement this Claude native control request"}}));
                }
                live.items.push(json!({"id":format!("unsupported:{key}"),"role":"system",
                    "content":format!("Unsupported Claude request: {}",native["subtype"]),
                    "text":format!("Unsupported Claude request: {}",native["subtype"]),"error":true}));
            }
        }
        "control_cancel_request" => {
            if let Some(key) = frame["request_id"].as_str()
                && let Some(card) = live.requests.remove(key)
            {
                retire_dialog(&mut live, &card);
            }
        }
        "assistant" => {
            if !live.state_observed {
                live.native_running = true;
            }
            let message = &frame["message"];
            let id = message["id"]
                .as_str()
                .or_else(|| frame["uuid"].as_str())
                .unwrap_or("assistant");
            usage::assistant(&mut live, id, &message["usage"]);
            if frame["context_usage"].is_object() {
                usage::context(
                    &mut live,
                    &frame["context_usage"],
                    "native /context summary",
                );
            }
            if let Some(model) = message["model"].as_str() {
                live.model = Some(model.into());
            }
            if let Some(blocks) = message["content"].as_array() {
                for (index, block) in blocks.iter().enumerate() {
                    // Claude emits complete assistant frames per block. A
                    // one-block frame still belongs to its observed native
                    // streaming index, rather than always replacing index 0.
                    let index = if blocks.len() == 1 && id == live.streaming_message {
                        live.streaming_blocks
                            .iter()
                            .rev()
                            .find(|(_, streamed)| streamed["type"] == block["type"])
                            .map(|(index, _)| *index)
                            .unwrap_or(index as u64)
                    } else {
                        index as u64
                    };
                    let mut row = block_row(id, index, block, frame["uuid"].as_str());
                    if let Some(parent) = frame.get("parent_tool_use_id") {
                        row["parent_tool_use_id"] = parent.clone();
                    }
                    upsert(&mut live, row);
                }
            }
            if let Some(error) = frame.get("error").filter(|v| !v.is_null()) {
                live.error = Some(text(error));
                live.error_kind = Some("turn");
            }
        }
        "user" => {
            let message = &frame["message"];
            if let Some(id) = frame["uuid"].as_str()
                && let Some(row) = live.items.iter_mut().find(|row| {
                    row["role"] == "user" && (row["id"] == id || row["native_id"] == id)
                })
            {
                row["native_echo"] = json!(true);
            }
            if let Some(blocks) = message["content"].as_array() {
                for (index, block) in blocks.iter().enumerate() {
                    if block["type"] == "tool_result" {
                        let call = block["tool_use_id"].as_str().unwrap_or("");
                        let output = text(&block["content"]);
                        let result = json!({"role":"tool_result","text":output,"content":output,"call_id":call,
                            "error":block["is_error"].as_bool().unwrap_or(false),"native_block":block});
                        if let Some(tool) = live
                            .items
                            .iter_mut()
                            .find(|row| row["role"] == "tool" && row["call_id"] == call)
                        {
                            tool["result"] = result;
                        } else {
                            upsert(
                                &mut live,
                                json!({"id":format!("tool-result:{call}:{index}"),"role":"tool",
                            "call_id":call,"name":"tool result","text":"","content":"","result":result}),
                            );
                        }
                    } else if block["type"] == "text" {
                        let id = frame["uuid"].as_str().unwrap_or("native-user");
                        let content = text(&block["text"]);
                        let duplicate = live.items.iter().any(|row| row["id"] == id);
                        if !duplicate {
                            upsert(
                                &mut live,
                                json!({"id":format!("{id}:{index}"),"native_id":id,"native_echo":true,"role":"user","text":content,"content":content}),
                            );
                        }
                    }
                }
            } else if let Some(content) = message["content"].as_str() {
                let id = frame["uuid"].as_str().unwrap_or("native-user");
                upsert(
                    &mut live,
                    json!({"id":id,"native_id":id,"native_echo":true,"role":"user","content":content,"text":content}),
                );
            }
        }
        "stream_event" => {
            if !live.state_observed {
                live.native_running = true;
            }
            let event = &frame["event"];
            match event["type"].as_str().unwrap_or("") {
                "message_start" => {
                    live.retry = Value::Null;
                    live.streaming_message =
                        event["message"]["id"].as_str().unwrap_or("stream").into();
                    live.streaming_blocks.clear();
                    if let Some(model) = event["message"]["model"].as_str() {
                        live.model = Some(model.into());
                    }
                }
                "content_block_start" => {
                    if let Some(index) = event["index"].as_u64() {
                        let block = event["content_block"].clone();
                        let row = block_row(&live.streaming_message, index, &block, None);
                        live.streaming_blocks.insert(index, block);
                        upsert(&mut live, row);
                    }
                }
                "content_block_delta" => {
                    if let Some(index) = event["index"].as_u64() {
                        let message = live.streaming_message.clone();
                        if let Some(block) = live.streaming_blocks.get_mut(&index) {
                            let delta = &event["delta"];
                            let (field, part) = match delta["type"].as_str().unwrap_or("") {
                                "text_delta" => ("text", delta["text"].as_str()),
                                "thinking_delta" => ("thinking", delta["thinking"].as_str()),
                                "input_json_delta" => {
                                    ("partial_json", delta["partial_json"].as_str())
                                }
                                "signature_delta" => ("signature", delta["signature"].as_str()),
                                _ => ("", None),
                            };
                            if let Some(part) = part {
                                let mut next = block[field].as_str().unwrap_or("").to_owned();
                                next.push_str(part);
                                block[field] = json!(next);
                                let mut row = block_row(&message, index, block, None);
                                if field == "partial_json" {
                                    row["text"] = json!(next);
                                    row["content"] = json!(next);
                                }
                                upsert(&mut live, row);
                            }
                        }
                    }
                }
                "content_block_stop" => {
                    if let Some(index) = event["index"].as_u64() {
                        let message = live.streaming_message.clone();
                        if let Some(block) = live.streaming_blocks.get_mut(&index) {
                            if let Some(partial) = block["partial_json"].as_str()
                                && let Ok(input) = parse_json(partial)
                            {
                                block["input"] = input;
                            }
                            let row = block_row(&message, index, block, None);
                            upsert(&mut live, row);
                        }
                    }
                }
                _ => {}
            }
        }
        "result" => {
            // modelUsage/cost are native running totals, while usage is the
            // main-loop-only value for this turn. Neither is added to itself.
            usage::result(&mut live, &frame);
            if frame["usage"].is_object() {
                live.usage["last_turn"] = frame["usage"].clone();
            }
            live.retry = Value::Null;
            live.turns.pop_front();
            if !live.state_observed {
                live.native_running = false;
            }
            if frame["is_error"].as_bool() == Some(true)
                || frame["subtype"].as_str().is_some_and(|s| s != "success")
            {
                let errors = frame["errors"]
                    .as_array()
                    .filter(|v| !v.is_empty())
                    .map(|v| v.iter().map(text).collect::<Vec<_>>().join("\n"));
                let error = errors
                    .or_else(|| frame["result"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| text(&frame["subtype"]));
                live.error = Some(error.clone());
                live.error_kind = Some("turn");
                let id = frame["uuid"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("result:{}", live.seq));
                upsert(
                    &mut live,
                    json!({"id":id,"role":"system","text":error,"content":error,"error":true}),
                );
            }
        }
        "system" => {
            if frame["subtype"] == "init" {
                if let Some(cwd) = frame["cwd"].as_str()
                    && Path::new(cwd) != session.cwd
                {
                    live.error = Some(
                        "claude_sdk: native working directory differs from selected session".into(),
                    );
                    live.error_kind = Some("transport");
                    if let Some(stop) = &live.runtime_stop {
                        stop.cancel();
                    }
                }
                if let Some(model) = frame["model"].as_str() {
                    live.model = Some(model.into());
                }
                if let Some(mode) = frame.get("permissionMode") {
                    live.settings["permission_mode"] = mode.clone();
                }
            } else if frame["subtype"] == "api_retry" {
                live.retry = frame.clone();
            } else if frame["subtype"] == "status" {
                live.status = frame["status"].clone();
                if let Some(mode) = frame.get("permissionMode") {
                    live.settings["permission_mode"] = mode.clone();
                }
                if frame["status"] == "compacting" {
                    live.compact = json!({"state":"compacting"});
                }
                if let Some(result) = frame.get("compact_result") {
                    live.compact = json!({"state":result,"error":frame["compact_error"]});
                    if result == "failed" {
                        live.error = frame["compact_error"].as_str().map(str::to_owned);
                        live.error_kind = Some("turn");
                    }
                }
            } else if frame["subtype"] == "compact_boundary" {
                live.compact = json!({"state":"success","native":frame});
                live.context_usage = Value::Null;
                live.usage.as_object_mut().unwrap().remove("context");
            } else if frame["subtype"] == "background_tasks_changed" {
                live.background_tasks = frame["tasks"].as_array().cloned().unwrap_or_default();
            } else if frame["subtype"] == "task_started" {
                live.background_tasks.push(frame.clone());
            } else if frame["subtype"] == "task_notification" {
                live.background_tasks
                    .retain(|task| task["task_id"] != frame["task_id"]);
            } else if frame["subtype"] == "session_state_changed" {
                // Stream-json can continue background work after a result. The
                // native state is observed separately from accepted input count.
                live.state_observed = true;
                live.native_running =
                    frame["state"] == "running" || frame["state"] == "requires_action";
            } else if let Some(content) = frame.get("content").or_else(|| frame.get("message")) {
                let id = frame["uuid"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("system:{}", live.seq));
                let content = text(content);
                upsert(
                    &mut live,
                    json!({"id":id,"role":"system","text":content,"content":content,"native":frame}),
                );
            }
        }
        "rate_limit_event" => {
            let info = &frame["rate_limit_info"];
            let key = info["rateLimitType"].as_str().unwrap_or("current");
            if !live.rate_limits.is_object() {
                live.rate_limits = json!({});
            }
            live.rate_limits[key] = info.clone();
            live.usage["rate_limit_event"] = info.clone();
        }
        _ => {}
    }
    session.publish(&mut live);
}

/// The caller answers one native card. Permission suggestions are displayed
/// as data; no blanket allow, permanent rule update or guessed RPC response.
pub(super) fn permission_result(card: &Value, result: &Value) -> Result<Value, String> {
    if card["type"] == "user_dialog" {
        if result["behavior"] == "cancelled" {
            return Ok(json!({"behavior":"cancelled"}));
        }
        let choice = result["result"]
            .as_str()
            .ok_or("claude_sdk: dialog requires explicit result")?;
        if result["behavior"] != "completed"
            || !["retry_fallback", "edit_prompt", "cancelled"].contains(&choice)
        {
            return Err("claude_sdk: invalid refusal fallback dialog choice".into());
        }
        return Ok(json!({"behavior":"completed","result":choice}));
    }
    if card["type"] == "elicitation" {
        let action = result["action"]
            .as_str()
            .ok_or("claude_sdk: elicitation requires explicit action")?;
        if !["accept", "decline", "cancel"].contains(&action) {
            return Err("claude_sdk: invalid elicitation action".into());
        }
        let mut response = json!({"action":action});
        if action == "accept" && card["mode"] != "url" {
            if !result["content"].is_object() {
                return Err("claude_sdk: accepted form requires content object".into());
            }
            response["content"] = result["content"].clone();
        }
        return Ok(response);
    }
    let behavior = result.as_str().or_else(|| result["behavior"].as_str());
    if behavior == Some("deny") {
        return Ok(
            json!({"behavior":"deny","message":result["message"].as_str().unwrap_or("Denied by user"),"toolUseID":card["tool_use_id"]}),
        );
    }
    if card["type"] == "question" {
        let answers = result["answers"].as_object().ok_or(
            "claude_sdk: AskUserQuestion requires result.answers keyed by native question text",
        )?;
        if answers.values().any(|v| !v.is_string()) {
            return Err("claude_sdk: native question answers must be strings".into());
        }
        let mut input = card["input"].clone();
        input["answers"] = json!(answers);
        return Ok(
            json!({"behavior":"allow","updatedInput":input,"toolUseID":card["tool_use_id"]}),
        );
    }
    if behavior == Some("allow") {
        return Ok(
            json!({"behavior":"allow","updatedInput":card["input"],"toolUseID":card["tool_use_id"]}),
        );
    }
    Err("claude_sdk: approval requires an explicit allow or deny behavior".into())
}

pub(super) fn retire_dialog(live: &mut Live, card: &Value) {
    if card["type"] == "user_dialog"
        && let Some(ids) = card["payload"]["retractedMessageUuids"].as_array()
    {
        live.items.retain(|row| {
            !ids.iter()
                .any(|id| id == &row["native_id"] || id == &row["id"])
        });
    }
}
