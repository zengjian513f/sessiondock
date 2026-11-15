//! Server-owned FIFO inputs. Persist before acknowledgment; never replay an
//! uncertain native RPC after a disconnect or a service restart.
use super::{CodexSdk, Inner, RpcError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs, io,
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
    #[serde(default)]
    pub direct_steer: bool,
    #[serde(default)]
    pub turn_status: Option<String>,
    #[serde(default)]
    pub receipt_dismissed: bool,
    #[serde(default)]
    pub recovery_cursor: Option<Value>,
}

fn hot(item: &Input) -> bool {
    !matches!(item.state.as_str(), "sent" | "cancelled")
        || (item.state == "sent" && !item.text.is_empty())
}

type Document = HashMap<String, Vec<Input>>;
pub(super) struct Queue {
    ledger: super::ledger::Ledger,
    rows: Mutex<Document>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

fn storage(error: io::Error) -> RpcError {
    RpcError::Storage(format!("待发送消息保存失败：{error}"))
}

impl Queue {
    pub fn open(directory: Option<PathBuf>) -> io::Result<Self> {
        let ledger = super::ledger::Ledger::open(directory.as_deref(), "codex-sdk-queue")?;
        if let Some(path) = directory
            .as_ref()
            .map(|d| d.join("codex-sdk-queue.json"))
            .filter(|p| p.exists())
        {
            let old: Document = serde_json::from_reader(fs::File::open(&path)?)?;
            ledger.import(old.into_iter().flat_map(|(thread, inputs)| {
                inputs
                    .into_iter()
                    .map(move |input| (thread.clone(), input.id.clone(), hot(&input), input))
            }))?;
            fs::remove_file(path)?;
        }
        let mut rows: Document = HashMap::new();
        for (thread, input) in ledger.load::<Input>(true)? {
            rows.entry(thread).or_default().push(input);
        }
        for item in rows
            .values_mut()
            .flatten()
            .filter(|item| item.state == "sending")
        {
            item.state = "failed".into();
            item.error = Some("服务已重启，消息可能已发送；请核对会话后重试或撤销。".into());
        }
        Ok(Self {
            ledger,
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
            .map(|item| json!({"id":item.id,"text":item.text,"state":item.state,"error":item.error,"direct_steer":item.direct_steer}))
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
                .map(|item| json!({"id":item.id,"text":item.text,"turn_id":item.turn_id,"turn_status":item.turn_status,"receipt_dismissed":item.receipt_dismissed}))
                .collect::<Vec<_>>()
        )
    }

    pub(super) fn recovery_pages(&self, thread: &str) -> Vec<(String, Value)> {
        self.rows
            .lock()
            .expect("queue")
            .get(thread)
            .into_iter()
            .flatten()
            .filter(|row| row.state == "sent" && !row.text.is_empty())
            .filter_map(|row| Some((row.turn_id.clone()?, row.recovery_cursor.clone()?)))
            .collect()
    }

    pub(super) fn remember_page(
        &self,
        thread: &str,
        turn: &str,
        cursor: &Value,
    ) -> Result<(), RpcError> {
        let rows: Vec<_> = self
            .rows
            .lock()
            .expect("queue")
            .get(thread)
            .into_iter()
            .flatten()
            .filter(|row| {
                row.state == "sent"
                    && row.turn_id.as_deref() == Some(turn)
                    && row.recovery_cursor.as_ref() != Some(cursor)
            })
            .cloned()
            .collect();
        for mut row in rows {
            row.recovery_cursor = Some(cursor.clone());
            self.update(thread, row)?;
        }
        Ok(())
    }

    pub(super) fn settle_turn(&self, thread: &str, turn: &Value) -> Result<(), RpcError> {
        let (Some(id), Some(status)) = (turn["id"].as_str(), turn["status"].as_str()) else {
            return Ok(());
        };
        if !matches!(status, "completed" | "interrupted" | "failed") {
            return Ok(());
        }
        for item in turn["items"].as_array().into_iter().flatten() {
            self.consumed(thread, id, item)?;
        }
        let inputs: Vec<_> = self
            .rows
            .lock()
            .expect("queue")
            .get(thread)
            .into_iter()
            .flatten()
            .filter(|row| {
                matches!(row.state.as_str(), "sending" | "sent")
                    && row.turn_id.as_deref() == Some(id)
                    && row.native_item_id.is_none()
                    && row.turn_status.as_deref() != Some(status)
            })
            .cloned()
            .collect();
        for mut input in inputs {
            input.turn_status = Some(status.into());
            self.update(thread, input)?;
        }
        Ok(())
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
            if self.ledger.consumed(thread, id).map_err(storage)? {
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

    fn get(&self, thread: &str, id: &str) -> Result<Option<Input>, RpcError> {
        if let Some(input) = self
            .rows
            .lock()
            .expect("queue")
            .get(thread)
            .and_then(|rows| rows.iter().find(|item| item.id == id))
            .cloned()
        {
            return Ok(Some(input));
        }
        self.ledger.get(thread, id).map_err(storage)
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
        let previous = match rows
            .get(thread)
            .and_then(|entries| entries.iter().find(|old| old.id == item.id))
            .cloned()
        {
            Some(old) => Some(old),
            None => self
                .ledger
                .get::<Input>(thread, &item.id)
                .map_err(storage)?,
        };
        if let Some(old) = previous.as_ref() {
            if old.state == "sent" && item.state == "sending" && item.native_item_id.is_some() {
                item.state = old.state.clone();
            }
            if old.native_item_id.is_some() {
                item.native_item_id = old.native_item_id.clone();
                item.turn_id = old.turn_id.clone();
            }
            if item.turn_id == old.turn_id && item.turn_status.is_none() {
                item.turn_status = old.turn_status.clone();
            }
            item.receipt_dismissed |= old.receipt_dismissed;
            if item.recovery_cursor.is_none() {
                item.recovery_cursor = old.recovery_cursor.clone();
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
        self.ledger
            .save(thread, &item.id, hot(&item), &item)
            .map_err(storage)?;
        let entries = rows.entry(thread.into()).or_default();
        if let Some(index) = entries.iter().position(|old| old.id == item.id) {
            if hot(&item) {
                entries[index] = item;
            } else {
                entries.remove(index);
            }
        } else if hot(&item) {
            entries.push(item);
        }
        if entries.is_empty() {
            rows.remove(thread);
        }
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
        if self.inner.queue.get(thread, &id)?.is_none() {
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
                    direct_steer: false,
                    turn_status: None,
                    receipt_dismissed: false,
                    recovery_cursor: None,
                },
            )?;
            live.cancel_capacity_retry();
            self.inner.queue_event(&live);
        }
        let item = self
            .inner
            .queue
            .get(thread, &id)?
            .ok_or(RpcError::UnknownRequest)?;
        Ok(json!({"id":id,"queued":item.state != "sent","state":item.state,"steered":false}))
    }

    /// Submit directly to a specific live turn under the same lock as FIFO.
    /// Never create a queued entry: a closing turn must not convert this to start.
    pub async fn send_steer(
        &self,
        thread: &str,
        id: String,
        text: String,
        input: Vec<Value>,
        expected_turn: String,
    ) -> Result<Value, RpcError> {
        let lock = self.inner.queue.lock(thread);
        let _guard = lock.lock().await;
        let live = self.inner.ensure(thread).await?;
        if self.inner.queue.get(thread, &id)?.is_none() {
            if live.active_turn().as_deref() != Some(expected_turn.as_str()) {
                return Err(RpcError::NoActiveTurn);
            }
            let item = Input {
                id: id.clone(),
                text,
                input,
                model: None,
                effort: None,
                state: "failed".into(),
                error: Some("引导尚未投递".into()),
                turn_id: None,
                native_item_id: None,
                direct_steer: true,
                turn_status: None,
                receipt_dismissed: false,
                recovery_cursor: None,
            };
            self.inner.queue.update(thread, item.clone())?;
            self.inner.deliver(&live, item, Some(expected_turn)).await?;
        }
        let item = self
            .inner
            .queue
            .get(thread, &id)?
            .ok_or(RpcError::UnknownRequest)?;
        Ok(
            json!({"id":id,"state":item.state,"accepted":item.state == "sent",
            "steered":item.state == "sent","error":item.error}),
        )
    }

    pub async fn queue_action(
        &self,
        thread: &str,
        id: &str,
        action: &str,
    ) -> Result<Value, RpcError> {
        let lock = self.inner.queue.lock(thread);
        let _guard = lock.lock().await;
        let mut item = self
            .inner
            .queue
            .get(thread, id)?
            .ok_or(RpcError::UnknownRequest)?;
        if action == "dismiss" && item.state == "sent" && item.turn_status.is_some() {
            // Acknowledge only the notice. Keep the completed delivery and its
            // unconsumed text; dismissal must never cancel or replay native input.
            item.receipt_dismissed = true;
            self.inner.queue.update(thread, item)?;
            if let Some(live) = self.inner.threads.lock().expect("threads").get(thread) {
                self.inner.queue_event(live);
            }
            return Ok(json!({"state":"sent","receipt_dismissed":true}));
        }
        let live = self.inner.ensure(thread).await?;
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
            "retry" if item.direct_steer => {
                let turn = live.active_turn().ok_or(RpcError::NoActiveTurn)?;
                self.inner.deliver(&live, item, Some(turn)).await?;
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
        let connection = self.thread_connection(live.id()).await?;
        // Queued inputs from older releases retain their published path text.
        item.input.retain(|part| part["type"] == "text");
        item.state = "sending".into();
        item.error = None;
        item.turn_id = steer.clone();
        item.turn_status = None;
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
        // A fast turn may finish before its acceptance RPC returns.
        self.queue
            .settle_turn(live.id(), &live.summary()["last_turn"])?;
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
