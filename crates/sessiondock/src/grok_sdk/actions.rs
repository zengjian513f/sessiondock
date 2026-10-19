//! Explicit native extensions; no transcript rewriting or model-emulated commands.
use super::{Bridge, Session, capabilities, live, required};
use serde_json::{Value, json};
use std::sync::Arc;

impl Bridge {
    pub(super) async fn read_extension(
        &self,
        session: &Session,
        method: &str,
    ) -> Result<Value, String> {
        let (native, params) = match method {
            "usage" => ("x.ai/session/usage", json!({"sessionId":session.sid})),
            "billing" => ("x.ai/billing", json!({})),
            "rewind_points" => ("x.ai/rewind/points", json!({"sessionId":session.sid})),
            _ => unreachable!(),
        };
        let result = session.connection.request(native, params).await;
        self.observe_extension(session, method, &result);
        let result = result?;
        // Context occupancy has a dedicated native read. Never derive it from
        // cumulative token counters or treat an unsupported read as zero.
        let context = if method == "usage" {
            Some(
                session
                    .connection
                    .request("x.ai/session/info", json!({"sessionId":session.sid}))
                    .await,
            )
        } else {
            None
        };
        let mut state = session.live.lock().unwrap();
        if let Some(context) = context {
            state.context = Value::Null;
            if let Ok(context) = context {
                let context = context.get("result").unwrap_or(&context);
                let native = &context["context"];
                if let (Some(used), Some(total)) =
                    (native["used"].as_u64(), native["total"].as_u64())
                    && total > 0
                {
                    state.context = json!({"used_tokens":used,"max_tokens":total,
                        "percent":native["usagePct"].as_f64().unwrap_or(used as f64 * 100.0 / total as f64),
                        "basis":"原生 session/info 上下文估计","native":native});
                }
            }
        }
        match method {
            "usage" => {
                let last = state.usage.get("last_turn").cloned();
                state.usage = live::scrub_usage(&result["usage"]);
                state.usage["basis"] =
                    json!("当前 ACP 进程累计；恢复进程后重置；输入含缓存，思考是输出的子集");
                if let Some(last) = last {
                    state.usage["last_turn"] = last;
                }
            }
            "billing" => state.billing = result.clone(),
            _ => {}
        }
        state.publish();
        let mut response = result;
        response["capabilities"] = capabilities(&state);
        response["raw"] = state.snapshot()["raw"].clone();
        Ok(response)
    }

    fn observe_extension(&self, session: &Session, method: &str, result: &Result<Value, String>) {
        let supported = match result {
            Ok(_) => Some(true),
            // Method-not-found is the only reliable negative capability signal.
            // Invalid params/auth/account/model errors remain visible failures.
            Err(error) if error == "grok_sdk_native_error: code -32601" => Some(false),
            _ => None,
        };
        if let Some(supported) = supported {
            let mut state = session.live.lock().unwrap();
            state.extensions.insert(method.to_owned(), supported);
            state.publish();
        }
    }

    pub(super) async fn mutate(
        self: &Arc<Self>,
        session: Arc<Session>,
        method: &str,
        input: &Value,
    ) -> Result<Value, String> {
        let request_id = required(input, "request_id")?;
        let key = format!("{}:{method}:{request_id}", session.uid);
        // Cache every dispatched outcome, including an uncertain transport error.
        // A lost ACK cannot repeat a fork, compaction or conversation rewind.
        let mut mutations = self.mutations.lock().await;
        if let Some(result) = mutations.get(&key) {
            return result.clone();
        }
        let _operation = session
            .turns
            .clone()
            .try_lock_owned()
            .map_err(|_| "grok_sdk_busy")?;
        self.check_conflict(&session.sid, &session.path, Some(session.connection.pid))
            .await?;
        {
            let state = session.live.lock().unwrap();
            if state.running || !state.requests.is_empty() {
                return Err("grok_sdk_busy".into());
            }
        }
        let result = self.mutate_once(&session, method, input).await;
        self.observe_extension(&session, method, &result);
        mutations.insert(key, result.clone());
        result
    }

