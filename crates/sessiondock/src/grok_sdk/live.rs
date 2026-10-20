//! Projection of observed ACP messages; persisted history remains independent.
use serde_json::{Value, json};
use std::time::Instant;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::broadcast;

type ErrorKind = Option<&'static str>;

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Live {
    #[serde(skip)]
    pub checkpoint: Option<std::path::PathBuf>,
    #[serde(default)]
    pub host_key: Option<String>,
    #[serde(default)]
    pub stdout_offset: u64,
    #[serde(default)]
    pub setup_pending: bool,
    #[serde(default)]
    pub initialized: Value,
    #[serde(default)]
    pub completed_rpcs: HashSet<u64>,
    pub sid: String,
    pub running: bool,
    pub cancel_requested: bool,
    pub prompt_started: bool,
    pub items: Vec<Value>,
    pub requests: HashMap<String, Value>,
    pub receipts: HashMap<String, Value>,
    pub error: Option<String>,
    #[serde(deserialize_with = "deserialize_error_kind")]
    pub error_kind: ErrorKind,
    pub config: Value,
    pub models: Value,
    pub modes: Value,
    pub usage: Value,
    pub billing: Value,
    pub context: Value,
    pub extensions: HashMap<String, bool>,
    pub steers: HashMap<String, Value>,
    #[serde(skip)]
    pub idle_since: Option<Instant>,
    pub idle_reclaimed: bool,
    pub native_tasks: HashSet<String>,
    pub incomplete_tasks: bool,
    pub seq: u64,
    pub turn: u64,
    pub generation: u64,
    segment: u64,
    #[serde(skip, default = "events")]
    pub events: broadcast::Sender<Arc<str>>,
}

