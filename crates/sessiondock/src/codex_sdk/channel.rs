//! Durable adapter state for an opaque sdkhost WebSocket channel.
use super::{Endpoint, RpcError};
use crate::sdk_common::save_atomic;
use sdkhost_client::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Entry {
    pub id: String,
    pub frame: Value,
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    key: String,
    offset: u64,
    initialized: Option<Value>,
    outbox: Vec<Entry>,
    requests: BTreeMap<String, Value>,
    #[serde(default)]
    subscriptions: Option<BTreeMap<String, Value>>,
}
pub(super) struct Channel {
    pub client: Client,
    path: PathBuf,
    state: Mutex<Record>,
    pub wake: Notify,
}
fn failure(e: impl std::fmt::Display) -> RpcError {
    RpcError::Storage(format!("Codex SDK channel: {e}"))
}

impl Channel {
    pub async fn open(
        root: &Path,
        endpoint: &Endpoint,
        address: &std::ffi::OsStr,
        token: Option<&str>,
    ) -> Result<Arc<Self>, RpcError> {
        let directory = crate::sdk_common::host::directory(
            &root.join("sdk-adapters/codex-channels"),
            endpoint.socket.as_os_str(),
        )
        .map_err(failure)?;
        let path = directory.join("channel.json");
        let existing = match std::fs::read(&path) {
            Ok(bytes) => Some(serde_json::from_slice::<Record>(&bytes).map_err(failure)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(failure(e)),
        };
        let host_dir = root.join("sdkhost");
        let mut attached = None;
        if let Some(record) = &existing {
            let client = Client::attach(&host_dir, &record.key)
                .await
                .map_err(failure)?;
            let status = client.status().await.map_err(failure)?;
            if status.exit.is_none() {
                attached = Some(client);
            }
            // Only a confirmed channel exit permits a new connection. Host loss is unknown.
        }
        let record = if attached.is_some() {
            existing.unwrap()
        } else {
            Record {
                key: format!(
                    "codex-channel-{}",
                    crate::sdk_common::uuid_v4().map_err(failure)?
                ),
                offset: 0,
                initialized: None,
                outbox: vec![],
                requests: BTreeMap::new(),
                subscriptions: Some(BTreeMap::new()),
            }
        };
        save_atomic(&path, &record).map_err(failure)?;
        let client = if let Some(client) = attached {
            client
        } else {
            let executable = sdkhost_client::executable().map_err(failure)?;
            let mut command = tokio::process::Command::new(executable);
            command
                .arg("--websocket")
                .arg(address)
                .env_remove("SDKHOST_CHANNEL_TOKEN");
            if let Some(token) = token {
                command.env("SDKHOST_CHANNEL_TOKEN", token);
            }
            Client::launch_or_attach(&host_dir, &record.key, &command)
                .await
                .map_err(failure)?
        };
        let channel = Arc::new(Self {
            client,
            path,
            state: Mutex::new(record),
            wake: Notify::new(),
        });
        channel.restore_subscription_checkpoint().await?;
        Ok(channel)
    }

    // Older checkpoints predate subscription summaries. Read their already
    // consumed output once, without dispatching notifications or replaying RPCs.
    // sdkhost serves compressed segments too; never inspect its files directly.
    async fn restore_subscription_checkpoint(&self) -> Result<(), RpcError> {
        let end = {
            let state = self.state.lock().unwrap();
            if state.subscriptions.is_some() {
                return Ok(());
            }
            state.offset
        };
        let mut subscriptions = BTreeMap::new();
        let mut offset = 0;
        let mut buffer = Vec::new();
        let mut scanned = 0;
        while offset < end {
            let chunk = self
                .client
                .read(sdkhost_client::Stream::Stdout, offset)
                .await
                .map_err(failure)?;
            if chunk.bytes.is_empty() {
                return Err(failure("subscription checkpoint output unavailable"));
            }
            let take = (end - offset).min(chunk.bytes.len() as u64) as usize;
            buffer.extend_from_slice(&chunk.bytes[..take]);
            offset += take as u64;
            let mut consumed = 0;
            for (index, byte) in buffer.iter().enumerate().skip(scanned) {
                if *byte != b'\n' {
                    continue;
                }
                let record: Value =
                    serde_json::from_slice(&buffer[consumed..=index]).map_err(failure)?;
                if let Some(text) = record.as_str() {
                    let frame = serde_json::from_str(text).map_err(failure)?;
                    observe_subscription(&mut subscriptions, &frame);
                }
                consumed = index + 1;
            }
            buffer.drain(..consumed);
            scanned = buffer.len();
        }
        let mut state = self.state.lock().unwrap();
        state.subscriptions = Some(subscriptions);
        save_atomic(&self.path, &*state).map_err(failure)
    }

    pub fn subscription(&self, thread: &str) -> Option<Value> {
        self.state
            .lock()
            .unwrap()
            .subscriptions
            .as_ref()?
            .get(thread)
            .cloned()
    }
    pub fn next_id(&self) -> i64 {
        self.state
            .lock()
            .unwrap()
            .outbox
            .iter()
            .filter(|e| e.frame.get("method").is_some())
            .filter_map(|e| e.frame["id"].as_i64())
            .max()
            .unwrap_or(1)
            + 1
    }
    pub fn initialized(&self) -> Option<Value> {
        self.state.lock().unwrap().initialized.clone()
    }
    pub async fn startup_error(&self) -> RpcError {
        let mut bytes = Vec::new();
        let mut offset = 0;
        while let Ok(chunk) = self
            .client
            .read(sdkhost_client::Stream::Stderr, offset)
            .await
        {
            let empty = chunk.bytes.is_empty();
            offset = chunk.next_offset;
            bytes.extend(chunk.bytes);
            if empty || chunk.eof {
                break;
            }
        }
        let detail = String::from_utf8_lossy(&bytes);
        RpcError::Unavailable(if detail.contains("WebSocket handshake failed") {
            "WebSocket 握手失败；检查 listener 和 capability token".into()
        } else {
            format!(
                "SDK host channel closed before initialize: {}",
                detail.trim()
            )
        })
    }
    pub fn offset(&self) -> u64 {
        self.state.lock().unwrap().offset
    }
    pub fn key(&self) -> String {
        self.state.lock().unwrap().key.clone()
    }
    pub fn entries(&self) -> Vec<Entry> {
        self.state.lock().unwrap().outbox.clone()
    }
    pub fn enqueue(&self, frame: Value) -> Result<(), RpcError> {
        let mut state = self.state.lock().unwrap();
        let id = if let Some(id) = frame.get("id") {
            format!(
                "{}:{id}",
                if frame.get("method").is_some() {
                    "rpc"
                } else {
                    "reply"
                }
            )
        } else if frame["method"] == "initialized" {
            "initialized".into()
        } else {
            crate::sdk_common::uuid_v4().map_err(failure)?
        };
        if state.outbox.iter().any(|e| e.id == id) {
            return Ok(());
        }
        let mut next = state.clone();
        next.outbox.push(Entry { id, frame });
        save_atomic(&self.path, &next).map_err(failure)?;
        *state = next;
        self.wake.notify_one();
        Ok(())
    }
    /// Retire only writes acknowledged by sdkhost. Keep the handshake and the
    /// highest RPC as small compatibility anchors: old readers derive their
    /// next ID from the outbox, and sdkhost deduplicates these retained entries.
    pub fn retire(&self, written: &[Entry]) -> Result<(), RpcError> {
        let mut state = self.state.lock().unwrap();
        let high = state
            .outbox
            .iter()
            .filter(|e| e.frame.get("method").is_some())
            .filter_map(|e| e.frame["id"].as_i64())
            .max();
        let ids: std::collections::BTreeSet<_> = written.iter().map(|e| e.id.as_str()).collect();
        let mut next = state.clone();
        next.outbox.retain(|e| {
            !ids.contains(e.id.as_str())
                || e.id == "rpc:1"
                || e.id == "initialized"
                || (e.frame.get("method").is_some() && e.frame["id"].as_i64() == high)
        });
        for entry in written {
            if entry.frame.get("method").is_none() && entry.frame.get("id").is_some() {
                next.requests.remove(&entry.frame["id"].to_string());
            }
        }
        if next.outbox.len() != state.outbox.len() || next.requests.len() != state.requests.len() {
            save_atomic(&self.path, &next).map_err(failure)?;
            *state = next;
        }
        Ok(())
    }
    /// Commit one complete read batch before dispatching it. Raw output is
    /// already durable in sdkhost; deltas need no individual full-state fsync.
    pub fn observe_batch(&self, frames: &[Value], offset: u64) -> Result<(), RpcError> {
        let mut state = self.state.lock().unwrap();
        let mut next = state.clone();
        next.offset = offset;
        for frame in frames {
            observe_subscription(next.subscriptions.get_or_insert_default(), frame);
            if frame.get("method").is_some() && frame.get("id").is_some() {
                next.requests.insert(frame["id"].to_string(), frame.clone());
            }
            if frame["method"] == "serverRequest/resolved" {
                next.requests
                    .remove(&frame["params"]["requestId"].to_string());
            }
            if frame["id"] == 1 && frame.get("method").is_none() {
                next.initialized = Some(frame.clone());
            }
        }
        save_atomic(&self.path, &next).map_err(failure)?;
        *state = next;
        Ok(())
    }
    pub fn pending(&self, thread: &str) -> Vec<Value> {
        let state = self.state.lock().unwrap();
        state
            .requests
            .values()
            .filter(|frame| {
                super::thread_of(&frame["params"]).as_deref() == Some(thread)
                    && !state
                        .outbox
                        .iter()
                        .any(|e| e.id == format!("reply:{}", frame["id"]))
            })
            .cloned()
            .collect()
    }
    pub async fn write(&self, entry: &Entry) -> Result<(), RpcError> {
        let mut bytes = serde_json::to_vec(&entry.frame.to_string()).map_err(failure)?;
        bytes.push(b'\n');
        // Write itself performs the durable idempotency lookup in sdkhost.
        let receipt = self
            .client
            .write(&entry.id, &bytes)
            .await
            .map_err(failure)?;
        if receipt.state != "written" {
            return Err(RpcError::Disconnected);
        }
        Ok(())
    }
    pub fn initialize_frame() -> Value {
        json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"sessiondock","title":"SessionDock","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}})
    }
}

fn observe_subscription(subscriptions: &mut BTreeMap<String, Value>, frame: &Value) {
    // start/resume/fork replies carry settings alongside thread metadata.
    // A thread/read reply or a loaded/list observation is not a subscription.
    let result = &frame["result"];
    if frame.get("method").is_none()
        && frame.get("error").is_none()
        && result.get("model").is_some()
        && let Some(id) = result["thread"]["id"].as_str()
        && result["thread"]["path"]
            .as_str()
            .is_some_and(|path| !Path::new(path).is_file())
    {
        let mut summary = result.clone();
        if let Some(thread) = summary["thread"].as_object_mut() {
            thread.remove("turns");
        }
        subscriptions.insert(id.to_owned(), summary);
    }
    let params = &frame["params"];
    let Some(id) = super::thread_of(params) else {
        return;
    };
    match frame["method"].as_str() {
        Some("thread/closed" | "thread/archived" | "thread/deleted") => {
            subscriptions.remove(&id);
        }
        Some("thread/settings/updated") => {
            if let Some(summary) = subscriptions.get_mut(&id)
                && let Some(settings) = params["threadSettings"].as_object()
            {
                for (key, value) in settings {
                    summary[if key == "effort" {
                        "reasoningEffort"
                    } else {
                        key
                    }] = value.clone();
                }
            }
        }
        Some("turn/completed")
            if subscriptions
                .get(&id)
                .and_then(|summary| summary["thread"]["path"].as_str())
                .is_some_and(|path| Path::new(path).is_file()) =>
        {
            subscriptions.remove(&id);
        }
        _ => {}
    }
}