    async fn mutate_once(
        &self,
        session: &Session,
        method: &str,
        input: &Value,
    ) -> Result<Value, String> {
        if method == "settings" {
            self.configure(&session.connection, &session.sid, input, &session.live)
                .await?;
            return Ok(self.receipt(session));
        }
        let (native, params) = match method {
            "compact" => (
                "x.ai/compact_conversation",
                json!({"sessionId":session.sid}),
            ),
            "rewind" => {
                let index = input["prompt_index"]
                    .as_u64()
                    .ok_or("grok_sdk_missing_prompt_index")?;
                (
                    "x.ai/rewind/execute",
                    json!({"sessionId":session.sid,"targetPromptIndex":index,"force":false,"mode":"conversation_only"}),
                )
            }
            "fork" => {
                // Native operation owns identity, copies, offsets and parent lineage.
                let mut params = json!({"sourceSessionId":session.sid,"sourceCwd":session.cwd,"newCwd":session.cwd});
                if let Some(index) = input.get("prompt_index").filter(|v| !v.is_null()) {
                    params["targetPromptIndex"] =
                        json!(index.as_u64().ok_or("grok_sdk_invalid_prompt_index")?);
                }
                ("x.ai/session/fork", params)
            }
            _ => unreachable!(),
        };
        {
            let mut state = session.live.lock().unwrap();
            state.running = true;
            state.prompt_started = method == "compact";
            state.error = None;
            state.publish();
        }
        let result = session.connection.request(native, params).await;
        {
            let mut state = session.live.lock().unwrap();
            state.running = false;
            state.prompt_started = false;
            if let Err(error) = &result {
                state.error = Some(error.clone());
            }
            if result.is_ok() && matches!(method, "compact" | "rewind") {
                // Session totals are not context occupancy. Invalidate any
                // previous context estimate and keep ordinary native history authoritative.
                state.context = Value::Null;
                if method == "rewind" {
                    state.items.clear();
                }
            }
            state.publish();
        }
        let result = result?;
        if method == "fork" {
            let sid = required(&result, "newSessionId")?;
            let cwd = required(&result, "newCwd")?;
            if cwd != session.cwd {
                return Err("grok_sdk_native_cwd_mismatch".into());
            }
            let path = self.native_path(sid)?;
            let uid = crate::sessions::uid_for("grok", &path);
            return Ok(json!({"uid":uid,"session_id":sid,"cwd":cwd,"native":result,
                "session":{"uid":uid,"source":"grok","session_id":sid,"sid":sid,"cwd":cwd,
                    "title":"Grok 会话分支","path":path,"sdk":true},
                "capabilities":capabilities(&session.live.lock().unwrap())}));
        }
        let mut receipt = self.receipt(session);
        receipt["native"] = result;
        Ok(receipt)
    }
}

pub(super) fn response_result(request: &Value, result: &Value) -> Result<Value, String> {
    match request["native_method"].as_str() {
        Some("x.ai/exit_plan_mode") => {
            let outcome = result["outcome"]
                .as_str()
                .or_else(|| result.as_str())
                .ok_or("grok_sdk_invalid_plan_result")?;
            if !matches!(outcome, "approved" | "cancelled" | "abandoned") {
                return Err("grok_sdk_invalid_plan_result".into());
            }
            let mut response = json!({"outcome":outcome});
            if outcome == "cancelled"
                && let Some(feedback) = result.get("feedback")
            {
                response["feedback"] =
                    json!(feedback.as_str().ok_or("grok_sdk_invalid_plan_feedback")?);
            }
            Ok(response)
        }
        Some("x.ai/ask_user_question") => question_result(request, result),
        _ => {
            let outcome = if result["outcome"]["outcome"] == "cancelled"
                || result["outcome"] == "cancelled"
            {
                json!({"outcome":"cancelled"})
            } else {
                let option = result["outcome"]["optionId"]
                    .as_str()
                    .or_else(|| result["optionId"].as_str())
                    .or_else(|| result.as_str())
                    .ok_or("grok_sdk_invalid_permission_result")?;
                if !request["options"]
                    .as_array()
                    .is_some_and(|options| options.iter().any(|o| o["value"] == option))
                {
                    return Err("grok_sdk_unknown_permission_option".into());
                }
                json!({"outcome":"selected","optionId":option})
            };
            Ok(json!({"outcome":outcome}))
        }
    }
}

