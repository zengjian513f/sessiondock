//! Shared SDK dispatch and private creation receipts. Each provider owns its
//! native transport; this module never writes native session records.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

enum Provider {
    Grok(Arc<crate::grok_sdk::Bridge>),
    Agy(Arc<crate::agy_sdk::Bridge>),
    OpenCode(Arc<crate::opencode_sdk::Bridge>),
    Claude(Arc<crate::claude_sdk::Bridge>),
}
macro_rules! dispatch {
    ($provider:expr, $bridge:ident, $body:expr) => {
        match $provider {
            Provider::Grok($bridge) => $body,
            Provider::Agy($bridge) => $body,
            Provider::OpenCode($bridge) => $body,
            Provider::Claude($bridge) => $body,
        }
    };
}

pub struct NativeSdks {
    providers: BTreeMap<&'static str, Provider>,
    unavailable: BTreeMap<&'static str, String>,
    receipts: Mutex<BTreeMap<String, Value>>,
    submissions: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<Submission>>>>,
    path: Option<PathBuf>,
}

#[derive(Default)]
pub(crate) struct Submission {
    pub input: Option<Value>,
    pub draft: Option<Value>,
    pub result: Option<Value>,
}

impl NativeSdks {
    pub fn new(
        config: &crate::config::Config,
        shutdown: CancellationToken,
    ) -> io::Result<Arc<Self>> {
        let mut providers = BTreeMap::new();
        let mut unavailable = BTreeMap::new();
        // Unsupported SDK topology must not invalidate an existing node's
        // read model or ordinary CLI launcher configuration.
        macro_rules! register {
            ($source:literal, $module:ident, $variant:ident) => {
                match if config.native_sdk_enabled {
                    crate::$module::new(config, shutdown.clone())
                } else {
                    Ok(None)
                } {
                    Ok(Some(bridge)) => {
                        providers.insert($source, Provider::$variant(bridge));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        unavailable.insert($source, error.to_string());
                    }
                }
            };
        }
        register!("grok", grok_sdk, Grok);
        register!("agy", agy_sdk, Agy);
        register!("opencode", opencode_sdk, OpenCode);
        register!("claude", claude_sdk, Claude);
        let path = config
            .state_dir
            .as_ref()
            .map(|dir| dir.join("native-sdk-sessions.json"));
        let receipts = match path.as_ref().map(std::fs::read) {
            Some(Ok(bytes)) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Some(Err(error)) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => BTreeMap::new(),
        };
        Ok(Arc::new(Self {
            providers,
            unavailable,
            receipts: Mutex::new(receipts),
            submissions: Mutex::new(BTreeMap::new()),
            path,
        }))
    }

    pub fn capabilities(&self) -> Value {
        json!(
            self.providers
                .keys()
                .map(|source| ((*source).to_owned(), true))
                .collect::<BTreeMap<_, _>>()
        )
    }

    /// Resolve staged attachments once, including when an HTTP response is
    /// lost between publication and native admission. Provider request IDs
    /// still own native input deduplication.
    pub(crate) fn submission(
        &self,
        target: &Value,
        request: &str,
    ) -> Arc<tokio::sync::Mutex<Submission>> {
        let key = json!([target["source"], target["uid"], target["agent"], request]).to_string();
        self.submissions
            .lock()
            .expect("SDK submissions")
            .entry(key)
            .or_default()
            .clone()
    }

    pub fn loaded_uids(&self) -> Vec<String> {
        self.providers
            .values()
            .flat_map(|provider| dispatch!(provider, bridge, bridge.loaded_uids()))
            .collect()
    }

    pub fn unavailable(&self) -> Value {
        json!(self.unavailable)
    }

    pub fn pending(&self) -> Vec<Value> {
        self.receipts
            .lock()
            .expect("SDK receipts")
            .values()
            .filter(|row| {
                row["sdk_persisted"] != true
                    && row["source"]
                        .as_str()
                        .is_some_and(|source| self.providers.contains_key(source))
            })
            .cloned()
            .collect()
    }

