//! Read-only observation of the unified-exec terminals owned by a native thread.

use futures_util::stream::{FuturesUnordered, StreamExt};
use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use super::{CodexSdk, Connection, Inner, RpcError, live::LiveThread};

pub(super) const LIST: &str = "thread/backgroundTerminals/list";
const INTERVAL: Duration = Duration::from_secs(1);
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const TAIL_BYTES: usize = 64 * 1024;
// A command can emit before the next list response confirms it is still running.
const RECENT_ITEMS: usize = 4;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    data: Vec<Command>,
    next_cursor: Option<String>,
}

enum Observation {
    Loaded(Vec<Command>),
    NotLoaded,
    Unsupported,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Command {
    item_id: String,
    process_id: String,
    command: String,
}

impl Command {
    fn row(&self, output_available: bool) -> Value {
        let mut row = json!({"task": self.item_id, "command": self.command});
        if output_available {
            row["output_available"] = json!(true);
        }
        row
    }
}

struct Tail {
    text: String,
    clipped: bool,
}

impl Default for Tail {
    fn default() -> Self {
        Self {
            text: String::new(),
            // Without an observed start, there may have been earlier output.
            clipped: true,
        }
    }
}

impl Tail {
    fn append(&mut self, delta: &str) {
        let excess = self
            .text
            .len()
            .saturating_add(delta.len())
            .saturating_sub(TAIL_BYTES);
        self.clipped |= excess != 0;
        if excess >= self.text.len() {
            let start = delta.ceil_char_boundary(excess - self.text.len());
            self.text.clear();
            self.text.push_str(&delta[start..]);
        } else {
            self.text.drain(..self.text.ceil_char_boundary(excess));
            self.text.push_str(delta);
        }
    }

    fn snapshot(&self) -> Value {
        let mut raw = json!({"output": self.text});
        if self.clipped {
            raw["clipped"] = json!(true);
            raw["note"] = json!("仅显示已观察到的最近 64 KiB，较早的输出无法补读。");
        }
        raw
    }
}

struct Running {
    command: Command,
    tail: Tail,
}

#[derive(Default)]
pub(super) struct Background {
    pub(super) known: bool,
    running: IndexMap<String, Running>,
    recent: IndexMap<String, Tail>,
}

impl Background {
    pub(super) fn rows(&self) -> Vec<Value> {
        self.running
            .values()
            .map(|task| task.command.row(true))
            .collect()
    }

    pub(super) fn output(&self, item: &str) -> Option<Value> {
        self.running.get(item).map(|task| task.tail.snapshot())
    }

    pub(super) fn is_running(&self) -> bool {
        !self.running.is_empty()
    }

    pub(super) fn replace(&mut self, commands: Vec<Command>) -> bool {
        self.known = true;
        let mut old = std::mem::take(&mut self.running);
        let mut changed = old.len() != commands.len();
        for command in commands {
            let item = command.item_id.clone();
            changed |= old.first().is_none_or(|(previous, _)| previous != &item);
            let previous = old.shift_remove(&item);
            changed |= previous.as_ref().is_none_or(|previous| {
                previous.command.command != command.command
                    || previous.command.process_id != command.process_id
            });
            let tail = match previous {
                Some(previous) if previous.command.process_id == command.process_id => {
                    previous.tail
                }
                Some(_) => Tail::default(),
                None => self.recent.shift_remove(&item).unwrap_or_default(),
            };
            self.running.insert(item, Running { command, tail });
        }
        changed
    }

    pub(super) fn observe(&mut self, method: &str, params: &Value, selected: bool) {
        if method == "item/started" && params["item"]["type"] == "commandExecution" {
            if let Some(item) = params["item"]["id"].as_str() {
                if let Some(task) = self.running.get_mut(item) {
                    if task.tail.text.is_empty() {
                        task.tail.clipped = false;
                    }
                } else if selected && !self.recent.contains_key(item) {
                    self.remember(
                        item,
                        Tail {
                            text: String::new(),
                            clipped: false,
                        },
                    );
                }
            }
        } else if method == "item/commandExecution/outputDelta"
            && let (Some(item), Some(delta)) = (params["itemId"].as_str(), params["delta"].as_str())
        {
            if let Some(task) = self.running.get_mut(item) {
                task.tail.append(delta);
            } else if selected {
                let mut tail = self.recent.shift_remove(item).unwrap_or_default();
                tail.append(delta);
                self.remember(item, tail);
            }
        }
    }

