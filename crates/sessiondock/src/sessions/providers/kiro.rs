//! Kiro native session projection (V3 KAS `session.json` + `messages.jsonl`
//! and V2 legacy CLI `<uuid>.json` + `<uuid>.jsonl`).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use super::super::index::summary::flatten_text;
use super::super::index::summary::kiro::{
    extract_content_text, extract_user_prompt_text, extract_v2_embedded_user_text,
};
use super::{clip, normalized, output_exit_code, string};

pub(super) fn metadata(
    path: &Path,
    summary: &Value,
    records: &[(Value, u64)],
    fallback: &str,
) -> Value {
    let default_sid = if path.file_name().and_then(|n| n.to_str()) == Some("session.json") {
        path.parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_owned()
    } else {
        path.file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_owned()
    };

    let sid = summary
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            summary
                .get("sessionId")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| {
            summary
                .get("session_id")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or(&default_sid)
        .trim()
        .to_owned();

    let is_v2 = summary.get("session_state").is_some()
        || (summary.get("session_id").is_some() && summary.get("id").is_none());

    let mut first_user_title = String::new();
    let mut first_turn_ts = Value::Null;
    let mut last_turn_ts = Value::Null;

    for (record, _) in records {
        if let Some(ts) = record_turn_ts(record, is_v2) {
            if first_turn_ts.is_null() {
                first_turn_ts = ts.clone();
            }
            last_turn_ts = ts;
        }
        if first_user_title.is_empty()
            && let Some(text) = extract_user_prompt_text(record, is_v2)
        {
            let c = clip(&text);
            if !c.is_empty() {
                first_user_title = c;
            }
        }
    }

    if first_user_title.is_empty()
        && let Some(hist) = summary.get("history").and_then(Value::as_array)
    {
        for entry in hist {
            if let Some(text) = extract_user_prompt_text(entry, true) {
                let c = clip(&text);
                if !c.is_empty() {
                    first_user_title = c;
                    break;
                }
            }
        }
    }

    let created_summary = normalized(
        summary
            .get("createdAt")
            .or_else(|| summary.get("created_at"))
            .unwrap_or(&Value::Null),
    );
    let created = if !created_summary.is_null() {
        created_summary
    } else if !first_turn_ts.is_null() {
        first_turn_ts
    } else {
        json!(fallback)
    };

    if last_turn_ts.is_null() && is_v2 {
        last_turn_ts = v2_metadata_turn_ts(summary);
    }

    let updated = if !last_turn_ts.is_null() {
        last_turn_ts
    } else {
        created.clone()
    };

    let explicit_title = string(
        summary
            .get("title")
            .or_else(|| summary.get("customTitle"))
            .or_else(|| summary.get("name"))
            .unwrap_or(&Value::Null),
    );
    let title = if explicit_title.is_empty()
        || explicit_title == "New Session"
        || explicit_title == "(no title)"
    {
        if !first_user_title.is_empty() {
            first_user_title
        } else {
            "新建 Kiro 会话".to_owned()
        }
    } else {
        clip(&explicit_title)
    };

    let cwd = extract_cwd(summary, is_v2);
    let model = extract_model(summary, is_v2);
    let branch = extract_branch(summary);

    let parent_sid = summary
        .get("parentSessionId")
        .or_else(|| summary.get("parent_session_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_owned();
    let created_reason_subagent = !is_v2
        && summary
            .get("createdReason")
            .or_else(|| summary.get("session_created_reason"))
            .and_then(Value::as_str)
            .is_some_and(|r| r.trim() == "subagent");
    let is_subagent = !parent_sid.is_empty() || created_reason_subagent;
    let agent_type = summary
        .get("agentName")
        .or_else(|| summary.get("subagentType"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("subagent");

    let mut meta = json!({
        "sid": sid,
        "title": title,
        "cwd": cwd,
        "created": created,
        "updated": updated,
        "model": model,
        "branch": branch,
    });
    if is_subagent {
        meta["_is_subagent"] = json!(true);
        meta["_parent_thread_id"] = json!(parent_sid);
        meta["_agent_title"] = meta["title"].clone();
        meta["_agent_type"] = json!(agent_type);
    }
    meta
}

fn record_turn_ts(record: &Value, is_v2: bool) -> Option<Value> {
    let msg = record.get("message").unwrap_or(record);
    let kind = msg
        .get("kind")
        .or_else(|| msg.get("type"))
        .or_else(|| record.get("kind"))
        .or_else(|| record.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let is_turn = if is_v2 {
        matches!(
            kind,
            "Prompt" | "AssistantMessage" | "ToolResults" | "user" | "assistant" | "tool_result"
        )
    } else {
        match kind {
            "user" | "assistant" | "tool_call" | "tool_result" | "error" => true,
            "session_start" => {
                let data = msg.get("data").unwrap_or(msg);
                !data
                    .get("forcedRole")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            }
            _ => false,
        }
    };
    if !is_turn {
        return None;
    }
    for candidate in [
        record.get("timestamp"),
        record.get("createdAt"),
        record.get("created_at"),
        msg.get("timestamp"),
        msg.get("createdAt"),
        msg.get("created_at"),
        msg.get("data").and_then(|d| d.get("timestamp")),
    ]
    .into_iter()
    .flatten()
    {
        let ts = normalized(candidate);
        if !ts.is_null() {
            return Some(ts);
        }
    }
    None
}

fn v2_metadata_turn_ts(summary: &Value) -> Value {
    let Some(turns) = summary
        .pointer("/session_state/conversation_metadata/user_turn_metadatas")
        .and_then(Value::as_array)
    else {
        return Value::Null;
    };
    for turn in turns.iter().rev() {
        for key in [
            "timestamp",
            "ended_at",
            "completed_at",
            "updated_at",
            "started_at",
            "created_at",
        ] {
            if let Some(val) = turn.get(key) {
                let ts = normalized(val);
                if !ts.is_null() {
                    return ts;
                }
            }
        }
    }
    Value::Null
}

fn extract_cwd(summary: &Value, is_v2: bool) -> String {
    for key in ["cwd", "workspaceDirectory", "workingDirectory", "directory"] {
        let s = string(summary.get(key).unwrap_or(&Value::Null));
        if !s.is_empty() {
            return s;
        }
    }
    for key in ["workspacePaths", "rootPaths"] {
        if let Some(arr) = summary.get(key).and_then(Value::as_array) {
            for item in arr {
                let s = string(item);
                if !s.is_empty() {
                    return s;
                }
            }
        }
    }
    if is_v2
        && let Some(turns) = summary
            .pointer("/session_state/conversation_metadata/user_turn_metadatas")
            .and_then(Value::as_array)
    {
        for turn in turns.iter().rev() {
            let s = string(
                turn.get("cwd")
                    .or_else(|| turn.get("working_directory"))
                    .unwrap_or(&Value::Null),
            );
            if !s.is_empty() {
                return s;
            }
        }
    }
    "unknown".to_owned()
}

fn extract_model(summary: &Value, is_v2: bool) -> Value {
    for key in ["modelId", "model_id", "model"] {
        let s = string(summary.get(key).unwrap_or(&Value::Null));
        if !s.is_empty() {
            return json!(s);
        }
    }
    if is_v2 {
        for ptr in [
            "/session_state/rts_model_state/model_info/model_id",
            "/session_state/rts_model_state/model_info/modelId",
            "/session_state/rts_model_state/model_id",
            "/session_state/rts_model_state/modelId",
        ] {
            if let Some(val) = summary.pointer(ptr) {
                let s = string(val);
                if !s.is_empty() {
                    return json!(s);
                }
            }
        }
        if let Some(turns) = summary
            .pointer("/session_state/conversation_metadata/user_turn_metadatas")
            .and_then(Value::as_array)
        {
            for turn in turns.iter().rev() {
                for key in ["model_id", "modelId", "model"] {
                    let s = string(turn.get(key).unwrap_or(&Value::Null));
                    if !s.is_empty() {
                        return json!(s);
                    }
                }
            }
        }
    }
    Value::Null
}

fn extract_branch(summary: &Value) -> Value {
    for key in ["gitBranch", "git_branch", "branch"] {
        let s = string(summary.get(key).unwrap_or(&Value::Null));
        if !s.is_empty() {
            return json!(s);
        }
    }
    Value::Null
}

impl super::Parser<'_> {
    pub(super) fn kiro(&mut self, record: &Value, end: u64) -> Result<(), String> {
        let msg = record.get("message").unwrap_or(record);
        let msg_id = record
            .get("id")
            .or_else(|| record.get("messageId"))
            .or_else(|| msg.get("id"))
            .or_else(|| msg.get("messageId"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !msg_id.is_empty() {
            self.kiro_history
                .push((msg_id.to_owned(), self.events.len()));
        }

        let ts = record
            .get("timestamp")
            .or_else(|| record.get("createdAt"))
            .or_else(|| record.get("created_at"))
            .or_else(|| msg.get("timestamp"))
            .or_else(|| msg.get("createdAt"))
            .or_else(|| msg.get("created_at"))
            .or_else(|| msg.get("data").and_then(|d| d.get("timestamp")))
            .map(normalized)
            .unwrap_or(Value::Null);

        let kind = msg
            .get("kind")
            .or_else(|| msg.get("type"))
            .or_else(|| record.get("kind"))
            .or_else(|| record.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let data = msg.get("data").unwrap_or(msg);

        match kind {
            // V3 KAS user message
            "user" => {
                let text = extract_content_text(data);
                if !text.is_empty() {
                    self.emit(end, "user", text, &ts, json!({}));
                }
            }
            // V3 KAS session_start
            "session_start" => {
                let forced = data
                    .get("forcedRole")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !forced {
                    let text = extract_content_text(data);
                    if !text.is_empty() {
                        self.emit(end, "user", text, &ts, json!({}));
                    }
                }
            }
            // V3 KAS assistant message
            "assistant" => {
                let intent = data
                    .get("intent")
                    .or_else(|| msg.get("intent"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let text = extract_content_text(data);
                if intent == "Reasoning" {
                    if !text.is_empty() {
                        self.emit(end, "thinking", text, &ts, json!({}));
                    }
                } else if !text.is_empty() {
                    self.emit(end, "assistant", text, &ts, json!({}));
                }
                if let Some(calls) = data
                    .get("toolCalls")
                    .or_else(|| data.get("tool_calls"))
                    .and_then(Value::as_array)
                {
                    for call in calls {
                        self.emit_kiro_tool_call(call, end, &ts);
                    }
                }
            }
            // V3 KAS tool_call
            "tool_call" => {
                if let Some(calls) = data
                    .get("toolCalls")
                    .or_else(|| data.get("tool_calls"))
                    .and_then(Value::as_array)
                {
                    for call in calls {
                        self.emit_kiro_tool_call(call, end, &ts);
                    }
                } else {
                    self.emit_kiro_tool_call(data, end, &ts);
                }
            }
            // V3 KAS tool_result
            "tool_result" => {
                if let Some(results) = data
                    .get("toolResults")
                    .or_else(|| data.get("tool_results"))
                    .or_else(|| data.get("results"))
                    .and_then(Value::as_array)
                {
                    for item in results {
                        self.emit_kiro_tool_result(item, end, &ts);
                    }
                } else {
                    self.emit_kiro_tool_result(data, end, &ts);
                }
            }
            // V3 KAS error
            "error" => {
                let text = first_error_text(data, record);
                if !text.is_empty() {
                    self.emit(end, "system", text, &ts, json!({}));
                }
            }
            // V3 KAS tombstone (`checkpoint_revert`)
            "tombstone" => {
                let reason = data
                    .get("reason")
                    .or_else(|| record.get("reason"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if reason == "checkpoint_revert" {
                    let target_id = data
                        .get("effectiveFromMessageId")
                        .or_else(|| record.get("effectiveFromMessageId"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim();
                    if !target_id.is_empty()
                        && let Some(pos) =
                            self.kiro_history.iter().position(|(id, _)| id == target_id)
                    {
                        let event_idx = self.kiro_history[pos].1;
                        self.events.truncate(event_idx);
                        self.kiro_history.truncate(pos);
                    }
                }
            }
            // V2 Legacy CLI Prompt
            "Prompt" => {
                let text = extract_content_text(data);
                if !text.is_empty() {
                    self.emit(end, "user", text, &ts, json!({}));
                }
            }
            // V2 Legacy CLI AssistantMessage
            "AssistantMessage" => {
                for r_key in ["reasoning", "thought", "thinking"] {
                    if let Some(r_val) = data.get(r_key) {
                        let r_text = match r_val {
                            Value::String(s) => s.trim().to_owned(),
                            Value::Object(map) => map
                                .get("text")
                                .or_else(|| map.get("content"))
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .trim()
                                .to_owned(),
                            _ => String::new(),
                        };
                        if !r_text.is_empty() {
                            self.emit(end, "thinking", r_text, &ts, json!({}));
                        }
                    }
                }

                // Check content blocks in V2 AssistantMessage
                let mut emitted_text = false;
                if let Some(arr) = data.get("content").and_then(Value::as_array) {
                    let mut text_parts = Vec::new();
                    for item in arr {
                        let item_kind = item
                            .get("kind")
                            .or_else(|| item.get("type"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        match item_kind {
                            "Reasoning" | "reasoning" | "thinking" | "Thought" => {
                                let r = item
                                    .get("data")
                                    .and_then(Value::as_str)
                                    .or_else(|| item.get("text").and_then(Value::as_str))
                                    .or_else(|| {
                                        item.get("data")
                                            .and_then(|d| d.get("text"))
                                            .and_then(Value::as_str)
                                    })
                                    .unwrap_or("")
                                    .trim();
                                if !r.is_empty() {
                                    self.emit(end, "thinking", r.to_owned(), &ts, json!({}));
                                }
                            }
                            "ToolUse" | "tool_use" | "toolUse" => {
                                let call_obj = item.get("data").unwrap_or(item);
                                self.emit_kiro_tool_call(call_obj, end, &ts);
                            }
                            _ => {
                                let t = item
                                    .get("data")
                                    .and_then(Value::as_str)
                                    .or_else(|| item.get("text").and_then(Value::as_str))
                                    .or_else(|| {
                                        item.get("data")
                                            .and_then(|d| d.get("text"))
                                            .and_then(Value::as_str)
                                    })
                                    .unwrap_or("")
                                    .trim();
                                if !t.is_empty() {
                                    text_parts.push(t.to_owned());
                                }
                            }
                        }
                    }
                    if !text_parts.is_empty() {
                        emitted_text = true;
                        self.emit(end, "assistant", text_parts.join("\n\n"), &ts, json!({}));
                    }
                }

                if !emitted_text {
                    let text = extract_content_text(data);
                    if !text.is_empty() {
                        self.emit(end, "assistant", text, &ts, json!({}));
                    }
                }

                // V2 toolUse / tool_uses fields on AssistantMessage
                for key in [
                    "toolUse",
                    "tool_use",
                    "toolUses",
                    "tool_uses",
                    "toolCalls",
                    "tool_calls",
                ] {
                    if let Some(val) = data.get(key) {
                        if let Some(arr) = val.as_array() {
                            for call in arr {
                                self.emit_kiro_tool_call(call, end, &ts);
                            }
                        } else if val.is_object() {
                            self.emit_kiro_tool_call(val, end, &ts);
                        }
                    }
                }
            }
            // V2 Legacy CLI ToolResults
            "ToolResults" => {
                self.emit_v2_tool_results(data, end, &ts);
            }
            "" if record.get("user").is_some() || record.get("assistant").is_some() => {
                if let Some(u) = record.get("user") {
                    let u_text = extract_v2_embedded_user_text(u);
                    if !u_text.is_empty() {
                        self.emit(end, "user", u_text, &ts, json!({}));
                    }
                }
                if let Some(a) = record.get("assistant") {
                    let a_text = if let Some(s) = a.as_str() {
                        s.trim().to_owned()
                    } else if let Some(s) = a.pointer("/Response/content").and_then(Value::as_str) {
                        s.trim().to_owned()
                    } else {
                        extract_content_text(a)
                    };
                    if !a_text.is_empty() {
                        self.emit(end, "assistant", a_text, &ts, json!({}));
                    }
                }
            }
            "system" | "checkpoint" | "System" | "Metadata" | "" => {}
            other => self.skipped.note("Kiro 记录类型", other),
        }
        Ok(())
    }

    fn emit_kiro_tool_call(&mut self, call: &Value, end: u64, ts: &Value) {
        let call_id = call
            .get("toolCallId")
            .or_else(|| call.get("toolUseId"))
            .or_else(|| call.get("tool_use_id"))
            .or_else(|| call.get("call_id"))
            .or_else(|| call.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        let name = call
            .get("toolName")
            .or_else(|| call.get("name"))
            .or_else(|| call.get("tool_name"))
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .trim()
            .to_owned();
        let args_val = call
            .get("args")
            .or_else(|| call.get("arguments"))
            .or_else(|| call.get("input"))
            .or_else(|| call.get("parameters"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let args = if let Some(s) = args_val.as_str() {
            serde_json::from_str::<Value>(s).unwrap_or(Value::String(s.to_owned()))
        } else {
            args_val
        };
        if !call_id.is_empty() {
            self.calls.insert(call_id.clone(), name.clone());
        }
        let mut extra = json!({
            "name": name,
            "args": args,
        });
        if !call_id.is_empty() {
            extra["tool_use_id"] = json!(call_id);
        }
        self.emit(end, "tool_call", "", ts, extra);
    }

    fn emit_kiro_tool_result(&mut self, data: &Value, end: u64, ts: &Value) {
        let call_id = data
            .get("toolCallId")
            .or_else(|| data.get("toolUseId"))
            .or_else(|| data.get("tool_use_id"))
            .or_else(|| data.get("call_id"))
            .or_else(|| data.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        let name = data
            .get("toolName")
            .or_else(|| data.get("name"))
            .or_else(|| data.get("tool_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| self.calls.get(&call_id).cloned())
            .unwrap_or_else(|| "tool".to_owned());

        let mut is_error = data
            .get("isError")
            .or_else(|| data.get("is_error"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || data.get("status").and_then(Value::as_str).is_some_and(|s| {
                s.eq_ignore_ascii_case("error") || s.eq_ignore_ascii_case("failed")
            });

        let mut text = extract_content_text(data);
        if text.is_empty() {
            for key in ["output", "result", "error"] {
                if let Some(val) = data.get(key) {
                    if key == "error" && !val.is_null() {
                        is_error = true;
                    }
                    if let Ok(flat) = flatten_text(val) {
                        let trimmed = flat.trim();
                        if !trimmed.is_empty() {
                            text = trimmed.to_owned();
                            break;
                        }
                    }
                    if val.is_object() || val.is_array() {
                        text = serde_json::to_string_pretty(val).unwrap_or_default();
                        break;
                    }
                }
            }
        }

        let mut extra = json!({
            "name": name,
        });
        if !call_id.is_empty() {
            extra["tool_use_id"] = json!(call_id);
        }
        if is_error {
            extra["is_error"] = json!(true);
        }
        if let Some(code) = output_exit_code(&text) {
            extra["exit_code"] = json!(code);
        }
        self.emit(end, "tool_result", text, ts, extra);
    }

    fn emit_v2_tool_results(&mut self, data: &Value, end: u64, ts: &Value) {
        let mut results_by_id: BTreeMap<String, (String, bool)> = BTreeMap::new();

        // 1. Check `data.results` map (`{ "<toolUseId>": { "result": { "Error": ... } } }`)
        if let Some(results_map) = data.get("results").and_then(Value::as_object) {
            for (id, entry) in results_map {
                let result_obj = entry.get("result").unwrap_or(entry);
                if let Some(err_val) = result_obj.get("Error").or_else(|| result_obj.get("error")) {
                    let err_text = extract_v2_value_text(err_val);
                    results_by_id.insert(id.clone(), (err_text, true));
                } else if let Some(ok_val) = result_obj
                    .get("Success")
                    .or_else(|| result_obj.get("success"))
                {
                    let ok_text = extract_v2_value_text(ok_val);
                    results_by_id.insert(id.clone(), (ok_text, false));
                } else {
                    let text = extract_v2_value_text(result_obj);
                    results_by_id.insert(id.clone(), (text, false));
                }
            }
        }

        // 2. Check `data.content` array
        if let Some(content_arr) = data.get("content").and_then(Value::as_array) {
            let mut handled_any = false;
            for item in content_arr {
                let inner = item.get("data").unwrap_or(item);
                let id = inner
                    .get("toolUseId")
                    .or_else(|| inner.get("tool_use_id"))
                    .or_else(|| inner.get("id"))
                    .or_else(|| item.get("toolUseId"))
                    .or_else(|| item.get("tool_use_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                if !id.is_empty() || inner.get("content").is_some() || inner.get("status").is_some()
                {
                    handled_any = true;
                    let mut is_error = inner
                        .get("status")
                        .and_then(Value::as_str)
                        .is_some_and(|s| s.eq_ignore_ascii_case("error"))
                        || inner
                            .get("isError")
                            .or_else(|| inner.get("is_error"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                    let mut text = extract_content_text(inner);
                    if let Some((map_text, map_err)) = results_by_id.remove(&id) {
                        if map_err {
                            is_error = true;
                        }
                        if text.is_empty() {
                            text = map_text;
                        }
                    }
                    let name = inner
                        .get("name")
                        .or_else(|| inner.get("toolName"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .or_else(|| self.calls.get(&id).cloned())
                        .unwrap_or_else(|| "tool".to_owned());
                    let mut extra = json!({ "name": name });
                    if !id.is_empty() {
                        extra["tool_use_id"] = json!(id);
                    }
                    if is_error {
                        extra["is_error"] = json!(true);
                    }
                    if let Some(code) = output_exit_code(&text) {
                        extra["exit_code"] = json!(code);
                    }
                    self.emit(end, "tool_result", text, ts, extra);
                }
            }
            if handled_any && results_by_id.is_empty() {
                return;
            }
        }

        // 3. Emit any remaining entries from `results_by_id`
        if !results_by_id.is_empty() {
            for (id, (text, is_error)) in results_by_id {
                let name = self
                    .calls
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| "tool".to_owned());
                let mut extra = json!({
                    "name": name,
                    "tool_use_id": id,
                });
                if is_error {
                    extra["is_error"] = json!(true);
                }
                if let Some(code) = output_exit_code(&text) {
                    extra["exit_code"] = json!(code);
                }
                self.emit(end, "tool_result", text, ts, extra);
            }
            return;
        }

        // 4. Fallback single tool_result
        self.emit_kiro_tool_result(data, end, ts);
    }
}

fn first_error_text(data: &Value, record: &Value) -> String {
    for candidate in [
        data.get("message"),
        data.get("error"),
        data.get("content"),
        record.get("error"),
        record.get("message"),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(s) = candidate.as_str() {
            let t = s.trim();
            if !t.is_empty() {
                return t.to_owned();
            }
        }
        if let Ok(flat) = flatten_text(candidate) {
            let t = flat.trim();
            if !t.is_empty() {
                return t.to_owned();
            }
        }
    }
    String::new()
}

fn extract_v2_value_text(val: &Value) -> String {
    if let Some(s) = val.as_str() {
        return s.trim().to_owned();
    }
    let text = extract_content_text(val);
    if !text.is_empty() {
        return text;
    }
    if let Ok(flat) = flatten_text(val) {
        let trimmed = flat.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    if val.is_object() || val.is_array() {
        return serde_json::to_string_pretty(val).unwrap_or_default();
    }
    String::new()
}
