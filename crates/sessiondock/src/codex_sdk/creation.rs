//! Bind a create intent to one hosted RPC before sending it. A late reply owns
//! the same receipt and display parent even after its HTTP/RPC waiter is gone.
use super::{Inner, RpcError};
use crate::{metadata::Predecessor, runtime::sdk_spawn::Initiator, sdk_common::save_atomic};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex};

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct Context {
    pub initiator: Option<Initiator>,
    pub continuation: Option<Predecessor>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Attempt {
    pub params: Value,
    pub context: Option<Context>,
    pub binding: Option<(String, i64)>,
}

pub(super) struct Creations {
    path: PathBuf,
    attempts: Mutex<BTreeMap<String, Attempt>>,
}

fn storage(error: impl std::fmt::Display) -> RpcError {
    RpcError::Storage(format!("Codex 创建请求保存失败：{error}"))
}

impl Creations {
    pub fn open(root: &std::path::Path) -> std::io::Result<Self> {
        crate::sdk_common::private_dir(root)?;
        let path = root.join("codex-sdk-creates.json");
        let attempts = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path,
            attempts: Mutex::new(attempts),
        })
    }

    pub fn pending(&self) -> BTreeMap<String, Attempt> {
        self.attempts.lock().expect("create attempts").clone()
    }

    fn reply(&self, channel: &str, rpc: i64) -> Option<(String, Attempt)> {
        self.attempts
            .lock()
            .expect("create attempts")
            .iter()
            .find(|(_, a)| {
                a.binding
                    .as_ref()
                    .is_some_and(|(key, id)| key == channel && *id == rpc)
            })
            .map(|(id, attempt)| (id.clone(), attempt.clone()))
    }

    fn update<T>(
        &self,
        change: impl FnOnce(&mut BTreeMap<String, Attempt>) -> T,
    ) -> Result<T, RpcError> {
        let mut attempts = self.attempts.lock().expect("create attempts");
        let mut next = attempts.clone();
        let result = change(&mut next);
        save_atomic(&self.path, &next).map_err(storage)?;
        *attempts = next;
        Ok(result)
    }

    pub fn begin(
        &self,
        id: &str,
        params: Value,
        context: Option<Context>,
    ) -> Result<Attempt, RpcError> {
        self.update(|attempts| {
            attempts
                .entry(id.to_owned())
                .or_insert(Attempt {
                    params,
                    context,
                    binding: None,
                })
                .clone()
        })
    }

    pub fn bind(&self, id: &str, channel: &str, rpc: i64) -> Result<i64, RpcError> {
        let binding = self.update(|attempts| {
            attempts.get_mut(id).map(|attempt| {
                attempt
                    .binding
                    .get_or_insert_with(|| (channel.to_owned(), rpc))
                    .clone()
            })
        })?;
        match binding {
            Some((owner, rpc)) if owner == channel => Ok(rpc),
            // A dead/replaced relay cannot prove the original start did not run.
            // Keep the uncertain attempt; never send it on a new native channel.
            _ => Err(RpcError::Disconnected),
        }
    }

    pub fn finish(&self, id: &str) -> Result<(), RpcError> {
        self.update(|attempts| {
            attempts.remove(id);
        })
    }
}

impl Inner {
    pub(super) fn creation_reply(
        &self,
        channel: &str,
        generation: u64,
        frame: &Value,
    ) -> Result<(), RpcError> {
        if frame.get("method").is_some() {
            return Ok(());
        }
        let Some(rpc) = frame["id"].as_i64() else {
            return Ok(());
        };
        let Some((request_id, attempt)) = self.creations.reply(channel, rpc) else {
            return Ok(());
        };
        if frame.get("error").is_none() {
            let result = &frame["result"];
            let path = result["thread"]["path"]
                .as_str()
                .ok_or(RpcError::Disconnected)?;
            let sid = result["thread"]["id"]
                .as_str()
                .ok_or(RpcError::Disconnected)?;
            let uid = crate::sessions::uid_for("codex", std::path::Path::new(path));
            if let Some(context) = attempt.context {
                if let (Some(store), Some(edge)) = (&self.metadata, context.continuation) {
                    store.initialize_continuation(&uid, edge).map_err(storage)?;
                }
                crate::runtime::sdk_spawn::record_metadata(
                    self.metadata.as_deref(),
                    &uid,
                    "codex",
                    sid,
                    context.initiator,
                )
                .map_err(|e| storage(e.message))?;
            }
            self.routes.lock().expect("routes").insert(
                sid.to_owned(),
                attempt.params["modelProvider"]
                    .as_str()
                    .unwrap_or("openai")
                    .to_owned(),
            );
            self.adopt_result(generation, result.clone(), Some(&request_id))?;
        } else {
            // A definitive late rejection releases the cached uncertain result
            // just like an error received by the original waiter.
            self.creates.lock().expect("creates").remove(&request_id);
        }
        self.creations.finish(&request_id)
    }
}
