use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::transport::{Session, now_secs};

pub(super) struct PendingRequest {
    pub(super) rpc_id: Value,
    pub(super) card: Value,
}

pub(super) struct LiveState {
    pub(super) uid: String,
    pub(super) cwd: String,
    pub(super) session_id: String,
    pub(super) title: Option<String>,
    pub(super) running: bool,
    pub(super) loading: bool,
    pub(super) interrupted_turn: bool,
    pub(super) items: Vec<Value>,
    pub(super) requests: HashMap<String, PendingRequest>,
    pub(super) last_turn: Option<Value>,
    pub(super) runtime_error: Option<String>,
    pub(super) retry: Option<Value>,
    pub(super) compact: Option<Value>,
    pub(super) status: Option<String>,
    pub(super) config_options: Vec<Value>,
    pub(super) modes: Value,
    pub(super) mode: Option<String>,
    pub(super) models: Value,
    pub(super) model: Option<String>,
    pub(super) model_observed: bool,
    pub(super) effort: Option<String>,
    pub(super) permission_mode: Option<String>,
    pub(super) autopilot: Option<bool>,
    pub(super) commands: Vec<Value>,
    pub(super) usage: Value,
    pub(super) billing: Option<Value>,
    pub(super) context_usage: Option<Value>,
    pub(super) agent_capabilities: Value,
    pub(super) seq: u64,
    pub(super) events: broadcast::Sender<Arc<str>>,
    next_item_id: u64,
    pub(super) current_assistant_id: Option<String>,
    pub(super) current_thinking_id: Option<String>,
    tool_item_ids: HashMap<String, String>,
    message_item_ids: HashMap<String, String>,
    replay_ids: HashSet<String>,
}

