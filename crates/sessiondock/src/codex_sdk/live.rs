//! Per-thread live state: the daemon's view of one loaded thread as this
//! bridge has observed it since `thread/resume`. Only the in-flight turn and
//! open server requests are kept — completed turns are in the native rollout.
//! Every change is applied and broadcast under one lock, so a page that takes
//! a snapshot and subscribes in the same critical section sees each event
//! exactly once.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use indexmap::IndexMap;
use serde_json::{Value, json};
use tokio::sync::broadcast;

const EVENT_BUFFER: usize = 4096;

pub struct LiveThread {
    id: String,
    tx: broadcast::Sender<Arc<str>>,
    state: Mutex<State>,
    subscribers: AtomicUsize,
    pub resume_lock: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct State {
    seq: u64,
    generation: Option<u64>,
    resume: Value,
    settings: Value,
    status: Value,
    turn: Option<Value>,
    items: IndexMap<String, Value>,
    /// Open server requests keyed by the JSON spelling of their id.
    requests: IndexMap<String, Request>,
    usage: Value,
    last_error: Value,
    idle_since: Option<Instant>,
}

struct Request {
    generation: u64,
    id: Value,
    method: String,
    params: Value,
}

impl LiveThread {
    pub fn new(id: &str) -> Arc<Self> {
        let (tx, _) = broadcast::channel(EVENT_BUFFER);
        Arc::new(Self {
            id: id.to_string(),
            tx,
            state: Mutex::new(State {
                idle_since: Some(Instant::now()),
                ..State::default()
            }),
            subscribers: AtomicUsize::new(0),
            resume_lock: tokio::sync::Mutex::new(()),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn loaded_on(&self) -> Option<u64> {
        self.state.lock().expect("live").generation
    }

    pub fn loaded(&self, generation: u64, mut resume: Value) {
        if let Some(thread) = resume.get_mut("thread").and_then(Value::as_object_mut) {
            thread.remove("turns");
        }
        let mut state = self.state.lock().expect("live");
        state.generation = Some(generation);
        state.status = resume["thread"]["status"].clone();
        state.settings = resume.clone();
        state
            .settings
            .as_object_mut()
            .expect("resume object")
            .remove("thread");
        state.settings["effort"] = resume["reasoningEffort"].clone();
        state.resume = resume;
        let raw = json!({"summary": summary(&state)});
        let event = state.event("thread/resumed", raw);
        let _ = self.tx.send(event);
    }

    pub fn summary(&self) -> Value {
        summary(&self.state.lock().expect("live"))
    }

    pub fn active_turn(&self) -> Option<String> {
        let state = self.state.lock().expect("live");
        state
            .turn
            .as_ref()
            .and_then(|turn| turn["id"].as_str())
            .map(str::to_string)
    }

    pub fn attach(&self) -> (Value, broadcast::Receiver<Arc<str>>) {
        let mut state = self.state.lock().expect("live");
        self.subscribers.fetch_add(1, Ordering::SeqCst);
        state.idle_since = None;
        let snapshot = json!({
            "seq": state.seq, "type": "snapshot",
            "raw": {
                "summary": summary(&state),
                "turn": state.turn,
                "items": state.items.values().collect::<Vec<_>>(),
                "requests": state.requests.iter().map(|(key, request)| json!({
                    "key": key, "method": request.method, "params": request.params,
                })).collect::<Vec<_>>(),
            },
        });
        (snapshot, self.tx.subscribe())
    }

    pub fn detach(&self) {
        let mut state = self.state.lock().expect("live");
        if self.subscribers.fetch_sub(1, Ordering::SeqCst) == 1 {
            state.idle_since = Some(Instant::now());
        }
    }

    pub fn idle_for(&self, limit: Duration) -> bool {
        let state = self.state.lock().expect("live");
        state.turn.is_none()
            && state.requests.is_empty()
            && state
                .idle_since
                .is_some_and(|since| since.elapsed() >= limit)
    }

    pub fn request_id(&self, key: &str, generation: u64) -> Option<Value> {
        let state = self.state.lock().expect("live");
        state
            .requests
            .get(key)
            .filter(|request| request.generation == generation)
            .map(|request| request.id.clone())
    }

    pub fn request(&self, generation: u64, id: Value, method: &str, params: Value) {
        let key = id.to_string();
        let mut state = self.state.lock().expect("live");
        state.requests.insert(
            key.clone(),
            Request {
                generation,
                id,
                method: method.to_string(),
                params: params.clone(),
            },
        );
        let event = state.event(method, json!({"key": key, "params": params}));
        let _ = self.tx.send(event);
    }

    pub fn apply(&self, method: &str, params: Value) {
        let mut state = self.state.lock().expect("live");
        state.reduce(method, &params);
        let event = state.event(method, params);
        let _ = self.tx.send(event);
    }

    /// Tell attached pages to reload; the sender drops with this thread.
    pub fn reset(&self, reason: &str) {
        let mut state = self.state.lock().expect("live");
        let event = state.event("bridge/reset", json!({"reason": reason}));
        let _ = self.tx.send(event);
    }
}

fn summary(state: &State) -> Value {
    json!({
        "resume": state.resume, "settings": state.settings, "status": state.status,
        "usage": state.usage, "last_error": state.last_error,
    })
}

impl State {
    fn event(&mut self, method: &str, raw: Value) -> Arc<str> {
        self.seq += 1;
        // `raw` is an opaque key on the Hub's namespace rewrite, so native
        // payloads cross the proxy byte-for-byte.
        Arc::from(json!({"seq": self.seq, "method": method, "raw": raw}).to_string())
    }

    fn reduce(&mut self, method: &str, params: &Value) {
        let item_id = params["itemId"].as_str().unwrap_or_default().to_string();
        let delta = params["delta"].as_str().unwrap_or_default();
        match method {
            "turn/started" => {
                self.turn = Some(params["turn"].clone());
                self.items.clear();
                self.last_error = Value::Null;
            }
            "turn/completed" => {
                self.turn = None;
                self.items.clear();
            }
            "thread/reverted" => {
                self.turn = None;
                self.items.clear();
                self.requests.clear();
                self.usage = Value::Null;
                self.last_error = Value::Null;
            }
            "item/started" | "item/completed" => {
                if let Some(id) = params["item"]["id"].as_str() {
                    let mut item = params["item"].clone();
                    // Native compaction items have no status; retain their event
                    // phase so reconnect snapshots do not report completion early.
                    if item["type"] == "contextCompaction" {
                        item["status"] = json!(if method == "item/started" {
                            "inProgress"
                        } else {
                            "completed"
                        });
                    }
                    self.items.insert(id.to_string(), item);
                }
            }
            "item/agentMessage/delta" | "item/plan/delta" => {
                append(&mut self.items, &item_id, "text", delta)
            }
            "item/commandExecution/outputDelta" => {
                append(&mut self.items, &item_id, "aggregatedOutput", delta)
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                let (field, index) = if method.ends_with("summaryTextDelta") {
                    ("summary", params["summaryIndex"].as_u64())
                } else {
                    ("content", params["contentIndex"].as_u64())
                };
                if let Some(item) = self.items.get_mut(&item_id) {
                    let index = index.unwrap_or(0) as usize;
                    if !item[field].is_array() {
                        item[field] = json!([]);
                    }
                    let parts = item[field].as_array_mut().expect("array");
                    while parts.len() <= index {
                        parts.push(json!(""));
                    }
                    let joined = format!("{}{delta}", parts[index].as_str().unwrap_or_default());
                    parts[index] = json!(joined);
                }
            }
            "serverRequest/resolved" => {
                self.requests.shift_remove(&params["requestId"].to_string());
            }
            "thread/status/changed" => self.status = params["status"].clone(),
            "thread/settings/updated" => self.settings = params["threadSettings"].clone(),
            "thread/tokenUsage/updated" => self.usage = params["tokenUsage"].clone(),
            "thread/name/updated" => {
                if self.resume["thread"].is_object() {
                    self.resume["thread"]["name"] = params["name"].clone();
                }
            }
            "error" => self.last_error = params["error"].clone(),
            _ => {}
        }
    }
}

fn append(items: &mut IndexMap<String, Value>, id: &str, field: &str, delta: &str) {
    if let Some(item) = items.get_mut(id) {
        let joined = format!("{}{delta}", item[field].as_str().unwrap_or_default());
        item[field] = json!(joined);
    }
}
