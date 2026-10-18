//! Projection of observed ACP messages; persisted history remains independent.
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::broadcast;

pub(super) struct Live {
    pub sid: String,
    pub running: bool,
    pub cancel_requested: bool,
    pub prompt_started: bool,
    pub items: Vec<Value>,
    pub requests: HashMap<String, Value>,
    pub receipts: HashMap<String, Value>,
    pub error: Option<String>,
    pub config: Value,
    pub models: Value,
    pub seq: u64,
    pub turn: u64,
    pub generation: u64,
    segment: u64,
    pub events: broadcast::Sender<Arc<str>>,
}

impl Live {
    pub fn new(sid: String) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            sid,
            running: false,
            cancel_requested: false,
            prompt_started: false,
            items: vec![],
            requests: HashMap::new(),
            receipts: HashMap::new(),
            error: None,
            config: Value::Null,
            models: Value::Null,
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
            "configOptions":self.config});
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
        }
        json!({"seq":self.seq,"type":"snapshot","raw":raw})
    }
    pub fn publish(&mut self) {
        self.seq += 1;
        let _ = self.events.send(Arc::from(self.snapshot().to_string()));
    }
    pub fn settings(&mut self, result: &Value) {
        if let Some(config) = result.get("configOptions") {
            self.config = config.clone();
        }
        if let Some(models) = result.get("models") {
            self.models = models.clone();
        }
    }
    pub fn receive(&mut self, message: &Value) -> Option<Value> {
        let params = &message["params"];
        let sid = params["sessionId"].as_str();
        let ours = sid.is_some_and(|sid| self.sid.is_empty() || sid == self.sid);
        if let Some(id) = message.get("id") {
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
                Some("session/update" | "x.ai/session/update")
            )
        {
            return None;
        }
        let update = &params["update"];
        if update["_meta"]["isReplay"] == true || params["_meta"]["isReplay"] == true {
            return None;
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
            "config_option_update" => self.config = update["configOptions"].clone(),
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
