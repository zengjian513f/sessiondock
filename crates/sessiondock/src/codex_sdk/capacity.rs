//! Retry confirmed model availability failures without replaying user input.
use super::Inner;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) const REASONING_DISABLED: &str =
    "This model's reasoning capability is disabled. Please choose a different model.";

pub(super) fn reasoning_disabled(message: &str) -> Option<Value> {
    (message.trim() == REASONING_DISABLED).then(|| json!({"message": REASONING_DISABLED}))
}

#[derive(Default)]
pub(super) struct Retry {
    pub deadline: Option<Instant>,
    pub starting: bool,
    pub checked: bool,
    delay: u64,
    at: i64,
    error: Value,
}

impl Retry {
    pub fn schedule(&mut self, error: &Value) -> bool {
        let capacity = error["codexErrorInfo"] == "serverOverloaded"
            || error["codex_error_info"] == "server_overloaded"
            || error["message"].as_str().is_some_and(|message| {
                message.contains("Selected model is at capacity. Please try a different model.")
                    || reasoning_disabled(message).is_some()
            });
        if !capacity || self.deadline.is_some() {
            return false;
        }
        self.delay = if self.delay == 0 {
            15
        } else {
            (self.delay * 2).min(600)
        };
        self.deadline = Some(Instant::now() + Duration::from_secs(self.delay));
        self.at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
            + self.delay as i64 * 1000;
        self.error = error.clone();
        true
    }

    pub fn cancel(&mut self) {
        *self = Self {
            checked: true,
            ..Self::default()
        };
    }

    pub fn due(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    pub fn summary(&self) -> Value {
        if self.deadline.is_none() {
            return Value::Null;
        }
        json!({"delay_seconds":self.delay,"retry_at":self.at,"error":self.error})
    }
}

impl Inner {
    pub(super) async fn run_capacity_retries(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! { _ = self.shutdown.cancelled() => return, _ = tick.tick() => {} }
            let threads: Vec<_> = self
                .threads
                .lock()
                .expect("threads")
                .values()
                .cloned()
                .collect();
            for live in threads {
                let lock = self.queue.lock(live.id());
                let Ok(guard) = lock.try_lock_owned() else {
                    continue;
                };
                let inner = self.clone();
                tokio::spawn(async move {
                    let _guard = guard;
                    if inner.stopped.lock().expect("stopped").contains(live.id()) {
                        return;
                    }
                    // Some daemons omit turn.error or return an availability
                    // refusal as a successful reply. Read only this completed
                    // turn; never revive an older
                    // failure on open, reconnect, or after explicit cancellation.
                    if let Some((turn, path)) = live.capacity_check() {
                        let expected = turn.clone();
                        if let Ok(Some(error)) =
                            tokio::task::spawn_blocking(move || native_error(&path, &expected))
                                .await
                        {
                            live.capacity_error(&turn, error);
                        }
                    }
                    let connection = inner.connection.lock().await.clone();
                    let Some(connection) = connection
                        .filter(|c| !c.is_closed() && live.loaded_on() == Some(c.generation()))
                    else {
                        return;
                    };
                    if let Some(state) = inner.queue.pending_state(live.id()) {
                        // Consumption confirmation may still be acquiring this
                        // same lock after a very fast failed turn. Wait for its
                        // receipt before deciding whether continuation is safe.
                        if state != "sending" {
                            live.cancel_capacity_retry();
                        }
                        return;
                    }
                    if !live.take_capacity_retry() {
                        return;
                    }
                    // Empty input is the native continuation operation. It does
                    // not duplicate a user message, attachment or prior tool call,
                    // and omitted settings preserve the thread's selected model.
                    if let Err(error) = connection
                        .request("turn/start", json!({"threadId":live.id(),"input":[]}))
                        .await
                    {
                        live.starting(false);
                        live.cancel_capacity_retry();
                        live.apply(
                            "error",
                            json!({"willRetry":false,"error":{"message":error.to_string()}}),
                        );
                    }
                });
            }
        }
    }
}

fn native_error(path: &str, turn: &str) -> Option<Value> {
    let mut file = File::open(path).ok()?;
    let mut offset = file.metadata().ok()?.len();
    let mut carry = Vec::new();
    while offset > 0 {
        let size = offset.min(64 * 1024) as usize;
        offset -= size as u64;
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut bytes = vec![0; size];
        file.read_exact(&mut bytes).ok()?;
        bytes.extend_from_slice(&carry);
        let boundary = if offset == 0 {
            0
        } else {
            bytes
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |i| i + 1)
        };
        for line in bytes[boundary..].rsplit(|b| *b == b'\n') {
            let Ok(row) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            if row["type"] != "event_msg" {
                continue;
            }
            let payload = &row["payload"];
            match payload["type"].as_str() {
                Some("task_complete") => {
                    return (payload["turn_id"] == turn).then(|| {
                        if !payload["error"].is_null() {
                            payload["error"].clone()
                        } else {
                            payload["last_agent_message"]
                                .as_str()
                                .and_then(reasoning_disabled)
                                .unwrap_or(Value::Null)
                        }
                    });
                }
                Some("task_started" | "turn_aborted") => return None,
                _ => {}
            }
        }
        carry = bytes[..boundary].to_vec();
    }
    None
}
