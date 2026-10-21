//! SessionDock-owned Kiro identity and cached history. Native storage is opaque.
use crate::{
    config::Config,
    lifecycle::{
        launcher::{CliProfile, DENIED_ENV},
        model::Source,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;

pub const FORMAT: &str = "sessiondock-kiro-projection-v1";

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub uid: String,
    pub sid: String,
    pub cwd: String,
    pub scope: String,
    pub path: PathBuf,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub host_key: Option<String>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Manifest {
    entries: BTreeMap<String, Entry>,
    #[serde(default)]
    capabilities: Value,
}

pub(crate) struct Catalog {
    pub root: PathBuf,
    pub scope: String,
    pub profile: CliProfile,
    pub native_root: PathBuf,
    manifest: Mutex<Manifest>,
    rpc_gate: tokio::sync::Mutex<()>,
}

pub(crate) fn digest(value: &str) -> String {
    format!("{:x}", Sha1::digest(value.as_bytes()))
}

pub(crate) fn random_key() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(crate) fn private_dir(path: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

pub(crate) fn save(path: &Path, value: &impl Serialize) -> Result<(), String> {
    publish(path, &serde_json::to_vec(value).map_err(|e| e.to_string())?)
}

pub(crate) fn publish(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::fs::OpenOptions;
    let parent = path.parent().ok_or("Kiro projection parent missing")?;
    private_dir(parent).map_err(|e| e.to_string())?;
    if std::fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    let temporary = parent.join(format!(".publish-{}", random_key()?));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> io::Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|e| e.to_string())
}

impl Catalog {
    pub fn new(config: &Config, shutdown: CancellationToken) -> io::Result<Option<Arc<Self>>> {
        let (Some(root), Some(native_root), Some(launcher)) = (
            &config.roots.kiro,
            &config.kiro_native_root,
            &config.launcher_config,
        ) else {
            return Ok(None);
        };
        let mut profiles = crate::lifecycle::launcher::read_config(launcher)
            .map_err(io::Error::other)?
            .profiles
            .into_iter()
            .filter(|p| p.source == Source::Kiro);
        let Some(profile) = profiles.next() else {
            return Ok(None);
        };
        if profiles.next().is_some() {
            return Err(io::Error::other("ambiguous Kiro account profiles"));
        }
        private_dir(root)?;
        let base = root.canonicalize()?;
        let account_env: BTreeMap<_, _> = profile
            .env
            .iter()
            .filter(|(key, _)| {
                matches!(
                    key.as_str(),
                    "HOME" | "KIRO_HOME" | "XDG_CONFIG_HOME" | "XDG_DATA_HOME"
                )
            })
            .collect();
        let scope = digest(
            &serde_json::to_string(&(native_root, &profile.id, account_env))
                .map_err(io::Error::other)?,
        );
        let legacy_marker = base.join("legacy-scope.json");
        let mut legacy: Value = match std::fs::read(&legacy_marker) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let marker = json!({"scope":scope,"imported":false});
                save(&legacy_marker, &marker).map_err(io::Error::other)?;
                marker
            }
            Err(error) => return Err(error),
        };
        let root = base.join(&scope);
        private_dir(&root)?;
        let root = root.canonicalize()?;
        let manifest = match std::fs::read(root.join("catalog.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Manifest::default(),
            Err(e) => return Err(e),
        };
        let catalog = Arc::new(Self {
            root,
            scope,
            profile,
            native_root: native_root.clone(),
            manifest: Mutex::new(manifest),
            rpc_gate: tokio::sync::Mutex::new(()),
        });
        // Recover the catalog-commit -> projection-removal crash window. Derive
        // the only permitted cleanup path from this scope and SID, never native metadata.
        for entry in catalog
            .manifest
            .lock()
            .unwrap()
            .entries
            .values()
            .filter(|entry| entry.deleted)
        {
            let hash = digest(&format!("{}\0{}", catalog.scope, entry.sid));
            let path = catalog.root.join("sessions").join(hash);
            match std::fs::remove_dir_all(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        // Import only explicit identities already owned by SessionDock. No native
        // path hashing, recency matching or reading private Kiro session files.
        if legacy["scope"] == catalog.scope && legacy["imported"] != true {
            if let Some(state) = &config.state_dir {
                for name in [
                    "native-sdk-bindings.json",
                    "native-sdk-sessions.json",
                    "kiro-legacy-identities.json",
                ] {
                    if let Ok(bytes) = std::fs::read(state.join(name)) {
                        let value: Value =
                            serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                        let rows: Vec<&Value> = match &value {
                            Value::Array(rows) => rows.iter().collect(),
                            Value::Object(rows) => rows.values().collect(),
                            _ => Vec::new(),
                        };
                        for row in rows {
                            if row["source"] != "kiro" {
                                continue;
                            }
                            let sid = row["sid"].as_str().or_else(|| row["session_id"].as_str());
                            if let (Some(sid), Some(uid)) = (sid, row["uid"].as_str()) {
                                if catalog
                                    .manifest
                                    .lock()
                                    .unwrap()
                                    .entries
                                    .get(sid)
                                    .is_some_and(|entry| entry.deleted)
                                {
                                    continue;
                                }
                                catalog
                                    .remember_native(sid, row, Some(uid))
                                    .map_err(io::Error::other)?;
                            }
                        }
                    }
                }
            }
            legacy["imported"] = json!(true);
            save(&legacy_marker, &legacy).map_err(io::Error::other)?;
        }
        let worker = catalog.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => {
                        if let Err(error) = worker.refresh().await { crate::log::warn("kiro.catalog_unavailable", json!({"error":error})); }
                    }
                }
            }
        });
        Ok(Some(catalog))
    }

    pub fn command(&self, cwd: &str) -> Command {
        let mut command = Command::new(&self.profile.executable);
        command
            .args(&self.profile.args)
            .args(["acp", "--agent-engine=v3", "--auth-method=cli"])
            .current_dir(cwd);
        for key in &self.profile.env_remove {
            command.env_remove(key);
        }
        for key in DENIED_ENV {
            command.env_remove(key);
        }
        command.envs(&self.profile.env);
        if !self.profile.env.contains_key("HOME") {
            let root = &self.native_root;
            let dot = root
                .ancestors()
                .find(|p| p.file_name().is_some_and(|n| n == ".kiro"));
            if let Some(home) = dot.and_then(Path::parent) {
                command.env("HOME", home);
            }
        }
        command
    }

    pub fn resolve_uid(&self, uid: &str) -> Result<Entry, String> {
        self.manifest
            .lock()
            .unwrap()
            .entries
            .values()
            .find(|entry| {
                !entry.deleted
                    && (entry.uid == uid || entry.aliases.iter().any(|alias| alias == uid))
            })
            .cloned()
            .ok_or_else(|| "Kiro 会话身份尚未通过官方目录确认".into())
    }
    pub fn resolve_sid(&self, sid: &str) -> Option<Entry> {
        self.manifest
            .lock()
            .unwrap()
            .entries
            .get(sid)
            .filter(|e| !e.deleted)
            .cloned()
    }
    fn persist(&self, manifest: &Manifest) -> Result<(), String> {
        save(&self.root.join("catalog.json"), manifest)
    }
    fn write_summary(&self, entry: &Entry) -> Result<(), String> {
        let path = entry.path.join("summary.json");
        let previous: Value = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null);
        let meta = &entry.metadata;
        let native = &meta["_meta"]["kiro"];
        let title = meta["title"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("新建 Kiro 会话");
        let created = native["createdAt"]
            .as_str()
            .or_else(|| meta["created"].as_str())
            .or_else(|| previous["session"]["created"].as_str());
        let updated = meta["updatedAt"]
            .as_str()
            .or_else(|| meta["updated"].as_str())
            .or(created);
        save(
            &path,
            &json!({"format":FORMAT,"uid":entry.uid,"aliases":entry.aliases,"scope":entry.scope,
            "session":{"id":entry.sid,"cwd":entry.cwd,"title":title,"created":created,"updated":updated,
            "model":native["modelId"].as_str().or_else(|| meta["modelId"].as_str()).or_else(|| previous["session"]["model"].as_str())},
            "history_synced":previous["history_synced"].as_bool().unwrap_or(false),"native":meta}),
        )
    }
    pub fn remember_native(
        &self,
        sid: &str,
        metadata: &Value,
        preferred_uid: Option<&str>,
    ) -> Result<Entry, String> {
        let mut guard = self.manifest.lock().unwrap();
        let mut next = guard.clone();
        let old = next.entries.get(sid).cloned();
        // A confirmed native deletion must not be undone by an old list response.
        if old.as_ref().is_some_and(|e| e.deleted) {
            return Err("Kiro 会话已删除".into());
        }
        let hash = digest(&format!("{}\0{sid}", self.scope));
        let uid = old
            .as_ref()
            .map(|e| e.uid.clone())
            .or_else(|| preferred_uid.map(str::to_owned))
            .unwrap_or_else(|| format!("kiro:{}", &hash[..16]));
        let mut merged = old
            .as_ref()
            .map(|e| e.metadata.clone())
            .unwrap_or_else(|| json!({}));
        if let (Some(target), Some(source)) = (merged.as_object_mut(), metadata.as_object()) {
            target.extend(source.clone());
        }
        let cwd = merged["cwd"]
            .as_str()
            .or_else(|| old.as_ref().map(|e| e.cwd.as_str()))
            .unwrap_or("")
            .to_owned();
        let mut aliases = old.as_ref().map(|e| e.aliases.clone()).unwrap_or_default();
        if let Some(preferred) =
            preferred_uid.filter(|value| *value != uid && !aliases.iter().any(|a| a == value))
        {
            aliases.push(preferred.to_owned());
        }
        let entry = Entry {
            uid,
            sid: sid.into(),
            cwd,
            scope: self.scope.clone(),
            path: self.root.join("sessions").join(&hash),
            metadata: merged,
            aliases,
            deleted: false,
            host_key: old.and_then(|e| e.host_key),
        };
        next.entries.insert(sid.into(), entry.clone());
        self.persist(&next)?;
        *guard = next;
        self.write_summary(&entry)?;
        Ok(entry)
    }
    pub fn bind_host(&self, uid: &str, host_key: Option<String>) -> Result<(), String> {
        let mut guard = self.manifest.lock().unwrap();
        let mut next = guard.clone();
        let entry = next
            .entries
            .values_mut()
            .find(|e| e.uid == uid)
            .ok_or("Kiro identity unavailable")?;
        entry.host_key = host_key;
        self.persist(&next)?;
        *guard = next;
        Ok(())
    }
    pub fn mark_deleted(&self, uid: &str) -> Result<(), String> {
        let mut guard = self.manifest.lock().unwrap();
        let mut next = guard.clone();
        let entry = next
            .entries
            .values_mut()
            .find(|e| e.uid == uid)
            .ok_or("Kiro identity unavailable")?;
        entry.deleted = true;
        entry.host_key = None;
        let path = entry.path.clone();
        self.persist(&next)?;
        *guard = next;
        match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
    pub fn project(&self, uid: &str, snapshot: &Value) -> Result<(), String> {
        if snapshot["loading"] == true {
            return Ok(());
        }
        let guard = self.manifest.lock().unwrap();
        let entry = guard
            .entries
            .values()
            .find(|entry| !entry.deleted && entry.uid == uid)
            .cloned()
            .ok_or("Kiro projection identity unavailable")?;
        let mut bytes = Vec::new();
        if let Some(items) = snapshot["items"].as_array() {
            for item in items {
                serde_json::to_writer(&mut bytes, &json!({"format":FORMAT,"item":item}))
                    .map_err(|e| e.to_string())?;
                bytes.push(b'\n');
            }
        }
        publish(&entry.path.join("messages.jsonl"), &bytes)?;
        let summary = entry.path.join("summary.json");
        let mut value: Value =
            serde_json::from_slice(&std::fs::read(&summary).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        value["history_synced"] = json!(true);
        if snapshot["model"].is_string() {
            value["session"]["model"] = snapshot["model"].clone();
        }
        save(&summary, &value)
    }
    pub async fn refresh(&self) -> Result<(), String> {
        let mut cursor = None;
        loop {
            let mut params = json!({});
            if let Some(value) = cursor {
                params["cursor"] = value;
            }
            let result = self.request("session/list", &params).await?;
            let rows = result["sessions"]
                .as_array()
                .ok_or("Kiro session/list 未返回 sessions")?;
            for row in rows {
                if let Some(sid) = row["sessionId"].as_str() {
                    if self
                        .manifest
                        .lock()
                        .unwrap()
                        .entries
                        .get(sid)
                        .is_some_and(|e| e.deleted)
                    {
                        continue;
                    }
                    self.remember_native(sid, row, None)?;
                }
            }
            cursor = result
                .get("nextCursor")
                .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                .cloned();
            if cursor.is_none() {
                break;
            }
        }
        Ok(())
    }
    /// A short-lived protocol connection; it never calls session/new or load.
    /// Mutation callers must supply an explicit user action and advertised method.
    pub async fn request(&self, method: &str, params: &Value) -> Result<Value, String> {
        let _gate = self.rpc_gate.lock().await;
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let mut command = self.command(&cwd.to_string_lossy());
        command
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let mut stdin = child.stdin.take().ok_or("Kiro stdin unavailable")?;
        let mut stdout =
            BufReader::new(child.stdout.take().ok_or("Kiro stdout unavailable")?).lines();
        let operation = async {
            async fn rpc(
                stdin: &mut tokio::process::ChildStdin,
                stdout: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
                id: u64,
                method: &str,
                params: &Value,
            ) -> Result<Value, String> {
                let frame = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
                stdin
                    .write_all(format!("{frame}\n").as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                while let Some(line) = stdout.next_line().await.map_err(|e| e.to_string())? {
                    let Ok(message) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if message.get("method").is_some() && message.get("id").is_some() {
                        let response = json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":"Read-only catalog client does not implement this request"}});
                        stdin
                            .write_all(format!("{response}\n").as_bytes())
                            .await
                            .map_err(|e| e.to_string())?;
                    } else if message["id"] == id {
                        if let Some(error) = message.get("error") {
                            return Err(error.to_string());
                        }
                        return Ok(message["result"].clone());
                    }
                }
                Err(
                    "Kiro ACP exited before response; authentication or transport unavailable"
                        .into(),
                )
            }
            let init = rpc(&mut stdin, &mut stdout, 1, "initialize", &json!({"protocolVersion":1,"clientInfo":{"name":"sessiondock-catalog","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{}})).await?;
            let caps = &init["agentCapabilities"];
            let advertised = if let Some(name) = method.strip_prefix("session/") {
                caps["sessionCapabilities"][name].is_object()
            } else {
                caps["_meta"]["kiro"]["extensionMethods"]
                    .as_array()
                    .is_some_and(|methods| methods.iter().any(|m| m == method))
            };
            {
                let mut guard = self.manifest.lock().unwrap();
                let mut next = guard.clone();
                next.capabilities = caps.clone();
                self.persist(&next)?;
                *guard = next;
            }
            if !advertised {
                return Err(format!("Kiro 未广告 {method} 能力"));
            }
            rpc(&mut stdin, &mut stdout, 2, method, params).await
        };
        let result = tokio::time::timeout(Duration::from_secs(30), operation)
            .await
            .map_err(|_| "Kiro catalog request timeout".to_owned())
            .and_then(|r| r);
        drop(stdin);
        let _ = child.kill().await;
        let _ = child.wait().await;
        result
    }
}

/// Canonical UID is persisted independently of the projection's physical path.
pub(crate) fn identity_for_projection(root: &Path, key: &str) -> Option<(String, Vec<String>)> {
    let info: Value = serde_json::from_slice(
        &std::fs::read(root.join("sessions").join(key).join("summary.json")).ok()?,
    )
    .ok()?;
    if info["format"] != FORMAT {
        return None;
    }
    let uid = info["uid"].as_str()?.to_owned();
    let aliases = info["aliases"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    Some((uid, aliases))
}