    pub fn receipt(&self, uid: &str) -> Option<Value> {
        self.receipts
            .lock()
            .expect("SDK receipts")
            .get(uid)
            .filter(|row| row["sdk_persisted"] != true)
            .cloned()
    }

    /// Native inventory takes over once a creation appears. Keep its request
    /// receipt for idempotency, but never resurrect it after native deletion.
    pub fn observe_inventory(&self, inventory: &[Value]) -> Result<(), String> {
        let mut rows = self.receipts.lock().expect("SDK receipts");
        let mut next = rows.clone();
        let mut changed = false;
        for row in inventory {
            if let Some(receipt) = row["uid"].as_str().and_then(|uid| next.get_mut(uid))
                && receipt["sdk_persisted"] != true
            {
                receipt["sdk_persisted"] = json!(true);
                changed = true;
            }
        }
        if changed {
            self.save(&next)?;
            *rows = next;
        }
        Ok(())
    }

    fn save(&self, rows: &BTreeMap<String, Value>) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let temporary = path.with_extension("tmp");
        let result = (|| -> io::Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec(rows)?)?;
            file.sync_all()?;
            std::fs::rename(&temporary, path)
        })();
        result.map_err(|error| format!("SDK 创建回执保存失败：{error}"))
    }

    pub async fn call(
        &self,
        source: &str,
        method: &str,
        target: Value,
        input: Value,
    ) -> Result<Value, String> {
        let bridge = self
            .providers
            .get(source)
            .ok_or_else(|| format!("此机器未配置 {source} SDK 模式"))?;
        if method == "create" {
            let rows = self.receipts.lock().expect("SDK receipts");
            if let Some(row) = rows.values().find(|row| {
                row["source"] == source
                    && row["request_id"] == input["request_id"]
                    && row["request_id"].is_string()
            }) {
                if row["cwd"] != target["cwd"] {
                    return Err("此创建请求已用于不同的目录".into());
                }
                return Ok(
                    json!({"uid":row["uid"],"session_id":row["sid"],"cwd":row["cwd"],"session":row}),
                );
            }
        }
        let result = dispatch!(
            bridge,
            bridge,
            bridge.call(method, target.clone(), input.clone()).await
        )?;
        if method == "create" {
            let uid = result["uid"].as_str().ok_or("SDK 未返回原生会话身份")?;
            let mut row = result
                .get("session")
                .filter(|row| row.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}));
            row["uid"] = json!(uid);
            row["source"] = json!(source);
            row["sid"] = result["session_id"].clone();
            row["cwd"] = target["cwd"].clone();
            if !row["title"].is_string() {
                row["title"] = json!("新会话");
            }
            row["sdk"] = json!(true);
            // Native SDK receipts have a UID/SID already. `pending` belongs
            // to a PTY launch and would route clicks to tmux:undefined.
            row.as_object_mut()
                .expect("SDK session row")
                .remove("pending");
            row["sdk_pending"] = json!(true);
            row["request_id"] = input["request_id"].clone();
            row["sdk_model"] = input["model"].clone();
            row["sdk_effort"] = input["effort"].clone();
            let mut rows = self.receipts.lock().expect("SDK receipts");
            let mut next = rows.clone();
            next.insert(uid.to_owned(), row.clone());
            self.save(&next)?;
            *rows = next;
            let mut result = result;
            result["session"] = row;
            return Ok(result);
        }
        if method == "discard" {
            let mut rows = self.receipts.lock().expect("SDK receipts");
            let mut next = rows.clone();
            next.remove(target["uid"].as_str().unwrap_or_default());
            self.save(&next)?;
            *rows = next;
        }
        Ok(result)
    }

    pub fn subscribe(
        &self,
        source: &str,
        key: &str,
    ) -> Result<(Value, broadcast::Receiver<Arc<str>>), String> {
        let bridge = self
            .providers
            .get(source)
            .ok_or_else(|| format!("此机器未配置 {source} SDK 模式"))?;
        dispatch!(bridge, bridge, bridge.subscribe(key))
    }
}
