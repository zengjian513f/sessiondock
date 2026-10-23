use super::checkpoint::{self, Rpc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::transport::{Session, now_secs};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PendingRequest {
    pub(super) rpc_id: Value,
    pub(super) card: Value,
}

#[derive(Serialize, Deserialize)]
pub(super) struct LiveState {
    #[serde(default)]
    transcript_version: u8,
    #[serde(default)]
    pub(super) subtasks: std::collections::BTreeMap<String, Subtask>,
    #[serde(default)]
    pub(super) journal: Vec<Rpc>,
    #[serde(default)]
    pub(super) next_rpc: u64,
    #[serde(default)]
    pub(super) next_request_id: u64,
    #[serde(default)]
    pub(super) active_prompt: Option<u64>,
    #[serde(default)]
    pub(super) stdout_offset: u64,
    #[serde(default)]
    pub(super) stderr_offset: u64,
    #[serde(default)]
    pub(super) stderr_tail: Vec<String>,
    #[serde(default)]
    pub(super) initialized: bool,
    #[serde(default)]
    pub(super) error: Option<String>,
    #[serde(default)]
    pub(super) error_kind: Option<String>,
    #[serde(skip)]
    pub(super) checkpoint: Option<PathBuf>,
    #[serde(skip)]
    pub(super) checkpoint_error: Option<String>,
    #[serde(skip)]
    pub(super) observing: bool,
    pub(super) uid: String,
    pub(super) cwd: String,
    pub(super) session_id: String,
    pub(super) title: Option<String>,
    pub(super) running: bool,
    pub(super) loading: bool,
    pub(super) interrupted_turn: bool,
    pub(super) items: Vec<Value>,
    /// `item["id"]` -> its index in `items`, so replay/live reducers locate the
    /// row to mutate in O(1) instead of a linear scan (the scan made cold-start
    /// `session/load` replay O(n^2)). `items` is append-only between bulk
    /// replacements, so indices stay valid; the map is rebuilt lazily whenever
    /// it falls out of sync with `items` (after checkpoint load, a wholesale
    /// `items` replacement, or a new-turn user push). Not persisted.
    #[serde(skip)]
    pub(super) item_index: HashMap<String, usize>,
    pub(super) requests: HashMap<String, PendingRequest>,
    pub(super) last_turn: Option<Value>,
    pub(super) runtime_error: Option<String>,
    #[serde(skip)]
    pub(super) retry: Option<Value>,
    #[serde(skip)]
    pub(super) model_retry: super::retry::Retry,
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
    #[serde(skip, default = "events")]
    pub(super) events: broadcast::Sender<Arc<str>>,
    next_item_id: u64,
    pub(super) current_assistant_id: Option<String>,
    pub(super) current_thinking_id: Option<String>,
    tool_item_ids: HashMap<String, String>,
    message_item_ids: HashMap<String, String>,
    replay_ids: HashSet<String>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Subtask {
    #[serde(default)]
    tool_call_id: Option<String>,
    pub(super) title: String,
    pub(super) kind: String,
    pub(super) status: String,
    pub(super) state: LiveState,
}

fn events() -> broadcast::Sender<Arc<str>> {
    broadcast::channel(256).0
}

impl LiveState {
    pub(super) fn new() -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            transcript_version: 2,
            subtasks: Default::default(),
            journal: Vec::new(),
            next_rpc: 1,
            next_request_id: 0,
            active_prompt: None,
            stdout_offset: 0,
            stderr_offset: 0,
            stderr_tail: Vec::new(),
            initialized: false,
            error: None,
            error_kind: None,
            checkpoint: None,
            checkpoint_error: None,
            observing: false,
            uid: String::new(),
            cwd: String::new(),
            session_id: String::new(),
            title: None,
            running: false,
            loading: false,
            interrupted_turn: false,
            items: Vec::new(),
            item_index: HashMap::new(),
            requests: HashMap::new(),
            last_turn: None,
            runtime_error: None,
            retry: None,
            model_retry: Default::default(),
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

    /// Upgrade only the read projection using already-received sdkhost frames.
    /// Delivery, prompts, permissions and stdout checkpoints remain authoritative.
    pub(super) fn upgrade_transcript(&mut self, stdout: &std::path::Path) -> Result<(), String> {
        use std::io::{BufRead, BufReader, Read};
        if self.transcript_version >= 2 {
            return Ok(());
        }
        if !self.items.iter().any(|item| {
            item["toolCallId"]
                .as_str()
                .is_some_and(|id| id.starts_with("invoke_subagent_"))
        }) {
            self.transcript_version = 2;
            return Ok(());
        }
        let file = std::fs::File::open(stdout).map_err(|e| e.to_string())?;
        let prompts: Vec<_> = self
            .journal
            .iter()
            .filter(|rpc| {
                rpc.frame["method"] == "session/prompt"
                    && rpc.delivery.as_deref() == Some("written")
            })
            .collect();
        let mut replay = Self::new();
        replay.observing = true;
        replay.session_id = self.session_id.clone();
        let mut prompt_index = 0;
        let mut started = false;
        for line in BufReader::new(file.take(self.stdout_offset)).lines() {
            let line = line.map_err(|e| e.to_string())?;
            let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if frame["method"] == "session/update" {
                let u = &frame["params"]["update"];
                if !started
                    && u["_meta"]["kiro"]["replay"] != true
                    && matches!(
                        u["sessionUpdate"].as_str(),
                        Some("agent_message_chunk" | "agent_thought_chunk" | "tool_call")
                    )
                    && let Some(rpc) = prompts.get(prompt_index)
                {
                    replay.replay_prompt(rpc);
                    started = true;
                }
                replay.handle_notification("session/update", &frame["params"]);
            } else if frame.get("method").is_none()
                && prompts
                    .get(prompt_index)
                    .is_some_and(|rpc| rpc.id == frame["id"].as_u64())
            {
                if !started {
                    replay.replay_prompt(prompts[prompt_index]);
                }
                if frame.get("result").is_some() {
                    replay.finish_turn(&frame["result"]);
                }
                replay.current_assistant_id = None;
                replay.current_thinking_id = None;
                replay.running = false;
                prompt_index += 1;
                started = false;
            }
        }
        if !started && let Some(rpc) = prompts.get(prompt_index) {
            replay.replay_prompt(rpc);
        }
        // No native subtask evidence: retain the old projection instead of guessing.
        if !replay.subtasks.is_empty() {
            self.items = replay.items;
            self.subtasks = replay.subtasks;
            self.next_item_id = replay.next_item_id;
            self.current_assistant_id = replay.current_assistant_id;
            self.current_thinking_id = replay.current_thinking_id;
            self.tool_item_ids = replay.tool_item_ids;
            self.message_item_ids = replay.message_item_ids;
            self.replay_ids = replay.replay_ids;
        }
        self.transcript_version = 2;
        Ok(())
    }

    fn replay_prompt(&mut self, rpc: &Rpc) {
        let text = rpc.frame["params"]["prompt"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        self.begin_turn(&text, rpc.request_key.as_deref(), None, None);
    }

    pub(super) fn alloc_item_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}-{}", self.next_item_id);
        self.next_item_id += 1;
        id
    }

    pub(super) fn persist(&mut self) -> Result<(), String> {
        if let Some(path) = self.checkpoint.clone() {
            if let Err(error) = checkpoint::save(&path, self) {
                self.checkpoint_error = Some(error.clone());
                self.runtime_error = Some(error.clone());
                return Err(error);
            }
        }
        self.checkpoint_error = None;
        Ok(())
    }

    pub(super) fn publish(&mut self) {
        self.seq += 1;
        if self.observing {
            return;
        }
        if self.persist().is_err() {
            return;
        }
        self.broadcast();
    }

    pub(super) fn broadcast(&self) {
        // Public SSE frame: windowed items, no subtask transcripts.
        let snap = self.snapshot(&self.uid, &self.cwd, true);
        let _ = self.events.send(Arc::from(snap.to_string()));
    }

    pub(super) fn supports_method(&self, method: &str) -> bool {
        if let Some(name) = method.strip_prefix("session/") {
            if matches!(name, "list" | "fork" | "delete" | "rename") {
                return self.agent_capabilities["sessionCapabilities"][name].is_object();
            }
        }
        self.agent_capabilities
            .pointer("/_meta/kiro/extensionMethods")
            .and_then(Value::as_array)
            .is_some_and(|methods| methods.iter().any(|entry| entry.as_str() == Some(method)))
    }

    pub(super) fn apply_initialize_result(&mut self, res: &Value) {
        self.initialized = true;
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

    pub(super) fn apply_config_options(&mut self, arr: &[Value]) {
        self.config_options = arr.to_vec();
        self.sync_from_config_options();
    }

    pub(super) fn sync_from_config_options(&mut self) {
        // Config options are the native authority; an omitted control must not
        // retain an effort value from a previous model or creation draft.
        self.effort = None;
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
        if !self.effort_supported() {
            self.effort = None;
        }
    }

    pub(super) fn effort_supported(&self) -> bool {
        // Schema 1: a standalone effort config option (Codex-style).
        let advertised = self.config_options.iter().any(|option| {
            let id = option["id"]
                .as_str()
                .or_else(|| option["configId"].as_str())
                .unwrap_or("");
            let category = option["category"].as_str().unwrap_or("");
            ["effortLevel", "effort", "reasoningEffort"]
                .iter()
                .any(|name| id.eq_ignore_ascii_case(name))
                || ["thought_level", "effort"]
                    .iter()
                    .any(|name| category.eq_ignore_ascii_case(name))
        });
        if advertised {
            return !self
                .config_options
                .iter()
                .filter_map(|option| option["options"].as_array())
                .flatten()
                .any(|option| {
                    option["value"].as_str().or_else(|| option["id"].as_str())
                        == self.model.as_deref()
                        && option["_meta"]["kiro"]["hasEffort"] == false
                });
        }
        // Schema 2: Kiro embeds effort per model via the model option's
        // `_meta.kiro` (`hasEffort` + `effortLevels`). Effort is selectable
        // when the currently selected model advertises non-empty levels. The
        // native set/echo path for this schema is not yet verified against the
        // real ACP, so the live footer gate is handled separately; here we only
        // report selectability for the picker surfaces.
        self.config_options
            .iter()
            .filter_map(|option| option["options"].as_array())
            .flatten()
            .any(|option| {
                let is_current = option["value"].as_str().or_else(|| option["id"].as_str())
                    == self.model.as_deref();
                let meta = &option["_meta"]["kiro"];
                is_current
                    && meta["hasEffort"].as_bool() != Some(false)
                    && meta["effortLevels"]
                        .as_array()
                        .is_some_and(|levels| !levels.is_empty())
            })
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
            && arr[0]
                .get("id")
                .or_else(|| arr[0].get("configId"))
                .is_some()
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
        self.error = None;
        self.error_kind = None;
        self.retry = None;
        if let Some(m) = model.filter(|s| !s.is_empty()) {
            self.model = Some(m);
            self.model_observed = false;
        }
        if let Some(e) = effort.filter(|s| !s.is_empty() && self.effort_supported()) {
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
        self.push_item(user_item);
        self.current_assistant_id = None;
        self.current_thinking_id = None;
        self.tool_item_ids.clear();
        self.message_item_ids.clear();
        self.last_turn = Some(json!({ "status": "in_progress" }));
        self.publish();
    }

    pub(super) fn finish_turn(&mut self, result: &Value) {
        self.cancel_model_retry();
        self.running = false;
        let stop = result["stopReason"].as_str().unwrap_or("unknown");
        let is_interrupted = stop == "cancelled" || stop == "interrupted";
        let status = if is_interrupted {
            "interrupted"
        } else {
            match stop {
                "end_turn" => "completed",
                "max_tokens" | "max_turn_requests" | "refusal" => "incomplete",
                _ => "unknown",
            }
        };
        if status != "completed" {
            self.error = Some(format!("Kiro prompt stopped: {stop}"));
            self.error_kind = Some(
                if is_interrupted {
                    "interrupted"
                } else {
                    status
                }
                .into(),
            );
        }
        self.last_turn = Some(json!({
            "status": status,
            "stopReason": stop,
            "turn_id": self.active_prompt,
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
                    item["role"] == "assistant" && !item["text"].as_str().unwrap_or("").is_empty()
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
        self.error = Some(err.to_string());
        self.error_kind = Some("turn".into());
        if err.contains("cancelled") || err.contains("interrupted") || err.contains("Canceled") {
            self.error_kind = Some("interrupted".into());
            self.last_turn = Some(json!({
                "status": "interrupted",
                "stopReason": "cancelled",
            }));
        } else {
            self.last_turn = Some(json!({
                "status": "failed",
                "error": err,
                "turn_id": self.active_prompt,
            }));
        }
        self.current_assistant_id = None;
        self.current_thinking_id = None;
        self.publish();
    }

    fn interrupt_subtasks(&mut self) {
        let mut interrupted_calls = HashSet::new();
        for task in self.subtasks.values_mut() {
            if matches!(task.status.as_str(), "pending" | "in_progress") {
                task.status = "interrupted".into();
                if let Some(id) = &task.tool_call_id {
                    interrupted_calls.insert(id.clone());
                }
            }
        }
        for item in &mut self.items {
            if item["toolCallId"]
                .as_str()
                .is_some_and(|id| interrupted_calls.contains(id))
            {
                item["status"] = json!("interrupted");
            }
        }
    }

    pub(super) fn mark_interrupted(&mut self) {
        // A cancellation request is an intent; only this prompt's response ends it.
        self.interrupted_turn = true;
        self.publish();
    }

    pub(super) fn handle_notification(&mut self, method: &str, params: &Value) {
        if params["sessionId"]
            .as_str()
            .is_some_and(|sid| !self.session_id.is_empty() && sid != self.session_id)
        {
            return;
        }
        match method {
            "_kiro.dev/metadata" | "_kiro/metadata" | "kiro/metadata" | "session/metadata"
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
        // Kiro multiplexes subtask streams on the parent's native session ID.
        // Route before reducing chunks or metadata so parallel agents cannot
        // concatenate text or overwrite the parent's usage/settings.
        if let Some(id) = u["_meta"]["kiro"]["agentSubtaskId"].as_str() {
            let task = self.subtasks.entry(id.to_owned()).or_insert_with(|| {
                let mut state = LiveState::new();
                state.observing = true;
                Subtask {
                    tool_call_id: None,
                    title: String::new(),
                    kind: "subagent".into(),
                    status: "in_progress".into(),
                    state,
                }
            });
            task.state.observing = true;
            if u["_meta"]["kiro"]["kind"] == "agent-subtask" {
                if let Some(id) = u["toolCallId"].as_str() {
                    task.tool_call_id = Some(id.to_owned());
                }
                if let Some(kind) = u["rawInput"]["name"].as_str() {
                    task.kind = kind.into();
                }
                if let Some(title) = u["rawInput"]["explanation"]
                    .as_str()
                    .or_else(|| u["title"].as_str())
                {
                    if task.title.is_empty() || u["rawInput"]["explanation"].is_string() {
                        task.title = title.into();
                    }
                }
                if let Some(status) = u["status"].as_str() {
                    task.status = status.into();
                }
                // The invocation and its result belong to the parent transcript.
            } else {
                let mut routed = params.clone();
                if let Some(meta) = routed["update"]["_meta"]["kiro"].as_object_mut() {
                    meta.remove("agentSubtaskId");
                }
                task.state.handle_notification(method, &routed);
                self.publish();
                return;
            }
        }
        // Replay completion frames can omit both subtask metadata fields.
        // Correlate the exact invocation ID; nested tools never end their owner.
        if u["_meta"]["kiro"]["agentSubtaskId"].is_null()
            && let Some(id) = u["toolCallId"].as_str()
            && let Some(status) = u["status"].as_str()
            && let Some(task) = self
                .subtasks
                .values_mut()
                .find(|task| task.tool_call_id.as_deref() == Some(id))
        {
            task.status = status.to_owned();
        }
        let kind = u["sessionUpdate"]
            .as_str()
            .or_else(|| u["type"].as_str())
            .unwrap_or("");
        let is_replay = self.loading
            || params["_meta"]["kiro"]["replay"].as_bool() == Some(true)
            || u["_meta"]["kiro"]["replay"].as_bool() == Some(true)
            || params["_meta"]["isReplay"].as_bool() == Some(true)
            || u["_meta"]["isReplay"].as_bool() == Some(true)
            || params["isReplay"].as_bool() == Some(true)
            || u["isReplay"].as_bool() == Some(true);

        let msg_id = u["messageId"]
            .as_str()
            .or_else(|| u["content"]["messageId"].as_str())
            .or_else(|| params["messageId"].as_str())
            .or_else(|| u["_meta"]["kiro"]["messageId"].as_str())
            .or_else(|| params["_meta"]["kiro"]["messageId"].as_str())
            .or_else(|| u["_meta"]["kiro"]["replayId"].as_str())
            .map(str::to_string);

        match kind {
            "user_message_chunk" => {
                if is_replay || !self.running {
                    let text = extract_chunk_text(u);
                    if !text.is_empty() {
                        self.append_chunk("user", &text, msg_id.as_deref());
                        self.publish();
                    }
                }
                return;
            }
            "agent_message_chunk" => {
                let text = extract_chunk_text(u);
                if !text.is_empty() {
                    self.append_chunk("assistant", &text, msg_id.as_deref());
                    self.publish();
                }
                return;
            }
            "agent_thought_chunk" => {
                let text = extract_chunk_text(u);
                if !text.is_empty() {
                    self.append_chunk("thinking", &text, msg_id.as_deref());
                    self.publish();
                }
                return;
            }
            "tool_call" | "tool_call_update" => {
                self.upsert_tool(u);
                self.publish();
                return;
            }
            "plan" => {
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
                // Historical turn-end evidence closes historical unfinished tasks,
                // but cannot release or overwrite a current prompt's FIFO ownership.
                if is_replay
                    && !self.running
                    && self.active_prompt.is_none()
                    && u["_meta"]["kiro"]["kind"] == "turn_end"
                {
                    self.interrupt_subtasks();
                }
                self.apply_metadata(u);
                self.publish();
            }
            // Uncorrelated notifications cannot complete a newer prompt.
            "turn_end" => {}
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
        if let Some(meta) = u.pointer("/_meta/kiro").or_else(|| u.get("kiro")) {
            self.apply_metadata(meta);
        }
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
            .filter(|s| !s.trim().is_empty() && self.effort_supported())
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
            .or_else(|| ctx_obj["usagePercentage"].as_f64())
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

        let context_evidence = u.get("contextUsage").is_some()
            || u.get("context_usage").is_some()
            || u.get("context").is_some()
            || u.get("contextUsagePercentage").is_some()
            || u["kind"] == "context_usage"
            || used_tokens.is_some()
            || max_tokens.is_some();
        if let Some(pct) = raw_ctx_pct.filter(|_| context_evidence) {
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

        let billing_evidence = u.get("credits").is_some()
            || u.get("creditUsage").is_some()
            || u.get("billing").is_some()
            || u.pointer("/usage/credits").is_some()
            || u.get("creditUsagePercent").is_some()
            || u.get("creditUsagePercentage").is_some()
            || u.get("creditsUsed").is_some()
            || u.get("creditsLimit").is_some();
        if billing_evidence && (cred_pct.is_some() || cred_used.is_some() || cred_limit.is_some()) {
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
            Value::String(s) => format!("s-{s}"),
            _ => "0".to_string(),
        };
        // The native peer may reuse a JSON-RPC id after its previous request
        // finishes. UI answers belong to this particular request occurrence.
        if self.requests.values().any(|pending| pending.rpc_id == id) {
            return None;
        }
        self.next_request_id += 1;
        let scoped_id = format!(
            "{}-{}-{id_str}",
            self.active_prompt.unwrap_or(0),
            self.next_request_id
        );
        match method {
            "session/request_permission" => {
                let key = format!("perm-{scoped_id}");
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
                let options: Vec<Value> = params["options"]
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
                                    "_meta": o["_meta"],
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
                let mut card = json!({
                    "key": key,
                    "type": "approval",
                    "title": title,
                    "options": options,
                    "_meta": params["_meta"],
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
                let key = format!("user-{scoped_id}");
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

    /// Append an item and record its id index. All pushes to `self.items` in
    /// the reducers go through here so `item_index` stays in sync, keeping
    /// later lookups O(1).
    fn push_item(&mut self, item: Value) {
        if let Some(id) = item["id"].as_str() {
            self.item_index.insert(id.to_owned(), self.items.len());
        }
        self.items.push(item);
    }

    /// O(1) mutable access to the item with `id`. Rebuilds `item_index` once if
    /// it is stale (after checkpoint load, a wholesale `items` replacement, or a
    /// new-turn user push that did not go through `push_item`), then looks up.
    fn item_mut(&mut self, id: &str) -> Option<&mut Value> {
        if self.item_index.len() != self.items.len() {
            self.item_index = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| item["id"].as_str().map(|id| (id.to_owned(), i)))
                .collect();
        }
        let idx = *self.item_index.get(id)?;
        // Defend against any index drift: verify the row still matches.
        match self.items.get(idx) {
            Some(item) if item["id"].as_str() == Some(id) => self.items.get_mut(idx),
            _ => {
                let found = self
                    .items
                    .iter()
                    .position(|item| item["id"].as_str() == Some(id))?;
                self.item_index.insert(id.to_owned(), found);
                self.items.get_mut(found)
            }
        }
    }

    pub(super) fn append_chunk(&mut self, role: &str, chunk: &str, message_id: Option<&str>) {
        if let Some(mid) = message_id
            && let Some(existing_id) = self.message_item_ids.get(mid).cloned()
            && let Some(item) = self.item_mut(&existing_id)
        {
            append_text(item, chunk);
            return;
        }

        if role == "user" {
            self.current_assistant_id = None;
            self.current_thinking_id = None;
            let existing = if message_id.is_none() {
                self.items
                    .last_mut()
                    .filter(|item| item["role"] == "user" && item.get("message_id").is_none())
            } else {
                None
            };
            if let Some(item) = existing {
                append_text(item, chunk);
            } else {
                let id = self.alloc_item_id("user");
                let mut item = json!({"id":id,"role":"user","text":chunk,"content":chunk});
                if let Some(mid) = message_id {
                    item["message_id"] = json!(mid);
                    self.message_item_ids.insert(mid.into(), id);
                }
                self.push_item(item);
            }
            return;
        }
        if message_id.is_some() {
            // An unseen native message ID starts a new item, even when text matches.
            if role == "thinking" {
                self.current_thinking_id = None;
            } else {
                self.current_assistant_id = None;
            }
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
                self.push_item(item);
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
            self.push_item(item);
            self.current_assistant_id = Some(id.clone());
            id
        };

        if let Some(item) = self.item_mut(&target_id) {
            append_text(item, chunk);
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

        if let Some(entry) = self.item_mut(&item_id) {
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
            self.push_item(entry);
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
        if let Some(existing) = self.item_mut("plan-1") {
            existing["text"] = json!(text);
            existing["content"] = json!(text);
        } else {
            self.push_item(json!({
                "id": "plan-1",
                "role": "thinking",
                "text": text,
                "content": text,
            }));
        }
    }

    /// Live items carried in a wire snapshot. Earlier history is served by the
    /// paginated `/api/messages` endpoint, which stitches in front of these live
    /// items at the first live user turn. Sending the whole transcript in every
    /// SSE frame was the bulk of the payload; cap it to a recent window like the
    /// terminal view.
    ///
    /// The window is counted in USER TURNS (not raw items) and begins on a user
    /// boundary, because the client dedups history against the first live user
    /// turn by identity. `MAX_TURNS` stays well under the history window's tail
    /// (`pages::WINDOW_TAIL_EVENTS` = 200 events, i.e. far more than this many
    /// turns) so that first live user turn is always present in `/api/messages`
    /// and the seam never gaps.
    fn windowed_items(&self) -> &[Value] {
        const MAX_TURNS: usize = 40;
        // Walk backward counting user-turn starts; stop once we've included
        // MAX_TURNS of them (or reach the beginning).
        let mut turns = 0usize;
        let mut start = self.items.len();
        for (i, item) in self.items.iter().enumerate().rev() {
            if item["role"].as_str() == Some("user") {
                turns += 1;
                start = i;
                if turns >= MAX_TURNS {
                    break;
                }
            }
        }
        // No user turn at all (or fewer than the cap): send everything.
        if start == self.items.len() {
            return &self.items;
        }
        &self.items[start..]
    }

    /// `wire` selects the payload shape:
    /// - `false` (projection): the COMPLETE transcript. SessionDock's durable
    ///   message store and subagent projections are written from this, and
    ///   `/api/messages` serves history from that store, so it must be full.
    /// - `true` (SSE broadcast / open response): a recent windowed `items` tail
    ///   and subtask metadata only, so the public link does not carry the whole
    ///   (multi-MB) transcript on every frame. Earlier history is fetched by the
    ///   client from `/api/messages`.
    pub(super) fn snapshot(&self, uid: &str, cwd: &str, wire: bool) -> Value {
        let mut requests: Vec<Value> = self.requests.values().map(|r| r.card.clone()).collect();
        requests.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
        let capabilities = json!({
            "send": true,
            "models": !self.config_options.is_empty() || !self.models.is_null(),
            "settings": !self.config_options.is_empty() || !self.modes.is_null(),
            "effort": self.effort_supported(),
            "usage": self.supports_method("_kiro/session/context") || !self.usage.is_null(),
            "billing": self.billing.is_some(),
            "interrupt": true,
            "steer": self.supports_method("_session/steer") || self.supports_method("session/steer"),
            "compact": self.supports_method("_kiro/session/compact"),
            "fork": self.supports_method("session/fork"),
            "fork_at_message": self.agent_capabilities["sessionCapabilities"]["fork"]["_meta"]["kiro"]["messageId"] == true,
            "rename": true,
            "history": self.supports_method("_kiro/session/history"),
            "export": self.supports_method("_kiro/session/export"),
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
            "loading": self.loading,
            "items": if wire { json!(self.windowed_items()) } else { json!(self.items) },
            // The projection needs each subtask's full `state.items` to write the
            // per-subagent message store that `/api/messages` serves. The wire
            // never renders subtask items from the snapshot (subagent views read
            // their own projected history), so the public frame carries only
            // lightweight subtask metadata.
            "subtasks": self.subtasks.iter().map(|(id, task)| {
                if wire {
                    json!({"id": id, "title": task.title, "type": task.kind, "status": task.status})
                } else {
                    json!({"id": id, "title": task.title, "type": task.kind,
                        "status": task.status, "items": task.state.items})
                }
            }).collect::<Vec<_>>(),
            "requests": requests,
            "last_turn": self.last_turn,
            "runtime_error": self.runtime_error,
            "error": self.error,
            "error_kind": self.error_kind,
            "cancel_requested": self.interrupted_turn,
            "retry": self.retry,
            "compact": self.compact,
            "configOptions": self.config_options,
            "modes": self.modes,
            "mode": self.mode,
            "model": self.model,
            "model_observed": self.model_observed,
            "effort": self.effort.as_ref().filter(|_| self.effort_supported()),
            "permission_mode": self.permission_mode,
            "autopilot": self.autopilot,
            "settings": {
                "model": self.model,
                "effort": self.effort.as_ref().filter(|_| self.effort_supported()),
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

/// Append streamed `chunk` to an item's `text`/`content` in place. The previous
/// code rebuilt the whole accumulated string with `format!` on every chunk,
/// which is O(text^2) across a streamed message; growing a `String` in place is
/// amortized O(1) per chunk.
fn append_text(item: &mut Value, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    let mut text = match item["text"].take() {
        Value::String(s) => s,
        _ => String::new(),
    };
    text.push_str(chunk);
    item["text"] = json!(text);
    item["content"] = item["text"].clone();
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

pub(super) fn mark_exit(session: &Arc<Session>, reason: &str) {
    let mut st = session.state.lock().unwrap();
    st.cancel_model_retry();
    if st.running {
        st.fail_turn(reason);
        st.error_kind = Some("kiro_sdk_process_exited".into());
        st.active_prompt = None;
    }
    st.interrupt_subtasks();
    st.requests.clear();
    st.runtime_error = Some(reason.to_string());
    session.project(&mut st);
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