impl LiveState {
    pub(super) fn new() -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            uid: String::new(),
            cwd: String::new(),
            session_id: String::new(),
            title: None,
            running: false,
            loading: false,
            interrupted_turn: false,
            items: Vec::new(),
            requests: HashMap::new(),
            last_turn: None,
            runtime_error: None,
            retry: None,
            compact: None,
            status: None,
            config_options: Vec::new(),
            modes: Value::Null,
            mode: None,
            models: Value::Null,
            model: None,
            model_observed: false,
            effort: None,
            permission_mode: None,
            autopilot: None,
            commands: Vec::new(),
            usage: Value::Null,
            billing: None,
            context_usage: None,
            agent_capabilities: Value::Null,
            seq: 0,
            events,
            next_item_id: 1,
            current_assistant_id: None,
            current_thinking_id: None,
            tool_item_ids: HashMap::new(),
            message_item_ids: HashMap::new(),
            replay_ids: HashSet::new(),
        }
    }

    pub(super) fn alloc_item_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}-{}", self.next_item_id);
        self.next_item_id += 1;
        id
    }

    pub(super) fn publish(&mut self) {
        self.seq += 1;
        let uid = self.uid.clone();
        let cwd = self.cwd.clone();
        let snap = self.snapshot(&uid, &cwd);
        let _ = self.events.send(Arc::from(snap.to_string()));
    }

    pub(super) fn apply_initialize_result(&mut self, res: &Value) {
        if let Some(caps) = res.get("agentCapabilities") {
            self.agent_capabilities = caps.clone();
        }
    }

    pub(super) fn apply_open_result(&mut self, sid: &str, res: &Value) {
        self.session_id = sid.to_string();
        if self.uid.is_empty() {
            self.uid = format!("kiro:{sid}");
        }
        if let Some(modes) = res.get("modes")
            && !modes.is_null()
        {
            self.modes = modes.clone();
            if let Some(cur) = modes["currentModeId"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
            {
                self.mode = Some(cur.to_string());
            }
        }
        if let Some(models) = res.get("models")
            && !models.is_null()
        {
            self.models = models.clone();
            if let Some(cur) = models["currentModelId"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
            {
                self.model = Some(cur.to_string());
                self.model_observed = false;
            }
        }
        if let Some(arr) = res
            .get("configOptions")
            .or_else(|| res.get("options"))
            .and_then(|v| v.as_array())
        {
            self.apply_config_options(arr);
        }
        if let Some(meta) = res.get("metadata").or_else(|| res.get("_meta")) {
            self.apply_metadata(meta);
        }
        self.publish();
    }

    pub(super) fn apply_disk_metadata(&mut self, disk: &Value) {
        if self.model.is_none() {
            if let Some(m) = disk["modelId"]
                .as_str()
                .or_else(|| disk["model"].as_str())
                .or_else(|| disk["currentModelId"].as_str())
                .or_else(|| {
                    disk.pointer("/session_state/rts_model_state/model_info/model_id")
                        .and_then(|v| v.as_str())
                })
                .or_else(|| {
                    disk.pointer("/session_state/rts_model_state/model_info/modelId")
                        .and_then(|v| v.as_str())
                })
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                self.model = Some(m.to_string());
                self.model_observed = true;
            }
        }
        if self.autopilot.is_none() {
            if let Some(ap) = disk["autopilot"].as_bool() {
                self.autopilot = Some(ap);
                self.permission_mode = Some(if ap { "autopilot" } else { "default" }.into());
            }
        }
        if self.effort.is_none() {
            if let Some(e) = disk["effortLevel"]
                .as_str()
                .or_else(|| disk["effort"].as_str())
                .or_else(|| disk["reasoningEffort"].as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                self.effort = Some(e.to_string());
            }
        }
        if self.mode.is_none() {
            if let Some(m) = disk["mode"]
                .as_str()
                .or_else(|| disk["currentModeId"].as_str())
                .or_else(|| disk["agentMode"].as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                self.mode = Some(m.to_string());
            }
        }
        if self.title.is_none() {
            if let Some(t) = disk["title"]
                .as_str()
                .or_else(|| disk["name"].as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                self.title = Some(t.to_string());
            }
        }
        if let Some(arr) = disk.get("configOptions").and_then(|v| v.as_array())
            && self.config_options.is_empty()
        {
            self.apply_config_options(arr);
        }
        self.apply_metadata(disk);
    }

    pub(super) fn apply_config_options(&mut self, arr: &[Value]) {
        self.config_options = arr.to_vec();
        self.sync_from_config_options();
    }

    pub(super) fn sync_from_config_options(&mut self) {
        let opts = self.config_options.clone();
        for opt in &opts {
            let id = opt["id"]
                .as_str()
                .or_else(|| opt["configId"].as_str())
                .unwrap_or("");
            let cat = opt["category"].as_str().unwrap_or("");
            let cur_str = opt["currentValue"]
                .as_str()
                .or_else(|| opt["value"].as_str())
                .or_else(|| opt["defaultValue"].as_str());

            if id.eq_ignore_ascii_case("model")
                || id.eq_ignore_ascii_case("modelId")
                || cat.eq_ignore_ascii_case("model")
            {
                if let Some(m) = cur_str.map(str::trim).filter(|s| !s.is_empty()) {
                    self.model = Some(m.to_string());
                    self.model_observed = false;
                }
            } else if id.eq_ignore_ascii_case("effortLevel")
                || id.eq_ignore_ascii_case("effort")
                || id.eq_ignore_ascii_case("reasoningEffort")
                || cat.eq_ignore_ascii_case("thought_level")
                || cat.eq_ignore_ascii_case("effort")
            {
                if let Some(e) = cur_str.map(str::trim) {
                    self.effort = if e.is_empty() {
                        None
                    } else {
                        Some(e.to_string())
                    };
                }
            } else if id.eq_ignore_ascii_case("mode") || cat.eq_ignore_ascii_case("mode") {
                if let Some(m) = cur_str.map(str::trim).filter(|s| !s.is_empty()) {
                    self.mode = Some(m.to_string());
                }
                if let Some(options) = opt["options"].as_array() {
                    let available_modes: Vec<Value> = options
                        .iter()
                        .filter_map(|item| {
                            let mode_id = item["value"]
                                .as_str()
                                .or_else(|| item["id"].as_str())
                                .or_else(|| item.as_str())?;
                            let name = item["name"]
                                .as_str()
                                .or_else(|| item["label"].as_str())
                                .unwrap_or(mode_id);
                            let mut row = json!({
                                "id": mode_id,
                                "name": name,
                            });
                            if let Some(desc) = item["description"].as_str() {
                                row["description"] = json!(desc);
                            }
                            Some(row)
                        })
                        .collect();
                    if !available_modes.is_empty() {
                        self.modes = json!({
                            "availableModes": available_modes,
                            "currentModeId": self.mode.clone().unwrap_or_default(),
                        });
                    }
                }
            } else if id.eq_ignore_ascii_case("autopilot")
                || id.eq_ignore_ascii_case("approval")
                || id.eq_ignore_ascii_case("permissionMode")
                || cat.eq_ignore_ascii_case("autopilot")
                || cat.eq_ignore_ascii_case("approval")
            {
                let enabled = opt["currentValue"]
                    .as_bool()
                    .or_else(|| opt["value"].as_bool())
                    .unwrap_or_else(|| {
                        matches!(
                            cur_str.unwrap_or("").trim().to_ascii_lowercase().as_str(),
                            "true"
                                | "1"
                                | "on"
                                | "enabled"
                                | "autopilot"
                                | "auto"
                                | "always"
                                | "bypasspermissions"
                        )
                    });
                self.autopilot = Some(enabled);
                self.permission_mode = Some(if enabled { "autopilot" } else { "default" }.into());
            }
        }
    }

    pub(super) fn update_config_option_value(&mut self, config_id: &str, value: &str) {
        let mut found = false;
        for opt in &mut self.config_options {
            let id = opt["id"]
                .as_str()
                .or_else(|| opt["configId"].as_str())
                .unwrap_or("");
            if id.eq_ignore_ascii_case(config_id) {
                if let Some(obj) = opt.as_object_mut() {
                    obj.insert("currentValue".into(), json!(value));
                }
                found = true;
            }
        }
        if !found {
            self.config_options.push(json!({
                "id": config_id,
                "currentValue": value,
            }));
        }
        self.sync_from_config_options();
    }

    pub(super) fn apply_single_config_update(&mut self, u: &Value) {
        if let Some(arr) = u
            .get("configOptions")
            .or_else(|| u.get("options"))
            .and_then(|v| v.as_array())
            && !arr.is_empty()
            && arr[0].get("id").or_else(|| arr[0].get("configId")).is_some()
        {
            self.apply_config_options(arr);
            return;
        }
        if let Some(cid) = u["configId"].as_str().or_else(|| u["id"].as_str()) {
            let val = u["currentValue"]
                .as_str()
                .or_else(|| u["value"].as_str())
                .unwrap_or("");
            let mut found = false;
            for opt in &mut self.config_options {
                let id = opt["id"]
                    .as_str()
                    .or_else(|| opt["configId"].as_str())
                    .unwrap_or("");
                if id.eq_ignore_ascii_case(cid) {
                    if let Some(obj) = opt.as_object_mut() {
                        obj.insert("currentValue".into(), json!(val));
                        if let Some(opts) = u.get("options").and_then(|v| v.as_array()) {
                            obj.insert("options".into(), json!(opts));
                        }
                    }
                    found = true;
                }
            }
            if !found {
                let mut entry = json!({
                    "id": cid,
                    "currentValue": val,
                });
                if let Some(opts) = u.get("options").and_then(|v| v.as_array()) {
                    entry["options"] = json!(opts);
                }
                self.config_options.push(entry);
            }
            self.sync_from_config_options();
        }
    }

    pub(super) fn begin_turn(
        &mut self,
        text: &str,
        request_id: Option<&str>,
        model: Option<String>,
        effort: Option<String>,
    ) {
        self.running = true;
        self.interrupted_turn = false;
        self.runtime_error = None;
        self.retry = None;
        if let Some(m) = model.filter(|s| !s.is_empty()) {
            self.model = Some(m);
            self.model_observed = false;
        }
        if let Some(e) = effort.filter(|s| !s.is_empty()) {
            self.effort = Some(e);
        }
        let user_id = self.alloc_item_id("user");
        let mut user_item = json!({
            "id": user_id,
            "role": "user",
            "text": text,
            "content": text,
        });
        if let Some(rid) = request_id.filter(|s| !s.is_empty()) {
            user_item["request_id"] = json!(rid);
        }
        self.items.push(user_item);
        self.current_assistant_id = None;
        self.current_thinking_id = None;
        self.tool_item_ids.clear();
        self.message_item_ids.clear();
        self.last_turn = Some(json!({ "status": "in_progress" }));
        self.publish();
    }

    pub(super) fn finish_turn(&mut self, result: &Value) {
        self.running = false;
        let stop = result["stopReason"].as_str().unwrap_or("end_turn");
        let is_interrupted =
            self.interrupted_turn || stop == "cancelled" || stop == "interrupted";
        self.last_turn = Some(json!({
            "status": if is_interrupted { "interrupted" } else { "completed" },
            "stopReason": stop,
        }));
        if let Some(meta) = result.get("metadata").or_else(|| result.get("_meta")) {
            self.apply_metadata(meta);
        }
        if let Some(usage) = result.get("usage") {
            self.apply_metadata(usage);
        }
        if let Some(text) = extract_result_text(result)
            && !text.is_empty()
        {
            let has_assistant_text = self
                .items
                .iter()
                .rev()
                .take_while(|item| item["role"] != "user")
                .any(|item| {
                    item["role"] == "assistant"
                        && !item["text"].as_str().unwrap_or("").is_empty()
                });
            if !has_assistant_text {
                self.append_chunk("assistant", &text, None);
            }
        }
        self.current_assistant_id = None;
        self.current_thinking_id = None;
        self.publish();
    }

    pub(super) fn fail_turn(&mut self, err: &str) {
        self.running = false;
        if self.interrupted_turn
            || err.contains("cancelled")
            || err.contains("interrupted")
            || err.contains("Canceled")
        {
            self.last_turn = Some(json!({
                "status": "interrupted",
                "stopReason": "cancelled",
            }));
        } else {
            self.last_turn = Some(json!({
                "status": "failed",
                "error": err,
            }));
        }
        self.current_assistant_id = None;
        self.current_thinking_id = None;
        self.publish();
    }

    pub(super) fn mark_interrupted(&mut self) {
        self.running = false;
        self.interrupted_turn = true;
        self.last_turn = Some(json!({
            "status": "interrupted",
            "stopReason": "cancelled",
        }));
        self.current_assistant_id = None;
        self.current_thinking_id = None;
        self.publish();
    }

    pub(super) fn handle_notification(&mut self, method: &str, params: &Value) {
        match method {
            "_kiro.dev/metadata"
            | "_kiro/metadata"
            | "kiro/metadata"
            | "session/metadata"
            | "usage/update" => {
                self.apply_metadata(params);
                self.publish();
                return;
            }
            "_kiro.dev/commands/available" | "commands/available" => {
                if let Some(cmds) = params
                    .get("availableCommands")
                    .or_else(|| params.get("commands"))
                    .and_then(|v| v.as_array())
                {
                    self.commands = cmds.clone();
                    self.publish();
                }
                return;
            }
            _ => {}
        }

        if method != "session/update" {
            return;
        }
        let u = params.get("update").unwrap_or(params);
        let kind = u["sessionUpdate"]
            .as_str()
            .or_else(|| u["type"].as_str())
            .unwrap_or("");
        let is_replay = self.loading
            || params["_meta"]["isReplay"].as_bool() == Some(true)
            || u["_meta"]["isReplay"].as_bool() == Some(true)
            || params["isReplay"].as_bool() == Some(true)
            || u["isReplay"].as_bool() == Some(true);

        let msg_id = u["messageId"]
            .as_str()
            .or_else(|| u["content"]["messageId"].as_str())
            .or_else(|| params["messageId"].as_str())
            .map(str::to_string);
        let tool_id = u["toolCallId"]
            .as_str()
            .or_else(|| u["id"].as_str())
            .map(str::to_string);

        match kind {
            "user_message_chunk" => {
                if is_replay {
                    if let Some(id) = msg_id {
                        self.replay_ids.insert(id);
                    }
                }
                return;
            }
            "agent_message_chunk" => {
                if is_replay {
                    if let Some(id) = msg_id {
                        self.replay_ids.insert(id);
                    }
                    return;
                }
                if let Some(ref id) = msg_id
                    && self.replay_ids.contains(id)
                {
                    return;
                }
                let text = extract_chunk_text(u);
                if !text.is_empty() {
                    self.append_chunk("assistant", &text, msg_id.as_deref());
                    self.publish();
                }
                return;
            }
            "agent_thought_chunk" => {
                if is_replay {
                    if let Some(id) = msg_id {
                        self.replay_ids.insert(id);
                    }
                    return;
                }
                if let Some(ref id) = msg_id
                    && self.replay_ids.contains(id)
                {
                    return;
                }
                let text = extract_chunk_text(u);
                if !text.is_empty() {
                    self.append_chunk("thinking", &text, msg_id.as_deref());
                    self.publish();
                }
                return;
            }
            "tool_call" | "tool_call_update" => {
                if is_replay {
                    if let Some(id) = tool_id {
                        self.replay_ids.insert(id);
                    }
                    return;
                }
                if let Some(ref id) = tool_id
                    && self.replay_ids.contains(id)
                {
                    return;
                }
                self.upsert_tool(u);
                self.publish();
                return;
            }
            "plan" => {
                if is_replay {
                    return;
                }
                self.upsert_plan(u);
                self.publish();
                return;
            }
            "current_mode_update" => {
                if let Some(m) = u["currentModeId"]
                    .as_str()
                    .or_else(|| u["modeId"].as_str())
                    .or_else(|| u["mode"].as_str())
                    .filter(|s| !s.trim().is_empty())
                {
                    self.mode = Some(m.to_string());
                    if self.modes.is_object() {
                        self.modes["currentModeId"] = json!(m);
                    }
                    self.update_config_option_value("mode", m);
                    self.publish();
                }
            }
            "config_option_update" | "config_options_update" => {
                self.apply_single_config_update(u);
                self.publish();
            }
            "available_commands_update" => {
                if let Some(cmds) = u
                    .get("availableCommands")
                    .or_else(|| u.get("commands"))
                    .and_then(|v| v.as_array())
                {
                    self.commands = cmds.clone();
                    self.publish();
                }
            }
            "usage_update" | "_kiro.dev/metadata" | "_kiro/metadata" | "metadata" => {
                self.apply_metadata(u);
                self.publish();
            }
            "session_info_update" => {
                if let Some(t) = u["title"].as_str().filter(|s| !s.trim().is_empty()) {
                    self.title = Some(t.to_string());
                }
                self.apply_metadata(u);
                self.publish();
            }
            "turn_end" => {
                if is_replay {
                    return;
                }
                self.running = false;
                let stop = u["stopReason"].as_str().unwrap_or("end_turn");
                let is_interrupted =
                    self.interrupted_turn || stop == "cancelled" || stop == "interrupted";
                self.last_turn = Some(json!({
                    "status": if is_interrupted { "interrupted" } else { "completed" },
                    "stopReason": stop,
                }));
                self.publish();
            }
            _ => {
                if u.get("contextUsagePercentage").is_some()
                    || u.get("credits").is_some()
                    || u.get("creditUsage").is_some()
                    || u.get("billing").is_some()
                {
                    self.apply_metadata(u);
                    self.publish();
                }
            }
        }
    }

    pub(super) fn apply_metadata(&mut self, u: &Value) {
        if let Some(m) = u["modelId"]
            .as_str()
            .or_else(|| u["currentModelId"].as_str())
            .filter(|s| !s.trim().is_empty())
        {
            self.model = Some(m.to_string());
            self.model_observed = false;
        }
        if let Some(e) = u["effortLevel"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
        {
            self.effort = Some(e.to_string());
        }
        if let Some(mode) = u["currentModeId"]
            .as_str()
            .or_else(|| u["mode"].as_str())
            .filter(|s| !s.trim().is_empty())
        {
            self.mode = Some(mode.to_string());
        }

        let ctx_obj = u
            .get("contextUsage")
            .or_else(|| u.get("context_usage"))
            .or_else(|| u.get("context"))
            .unwrap_or(u);

        let used_tokens = ctx_obj["usedTokens"]
            .as_u64()
            .or_else(|| ctx_obj["used_tokens"].as_u64())
            .or_else(|| ctx_obj["contextTokens"].as_u64())
            .or_else(|| ctx_obj["tokens"].as_u64())
            .or_else(|| u["usedTokens"].as_u64())
            .or_else(|| u["used_tokens"].as_u64())
            .or_else(|| u["usage"]["total_tokens"].as_u64())
            .or_else(|| u["usage"]["input_tokens"].as_u64());

        let max_tokens = ctx_obj["maxTokens"]
            .as_u64()
            .or_else(|| ctx_obj["max_tokens"].as_u64())
            .or_else(|| ctx_obj["contextWindow"].as_u64())
            .or_else(|| ctx_obj["contextLimit"].as_u64())
            .or_else(|| ctx_obj["limit"].as_u64())
            .or_else(|| u["maxTokens"].as_u64())
            .or_else(|| u["max_tokens"].as_u64())
            .or_else(|| u["contextWindow"].as_u64());

        let raw_ctx_pct = u["contextUsagePercentage"]
            .as_f64()
            .or_else(|| ctx_obj["contextUsagePercentage"].as_f64())
            .or_else(|| ctx_obj["percentage"].as_f64())
            .or_else(|| ctx_obj["percent"].as_f64())
            .or_else(|| {
                if let (Some(used), Some(max)) = (used_tokens, max_tokens)
                    && max > 0
                {
                    Some((used as f64 / max as f64) * 100.0)
                } else {
                    None
                }
            });

        if let Some(pct) = raw_ctx_pct {
            let mut ctx = json!({
                "percent": (pct * 10.0).round() / 10.0,
                "percentage": pct,
                "contextUsagePercentage": pct,
                "basis": "Context Window",
            });
            if let Some(used) = used_tokens {
                ctx["used_tokens"] = json!(used);
                ctx["tokens"] = json!(used);
            }
            if let Some(max) = max_tokens {
                ctx["max_tokens"] = json!(max);
                ctx["limit"] = json!(max);
            }
            self.context_usage = Some(ctx);
        }

        let cred_obj = u
            .get("credits")
            .or_else(|| u.get("creditUsage"))
            .or_else(|| u.get("billing"))
            .or_else(|| u.get("usage").and_then(|v| v.get("credits")))
            .unwrap_or(u);

        let cred_used = cred_obj["used"]
            .as_f64()
            .or_else(|| cred_obj["creditsUsed"].as_f64())
            .or_else(|| cred_obj["creditUsed"].as_f64());
        let cred_limit = cred_obj["limit"]
            .as_f64()
            .or_else(|| cred_obj["creditsLimit"].as_f64())
            .or_else(|| cred_obj["creditLimit"].as_f64())
            .or_else(|| cred_obj["total"].as_f64());
        let cred_pct = cred_obj["percentage"]
            .as_f64()
            .or_else(|| cred_obj["creditUsagePercent"].as_f64())
            .or_else(|| cred_obj["percent"].as_f64())
            .or_else(|| u["creditUsagePercent"].as_f64())
            .or_else(|| u["creditUsagePercentage"].as_f64())
            .or_else(|| {
                if let (Some(used), Some(limit)) = (cred_used, cred_limit)
                    && limit > 0.0
                {
                    Some((used / limit) * 100.0)
                } else {
                    None
                }
            });

        let raw_reset = cred_obj
            .get("billingCycleReset")
            .or_else(|| cred_obj.get("resetsAt"))
            .or_else(|| cred_obj.get("resetAt"))
            .or_else(|| cred_obj.get("nextReset"))
            .or_else(|| cred_obj.get("currentPeriod").and_then(|p| p.get("end")))
            .or_else(|| u.get("billingCycleReset"))
            .or_else(|| u.get("resetsAt"));

        let normalized_reset = normalize_reset_value(raw_reset);
        let reset_remaining = format_reset_remaining(raw_reset);

        if cred_pct.is_some() || cred_used.is_some() || cred_limit.is_some() {
            let pct = cred_pct.unwrap_or(0.0);
            let mut billing = json!({
                "creditUsagePercent": (pct * 10.0).round() / 10.0,
                "percentage": pct,
            });
            if let Some(used) = cred_used {
                billing["used"] = json!(used);
            }
            if let Some(limit) = cred_limit {
                billing["limit"] = json!(limit);
            }
            if let Some(ref end_val) = normalized_reset {
                billing["billingCycleReset"] = end_val.clone();
                billing["currentPeriod"] = json!({ "end": end_val });
            }
            if let Some(ref rem) = reset_remaining {
                billing["resetRemaining"] = json!(rem);
            }
            self.billing = Some(billing);
        }

        let mut usage_obj = if self.usage.is_object() {
            self.usage.clone()
        } else {
            json!({})
        };
        if let Some(ref ctx) = self.context_usage {
            usage_obj["context"] = ctx.clone();
            usage_obj["context_usage"] = ctx.clone();
            if let Some(pct) = ctx.get("contextUsagePercentage") {
                usage_obj["contextUsagePercentage"] = pct.clone();
            }
        }
        if let Some(ref bill) = self.billing {
            usage_obj["billing"] = bill.clone();
            usage_obj["credits"] = bill.clone();
        }
        self.usage = usage_obj;
    }

    pub(super) fn handle_incoming_request(
        &mut self,
        id: Value,
        method: &str,
        params: &Value,
    ) -> Option<Value> {
        let id_str = match &id {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => "0".to_string(),
        };
        match method {
            "session/request_permission" => {
                let key = format!("perm-{id_str}");
                let title = params["toolCall"]["title"]
                    .as_str()
                    .or_else(|| params["toolCall"]["name"].as_str())
                    .or_else(|| params["title"].as_str())
                    .unwrap_or("工具权限请求");
                let desc = params["description"]
                    .as_str()
                    .or_else(|| params["message"].as_str());
                let input = params["toolCall"]
                    .get("rawInput")
                    .or_else(|| params["toolCall"].get("input"))
                    .or_else(|| params.get("input"))
                    .cloned();
                let mut options: Vec<Value> = params["options"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|o| {
                                let oid = o["optionId"]
                                    .as_str()
                                    .or_else(|| o["id"].as_str())
                                    .or_else(|| o["value"].as_str())?;
                                let label = o["name"]
                                    .as_str()
                                    .or_else(|| o["label"].as_str())
                                    .unwrap_or(oid);
                                let kind = o["kind"].as_str().unwrap_or("");
                                Some(json!({
                                    "id": oid,
                                    "optionId": oid,
                                    "label": label,
                                    "name": label,
                                    "value": oid,
                                    "kind": kind,
                                }))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if options.is_empty() {
                    options = vec![
                        json!({"id": "allow_once", "optionId": "allow_once", "label": "允许一次", "value": "allow_once", "kind": "allow_once"}),
                        json!({"id": "allow_always", "optionId": "allow_always", "label": "始终允许", "value": "allow_always", "kind": "allow_always"}),
                        json!({"id": "reject_once", "optionId": "reject_once", "label": "拒绝", "value": "reject_once", "kind": "reject_once"}),
                    ];
                }
                let mut card = json!({
                    "key": key,
                    "type": "approval",
                    "title": title,
                    "options": options,
                });
                if let Some(d) = desc {
                    card["description"] = json!(d);
                }
                if let Some(inp) = input
                    && !inp.is_null()
                {
                    card["input"] = inp;
                }
                if let Some(tc) = params.get("toolCall") {
                    card["toolCall"] = tc.clone();
                }
                self.requests
                    .insert(key, PendingRequest { rpc_id: id, card });
                self.publish();
                None
            }
            "_kiro/userInput" | "_kiro.dev/userInput" | "kiro/userInput" | "elicitation/create" => {
                let key = format!("user-{id_str}");
                let title = params["title"]
                    .as_str()
                    .or_else(|| params["question"].as_str())
                    .or_else(|| params["prompt"].as_str())
                    .or_else(|| params["message"].as_str())
                    .unwrap_or("需要输入");
                let desc = params["description"].as_str().or_else(|| {
                    if params.get("title").is_some() {
                        params["message"]
                            .as_str()
                            .or_else(|| params["prompt"].as_str())
                    } else {
                        None
                    }
                });
                let schema = params
                    .get("requestedSchema")
                    .or_else(|| params.get("schema"))
                    .cloned();

                let normalized_options = extract_question_options(params);
                let questions = if let Some(arr) = params["questions"].as_array() {
                    arr.iter()
                        .enumerate()
                        .map(|(idx, q)| {
                            let q_key = q["id"]
                                .as_str()
                                .or_else(|| q["key"].as_str())
                                .or_else(|| q["question"].as_str())
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("q_{idx}"));
                            let q_title = q["question"]
                                .as_str()
                                .or_else(|| q["prompt"].as_str())
                                .or_else(|| q["title"].as_str())
                                .or_else(|| q["message"].as_str())
                                .unwrap_or(title);
                            let q_opts = extract_question_options(q);
                            let multi = q["multiSelect"]
                                .as_bool()
                                .or_else(|| q["multiple"].as_bool())
                                .unwrap_or(false);
                            json!({
                                "id": q_key,
                                "key": q_key,
                                "question": q_title,
                                "title": q_title,
                                "options": q_opts,
                                "multiSelect": multi,
                            })
                        })
                        .collect::<Vec<Value>>()
                } else if schema.is_none() {
                    let q_key = params["id"]
                        .as_str()
                        .or_else(|| params["key"].as_str())
                        .unwrap_or("answer");
                    let multi = params["multiSelect"]
                        .as_bool()
                        .or_else(|| params["multiple"].as_bool())
                        .unwrap_or(false);
                    vec![json!({
                        "id": q_key,
                        "key": q_key,
                        "question": title,
                        "title": title,
                        "options": normalized_options,
                        "multiSelect": multi,
                    })]
                } else {
                    Vec::new()
                };

                let req_type = if schema.is_some() && questions.is_empty() {
                    "elicitation"
                } else {
                    "question"
                };
                let mut card = json!({
                    "key": key,
                    "type": req_type,
                    "title": title,
                });
                if let Some(d) = desc {
                    card["description"] = json!(d);
                }
                if !questions.is_empty() {
                    card["questions"] = json!(questions);
                }
                if !normalized_options.is_empty() {
                    card["options"] = json!(normalized_options);
                }
                if let Some(s) = schema {
                    card["requestedSchema"] = s;
                }
                self.requests
                    .insert(key, PendingRequest { rpc_id: id, card });
                self.publish();
                None
            }
            _ => Some(json!(null)),
        }
    }

    pub(super) fn append_chunk(&mut self, role: &str, chunk: &str, message_id: Option<&str>) {
        if let Some(mid) = message_id
            && let Some(existing_id) = self.message_item_ids.get(mid).cloned()
            && let Some(item) = self.items.iter_mut().find(|i| i["id"] == existing_id)
        {
            let prev = item["text"].as_str().unwrap_or("");
            let next_text = format!("{prev}{chunk}");
            item["text"] = json!(next_text);
            item["content"] = json!(next_text);
            return;
        }

        let target_id = if role == "thinking" {
            if let Some(ref id) = self.current_thinking_id {
                id.clone()
            } else {
                let id = self.alloc_item_id("think");
                let mut item = json!({
                    "id": id,
                    "role": "thinking",
                    "text": "",
                    "content": "",
                });
                if let Some(mid) = message_id {
                    item["message_id"] = json!(mid);
                    self.message_item_ids.insert(mid.to_string(), id.clone());
                }
                self.items.push(item);
                self.current_thinking_id = Some(id.clone());
                self.current_assistant_id = None;
                id
            }
        } else if let Some(ref id) = self.current_assistant_id {
            id.clone()
        } else {
            let id = self.alloc_item_id("asst");
            let mut item = json!({
                "id": id,
                "role": "assistant",
                "text": "",
                "content": "",
            });
            if let Some(mid) = message_id {
                item["message_id"] = json!(mid);
                self.message_item_ids.insert(mid.to_string(), id.clone());
            }
            self.items.push(item);
            self.current_assistant_id = Some(id.clone());
            id
        };

        if let Some(item) = self.items.iter_mut().find(|i| i["id"] == target_id) {
            let prev = item["text"].as_str().unwrap_or("");
            let next_text = format!("{prev}{chunk}");
            item["text"] = json!(next_text);
            item["content"] = json!(next_text);
        }
    }

    fn upsert_tool(&mut self, u: &Value) {
        let tc_id = u["toolCallId"]
            .as_str()
            .or_else(|| u["id"].as_str())
            .unwrap_or("tool")
            .to_string();
        let incoming_name = u["title"]
            .as_str()
            .or_else(|| u["name"].as_str())
            .or_else(|| u["toolName"].as_str())
            .or_else(|| u["kind"].as_str());
        let status = u["status"].as_str().unwrap_or("in_progress");
        let raw_input = u
            .get("rawInput")
            .or_else(|| u.get("input"))
            .or_else(|| u.get("arguments"))
            .cloned();
        let output = extract_tool_output(u);

        let is_new = !self.tool_item_ids.contains_key(&tc_id);
        let item_id = if let Some(id) = self.tool_item_ids.get(&tc_id) {
            id.clone()
        } else {
            let id = self.alloc_item_id("tool");
            self.tool_item_ids.insert(tc_id.clone(), id.clone());
            id
        };

        if is_new {
            self.current_assistant_id = None;
            self.current_thinking_id = None;
        }

        if let Some(entry) = self.items.iter_mut().find(|i| i["id"] == item_id) {
            if let Some(name) = incoming_name {
                entry["name"] = json!(name);
                entry["tool_name"] = json!(name);
            }
            entry["status"] = json!(status);
            if let Some(inp) = raw_input
                && !inp.is_null()
            {
                entry["input"] = inp.clone();
                entry["tool_input"] = inp;
            }
            if let Some(out) = output
                && !out.is_null()
            {
                entry["result"] = out.clone();
                entry["tool_result"] = out;
                entry["error"] = json!(status == "failed" || status == "error");
            }
            if let Some(locs) = u.get("locations") {
                entry["locations"] = locs.clone();
            }
            if let Some(kind) = u["kind"].as_str() {
                entry["kind"] = json!(kind);
            }
        } else {
            let name = incoming_name.unwrap_or("tool");
            let mut entry = json!({
                "id": item_id,
                "toolCallId": tc_id,
                "role": "tool",
                "name": name,
                "tool_name": name,
                "status": status,
            });
            if let Some(inp) = raw_input
                && !inp.is_null()
            {
                entry["input"] = inp.clone();
                entry["tool_input"] = inp;
            }
            if let Some(out) = output
                && !out.is_null()
            {
                entry["result"] = out.clone();
                entry["tool_result"] = out;
                entry["error"] = json!(status == "failed" || status == "error");
            }
            if let Some(locs) = u.get("locations") {
                entry["locations"] = locs.clone();
            }
            if let Some(kind) = u["kind"].as_str() {
                entry["kind"] = json!(kind);
            }
            self.items.push(entry);
        }
    }

    fn upsert_plan(&mut self, u: &Value) {
        let Some(entries) = u.get("entries").and_then(|v| v.as_array()) else {
            return;
        };
        let mut lines = Vec::new();
        for e in entries {
            let content = e["content"]
                .as_str()
                .or_else(|| e["title"].as_str())
                .unwrap_or("");
            let status = e["status"].as_str().unwrap_or("pending");
            if !content.is_empty() {
                lines.push(format!("[{status}] {content}"));
            }
        }
        let text = lines.join("\n");
        if text.is_empty() {
            return;
        }
        if let Some(existing) = self.items.iter_mut().find(|i| i["id"] == "plan-1") {
            existing["text"] = json!(text);
            existing["content"] = json!(text);
        } else {
            self.items.push(json!({
                "id": "plan-1",
                "role": "thinking",
                "text": text,
                "content": text,
            }));
        }
    }

    pub(super) fn snapshot(&self, uid: &str, cwd: &str) -> Value {
        let mut requests: Vec<Value> = self.requests.values().map(|r| r.card.clone()).collect();
        requests.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
        let capabilities = json!({
            "send": true,
            "models": true,
            "settings": true,
            "usage": true,
            "billing": true,
            "interrupt": true,
            "steer": true,
            "compact": true,
            "fork": true,
            "rename": true,
            "revert": false,
            "rewind": false,
            "permissions": true,
            "approval": true,
            "elicitation": true,
            "question": true,
            "Question": true,
        });
        let mut raw = json!({
            "uid": uid,
            "session_id": self.session_id,
            "cwd": cwd,
            "running": self.running,
            "items": self.items,
            "requests": requests,
            "last_turn": self.last_turn,
            "runtime_error": self.runtime_error,
            "retry": self.retry,
            "compact": self.compact,
            "configOptions": self.config_options,
            "modes": self.modes,
            "mode": self.mode,
            "model": self.model,
            "model_observed": self.model_observed,
            "effort": self.effort,
            "permission_mode": self.permission_mode,
            "autopilot": self.autopilot,
            "settings": {
                "model": self.model,
                "effort": self.effort,
                "mode": self.mode,
                "permission_mode": self.permission_mode,
                "autopilot": self.autopilot,
            },
            "commands": self.commands,
            "usage": self.usage,
            "billing": self.billing,
            "context_usage": self.context_usage,
            "agentCapabilities": self.agent_capabilities,
            "capabilities": capabilities,
        });
        if let Some(ref status) = self.status {
            raw["status"] = json!(status);
        }
        if let Some(ref title) = self.title {
            raw["title"] = json!(title);
        }
        json!({
            "seq": self.seq,
            "type": "snapshot",
            "uid": uid,
            "session_id": self.session_id,
            "cwd": cwd,
            "capabilities": capabilities,
            "raw": raw,
        })
    }
}

fn extract_question_options(obj: &Value) -> Vec<Value> {
    let Some(arr) = obj
        .get("options")
        .or_else(|| obj.get("choices"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            if let Some(s) = item.as_str() {
                Some(json!({
                    "id": s,
                    "label": s,
                    "value": s,
                }))
            } else if item.is_object() {
                let val = item["value"]
                    .as_str()
                    .or_else(|| item["id"].as_str())
                    .or_else(|| item["optionId"].as_str())
                    .or_else(|| item["label"].as_str())
                    .or_else(|| item["name"].as_str())?;
                let label = item["label"]
                    .as_str()
                    .or_else(|| item["name"].as_str())
                    .or_else(|| item["title"].as_str())
                    .unwrap_or(val);
                let mut row = json!({
                    "id": val,
                    "label": label,
                    "value": val,
                });
                if let Some(desc) = item["description"].as_str() {
                    row["description"] = json!(desc);
                }
                Some(row)
            } else {
                None
            }
        })
        .collect()
}

fn extract_tool_output(u: &Value) -> Option<Value> {
    if let Some(raw) = u.get("rawOutput")
        && !raw.is_null()
    {
        return Some(raw.clone());
    }
    if let Some(out) = u.get("output")
        && !out.is_null()
    {
        return Some(out.clone());
    }
    if let Some(res) = u.get("result")
        && !res.is_null()
    {
        return Some(res.clone());
    }
    if let Some(content) = u.get("content")
        && !content.is_null()
    {
        if let Some(s) = content.as_str() {
            return Some(Value::String(s.to_string()));
        }
        if let Some(arr) = content.as_array() {
            let mut parts = Vec::new();
            for item in arr {
                if let Some(t) = item["content"]["text"]
                    .as_str()
                    .or_else(|| item["text"].as_str())
                {
                    parts.push(t.to_string());
                } else if item["type"].as_str() == Some("diff") {
                    let path = item["path"].as_str().unwrap_or("");
                    let new_text = item["newText"].as_str().unwrap_or("");
                    if !path.is_empty() || !new_text.is_empty() {
                        parts.push(format!("{path}\n{new_text}").trim().to_string());
                    }
                }
            }
            if !parts.is_empty() {
                return Some(Value::String(parts.join("\n")));
            }
            if !arr.is_empty() {
                return Some(content.clone());
            }
        }
    }
    None
}

fn normalize_reset_value(raw: Option<&Value>) -> Option<Value> {
    let val = raw?;
    if let Some(n) = val.as_f64() {
        if n > 0.0 && n < 10_000_000_000.0 {
            return Some(json!((n * 1000.0) as i64));
        }
        return Some(val.clone());
    }
    if let Some(s) = val.as_str()
        && !s.trim().is_empty()
    {
        return Some(Value::String(s.trim().to_string()));
    }
    None
}

fn format_reset_remaining(raw: Option<&Value>) -> Option<String> {
    let val = raw?;
    let target_secs = if let Some(n) = val.as_f64() {
        if n > 10_000_000_000.0 {
            (n / 1000.0) as i64
        } else {
            n as i64
        }
    } else if let Some(s) = val.as_str() {
        chrono::DateTime::parse_from_rfc3339(s.trim())
            .ok()
            .map(|dt| dt.timestamp())?
    } else {
        return None;
    };
    let now = now_secs() as i64;
    let rem = target_secs - now;
    if rem <= 0 {
        return Some("已到期".to_string());
    }
    let mins = rem / 60;
    let days = mins / 1440;
    let hours = (mins / 60) % 24;
    let rem_mins = mins % 60;
    if days > 0 {
        Some(format!("{days}d {hours:02}:{rem_mins:02}"))
    } else {
        Some(format!("{hours:02}:{rem_mins:02}"))
    }
}

pub(super) fn handle_line(session: &Arc<Session>, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
        return;
    };
    session.touch();

    // JSON-RPC response to a request we sent
    if val.get("method").is_none()
        && let Some(id) = val.get("id").and_then(|v| v.as_u64())
    {
        if let Some(tx) = session.pending.lock().unwrap().remove(&id) {
            if let Some(err) = val.get("error") {
                let msg = err["message"]
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| err.to_string());
                let _ = tx.send(Err(msg));
            } else {
                let _ = tx.send(Ok(val.get("result").cloned().unwrap_or(Value::Null)));
            }
        }
        return;
    }

    // Incoming JSON-RPC request from agent (has both id and method)
    if let (Some(id), Some(method)) = (
        val.get("id").cloned(),
        val.get("method").and_then(|v| v.as_str()),
    ) {
        let params = val.get("params").cloned().unwrap_or(Value::Null);
        let auto_reply = {
            let mut st = session.state.lock().unwrap();
            st.handle_incoming_request(id.clone(), method, &params)
        };
        if let Some(reply) = auto_reply {
            let s = session.clone();
            tokio::spawn(async move {
                let _ = s.respond_rpc(id, reply).await;
            });
        }
        return;
    }

    // Incoming JSON-RPC notification from agent
    if let Some(method) = val.get("method").and_then(|v| v.as_str()) {
        let params = val.get("params").cloned().unwrap_or(Value::Null);
        let mut st = session.state.lock().unwrap();
        st.handle_notification(method, &params);
    }
}

