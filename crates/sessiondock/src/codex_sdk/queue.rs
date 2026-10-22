//! Server-owned FIFO inputs. Persist before acknowledgment; never replay an
//! uncertain native RPC after a disconnect or a service restart.
use super::{CodexSdk, Inner, RpcError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Input {
    pub id: String,
    pub text: String,
    pub input: Vec<Value>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub state: String,
    pub error: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub native_item_id: Option<String>,
}

type Document = HashMap<String, Vec<Input>>;
pub(super) struct Queue {
    path: Option<PathBuf>,
    rows: Mutex<Document>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

fn storage(error: io::Error) -> RpcError {
    RpcError::Storage(format!("待发送消息保存失败：{error}"))
}

impl Queue {
    pub fn open(directory: Option<PathBuf>) -> io::Result<Self> {
        let path = directory.map(|dir| dir.join("codex-sdk-queue.json"));
        if let Some(parent) = path.as_ref().and_then(|p| p.parent()) {
            fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            }
        }
        let mut rows: Document = match path.as_ref().map(fs::read).transpose() {
            Ok(Some(bytes)) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Ok(None) => HashMap::new(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(error) => return Err(error),
        };
        for item in rows
            .values_mut()
            .flatten()
            .filter(|item| item.state == "sending")
        {
            item.state = "failed".into();
            item.error = Some("服务已重启，消息可能已发送；请核对会话后重试或撤销。".into());
        }
        Ok(Self {
            path,
            rows: Mutex::new(rows),
            locks: Mutex::new(HashMap::new()),
        })
    }

    pub(super) fn lock(&self, thread: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .lock()
            .expect("queue locks")
            .entry(thread.into())
            .or_default()
            .clone()
    }

    pub fn visible(&self, thread: &str) -> Value {
        json!(self.rows.lock().expect("queue").get(thread).into_iter().flatten()
            .filter(|item| !matches!(item.state.as_str(), "sent" | "cancelled"))
            .map(|item| json!({"id":item.id,"text":item.text,"state":item.state,"error":item.error}))
            .collect::<Vec<_>>())
    }

    pub fn accepted(&self, thread: &str) -> Value {
        json!(
            self.rows
                .lock()
                .expect("queue")
                .get(thread)
                .into_iter()
                .flatten()
                .filter(|item| item.state == "sent" && !item.text.is_empty())
                .map(|item| json!({"id":item.id,"text":item.text,"turn_id":item.turn_id}))
                .collect::<Vec<_>>()
        )
    }

    pub(super) fn consumed(&self, thread: &str, turn: &str, item: &Value) -> Result<(), RpcError> {
        if item["type"] != "userMessage" {
            return Ok(());
        }
        let Some(id) = item["id"].as_str() else {
            return Ok(());
        };
        let text = item["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let input = {
            let rows = self.rows.lock().expect("queue");
            let Some(entries) = rows.get(thread) else {
                return Ok(());
            };
            if entries
                .iter()
                .any(|row| row.native_item_id.as_deref() == Some(id))
            {
                return Ok(());
            }
            entries
                .iter()
                .find(|row| {
                    matches!(row.state.as_str(), "sending" | "sent")
                        && row.native_item_id.is_none()
                        && row.turn_id.as_deref().is_none_or(|value| value == turn)
                        && row.text.trim() == text.trim()
                })
                .cloned()
        };
        if let Some(mut input) = input {
            input.turn_id = Some(turn.into());
            input.native_item_id = Some(id.into());
            self.update(thread, input)?;
        }
        Ok(())
    }

    fn get(&self, thread: &str, id: &str) -> Option<Input> {
        self.rows
            .lock()
            .expect("queue")
            .get(thread)?
            .iter()
            .find(|item| item.id == id)
            .cloned()
    }

    pub(super) fn pending_state(&self, thread: &str) -> Option<String> {
        self.first(thread).map(|item| item.state)
    }

    fn first(&self, thread: &str) -> Option<Input> {
        self.rows
            .lock()
            .expect("queue")
            .get(thread)?
            .iter()
            .find(|item| !matches!(item.state.as_str(), "sent" | "cancelled"))
            .cloned()
    }

    fn threads(&self) -> Vec<String> {
        self.rows
            .lock()
            .expect("queue")
            .iter()
            .filter(|(_, items)| items.iter().any(|item| item.state == "queued"))
            .map(|(thread, _)| thread.clone())
            .collect()
    }

    fn update(&self, thread: &str, mut item: Input) -> Result<(), RpcError> {
        let mut rows = self.rows.lock().expect("queue");
        // Native consumption can arrive before the RPC reply. Preserve that
        // evidence when the dispatch task later publishes its captured input.
        if let Some(old) = rows
            .get(thread)
            .into_iter()
            .flatten()
            .find(|old| old.id == item.id)
        {
            if old.state == "sent" && item.state == "sending" && item.native_item_id.is_some() {
                item.state = old.state.clone();
            }
            if old.native_item_id.is_some() {
                item.native_item_id = old.native_item_id.clone();
                item.turn_id = old.turn_id.clone();
            }
        }
        if item.state == "cancelled" || (item.state == "sent" && item.native_item_id.is_some()) {
            item.text.clear();
        }
        if matches!(item.state.as_str(), "sent" | "cancelled") {
            item.input.clear();
            item.model = None;
            item.effort = None;
        }
        let mut next = rows.clone();
        let entries = next.entry(thread.into()).or_default();
        if let Some(old) = entries.iter_mut().find(|old| old.id == item.id) {
            *old = item;
        } else {
            entries.push(item);
        }
        if let Some(path) = &self.path {
            let temporary = path.with_extension("tmp");
            let result = (|| -> io::Result<()> {
                let mut options = fs::OpenOptions::new();
                options.create(true).truncate(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&temporary)?;
                file.write_all(&serde_json::to_vec(&next)?)?;
                file.sync_all()?;
                fs::rename(&temporary, path)?;
                Ok(())
            })();
            result.map_err(storage)?;
        }
        *rows = next;
        Ok(())
    }
}

impl CodexSdk {
    pub async fn send(
        &self,
        thread: &str,
        id: String,
        text: String,
        input: Vec<Value>,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<Value, RpcError> {
        let lock = self.inner.queue.lock(thread);
        let _guard = lock.lock().await;
        let live = self.inner.ensure(thread).await?;
        if self.inner.queue.get(thread, &id).is_none() {
            self.inner.queue.update(
                thread,
                Input {
                    id: id.clone(),
                    text,
                    input,
                    model,
                    effort,
                    state: "queued".into(),
                    error: None,
                    turn_id: None,
                    native_item_id: None,
                },
            )?;
            live.cancel_capacity_retry();
            self.inner.queue_event(&live);
        }
        let item = self
            .inner
            .queue
            .get(thread, &id)
            .ok_or(RpcError::UnknownRequest)?;
        Ok(json!({"id":id,"queued":item.state != "sent","state":item.state,"steered":false}))
    }

    pub async fn queue_action(
        &self,
        thread: &str,
        id: &str,
        action: &str,
    ) -> Result<Value, RpcError> {
        let lock = self.inner.queue.lock(thread);
        let _guard = lock.lock().await;
        let live = self.inner.ensure(thread).await?;
        let mut item = self
            .inner
            .queue
            .get(thread, id)
            .ok_or(RpcError::UnknownRequest)?;
        if matches!(item.state.as_str(), "sent" | "sending") {
            return Err(RpcError::UnknownRequest);
        }
        if item.state == "cancelled" {
            return Ok(json!({"state":item.state}));
        }
        match action {
            "cancel" => {
                item.state = "cancelled".into();
                self.inner.queue.update(thread, item)?;
                self.inner.queue_event(&live);
            }
            "retry" => {
                item.state = "queued".into();
                item.error = None;
                self.inner.queue.update(thread, item)?;
                self.inner.queue_event(&live);
            }
            "steer" => {
                let turn = live.active_turn().ok_or(RpcError::NoActiveTurn)?;
                self.inner.deliver(&live, item, Some(turn)).await?;
            }
            _ => return Err(RpcError::UnknownRequest),
        }
        self.inner.dispatch(&live).await?;
        Ok(json!({"queue":self.inner.queue.visible(thread)}))
    }
}

impl Inner {
    pub(super) fn queue_event(&self, live: &super::LiveThread) {
        live.apply(
            "bridge/queue",
            json!({"queue":self.queue.visible(live.id()), "accepted":self.queue.accepted(live.id())}),
        );
    }

    async fn dispatch(self: &Arc<Self>, live: &Arc<super::LiveThread>) -> Result<(), RpcError> {
        if self.stopped.lock().expect("stopped").contains(live.id()) {
            return Ok(());
        }
        if !live.ready_to_start() {
            return Ok(());
        }
        if let Some(item) = self
            .queue
            .first(live.id())
            .filter(|item| item.state == "queued")
        {
            live.cancel_capacity_retry();
            self.deliver(live, item, None).await?;
        }
        Ok(())
    }

    async fn deliver(
        self: &Arc<Self>,
        live: &Arc<super::LiveThread>,
        mut item: Input,
        steer: Option<String>,
    ) -> Result<(), RpcError> {
        let connection = self.connection().await?;
        // Queued inputs from older releases retain their published path text.
        item.input.retain(|part| part["type"] == "text");
        item.state = "sending".into();
        item.error = None;
        self.queue.update(live.id(), item.clone())?;
        self.queue_event(live);
        let mut params = json!({"threadId":live.id(),"input":item.input});
        let method = if let Some(turn) = &steer {
            params["expectedTurnId"] = json!(turn);
            "turn/steer"
        } else {
            if let Some(model) = &item.model {
                params["model"] = json!(model);
            }
            if let Some(effort) = &item.effort {
                params["effort"] = json!(effort);
            }
            live.starting(true);
            "turn/start"
        };
        // Native acceptance completes delivery. Consumption, compaction and
        // execution outcomes belong to the live turn, never to send retries.
        let result = connection.request(method, params).await;
        match result {
            Ok(result) => {
                item.turn_id = result["turn"]["id"]
                    .as_str()
                    .or(result["turnId"].as_str())
                    .map(str::to_owned)
                    .or(steer);
                item.state = "sent".into();
            }
            Err(error) => {
                live.starting(false);
                item.state = "failed".into();
                item.error = Some(error.to_string());
            }
        }
        self.queue.update(live.id(), item)?;
        self.queue_event(live);
        Ok(())
    }

    pub(super) async fn run_queue(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! { _ = self.shutdown.cancelled() => return, _ = tick.tick() => {} }
            for thread in self.queue.threads() {
                if self.stopped.lock().expect("stopped").contains(&thread) {
                    continue;
                }
                let lock = self.queue.lock(&thread);
                let Ok(guard) = lock.try_lock_owned() else {
                    continue;
                };
                let inner = self.clone();
                tokio::spawn(async move {
                    let _guard = guard;
                    if let Ok(live) = inner.ensure(&thread).await {
                        let _ = inner.dispatch(&live).await;
                    }
                });
            }
        }
    }
}