    fn remember(&mut self, item: &str, tail: Tail) {
        self.recent.insert(item.to_owned(), tail);
        if self.recent.len() > RECENT_ITEMS {
            self.recent.shift_remove_index(0);
        }
    }
}

impl CodexSdk {
    /// Read an exact native thread on an existing daemon, including a thread
    /// owned by another client. This does not resume or subscribe to it.
    pub async fn background_commands(&self, thread_id: &str) -> Option<Vec<Value>> {
        let thread = self
            .inner
            .threads
            .lock()
            .expect("threads")
            .get(thread_id)
            .cloned();
        if let Some(thread) = thread
            && let Some(connection) = self.inner.background_connection(&thread).await
        {
            self.inner.refresh_background_on(&thread, &connection).await;
            if connection.is_closed() {
                return None;
            }
            return thread.background_rows(connection.generation());
        }
        tokio::time::timeout(READ_TIMEOUT, self.inner.read_background(thread_id))
            .await
            .ok()
            .flatten()
    }

    pub async fn background_output(&self, thread_id: &str, task: &str) -> Option<Value> {
        let thread = self
            .inner
            .threads
            .lock()
            .expect("threads")
            .get(thread_id)
            .cloned()?;
        let connection = self.inner.background_connection(&thread).await?;
        self.inner.refresh_background_on(&thread, &connection).await;
        if connection.is_closed() {
            return None;
        }
        thread.background_output(connection.generation(), task)
    }
}

impl Inner {
    async fn read_background(self: &Arc<Self>, thread_id: &str) -> Option<Vec<Value>> {
        let connections = self.observed_connections().await;
        if connections.is_empty() {
            return None;
        }
        let mut reads: FuturesUnordered<_> = connections
            .iter()
            .map(|connection| list(connection, thread_id))
            .collect();
        let mut failed = false;
        while let Some(result) = reads.next().await {
            match result {
                Some(Observation::Loaded(commands)) => {
                    return Some(
                        commands
                            .into_iter()
                            .map(|command| command.row(false))
                            .collect(),
                    );
                }
                Some(Observation::NotLoaded | Observation::Unsupported) => {}
                None => failed = true,
            }
        }
        (!failed).then(Vec::new)
    }

    async fn background_connection(&self, thread: &LiveThread) -> Option<Arc<Connection>> {
        let generation = thread.loaded_on()?;
        self.connected()
            .await
            .into_iter()
            .find(|connection| connection.generation() == generation)
    }

    pub(super) async fn refresh_background(&self, thread: &LiveThread) {
        if let Some(connection) = self.background_connection(thread).await {
            self.refresh_background_on(thread, &connection).await;
        }
    }

    async fn refresh_background_on(&self, thread: &LiveThread, connection: &Arc<Connection>) {
        let mut last = thread.background_poll.lock().await;
        if last.is_some_and(|at| at.elapsed() < INTERVAL) {
            return;
        }
        *last = Some(Instant::now());
        let result = tokio::time::timeout(READ_TIMEOUT, list(connection, thread.id())).await;
        if let Ok(Some(observation)) = result
            && !connection.is_closed()
        {
            let commands = match observation {
                Observation::Loaded(commands) => commands,
                Observation::NotLoaded | Observation::Unsupported => Vec::new(),
            };
            thread.set_background(connection.generation(), commands);
        }
    }

    pub(super) async fn watch_background(self: Arc<Self>) {
        let mut tick = tokio::time::interval(INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => return,
                _ = tick.tick() => {},
            }
            let threads: Vec<_> = self
                .threads
                .lock()
                .expect("threads")
                .values()
                .filter(|thread| thread.watching_background())
                .cloned()
                .collect();
            tokio::select! {
                _ = self.shutdown.cancelled() => return,
                _ = futures_util::future::join_all(threads.iter().map(|thread| self.refresh_background(thread))) => {},
            }
        }
    }
}

async fn list(connection: &Arc<Connection>, thread: &str) -> Option<Observation> {
    if !connection.background_list_supported() {
        return Some(Observation::Unsupported);
    }
    let mut items = IndexMap::new();
    let mut cursors = HashSet::new();
    let mut cursor = None;
    loop {
        let reply = connection
            .request(LIST, json!({"threadId": thread, "cursor": cursor}))
            .await;
        let result = match reply {
            Ok(result) => result,
            Err(error) => {
                if matches!(&error, RpcError::Remote { code: -32601, .. })
                    || matches!(&error, RpcError::Remote { code: -32600, message }
                        if message.contains(LIST) && (message.contains("unknown variant") || message.contains("requires experimentalApi")))
                {
                    connection.disable_background_list();
                    return Some(Observation::Unsupported);
                }
                if matches!(&error, RpcError::Remote { code: -32600, message }
                    if message.starts_with("thread not found: "))
                {
                    return Some(Observation::NotLoaded);
                }
                return None;
            }
        };
        let page: Page = serde_json::from_value(result).ok()?;
        for command in page.data {
            items.insert(command.item_id.clone(), command);
        }
        let Some(next) = page.next_cursor else {
            return Some(Observation::Loaded(items.into_values().collect()));
        };
        if !cursors.insert(next.clone()) {
            return None;
        }
        cursor = Some(next);
    }
}
