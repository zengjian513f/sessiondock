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
        Ok(Arc::new(Self {
            client,
            path,
            state: Mutex::new(record),
            wake: Notify::new(),
        }))
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
    pub fn observe(&self, frame: &Value, offset: u64) -> Result<(), RpcError> {
        let mut state = self.state.lock().unwrap();
        let mut next = state.clone();
        next.offset = offset;
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
        let receipt = match self.client.receipt(&entry.id).await.map_err(failure)? {
            Some(receipt) => receipt,
            None => self
                .client
                .write(&entry.id, &bytes)
                .await
                .map_err(failure)?,
        };
        if receipt.state != "written" {
            return Err(RpcError::Disconnected);
        }
        Ok(())
    }
    pub fn initialize_frame() -> Value {
        json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"sessiondock","title":"SessionDock","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}})
    }
}