fn question_result(request: &Value, result: &Value) -> Result<Value, String> {
    let outcome = result["outcome"]
        .as_str()
        .unwrap_or(if result["behavior"] == "deny" {
            "cancelled"
        } else {
            "accepted"
        });
    if outcome == "cancelled" {
        return Ok(json!({"outcome":"cancelled"}));
    }
    if matches!(outcome, "chat_about_this" | "skip_interview") {
        if request["mode"] != "plan" {
            return Err("grok_sdk_question_outcome_unsupported".into());
        }
        let partial = result.get("partial_answers").cloned().unwrap_or(json!({}));
        let partial = partial
            .as_object()
            .ok_or("grok_sdk_invalid_question_answers")?;
        let questions = request["questions"]
            .as_array()
            .ok_or("grok_sdk_invalid_question")?;
        if partial.iter().any(|(question, answer)| {
            !answer.is_string() || !questions.iter().any(|q| q["question"] == question.as_str())
        }) {
            return Err("grok_sdk_invalid_question_answers".into());
        }
        return Ok(json!({"outcome":outcome,"partial_answers":partial}));
    }
    if outcome != "accepted" {
        return Err("grok_sdk_invalid_question_outcome".into());
    }
    let answers = result["answers"]
        .as_object()
        .ok_or("grok_sdk_invalid_question_answers")?;
    let questions = request["questions"]
        .as_array()
        .ok_or("grok_sdk_invalid_question")?;
    let mut native = serde_json::Map::new();
    let mut annotations = serde_json::Map::new();
    for (key, answer) in answers {
        let question = questions
            .iter()
            .find(|q| q["question"] == key.as_str())
            .ok_or("grok_sdk_unknown_question")?;
        let labels: Vec<&str> = question["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| o["label"].as_str())
            .collect();
        let values = if let Some(answer) = answer.as_str() {
            vec![answer]
        } else {
            answer
                .as_array()
                .ok_or("grok_sdk_invalid_question_answer")?
                .iter()
                .map(|a| a.as_str().ok_or("grok_sdk_invalid_question_answer"))
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut selected = Vec::new();
        let mut notes = Vec::new();
        for value in values.into_iter().filter(|v| !v.is_empty()) {
            if labels.contains(&value) {
                selected.push(value.to_owned());
            } else {
                notes.push(value.to_owned());
            }
        }
        if !notes.is_empty() {
            selected.push("Other".into());
            annotations.insert(key.clone(), json!({"notes":notes.join("\n")}));
        }
        if !selected.is_empty() {
            native.insert(key.clone(), json!(selected));
        }
    }
    // Explicit native notes are allowed, but only for an answered question.
    if let Some(extra) = result["annotations"].as_object() {
        for (key, value) in extra {
            if !native.contains_key(key) {
                return Err("grok_sdk_unknown_question_annotation".into());
            }
            if let Some(notes) = value["notes"].as_str() {
                annotations.insert(key.clone(), json!({"notes":notes}));
            }
        }
    }
    let mut response = json!({"outcome":"accepted","answers":native});
    if !annotations.is_empty() {
        response["annotations"] = json!(annotations);
    }
    Ok(response)
}