pub(super) fn mark_exit(session: &Arc<Session>, reason: &str) {
    let pending: Vec<_> = session
        .pending
        .lock()
        .unwrap()
        .drain()
        .map(|(_, tx)| tx)
        .collect();
    for tx in pending {
        let _ = tx.send(Err(reason.to_string()));
    }
    let mut st = session.state.lock().unwrap();
    if st.running {
        st.running = false;
        st.last_turn = Some(json!({
            "status": "failed",
            "error": reason,
        }));
    }
    st.runtime_error = Some(reason.to_string());
    st.publish();
}

fn extract_chunk_text(u: &Value) -> String {
    if let Some(s) = u["content"]["text"].as_str() {
        return s.to_string();
    }
    if let Some(s) = u["text"].as_str() {
        return s.to_string();
    }
    if let Some(s) = u["content"].as_str() {
        return s.to_string();
    }
    if let Some(arr) = u["content"].as_array() {
        let mut out = String::new();
        for item in arr {
            if let Some(t) = item["text"]
                .as_str()
                .or_else(|| item["content"]["text"].as_str())
            {
                out.push_str(t);
            }
        }
        return out;
    }
    String::new()
}

fn extract_result_text(result: &Value) -> Option<String> {
    if let Some(s) = result["text"].as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = result["content"].as_array() {
        let mut out = String::new();
        for c in arr {
            if let Some(t) = c["text"].as_str() {
                out.push_str(t);
            }
        }
        if !out.is_empty() {
            return Some(out);
        }
    }
    None
}
