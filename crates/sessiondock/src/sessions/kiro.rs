//! SessionDock-owned Kiro identity and ACP/native read-only history projections.
use crate::{
    config::Config,
    lifecycle::{launcher::CliProfile, model::Source},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

mod native;

pub const FORMAT: &str = "sessiondock-kiro-projection-v1";

/// Account HOME implied by the `.kiro` scope root, used when the profile sets none.
pub(crate) fn account_home(native_root: &Path) -> Option<&Path> {
    native_root
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == ".kiro"))
        .and_then(Path::parent)
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Default)]
struct PendingWrites {
    sync_manifest: bool,
    summaries: BTreeSet<String>,
    removals: BTreeSet<PathBuf>,
}

pub(crate) struct Catalog {
    pub root: PathBuf,
    pub scope: String,
    pub profile: CliProfile,
    pub native_root: PathBuf,
    host_state: PathBuf,
    manifest: Mutex<Arc<Manifest>>,
    // Serialize all projection writers while keeping identity reads independent of file I/O.
    writes: Mutex<PendingWrites>,
    native_observed: Mutex<BTreeMap<PathBuf, native::Version>>,
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
    if std::fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    replace(path, bytes, || {})
}

fn replace(path: &Path, bytes: &[u8], installed: impl FnOnce()) -> Result<(), String> {
    use std::fs::OpenOptions;
    let parent = path.parent().ok_or("Kiro projection parent missing")?;
    private_dir(parent).map_err(|e| e.to_string())?;
    let temporary = parent.join(format!(".publish-{}", random_key()?));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> io::Result<()> {
        #[cfg(unix)]
        let directory = std::fs::File::open(parent)?;
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        installed();
        #[cfg(unix)]
        directory.sync_all()?;
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
            host_state: crate::sdk_common::host::state_dir(config.state_dir.as_deref()),
            manifest: Mutex::new(Arc::new(manifest)),
            writes: Mutex::default(),
            native_observed: Mutex::new(BTreeMap::new()),
        });
        // Repair any projection changes interrupted after catalog publication.
        // Derive tombstone cleanup paths from this scope and SID.
        {
            let mut writes = catalog.writes.lock().unwrap();
            for entry in catalog.manifest().entries.values() {
                if entry.deleted {
                    writes.removals.insert(catalog.projection_path(&entry.sid));
                } else if !Self::summary_matches(entry) {
                    writes.summaries.insert(entry.sid.clone());
                }
            }
            catalog
                .flush_projections(&mut writes)
                .map_err(io::Error::other)?;
        }
        // Import only explicit identities already owned by SessionDock. No native
        // path hashing, recency matching or reading private Kiro session files.
        if legacy["scope"] == catalog.scope && legacy["imported"] != true {
            let mut identities = Vec::new();
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
                                identities.push((sid.to_owned(), row.clone(), uid.to_owned()));
                            }
                        }
                    }
                }
            }
            catalog
                .merge_batch(
                    identities
                        .iter()
                        .map(|(sid, row, uid)| (sid.as_str(), row, Some(uid.as_str()))),
                    None,
                )
                .map_err(io::Error::other)?;
            legacy["imported"] = json!(true);
            save(&legacy_marker, &legacy).map_err(io::Error::other)?;
        }
        let worker = catalog.clone();
        let catalog_shutdown = shutdown.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = catalog_shutdown.cancelled() => break,
                    _ = interval.tick() => {
                        if let Err(error) = worker.refresh().await { crate::log::warn("kiro.catalog_unavailable", json!({"error":error})); }
                    }
                }
            }
        });
        let observer = catalog.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(3));
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => {
                        let observer = observer.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            if let Err(error) = observer.observe_native() {
                                crate::log::warn("kiro.native_observation", json!({"error":error}));
                            }
                        }).await;
                    }
                }
            }
        });
        Ok(Some(catalog))
    }

    pub fn command(&self, cwd: &str) -> Command {
        let mut command = crate::sdk_common::host::command(&self.profile);
        command
            .args(["acp", "--agent-engine=v3", "--auth-method=cli"])
            .current_dir(cwd);
        if !self.profile.env.contains_key("HOME")
            && let Some(home) = account_home(&self.native_root)
        {
            command.env("HOME", home);
        }
        command
    }

    pub fn resolve_uid(&self, uid: &str) -> Result<Entry, String> {
        self.manifest()
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
        self.manifest()
            .entries
            .get(sid)
            .filter(|e| !e.deleted)
            .cloned()
    }
    fn manifest(&self) -> Arc<Manifest> {
        self.manifest.lock().unwrap().clone()
    }

    fn commit(
        &self,
        writes: &mut PendingWrites,
        next: Manifest,
        installed: impl FnOnce(&mut PendingWrites),
    ) -> Result<(), String> {
        let bytes = serde_json::to_vec(&next).map_err(|e| e.to_string())?;
        replace(&self.root.join("catalog.json"), &bytes, || {
            *self.manifest.lock().unwrap() = Arc::new(next);
            writes.sync_manifest = true;
            installed(writes);
        })?;
        writes.sync_manifest = false;
        Ok(())
    }

    fn sync_manifest(&self, writes: &mut PendingWrites) -> Result<(), String> {
        // Rename makes the new file visible even if the following directory
        // sync fails. Keep memory on the installed version and retry the sync
        // before publishing or removing any dependent projections.
        #[cfg(unix)]
        if writes.sync_manifest {
            std::fs::File::open(&self.root)
                .and_then(|directory| directory.sync_all())
                .map_err(|e| e.to_string())?;
        }
        writes.sync_manifest = false;
        Ok(())
    }

    fn projection_path(&self, sid: &str) -> PathBuf {
        self.root
            .join("sessions")
            .join(digest(&format!("{}\0{sid}", self.scope)))
    }

    fn read_summary(entry: &Entry) -> Value {
        std::fs::read(entry.path.join("summary.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(Value::Null)
    }

    fn summary_matches(entry: &Entry) -> bool {
        let summary = Self::read_summary(entry);
        summary["format"] == FORMAT
            && summary["uid"] == entry.uid
            && (summary["aliases"] == json!(entry.aliases)
                || (summary["aliases"].is_null() && entry.aliases.is_empty()))
            && summary["scope"] == entry.scope
            && summary["session"]["id"] == entry.sid
            && summary["session"]["cwd"] == entry.cwd
            && (entry.metadata.is_null() || summary["native"] == entry.metadata)
    }
    fn write_summary(&self, entry: &Entry) -> Result<(), String> {
        let path = entry.path.join("summary.json");
        let previous = Self::read_summary(entry);
        let meta = if entry.metadata.is_null() {
            &previous["native"]
        } else {
            &entry.metadata
        };
        let native = &meta["_meta"]["kiro"];
        let title = meta["title"]
            .as_str()
            .or_else(|| previous["session"]["title"].as_str())
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
        let value = json!({"format":FORMAT,"uid":entry.uid,"aliases":entry.aliases,"scope":entry.scope,
            "session":{"id":entry.sid,"cwd":entry.cwd,"title":title,"created":created,"updated":updated,
            "model":native["modelId"].as_str().or_else(|| meta["modelId"].as_str()).or_else(|| previous["session"]["model"].as_str())},
            "history_synced":previous["history_synced"].as_bool().unwrap_or(false),
            "native_observed":previous["native_observed"],"native_status":previous["native_status"],"native":meta});
        let bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        replace(&path, &bytes, || {})
    }

    fn flush_projections(&self, writes: &mut PendingWrites) -> Result<(), String> {
        self.sync_manifest(writes)?;
        let manifest = self.manifest();
        let mut result = Ok(());
        writes.summaries.retain(|sid| {
            if let Err(error) = self.write_summary(&manifest.entries[sid]) {
                result = Err(error);
                true
            } else {
                false
            }
        });
        writes.removals.retain(|path| {
            if let Err(error) = Self::remove_projection(path) {
                result = Err(error);
                true
            } else {
                false
            }
        });
        result
    }

    fn flush_summary(&self, writes: &mut PendingWrites, entry: &Entry) -> Result<(), String> {
        if writes.summaries.contains(&entry.sid) {
            self.write_summary(entry)?;
            writes.summaries.remove(&entry.sid);
        }
        Ok(())
    }

    fn remove_projection(path: &Path) -> Result<(), String> {
        match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn merged_entry(
        &self,
        old: Option<&Entry>,
        sid: &str,
        metadata: &Value,
        preferred_uid: Option<&str>,
    ) -> Entry {
        let hash = digest(&format!("{}\0{sid}", self.scope));
        let uid = old
            .map(|e| e.uid.clone())
            .or_else(|| preferred_uid.map(str::to_owned))
            .unwrap_or_else(|| format!("kiro:{}", &hash[..16]));
        let mut merged = old
            .and_then(|e| e.metadata.as_object())
            .map(|meta| Value::Object(meta.clone()))
            .unwrap_or_else(|| json!({}));
        if let (Some(target), Some(source)) = (merged.as_object_mut(), metadata.as_object()) {
            target.extend(source.clone());
        }
        let cwd = merged["cwd"]
            .as_str()
            .or_else(|| old.map(|e| e.cwd.as_str()))
            .unwrap_or("")
            .to_owned();
        let mut aliases = old.map(|e| e.aliases.clone()).unwrap_or_default();
        if let Some(preferred) =
            preferred_uid.filter(|value| *value != uid && !aliases.iter().any(|a| a == value))
        {
            aliases.push(preferred.to_owned());
        }
        Entry {
            uid,
            sid: sid.into(),
            cwd,
            scope: self.scope.clone(),
            path: self.root.join("sessions").join(hash),
            metadata: merged,
            aliases,
            deleted: false,
            host_key: old.and_then(|e| e.host_key.clone()),
        }
    }

    fn merge_batch<'a>(
        &self,
        rows: impl Iterator<Item = (&'a str, &'a Value, Option<&'a str>)>,
        capabilities: Option<Value>,
    ) -> Result<(), String> {
        let mut writes = self.writes.lock().unwrap();
        let current = self.manifest();
        let mut changed = BTreeMap::new();
        for (sid, metadata, preferred_uid) in rows {
            let old = changed.get(sid).or_else(|| current.entries.get(sid));
            // A confirmed deletion stays authoritative over in-flight pages.
            if old.is_some_and(|entry| entry.deleted) {
                continue;
            }
            let entry = self.merged_entry(old, sid, metadata, preferred_uid);
            if old != Some(&entry) {
                changed.insert(sid.to_owned(), entry);
            }
        }
        changed.retain(|sid, entry| current.entries.get(sid) != Some(entry));
        let capabilities = capabilities.filter(|caps| *caps != current.capabilities);
        if !changed.is_empty() || capabilities.is_some() {
            let mut next = (*current).clone();
            let summaries: Vec<_> = changed.keys().cloned().collect();
            next.entries.extend(changed);
            if let Some(capabilities) = capabilities {
                next.capabilities = capabilities;
            }
            self.commit(&mut writes, next, |writes| {
                writes.summaries.extend(summaries)
            })?;
        }
        self.flush_projections(&mut writes)
    }

    pub async fn remember_native(
        self: &Arc<Self>,
        sid: &str,
        metadata: Value,
        preferred_uid: Option<&str>,
    ) -> Result<Entry, String> {
        let sid = sid.to_owned();
        let preferred_uid = preferred_uid.map(str::to_owned);
        let catalog = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut writes = catalog.writes.lock().unwrap();
            let current = catalog.manifest();
            let old = current.entries.get(&sid);
            if old.is_some_and(|e| e.deleted) {
                return Err("Kiro 会话已删除".into());
            }
            let entry = catalog.merged_entry(old, &sid, &metadata, preferred_uid.as_deref());
            if old != Some(&entry) {
                let mut next = (*current).clone();
                next.entries.insert(sid.clone(), entry.clone());
                catalog.commit(&mut writes, next, |writes| {
                    writes.summaries.insert(sid.clone());
                })?;
            }
            catalog.sync_manifest(&mut writes)?;
            catalog.flush_summary(&mut writes, &entry)?;
            Ok(entry)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    pub async fn bind_host(
        self: &Arc<Self>,
        uid: &str,
        host_key: Option<String>,
    ) -> Result<(), String> {
        let uid = uid.to_owned();
        let catalog = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut writes = catalog.writes.lock().unwrap();
            let entry = catalog.resolve_uid(&uid)?;
            if entry.host_key == host_key {
                return catalog.sync_manifest(&mut writes);
            }
            let mut next = (*catalog.manifest()).clone();
            next.entries.get_mut(&entry.sid).unwrap().host_key = host_key;
            catalog.commit(&mut writes, next, |_| {
                catalog.native_observed.lock().unwrap().clear();
            })
        })
        .await
        .map_err(|e| e.to_string())?
    }

    pub async fn mark_deleted(self: &Arc<Self>, uid: &str) -> Result<(), String> {
        let uid = uid.to_owned();
        let catalog = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut writes = catalog.writes.lock().unwrap();
            let entry = catalog.resolve_uid(&uid)?;
            let mut next = (*catalog.manifest()).clone();
            let deleted = next.entries.get_mut(&entry.sid).unwrap();
            deleted.deleted = true;
            deleted.host_key = None;
            catalog.commit(&mut writes, next, |writes| {
                writes.summaries.remove(&entry.sid);
                writes.removals.insert(entry.path.clone());
            })?;
            Self::remove_projection(&entry.path)?;
            writes.removals.remove(&entry.path);
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?
    }
    pub fn project(&self, uid: &str, snapshot: &Value) -> Result<(), String> {
        if snapshot["loading"] == true {
            return Ok(());
        }
        let mut writes = self.writes.lock().unwrap();
        self.sync_manifest(&mut writes)?;
        let manifest = self.manifest();
        let entry = manifest
            .entries
            .values()
            .find(|entry| !entry.deleted && entry.uid == uid)
            .ok_or("Kiro projection identity unavailable")?;
        self.flush_summary(&mut writes, entry)?;
        let mut bytes = Vec::new();
        if let Some(items) = snapshot["items"].as_array() {
            for item in items {
                serde_json::to_writer(&mut bytes, &json!({"format":FORMAT,"item":item}))
                    .map_err(|e| e.to_string())?;
                bytes.push(b'\n');
            }
        }
        if snapshot["items"].is_array() {
            publish(&entry.path.join("messages.jsonl"), &bytes)?;
        }
        let summary = entry.path.join("summary.json");
        let mut value: Value =
            serde_json::from_slice(&std::fs::read(&summary).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        value["history_synced"] = json!(true);
        value.as_object_mut().unwrap().remove("native_observed");
        value.as_object_mut().unwrap().remove("native_status");
        if snapshot["model"].is_string() {
            value["session"]["model"] = snapshot["model"].clone();
        }
        // Subtasks are private projections owned by this session, not native
        // sessions. Keeping them below the owner also makes deletion atomic
        // with the existing owner-cache cleanup.
        for task in snapshot["subtasks"].as_array().into_iter().flatten() {
            let Some(id) = task["id"].as_str() else {
                continue;
            };
            let key = digest(&format!("{}\0{}", uid, id));
            let path = entry.path.join("agents").join(&key);
            let mut bytes = Vec::new();
            for item in task["items"].as_array().into_iter().flatten() {
                serde_json::to_writer(&mut bytes, &json!({"format":FORMAT,"item":item}))
                    .map_err(|e| e.to_string())?;
                bytes.push(b'\n');
            }
            if task["status"] == "failed" && !task["failure"].is_null() {
                let failure = task["failure"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| task["failure"].to_string());
                let item = json!({"id":format!("subtask-failure:{id}"),
                    "role":"event", "event_kind":"task", "event_status":"failed",
                    "text":format!("子代理执行失败：{failure}"), "counted":false,
                    "subtask_outcome":true});
                serde_json::to_writer(&mut bytes, &json!({"format":FORMAT,"item":item}))
                    .map_err(|e| e.to_string())?;
                bytes.push(b'\n');
            }
            if task["items"].is_array() {
                publish(&path.join("messages.jsonl"), &bytes)?;
            }
            let mut child = value.clone();
            child["uid"] = json!(format!("kiro:{}", &key[..16]));
            child["aliases"] = json!([]);
            child["session"]["id"] = json!(id);
            child["session"]["title"] = task["title"].clone();
            child["agent"] = json!({"id":id,"parent_sid":entry.sid,
                "title":task["title"],"type":task["type"],"status":task["status"]});
            save(&path.join("summary.json"), &child)?;
        }
        save(&summary, &value)
    }
    async fn refresh(self: &Arc<Self>) -> Result<(), String> {
        let host_state = self.host_state.clone();
        let cwd = tokio::task::spawn_blocking(move || {
            crate::sdk_common::private_dir(&host_state)?;
            std::env::current_dir()
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        let mut params = json!({});
        let mut batch = Vec::new();
        let capabilities = loop {
            let (caps, mut result) = self.request_list_page(&cwd, &params).await?;
            let Some(Value::Array(mut rows)) = result.get_mut("sessions").map(Value::take) else {
                return Err("Kiro session/list 未返回 sessions".into());
            };
            batch.append(&mut rows);
            let cursor = result
                .get("nextCursor")
                .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()));
            let Some(cursor) = cursor else { break caps };
            params["cursor"] = cursor.clone();
        };
        let catalog = self.clone();
        tokio::task::spawn_blocking(move || {
            let rows = batch
                .iter()
                .filter_map(|row| row["sessionId"].as_str().map(|sid| (sid, row, None)));
            catalog.merge_batch(rows, Some(capabilities))
        })
        .await
        .map_err(|e| e.to_string())?
    }
    /// A short-lived protocol connection; it never calls session/new or load.
    async fn request_list_page(
        &self,
        cwd: &Path,
        params: &Value,
    ) -> Result<(Value, Value), String> {
        let command = self.command(&cwd.to_string_lossy());
        let mut spec = sdkhost_client::LaunchSpec::from_command(&command);
        spec.lifetime_ms = Some(30_000);
        let key = format!("kiro-catalog-{}", random_key()?);
        let client =
            sdkhost_client::Client::launch_spec(&self.host_state.join("sdkhost"), &key, spec)
                .await
                .map_err(|e| e.to_string())?;
        let mut stdout = crate::sdk_common::host::Lines::new(client.clone());
        let operation = async {
            async fn rpc(
                stdout: &mut crate::sdk_common::host::Lines,
                id: u64,
                method: &str,
                params: &Value,
            ) -> Result<Value, String> {
                let frame = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
                let receipt = stdout
                    .client
                    .write(&format!("rpc-{id}"), format!("{frame}\n").as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                if receipt.state != "written" {
                    return Err("Kiro catalog write unknown".into());
                }
                while let Some(line) = stdout.next().await.map_err(|e| e.to_string())? {
                    let Ok(message) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if message.get("method").is_some() && message.get("id").is_some() {
                        let response = json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":"Read-only catalog client does not implement this request"}});
                        let receipt = stdout
                            .client
                            .write(
                                &format!("reply:{}", message["id"]),
                                format!("{response}\n").as_bytes(),
                            )
                            .await
                            .map_err(|e| e.to_string())?;
                        if receipt.state != "written" {
                            return Err("Kiro catalog reply unknown".into());
                        }
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
            let init = rpc(&mut stdout, 1, "initialize", &json!({"protocolVersion":1,"clientInfo":{"name":"sessiondock-catalog","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{}})).await?;
            let caps = init["agentCapabilities"].clone();
            if !caps["sessionCapabilities"]["list"].is_object() {
                return Err("Kiro 未广告 session/list 能力".into());
            }
            let page = rpc(&mut stdout, 2, "session/list", params).await?;
            Ok((caps, page))
        };
        let result = tokio::time::timeout(Duration::from_secs(30), operation)
            .await
            .map_err(|_| "Kiro catalog request timeout".to_owned())
            .and_then(|r| r);
        let _ = client.signal(9).await;
        let _ = client.wait().await;
        let _ = client.remove().await;
        result
    }
}

/// Canonical UID is persisted independently of the projection's physical path.
pub(crate) fn identity_for_projection(path: &Path) -> Option<(String, Vec<String>)> {
    let info: Value =
        serde_json::from_slice(&std::fs::read(path.join("summary.json")).ok()?).ok()?;
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