impl Live {
    pub fn new(sid: String) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            checkpoint: None,
            host_key: None,
            stdout_offset: 0,
            setup_pending: false,
            initialized: Value::Null,
            completed_rpcs: HashSet::new(),
            sid,
            running: false,
            cancel_requested: false,
            prompt_started: false,
            items: vec![],
            requests: HashMap::new(),
            receipts: HashMap::new(),
            error: None,
            error_kind: None,
            config: Value::Null,
            models: Value::Null,
            modes: Value::Null,
            usage: Value::Null,
            billing: Value::Null,
            context: Value::Null,
            extensions: HashMap::new(),
            steers: HashMap::new(),
            idle_since: None,
            idle_reclaimed: false,
            native_tasks: HashSet::new(),
            incomplete_tasks: false,
            seq: 0,
            turn: 0,
            generation: 0,
            segment: 0,
            events,
        }
    }
    pub fn snapshot(&self) -> Value {
        let mut requests: Vec<_> = self.requests.values().cloned().collect();
        requests.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
        let mut steers: Vec<_> = self.steers.values().cloned().collect();
        steers.sort_by_key(|r| r["order"].as_u64());
        let items: Vec<_> = self.items.iter().cloned().map(|mut item| {
            if item["role"] == "tool" {
                item["name"] = item["tool_name"].clone();
                item["text"] = json!(content_text(&item["tool_input"]));
                if !item["tool_result"].is_null() || !item["tool_content"].is_null() {
                    item["result"] = json!({"role":"tool_result","text":item["content"],"error":item["error"]});
                }
            } else { item["text"] = item["content"].clone(); }
            item
        }).collect();
        let mut raw = json!({"session_id":self.sid,"running":self.running,"items":items,"requests":requests,
            "native_tasks":self.native_tasks,"native_tasks_incomplete":self.incomplete_tasks,"steers":steers,"idle_reclaimed":self.idle_reclaimed,"configOptions":self.config,"modes":self.modes,"mode":self.modes["currentModeId"],
            "usage":self.usage,"billing":self.billing,"context_usage":self.context,"extension_support":self.extensions});
        for (id, name) in [("model", "model"), ("reasoning_effort", "effort")] {
            if let Some(option) = self
                .config
                .as_array()
                .and_then(|options| options.iter().find(|o| o["id"] == id))
            {
                raw[name] = option["currentValue"].clone();
            }
        }
        if let Some(error) = &self.error {
            raw["error"] = json!(error);
            raw["error_kind"] = json!(self.error_kind);
        }
        json!({"seq":self.seq,"type":"snapshot","raw":raw})
    }
    pub fn has_pending_steers(&self) -> bool {
        self.steers.values().any(|r| {
            r["generation"] == self.generation
                && matches!(
                    r["status"].as_str(),
                    Some("dispatching" | "native_queued" | "native_received" | "uncertain")
                )
        })
    }
    pub fn save_checkpoint(&self) -> std::io::Result<()> {
        if let Some(path) = &self.checkpoint {
            super::transport::save(path, self)?;
        }
        Ok(())
    }
    pub fn publish(&mut self) {
        self.seq += 1;
        if let Err(error) = self.save_checkpoint() {
            self.error = Some(format!("grok_sdk_checkpoint: {error}"));
            self.error_kind = Some("transport");
        }
        let _ = self.events.send(Arc::from(self.snapshot().to_string()));
    }
    pub fn settings(&mut self, result: &Value) {
        if let Some(config) = result.get("configOptions") {
            self.config = config.clone();
        }
        if let Some(modes) = result.get("modes") {
            self.modes = modes.clone();
        }
        if let Some(models) = result.get("models") {
            self.models = models.clone();
        }
    }
    pub fn receive(&mut self, message: &Value) -> Option<Value> {
        let params = &message["params"];
        let sid = params["sessionId"].as_str();
        let ours = sid.is_some_and(|sid| self.sid.is_empty() || sid == self.sid);
        if ours
            && matches!(
                message["method"].as_str(),
                Some("_x.ai/session/interjection" | "x.ai/session/interjection")
            )
        {
            if params["_meta"]["isReplay"] == true {
                return None;
            }
            self.idle_since = None;
            if let Some(id) = params["interjectionId"].as_str()
                && let Some(receipt) = self.steers.get_mut(id)
            {
                receipt["native_received"] = json!(true);
                receipt["status"] = json!("native_received");
                // Actor enqueue is observable; safe-point/model consumption is not.
                receipt["consumed"] = Value::Null;
                self.publish();
                return None;
            }
            self.items.push(
                json!({"id":format!("grok:{}:native-steer:{}",self.sid,self.seq),
                "role":"user","content":params["text"],"delivery":"steer","native":params}),
            );
            self.publish();
            return None;
        }
        if let Some(id) = message.get("id") {
            let method = message["method"].as_str().unwrap_or("");
            let method = method.strip_prefix('_').unwrap_or(method);
            if ours && matches!(method, "x.ai/ask_user_question" | "x.ai/exit_plan_mode") {
                let key = format!("{}:{}", self.generation, id);
                let mut request =
                    json!({"key":key,"native_method":method,"native":params,"rpc_id":id});
                if method == "x.ai/ask_user_question" {
                    let mut questions = params["questions"].clone();
                    if let Some(questions) = questions.as_array_mut() {
                        for question in questions {
                            question["key"] = question["question"].clone();
                            if let Some(multiple) = question.get("multi_select").cloned() {
                                question["multiSelect"] = multiple;
                            }
                        }
                    }
                    request["type"] = json!("question");
                    request["title"] = json!("Grok 需要回答");
                    request["questions"] = questions;
                    request["mode"] = params["mode"].clone();
                } else {
                    request["type"] = json!("approval");
                    request["title"] = json!("Grok 计划批准");
                    request["description"] = params["planContent"].clone();
                    request["options"] = json!([
                        {"label":"批准计划","value":{"outcome":"approved"}},
                        {"label":"要求修改","value":{"outcome":"cancelled"}},
                        {"label":"放弃计划","value":{"outcome":"abandoned"}}]);
                }
                self.requests.insert(key, request);
                self.publish();
                return None;
            }
            if message["method"] == "session/request_permission" && ours {
                let key = format!("{}:{}", self.generation, id);
                let options = params["options"]
                    .as_array()
                    .map(|options| {
                        options
                            .iter()
                            .map(|o| {
                                json!({
                    "label":o["name"],"value":o["optionId"],"kind":o["kind"]})
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.requests.insert(key.clone(), json!({"key":key,"type":"approval",
                    "title":params["toolCall"]["title"].as_str().unwrap_or("Grok 工具执行审批"),
                    "description":params["toolCall"].to_string(),"options":options,"native":params,"rpc_id":id}));
                self.publish();
                return None;
            }
            return Some(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unsupported ACP client method"}}),
            );
        }
        if !ours
            || !matches!(
                message["method"].as_str(),
                Some("session/update" | "x.ai/session/update" | "_x.ai/session/update")
            )
        {
            return None;
        }
        let update = &params["update"];
        if update["_meta"]["isReplay"] == true || params["_meta"]["isReplay"] == true {
            return None;
        }
        if matches!(
            update["sessionUpdate"].as_str(),
            Some(
                "agent_message_chunk"
                    | "agent_thought_chunk"
                    | "user_message_chunk"
                    | "tool_call"
                    | "tool_call_update"
                    | "turn_completed"
                    | "task_backgrounded"
                    | "subagent_spawned"
                    | "subagent_finished"
                    | "task_completed"
                    | "scheduled_task_created"
                    | "scheduled_task_deleted"
            )
        ) {
            self.idle_since = None;
        }
        match update["sessionUpdate"].as_str().unwrap_or("") {
            "agent_message_chunk" => self.chunk("assistant", &update["content"]),
            "agent_thought_chunk" => self.chunk("thinking", &update["content"]),
            "user_message_chunk" => {
                // Grok may echo the accepted prompt. Keep the bridge's stable
                // request-id user row instead of showing it twice.
                if !self.running {
                    self.chunk("user", &update["content"]);
                }
            }
            "tool_call" | "tool_call_update" => {
                self.segment += 1;
                let id = update["toolCallId"].as_str()?;
                let item_id = format!("grok:{}:tool:{id}", self.sid);
                let index = self
                    .items
                    .iter()
                    .position(|i| i["id"] == item_id)
                    .unwrap_or_else(|| {
                        self.items.push(
                            json!({"id":item_id,"role":"tool","content":"","tool_call_id":id}),
                        );
                        self.items.len() - 1
                    });
                let item = &mut self.items[index];
                for (native, projected) in [
                    ("title", "tool_name"),
                    ("rawInput", "tool_input"),
                    ("rawOutput", "tool_result"),
                    ("status", "status"),
                    ("kind", "kind"),
                    ("content", "tool_content"),
                ] {
                    if let Some(value) = update.get(native) {
                        item[projected] = value.clone();
                    }
                }
                if let Some(content) = update.get("content") {
                    item["content"] = json!(content_text(content));
                } else if let Some(output) = update.get("rawOutput") {
                    item["content"] = json!(content_text(output));
                }
                item["error"] = json!(item["status"] == "failed");
                // Preserve extension fields, locations and complete raw input.
                let mut native = item["native"].as_object().cloned().unwrap_or_default();
                if let Some(update) = update.as_object() {
                    native.extend(update.clone());
                }
                item["native"] = json!(native);
            }
            "task_backgrounded" => {
                if let Some(id) = update["task_id"].as_str() {
                    self.native_tasks.insert(format!("task:{id}"));
                } else {
                    self.incomplete_tasks = true;
                }
            }
            "background_tasks" => {
                if update["truncated"] != true {
                    self.native_tasks.retain(|id| !id.starts_with("task:"));
                    self.incomplete_tasks = false;
                } else {
                    self.incomplete_tasks = true;
                }
                for task in update["tasks"].as_array().into_iter().flatten() {
                    if !matches!(
                        task["status"].as_str(),
                        Some("completed" | "failed" | "cancelled" | "exited")
                    ) {
                        if let Some(id) = task["id"].as_str().or_else(|| task["task_id"].as_str()) {
                            self.native_tasks.insert(format!("task:{id}"));
                        } else {
                            self.incomplete_tasks = true;
                        }
                    }
                }
            }
            "subagent_spawned" | "scheduled_task_created" => {
                let key = if update["sessionUpdate"] == "subagent_spawned" {
                    "subagent_id"
                } else {
                    "task_id"
                };
                if let Some(id) = update[key].as_str() {
                    self.native_tasks.insert(format!("{key}:{id}"));
                } else {
                    self.native_tasks.insert(format!("unknown-{key}"));
                }
            }
            "subagent_finished" | "scheduled_task_deleted" => {
                let key = if update["sessionUpdate"] == "subagent_finished" {
                    "subagent_id"
                } else {
                    "task_id"
                };
                if let Some(id) = update[key].as_str() {
                    self.native_tasks.remove(&format!("{key}:{id}"));
                }
                if update["will_wake"] == true {
                    self.native_tasks.insert("native-wake".into());
                }
            }
            "task_completed" => {
                // A completion can schedule a hidden native wake prompt; a
                // bridge prompt ending is not proof that wake has finished.
                if update["will_wake"] == true {
                    self.native_tasks.insert("native-wake".into());
                }
            }
            "config_option_update" => self.config = update["configOptions"].clone(),
            "current_mode_update" => self.modes["currentModeId"] = update["currentModeId"].clone(),
            "turn_completed" => {
                if let Some(usage) = update.get("usage").filter(|v| !v.is_null()) {
                    // Per-turn usage is distinct from the process-lifetime ledger.
                    self.usage["last_turn"] = scrub_usage(usage);
                }
            }
            "interaction_resolved" => {
                self.requests
                    .retain(|_, r| r["native"]["toolCallId"] != update["tool_call_id"]);
            }
            "auto_compact_started"
            | "auto_compact_completed"
            | "auto_compact_failed"
            | "retry_state"
            | "image_dropped"
            | "image_compressed" => {
                self.segment += 1;
                self.items.push(
                    json!({"id":format!("grok:{}:status:{}:{}",self.sid,self.turn,self.segment),
                    "role":"system","content":content_text(update),"native":update}),
                );
            }
            "plan" => {
                self.segment += 1;
                self.items.push(
                    json!({"id":format!("grok:{}:plan:{}:{}",self.sid,self.turn,self.segment),
                "role":"system","content":content_text(&update["entries"]),"native":update}),
                );
            }
            _ => return None,
        }
        self.publish();
        None
    }
    fn chunk(&mut self, role: &str, content: &Value) {
        if self.items.last().is_none_or(|item| item["role"] != role) {
            self.segment += 1;
        }
        let id = format!("grok:{}:{}:{}:{}", self.sid, self.turn, self.segment, role);
        let text = content_text(content);
        if let Some(last) = self.items.last_mut().filter(|i| i["id"] == id) {
            let old = last["content"].as_str().unwrap_or("");
            last["content"] = json!(format!("{old}{text}"));
            if let Some(blocks) = last["native_content"].as_array_mut() {
                blocks.push(content.clone());
            }
        } else {
            self.items
                .push(json!({"id":id,"role":role,"content":text,"native_content":[content]}));
        }
    }
}

fn content_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    if let Some(text) = content["text"].as_str() {
        return text.to_owned();
    }
    if let Some(inner) = content.get("content") {
        return content_text(inner);
    }
    if let Some(items) = content.as_array() {
        return items
            .iter()
            .map(content_text)
            .collect::<Vec<_>>()
            .join("\n");
    }
    if content.is_null() {
        String::new()
    } else {
        content.to_string()
    }
}

// Unknown/partial cost is not a zero bill. Keep token counters and diagnostics.
pub(super) fn scrub_usage(value: &Value) -> Value {
    let mut usage = value.clone();
    if usage["usageIsIncomplete"] == true || usage["costIsPartial"] == true {
        if let Some(object) = usage.as_object_mut() {
            object.remove("costUsdTicks");
        }
        if let Some(models) = usage["modelUsage"].as_object_mut() {
            for model in models.values_mut() {
                if let Some(object) = model.as_object_mut() {
                    object.remove("costUsdTicks");
                }
            }
        }
    }
    usage
}

fn events() -> broadcast::Sender<Arc<str>> {
    broadcast::channel(256).0
}
fn deserialize_error_kind<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<&'static str>, D::Error> {
    let kind: Option<String> = serde::Deserialize::deserialize(deserializer)?;
    Ok(kind
        .as_deref()
        .map(|kind| if kind == "turn" { "turn" } else { "transport" }))
}
