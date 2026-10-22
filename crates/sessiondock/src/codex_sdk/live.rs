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
    turn_revision: u64,
    queue: Value,
    accepted: Value,
    starting: bool,
    generation: Option<u64>,
    resume: Value,
    settings: Value,
    status: Value,
    turn: Option<Value>,
    /// Last observed terminal outcome, without retaining completed turn items.
    last_turn: Value,
    items: IndexMap<String, Value>,
    /// Open server requests keyed by the JSON spelling of their id.
    requests: IndexMap<String, Request>,
    usage: Value,
    recorded_limits: Value,
    last_error: Value,
    retry_error: Value,
    idle_since: Option<Instant>,
    capacity: super::capacity::Retry,
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

    pub fn seed_usage(&self, snapshot: super::usage::Snapshot) {
        let mut state = self.state.lock().expect("live");
        // A live notification arriving during disk recovery is newer.
        if state.usage.is_null() {
            state.usage = snapshot.usage;
        }
        state.recorded_limits = snapshot.limits;
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

    pub(super) fn missing_active_turn(&self) -> Option<u64> {
        let state = self.state.lock().expect("live");
        (state.turn.is_none() && state.status["type"] == "active").then_some(state.turn_revision)
    }

    /// A turn that began before subscription has no turn/started event here.
    /// Its paginated snapshot must not resurrect a turn completed meanwhile.
    pub(super) fn recover_active_turn(&self, revision: u64, turn: &Value) {
        let mut state = self.state.lock().expect("live");
        if state.turn_revision != revision
            || state.turn.is_some()
            || state.status["type"] != "active"
            || turn["status"] != "inProgress"
            || turn["id"].as_str().is_none_or(str::is_empty)
        {
            return;
        }
        state.turn = Some(turn.clone());
        state.starting = false;
        let mut recovered = IndexMap::new();
        for item in turn["items"].as_array().into_iter().flatten() {
            if let Some(id) = item["id"].as_str() {
                recovered.insert(id.to_owned(), item.clone());
            }
        }
        // Keep native order while giving notifications received during the
        // read precedence over the older recovered bodies.
        recovered.extend(std::mem::take(&mut state.items));
        state.items = recovered;
        let event = state.event("turn/started", json!({"threadId":self.id,"turn":turn}));
        let _ = self.tx.send(event);
    }

    pub(super) fn capacity_check(&self) -> Option<(String, String)> {
        let mut state = self.state.lock().expect("live");
        if state.last_turn["status"] != "failed" || state.capacity.checked {
            return None;
        }
        if !state.last_error.is_null() || state.resume["thread"]["path"].as_str().is_none() {
            state.capacity.checked = true;
            return None;
        }
        Some((
            state.last_turn["id"].as_str()?.to_owned(),
            state.resume["thread"]["path"].as_str()?.to_owned(),
        ))
    }

    pub(super) fn capacity_error(&self, turn: &str, error: Value) {
        let mut state = self.state.lock().expect("live");
        if state.last_turn["id"] != turn || state.last_turn["status"] != "failed" {
            return;
        }
        if state.capacity.checked {
            return;
        }
        state.capacity.checked = true;
        if state.capacity.schedule(&error) {
            state.last_error = error;
            let retry = state.capacity.summary();
            let event = state.event("bridge/capacityRetry", retry);
            let _ = self.tx.send(event);
        }
    }

    pub(super) fn take_capacity_retry(&self) -> bool {
        let mut state = self.state.lock().expect("live");
        if state.starting
            || state.turn.is_some()
            || !state.requests.is_empty()
            || !matches!(state.status["type"].as_str(), Some("idle" | "systemError"))
            || !state.capacity.due()
        {
            return false;
        }
        state.capacity.deadline = None;
        state.capacity.starting = true;
        state.starting = true;
        let event = state.event("bridge/capacityRetry", Value::Null);
        let _ = self.tx.send(event);
        true
    }

    pub(super) fn cancel_capacity_retry(&self) -> bool {
        let mut state = self.state.lock().expect("live");
        let pending = state.capacity.deadline.is_some();
        state.capacity.cancel();
        if pending {
            let event = state.event("bridge/capacityRetry", Value::Null);
            let _ = self.tx.send(event);
        }
        pending
    }

    pub fn ready_to_start(&self) -> bool {
        let state = self.state.lock().expect("live");
        !state.starting
            && state.turn.is_none()
            && state.requests.is_empty()
            && matches!(state.status["type"].as_str(), Some("idle" | "systemError"))
    }

    pub fn starting(&self, value: bool) {
        self.state.lock().expect("live").starting = value;
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
            && state.capacity.deadline.is_none()
            && !state.capacity.starting
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
        let previous = state.capacity.summary();
        state.reduce(method, &params);
        let retry = state.capacity.summary();
        let event = state.event(method, params);
        let _ = self.tx.send(event);
        if previous != retry {
            let event = state.event("bridge/capacityRetry", retry);
            let _ = self.tx.send(event);
        }
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
        "usage": state.usage, "last_error": state.last_error, "retry_error": state.retry_error,
        "last_turn": state.last_turn, "capacity_retry": state.capacity.summary(),
        "queue":state.queue, "accepted":state.accepted,
        "recorded_limits": state.recorded_limits,
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
        if matches!(
            method,
            "turn/started" | "turn/completed" | "thread/reverted"
        ) {
            self.turn_revision += 1;
        }
        let item_id = params["itemId"].as_str().unwrap_or_default().to_string();
        let delta = params["delta"].as_str().unwrap_or_default();
        // Native item activity means a recoverable error's retry resumed.
        // Keep final failures until a new turn or a successful completion.
        if matches!(
            method,
            "item/started"
                | "item/completed"
                | "item/agentMessage/delta"
                | "item/plan/delta"
                | "item/commandExecution/outputDelta"
                | "item/reasoning/summaryTextDelta"
                | "item/reasoning/textDelta"
        ) {
            self.retry_error = Value::Null;
        }
        match method {
            "bridge/queue" => {
                self.queue = params["queue"].clone();
                self.accepted = params["accepted"].clone();
            }
            "turn/started" => {
                if !self.capacity.starting {
                    self.capacity.cancel();
                }
                self.capacity.starting = false;
                self.capacity.checked = false;
                self.starting = false;
                self.turn = Some(params["turn"].clone());
                self.last_turn = Value::Null;
                self.items.clear();
                self.last_error = Value::Null;
                self.retry_error = Value::Null;
            }
            "turn/completed" => {
                self.capacity.starting = false;
                self.capacity.checked = false;
                self.starting = false;
                self.turn = None;
                self.last_turn = json!({
                    "id": params["turn"]["id"], "status": params["turn"]["status"],
                });
                self.items.clear();
                self.retry_error = Value::Null;
                if params["turn"]["status"] == "completed" {
                    self.last_error = Value::Null;
                } else if params["turn"]["status"] == "failed" && !params["turn"]["error"].is_null()
                {
                    self.last_error = params["turn"]["error"].clone();
                }
                if params["turn"]["status"] == "failed" {
                    if !self.capacity.schedule(&self.last_error) && !self.last_error.is_null() {
                        self.capacity.cancel();
                    }
                } else {
                    self.capacity.cancel();
                }
            }
            "thread/reverted" => {
                self.capacity.cancel();
                self.turn = None;
                self.last_turn = Value::Null;
                self.items.clear();
                self.requests.clear();
                self.usage = Value::Null;
                self.last_error = Value::Null;
                self.retry_error = Value::Null;
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
            "thread/settings/updated" => {
                if self.settings != params["threadSettings"] {
                    self.capacity.cancel();
                }
                self.settings = params["threadSettings"].clone();
            }
            "thread/tokenUsage/updated" => self.usage = params["tokenUsage"].clone(),
            "thread/name/updated" => {
                if self.resume["thread"].is_object() {
                    self.resume["thread"]["name"] = params["threadName"].clone();
                }
            }
            "error" => {
                if params["willRetry"] == true {
                    self.retry_error = params["error"].clone();
                    self.last_error = Value::Null;
                } else {
                    self.last_error = params["error"].clone();
                    self.retry_error = Value::Null;
                }
            }
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
