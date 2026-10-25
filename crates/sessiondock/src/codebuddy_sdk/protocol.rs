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
    observe_at(session, frame, None);
}

pub(super) fn observe_at(session: &Session, frame: Value, offset: Option<u64>) {
    // Native SessionStart hooks run before --resume loads its persisted SID.
    // These unprojected startup notifications carry a temporary process UUID;
    // discard their telemetry without accepting any of its content or identity.
    if frame["type"] == "system"
        && frame["hook_event"] == "SessionStart"
        && matches!(
            frame["subtype"].as_str(),
            Some("hook_started" | "hook_response")
        )
    {
        if let Some(offset) = offset {
            let mut live = lock(&session.live);
            live.stdout_offset = offset;
            if let Err(error) = live.save_checkpoint() {
                live.last_checkpoint_error = Some(format!("codebuddy_sdk_checkpoint: {error}"));
            } else {
                live.last_checkpoint_error = None;
            }
        }
        return;
    }
    let is_delta =
        frame["type"] == "stream_event" && frame["event"]["type"] == "content_block_delta";
    let mut live = lock(&session.live);
    if let Some(offset) = offset {
        live.stdout_offset = offset;
    }
    // CodeBuddy replays the whole persisted conversation on stdout after
    // `initialize` when it resumes a session. History belongs to the read
    // model, so those frames must not enter the live projection. The replay
    // precedes the first live run, which announces itself with its own
    // `system/init` once an accepted turn is being processed.
    if live.replaying {
        let live_run = frame["type"] == "system"
            && frame["subtype"] == "init"
            && !live.turns.is_empty()
            && frame["model"] != "unknown";
        let control = matches!(
            frame["type"].as_str(),
            Some("control_response" | "control_request" | "control_cancel_request")
        );
        if live_run {
            live.replaying = false;
        } else if !control {
            return;
        }
    }
    // A process is tied to one exact SID; don't let a foreign frame's content
    // or approval cross into the selected native session.
    if frame["session_id"]
        .as_str()
        .is_some_and(|sid| !sid.is_empty() && sid != session.sid)
    {
        live.error =
            Some("codebuddy_sdk: native stream session_id differs from selected session".into());
        live.error_kind = Some("transport");
        if let Some(stop) = &live.runtime_stop {
            stop.cancel();
        }
        session.publish(&mut live);
        return;
    }
    match frame["type"].as_str().unwrap_or("") {
        "control_response" => {
            let response = &frame["response"];
            if let Some(request) = response["request_id"]
                .as_str()
                .and_then(|id| live.control_requests.remove(id))
                && response["subtype"] != "error"
            {
                match request["subtype"].as_str() {
                    Some("initialize") => {
                        let initialized = &response["response"];
                        live.models = initialized["models"].clone();
                        normalize_models(&mut live.models);
                        live.ready = true;
                    }
                    Some("set_model") => live.model = request["model"].as_str().map(str::to_owned),
                    Some("interrupt") => {
                        live.interrupt_requested = true;
                        // A result can precede its control response. Record the
                        // explicit native acknowledgement without claiming work
                        // has stopped while native running evidence remains.
                        live.last_turn = json!({"status":"interrupted"});
                    }
                    Some("set_permission_mode") => {
                        live.settings["permission_mode"] = request["mode"].clone()
                    }
                    Some("set_config") => {
                        if let Some(value) = request["config"]["reasoningEffort"].as_str() {
                            live.effort = Some(value.into());
                        }
                    }
                    _ => {}
                }
            }
            if let Some(id) = response["request_id"].as_str()
                && let Some(waiter) = live.controls.remove(id)
            {
                let result = if response["subtype"] == "error" {
                    Err(format!(
                        "codebuddy_sdk_rejected: {}",
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
            session.publish(&mut live);
            drop(live);
            restore_initial_requests(session);
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
            } else {
                if let Some(sender) = &live.outbound {
                    let _=sender.send(json!({"type":"control_response","response":{"subtype":"error","request_id":key,
                        "error":"SessionDock does not implement this CodeBuddy native control request"}}));
                }
                live.items.push(json!({"id":format!("unsupported:{key}"),"role":"system",
                    "content":format!("Unsupported CodeBuddy request: {}",native["subtype"]),
                    "text":format!("Unsupported CodeBuddy request: {}",native["subtype"]),"error":true}));
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
                        // CodeBuddy does not keep the client's frame uuid, so the
                        // oldest accepted input with this exact text is its echo.
                        let pending = live.items.iter_mut().find(|row| {
                            row["role"] == "user"
                                && row["native_echo"] != true
                                && row["request_id"].is_string()
                                && row["text"] == content
                        });
                        if let Some(row) = pending {
                            row["native_id"] = json!(id);
                            row["native_echo"] = json!(true);
                        } else if !live.items.iter().any(|row| row["native_id"] == id) {
                            upsert(
                                &mut live,
                                json!({"id":format!("{id}:{index}"),"native_id":id,"native_echo":true,"role":"user","text":content,"content":content}),
                            );
                        }
                    }
                }
            } else if let Some(content) = message["content"].as_str() {
                let id = frame["uuid"].as_str().unwrap_or("native-user");
                let pending = live.items.iter_mut().find(|row| {
                    row["role"] == "user"
                        && row["native_echo"] != true
                        && row["request_id"].is_string()
                        && row["text"] == content
                });
                if let Some(row) = pending {
                    row["native_id"] = json!(id);
                    row["native_echo"] = json!(true);
                } else if !live.items.iter().any(|row| row["native_id"] == id) {
                    upsert(
                        &mut live,
                        json!({"id":id,"native_id":id,"native_echo":true,"role":"user","content":content,"text":content}),
                    );
                }
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
                                if let Some(Value::String(s)) = block.get_mut(field) {
                                    s.push_str(part);
                                } else {
                                    block[field] = json!(part);
                                }
                                let mut row = block_row(&message, index, block, None);
                                if field == "partial_json" {
                                    let next = block[field].clone();
                                    row["text"] = next.clone();
                                    row["content"] = next;
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
            live.last_turn = json!({"status":if live.interrupt_requested {
                "interrupted"
            } else if frame["is_error"] == true || frame["subtype"].as_str().is_some_and(|s| s != "success") {
                "failed"
            } else {"completed"},"id":frame["uuid"]});
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
                live.error_kind = Some(if live.interrupt_requested {
                    "interrupted"
                } else {
                    "turn"
                });
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
                        "codebuddy_sdk: native working directory differs from selected session"
                            .into(),
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
    if is_delta {
        session.publish_throttled(&mut live);
    } else {
        session.publish(&mut live);
    }
}

/// The caller answers one native card. CodeBuddy's `can_use_tool` reply is
/// `{allowed,updatedInput,tool_use_id}` / `{allowed:false,reason,interrupt,tool_use_id}`
/// (not Claude's `behavior`). No blanket allow or permanent rule is created.
pub(super) fn permission_result(card: &Value, result: &Value) -> Result<Value, String> {
    let behavior = result.as_str().or_else(|| result["behavior"].as_str());
    if behavior == Some("deny") {
        return Ok(json!({"allowed":false,
            "reason":result["message"].as_str().unwrap_or("Denied by user"),
            "interrupt":result["interrupt"].as_bool().unwrap_or(false),
            "tool_use_id":card["tool_use_id"]}));
    }
    if card["type"] == "question" {
        let answers = result["answers"].as_object().ok_or(
            "codebuddy_sdk: AskUserQuestion requires result.answers keyed by native question text",
        )?;
        if answers.values().any(|v| !v.is_string()) {
            return Err("codebuddy_sdk: native question answers must be strings".into());
        }
        let mut input = card["input"].clone();
        input["answers"] = json!(answers);
        return Ok(json!({"allowed":true,"updatedInput":input,"tool_use_id":card["tool_use_id"]}));
    }
    if behavior == Some("allow") {
        return Ok(
            json!({"allowed":true,"updatedInput":card["input"],"tool_use_id":card["tool_use_id"]}),
        );
    }
    Err("codebuddy_sdk: approval requires an explicit allow or deny behavior".into())
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

// The initialize response and its pending requests share a durable checkpoint;
// interruption during projection leaves the remaining raw frames recoverable.
pub(super) fn restore_initial_requests(session: &Session) {
    loop {
        let frame = lock(&session.live).initialization_requests.first().cloned();
        let Some(frame) = frame else {
            return;
        };
        observe(session, frame);
        let mut live = lock(&session.live);
        if !live.initialization_requests.is_empty() {
            live.initialization_requests.remove(0);
        }
        session.publish(&mut live);
    }
}
