//! Native SDK bridge for Kiro CLI via ACP (`kiro-cli acp`) over JSON-RPC 2.0.
//! V3 only; identities and history come from ACP and SessionDock-owned state.

mod actions;
mod checkpoint;
mod live;
mod transport;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::process::Command;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::lifecycle::launcher::CliProfile;
use crate::sessions::kiro::{Catalog, digest, random_key, save};

use actions::build_prompt_blocks;
use transport::{Session, SpawnedSession, now_secs};

const IDLE_TIMEOUT_SECS: u64 = 30 * 60;
const REAPER_INTERVAL_SECS: u64 = 60;

pub(crate) struct CachedModels {
    pub(crate) at: Instant,
    pub(crate) models: Vec<Value>,
    pub(crate) default_model: Option<String>,
}

pub struct Bridge {
    pub(super) catalog: Arc<Catalog>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    creating: Mutex<HashMap<String, Arc<Session>>>,
    pub(super) profile: CliProfile,
    pub(super) root: PathBuf,
    pub(super) proc_root: PathBuf,
    pub(super) host: Option<PathBuf>,
    pub(super) sessions: Mutex<HashMap<String, Arc<Session>>>,
    pub(super) starting: Mutex<HashSet<String>>,
    pub(super) models_cache: Mutex<Option<CachedModels>>,
    pub(super) shutdown: CancellationToken,
}

struct StartingGuard<'a> {
    set: &'a Mutex<HashSet<String>>,
    uid: String,
}

impl Drop for StartingGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.set.lock() {
            guard.remove(&self.uid);
        }
    }
}

pub fn new(
    config: &Config,
    shutdown: CancellationToken,
    catalog: Option<Arc<Catalog>>,
) -> io::Result<Option<Arc<Bridge>>> {
    if !config.native_sdk_enabled {
        return Ok(None);
    }
    let Some(catalog) = catalog else {
        return Ok(None);
    };
    let bridge = Arc::new(Bridge {
        profile: catalog.profile.clone(),
        root: catalog.root.clone(),
        catalog,
        locks: Mutex::new(HashMap::new()),
        creating: Mutex::new(HashMap::new()),
        proc_root: config.proc_root.clone(),
        host: config
            .state_dir
            .as_ref()
            .filter(|_| cfg!(unix))
            .map(|dir| dir.join("sdkhost")),
        sessions: Mutex::new(HashMap::new()),
        starting: Mutex::new(HashSet::new()),
        models_cache: Mutex::new(None),
        shutdown,
    });
    Bridge::spawn_idle_reaper(bridge.clone());
    Ok(Some(bridge))
}

impl Bridge {
    fn spawn_idle_reaper(bridge: Arc<Self>) {
        let sd = bridge.shutdown.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(REAPER_INTERVAL_SECS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        bridge.reap_idle().await;
                    }
                    _ = sd.cancelled() => {
                        break;
                    }
                }
            }
        });
    }

    async fn reap_idle(&self) {
        let now = now_secs();
        let candidates: Vec<(String, Arc<Session>)> = {
            let guard = self.sessions.lock().unwrap();
            guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };

        for (uid, session) in candidates {
            let lock = self.lock_for(&uid);
            let _guard = lock.lock().await;
            // A concurrent explicit open may have installed a new generation.
            if !self
                .sessions
                .lock()
                .unwrap()
                .get(&uid)
                .is_some_and(|current| Arc::ptr_eq(current, &session))
            {
                continue;
            }
            let last = session.last_active.load(Ordering::Acquire);
            if now.saturating_sub(last) < IDLE_TIMEOUT_SECS {
                continue;
            }
            if session.prompt_starting.load(Ordering::Acquire) {
                continue;
            }
            let busy = {
                let st = session.state.lock().unwrap();
                st.running || st.loading || !st.requests.is_empty()
            };
            if busy {
                continue;
            }
            if session.kill().await.is_ok() {
                if self.catalog.bind_host(&uid, None).is_ok() {
                    self.sessions.lock().unwrap().remove(&uid);
                }
            }
        }
    }

    pub fn loaded_uids(&self) -> Vec<String> {
        let mut set = HashSet::new();
        if let Ok(guard) = self.sessions.lock() {
            for (k, v) in guard.iter() {
                if !v.dead.load(Ordering::Acquire) {
                    set.insert(k.clone());
                }
            }
        }
        if let Ok(guard) = self.starting.lock() {
            for k in guard.iter() {
                set.insert(k.clone());
            }
        }
        set.into_iter().collect()
    }

    pub async fn stop(&self, uid: &str) -> Result<bool, String> {
        let lock = self.lock_for(uid);
        let _guard = lock.lock().await;
        let existing = self.sessions.lock().unwrap().get(uid).cloned();
        let session = if let Some(session) = existing {
            session
        } else {
            let Some(key) = self.catalog.resolve_uid(uid)?.host_key else {
                return Ok(false);
            };
            let dir = self
                .host
                .as_deref()
                .ok_or("kiro_sdk_stop_unknown: no host directory")?;
            Session::attach_existing(dir, &key, uid, "", "", self.shutdown.clone()).await?
        };
        session.kill().await?;
        self.catalog.bind_host(uid, None)?;
        self.sessions.lock().unwrap().remove(uid);
        let st = session.state.lock().unwrap();
        let _ = st
            .events
            .send(Arc::from(json!({"method":"bridge/stopped"}).to_string()));
        Ok(true)
    }

    pub fn subscribe(&self, key: &str) -> Result<(Value, broadcast::Receiver<Arc<str>>), String> {
        let session = self
            .sessions
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or_else(|| "kiro_sdk_not_open".to_string())?;
        session.touch();
        let st = session.state.lock().unwrap();
        let snap = st.snapshot(&session.uid(), &session.cwd);
        let rx = st.events.subscribe();
        Ok((snap, rx))
    }

    pub(super) fn lock_for(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .lock()
            .unwrap()
            .entry(key.into())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
    pub(super) fn host_key_for(&self, uid: &str) -> Result<String, String> {
        self.catalog
            .resolve_uid(uid)?
            .host_key
            .ok_or_else(|| "kiro_sdk_recovery_unknown: no persistent host binding".into())
    }
    pub(super) fn command(&self, cwd: &str) -> Command {
        self.catalog.command(cwd)
    }
    pub(super) fn resolve_sid(&self, uid: &str, _target: &Value, _payload: &Value) -> String {
        self.catalog
            .resolve_uid(uid)
            .map(|e| e.sid)
            .unwrap_or_default()
    }
    fn resolve_cwd(&self, target: &Value) -> String {
        if let Some(uid) = target["uid"].as_str() {
            if let Ok(entry) = self.catalog.resolve_uid(uid) {
                return entry.cwd;
            }
        }
        target["cwd"]
            .as_str()
            .or_else(|| target["folder"].as_str())
            .unwrap_or("")
            .to_owned()
    }

    pub async fn call(
        self: &Arc<Self>,
        method: &str,
        target: Value,
        payload: Value,
    ) -> Result<Value, String> {
        if self.shutdown.is_cancelled() {
            return Err("kiro_sdk_shutdown".into());
        }
        let uid = target["uid"]
            .as_str()
            .or_else(|| payload["uid"].as_str())
            .unwrap_or("");
        let cwd = self.resolve_cwd(if target.is_null() { &payload } else { &target });

        match method {
            "reattach" | "recover" => {
                let lock = self.lock_for(uid);
                let _guard = lock.lock().await;
                let existing = self.sessions.lock().unwrap().get(uid).cloned();
                let session = if let Some(session) = existing {
                    session
                } else {
                    let key = self.host_key_for(uid)?;
                    let dir = self
                        .host
                        .as_deref()
                        .ok_or("kiro_sdk_recovery_unknown: no host directory")?;
                    let sid = self.resolve_sid(uid, &target, &payload);
                    let session =
                        Session::attach_existing(dir, &key, uid, &sid, &cwd, self.shutdown.clone())
                            .await?;
                    session.set_projection(self.catalog.clone())?;
                    self.sessions
                        .lock()
                        .unwrap()
                        .insert(uid.to_string(), session.clone());
                    session
                };
                if method == "recover" {
                    session.recover_delivery(
                        payload["request_id"]
                            .as_str()
                            .ok_or("kiro_sdk_recovery_unknown: no delivery id")?,
                    )
                } else {
                    let mut snap = session.snapshot();
                    snap["attached"] = json!(true);
                    Ok(snap)
                }
            }
            "create" => self.create_session(&cwd, &payload).await,
            "open" => {
                let session = self.ensure_open(uid, &cwd, &target).await?;
                Ok(session.snapshot())
            }
            "turn" => self.turn(uid, &cwd, &target, &payload).await,
            "interrupt" => {
                let lock = self.lock_for(uid);
                let _guard = lock.lock().await;
                self.get_session(uid)?.cancel_turn().await
            }
            "steer" => self.steer(uid, &payload).await,
            "input_queue" => self.handle_queue_action(uid, &payload).await,
            "respond" => self.respond(uid, &payload).await,
            "models" => {
                let refresh = payload["refresh"] == true;
                self.models_action(uid, &cwd, &target, refresh).await
            }
            "settings" => {
                let session = self.get_session(uid)?;
                if let (Some(cid), Some(val)) =
                    (payload["configId"].as_str(), payload["value"].as_str())
                {
                    self.apply_setting(&session, cid, val).await?;
                }
                if let Some(m) = payload["model"].as_str().filter(|s| !s.is_empty()) {
                    self.apply_setting(&session, "model", m).await?;
                }
                if let Some(e) = payload["effort"]
                    .as_str()
                    .or_else(|| payload["effortLevel"].as_str())
                {
                    self.apply_setting(&session, "effort", e).await?;
                }
                if let Some(mode) = payload["mode"].as_str().filter(|s| !s.is_empty()) {
                    self.apply_setting(&session, "mode", mode).await?;
                }
                if let Some(pm) = payload["permission_mode"]
                    .as_str()
                    .or_else(|| payload["autopilot"].as_str())
                    .filter(|s| !s.is_empty())
                {
                    self.apply_setting(&session, "permission_mode", pm).await?;
                } else if let Some(ap) = payload["autopilot"].as_bool() {
                    self.apply_setting(&session, "autopilot", if ap { "true" } else { "false" })
                        .await?;
                }
                Ok(session.snapshot())
            }
            "usage" | "billing" => {
                if let Ok(session) = self.get_session(uid) {
                    let raw = session.snapshot()["raw"].clone();
                    Ok(
                        json!({"usage":raw["usage"],"billing":raw["billing"],"context_usage":raw["context_usage"],"raw":raw}),
                    )
                } else {
                    Ok(json!({"usage":null,"billing":null,"context_usage":null}))
                }
            }
            "compact" => self.compact(uid, &cwd, &target).await,
            "rename" => self.rename_session(uid, &target, &payload).await,
            "fork" => self.fork_session(uid, &cwd, &target, &payload).await,
            "discard" => self.delete_session(uid).await,
            "stop" | "close" => {
                self.stop(uid).await?;
                Ok(json!({
                    "uid": uid,
                    "stopped": true,
                    "discarded": true,
                    "native_preserved": true,
                }))
            }
            other => Err(format!("unsupported kiro sdk action: {other}")),
        }
    }

    /// Mutations use their own durable host so a lost HTTP/RPC response never
    /// causes a second fork/delete. This connection never loads the parent.
    pub(super) async fn mutate_once(
        &self,
        operation_id: &str,
        method: &str,
        cwd: &str,
        params: Value,
    ) -> Result<Value, String> {
        let key = format!("mutation:{operation_id}");
        let lock = self.lock_for(&key);
        let _guard = lock.lock().await;
        let path = self
            .root
            .join("mutations")
            .join(format!("{}.json", digest(operation_id)));
        let (mut intent, fresh) = match std::fs::read(&path) {
            Ok(bytes) => (
                serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string())?,
                false,
            ),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (
                json!({"method":method,"params":params,"cwd":cwd,"host_key":format!("kiro_mutation_{}",random_key()?)}),
                true,
            ),
            Err(e) => return Err(e.to_string()),
        };
        if intent["method"] != method || intent["params"] != params || intent["cwd"] != cwd {
            return Err("Kiro mutation request reused with different arguments".into());
        }
        if intent["stopped"] == true {
            if let Some(result) = intent.get("result") {
                return Ok(result.clone());
            }
        }
        if fresh {
            save(&path, &intent)?;
        }
        let host_key = intent["host_key"]
            .as_str()
            .ok_or("Kiro mutation host missing")?;
        let existing = self.creating.lock().unwrap().get(&key).cloned();
        let session = if let Some(session) = existing {
            session
        } else if fresh {
            match Session::spawn_or_attach(
                self.host.as_deref(),
                Some(host_key),
                "",
                "",
                cwd,
                self.command(cwd),
                self.shutdown.clone(),
            )
            .await?
            {
                SpawnedSession::Fresh(session) | SpawnedSession::Reused(session) => session,
            }
        } else {
            Session::attach_existing(
                self.host
                    .as_deref()
                    .ok_or("Kiro mutation recovery requires host")?,
                host_key,
                "",
                "",
                cwd,
                self.shutdown.clone(),
            )
            .await?
        };
        self.creating
            .lock()
            .unwrap()
            .insert(key.clone(), session.clone());
        let init=session.request_once("mutation:initialize","initialize",json!({"protocolVersion":1,"clientInfo":{"name":"sessiondock","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{}})).await?;
        let advertised = if let Some(name) = method.strip_prefix("session/") {
            init["agentCapabilities"]["sessionCapabilities"][name].is_object()
        } else {
            init["agentCapabilities"]["_meta"]["kiro"]["extensionMethods"]
                .as_array()
                .is_some_and(|methods| methods.iter().any(|value| value == method))
        };
        if !advertised {
            let _ = session.kill().await;
            return Err(format!("Kiro 未广告 {method} 能力"));
        }
        let result = session
            .request_once("mutation:execute", method, params)
            .await?;
        intent["result"] = result.clone();
        save(&path, &intent)?;
        session.kill().await?;
        intent["stopped"] = json!(true);
        save(&path, &intent)?;
        self.creating.lock().unwrap().remove(&key);
        Ok(result)
    }

    fn create_intent_path(&self, request_id: &str) -> PathBuf {
        self.root
            .join("creates")
            .join(format!("{}.json", digest(request_id)))
    }
    async fn create_session(&self, cwd: &str, payload: &Value) -> Result<Value, String> {
        let request_id = payload["request_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("Kiro 创建请求缺少稳定 request_id")?;
        let lock = self.lock_for(&format!("create:{request_id}"));
        let _guard = lock.lock().await;
        let path = self.create_intent_path(request_id);
        let choices = json!({"model":payload["model"],"effort":payload["effort"],"mode":payload["mode"],"autopilot":payload["autopilot"]});
        let (mut intent, fresh) = match std::fs::read(&path) {
            Ok(bytes) => (
                serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string())?,
                false,
            ),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (
                json!({"request_id":request_id,"cwd":cwd,"choices":choices,"host_key":format!("kiro_create_{}",random_key()?),"phase":"prepared"}),
                true,
            ),
            Err(e) => return Err(e.to_string()),
        };
        if intent["cwd"] != cwd || intent["choices"] != choices {
            return Err("同一 Kiro 创建请求不能变更目录或选项".into());
        }
        if intent["result"].is_object() {
            return Ok(intent["result"].clone());
        }
        if fresh {
            save(&path, &intent)?;
        }
        let host_key = intent["host_key"]
            .as_str()
            .ok_or("Kiro 创建宿主绑定缺失")?
            .to_owned();
        let existing_create = self.creating.lock().unwrap().get(request_id).cloned();
        let session = if let Some(session) = existing_create {
            session
        } else if fresh {
            match Session::spawn_or_attach(
                self.host.as_deref(),
                Some(&host_key),
                "",
                "",
                cwd,
                self.command(cwd),
                self.shutdown.clone(),
            )
            .await?
            {
                SpawnedSession::Fresh(session) | SpawnedSession::Reused(session) => session,
            }
        } else {
            let dir = self
                .host
                .as_deref()
                .ok_or("Kiro 创建结果待核对，缺少持久宿主")?;
            Session::attach_existing(dir, &host_key, "", "", cwd, self.shutdown.clone()).await?
        };
        self.creating
            .lock()
            .unwrap()
            .insert(request_id.into(), session.clone());
        session.request_once("create:initialize","initialize",json!({"protocolVersion":1,"clientInfo":{"name":"sessiondock","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{"_meta":{"kiro":{"userInput":true}}}})).await?;
        let mut settings = json!({});
        for (from, to) in [
            ("model", "modelId"),
            ("effort", "effortLevel"),
            ("mode", "modeId"),
        ] {
            if let Some(value) = payload[from].as_str().filter(|s| !s.is_empty()) {
                settings[to] = json!(value);
            }
        }
        let new_params = json!({"cwd":cwd,"mcpServers":[],"_meta":{"kiro":settings}});
        let mut result = match session.creation_result() {
            Some(result) => result?,
            None => {
                session
                    .request_once("create:new", "session/new", new_params)
                    .await?
            }
        };
        if let Some(wanted) = payload["model"].as_str().filter(|value| !value.is_empty()) {
            let actual = result["configOptions"]
                .as_array()
                .and_then(|options| options.iter().find(|option| option["id"] == "model"))
                .and_then(|option| option["currentValue"].as_str())
                .or_else(|| result["_meta"]["modelId"].as_str());
            if actual != Some(wanted) {
                let sid = result["sessionId"].as_str().ok_or("Kiro new missing SID")?;
                let confirmed = session
                    .request_once(
                        "create:model",
                        "session/set_config_option",
                        json!({"sessionId":sid,"configId":"model","value":wanted}),
                    )
                    .await?;
                let actual = confirmed["configOptions"]
                    .as_array()
                    .and_then(|options| options.iter().find(|option| option["id"] == "model"))
                    .and_then(|option| option["currentValue"].as_str());
                if actual != Some(wanted) {
                    return Err("Kiro 未确认所选模型，原会话身份已保留，请勿改请求 ID 重建".into());
                }
                result["configOptions"] = confirmed["configOptions"].clone();
            }
        }
        let sid = result["sessionId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Kiro session/new did not return sessionId")?;
        let metadata = json!({"sessionId":sid,"cwd":cwd,"title":result["_meta"]["title"].as_str().unwrap_or("新建 Kiro 会话"),"_meta":{"kiro":{"modelId":settings["modelId"],"createdAt":result["_meta"]["createdAt"]}}});
        let entry = self.catalog.remember_native(sid, &metadata, None)?;
        self.catalog.bind_host(&entry.uid, Some(host_key))?;
        session.bind_identity(&entry.uid, sid)?;
        session.set_projection(self.catalog.clone())?;
        self.sessions
            .lock()
            .unwrap()
            .insert(entry.uid.clone(), session.clone());
        let mut snap = session.snapshot();
        snap["session"] = json!({"uid":entry.uid,"source":"kiro","session_id":sid,"sid":sid,"cwd":cwd,"path":entry.path,"title":metadata["title"],"sdk":true});
        // Save the original identity response before returning, including when the
        // public HTTP receipt is lost. A retry never creates another session.
        intent["uid"] = json!(entry.uid);
        intent["sid"] = json!(sid);
        intent["phase"] = json!("created");
        intent["result"] = snap.clone();
        save(&path, &intent)?;
        self.creating.lock().unwrap().remove(request_id);
        Ok(snap)
    }

    pub async fn release_created(&self, request_id: &str, uid: &str) -> Result<Value, String> {
        let lock = self.lock_for(&format!("create:{request_id}"));
        let _guard = lock.lock().await;
        let path = self.create_intent_path(request_id);
        let mut intent: Value =
            serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if intent["uid"] != uid {
            return Err("Kiro CLI 移交身份不匹配".into());
        }
        if intent["phase"] == "released" {
            return Ok(json!({"uid":uid,"released":true,"released_now":false}));
        }
        let uid_lock = self.lock_for(uid);
        let _uid_guard = uid_lock.lock().await;
        let key = intent["host_key"].as_str().ok_or("Kiro 创建宿主缺失")?;
        let dir = self.host.as_deref().ok_or("Kiro CLI 移交需要持久宿主")?;
        let is_current = self.host_key_for(uid).ok().as_deref() == Some(key);
        let existing = if is_current {
            self.sessions.lock().unwrap().get(uid).cloned()
        } else {
            None
        };
        let session = if let Some(session) = existing {
            session
        } else {
            Session::attach_existing(
                dir,
                key,
                uid,
                intent["sid"].as_str().unwrap_or(""),
                intent["cwd"].as_str().unwrap_or(""),
                self.shutdown.clone(),
            )
            .await?
        };
        session.kill().await?;
        if is_current {
            self.sessions.lock().unwrap().remove(uid);
            self.catalog.bind_host(uid, None)?;
        }
        intent["phase"] = json!("released");
        save(&path, &intent)?;
        Ok(json!({"uid":uid,"released":true,"released_now":true}))
    }

    async fn finish_open_setup(
        &self,
        session: &Arc<Session>,
        uid: &str,
        sid: &str,
        cwd: &str,
    ) -> Result<(), String> {
        if session.unavailable.load(Ordering::Acquire) {
            return Err("kiro_sdk_transport_unavailable: original host retained".into());
        }
        // Only explicit open reaches this function. Startup reattachment merely
        // observes the original journal and must not load a native session.
        let initialize = {
            let st = session.state.lock().unwrap();
            if st.initialized {
                None
            } else {
                Some(
                    st.journal
                        .iter()
                        .find(|rpc| rpc.frame["method"] == "initialize")
                        .cloned(),
                )
            }
        };
        if let Some(existing) = initialize {
            let (key, params) = existing.map(|rpc| (rpc.key, rpc.frame["params"].clone())).unwrap_or_else(|| (
                "open:initialize".into(), json!({"protocolVersion":1,"clientInfo":{"name":"sessiondock","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{"_meta":{"kiro":{"userInput":true}}}})));
            session.request_once(&key, "initialize", params).await?;
        }
        let created = session
            .state
            .lock()
            .unwrap()
            .journal
            .iter()
            .find(|rpc| rpc.frame["method"] == "session/new")
            .cloned();
        if let Some(rpc) = created {
            let result = session
                .request_once(&rpc.key, "session/new", rpc.frame["params"].clone())
                .await?;
            if result["sessionId"].as_str() != Some(sid) {
                return Err("kiro_sdk_identity_mismatch".into());
            }
        } else {
            let existing = session
                .state
                .lock()
                .unwrap()
                .journal
                .iter()
                .find(|rpc| rpc.frame["method"] == "session/load")
                .cloned();
            let (key, params) = existing
                .map(|rpc| (rpc.key, rpc.frame["params"].clone()))
                .unwrap_or_else(|| {
                    (
                        "open:load".into(),
                        json!({"sessionId":sid,"cwd":cwd,"mcpServers":[]}),
                    )
                });
            session.request_once(&key, "session/load", params).await?;
        }
        session.bind_identity(uid, sid)?;
        session.set_projection(self.catalog.clone())?;
        Ok(())
    }

    pub(super) async fn ensure_open(
        &self,
        uid: &str,
        _cwd: &str,
        _target: &Value,
    ) -> Result<Arc<Session>, String> {
        let lock = self.lock_for(uid);
        let _guard = lock.lock().await;
        let entry = self.catalog.resolve_uid(uid)?;
        let existing = self.sessions.lock().unwrap().get(uid).cloned();
        if let Some(session) = existing
            .filter(|s| !s.dead.load(Ordering::Acquire) && !s.unavailable.load(Ordering::Acquire))
        {
            self.finish_open_setup(&session, uid, &entry.sid, &entry.cwd)
                .await?;
            session.touch();
            return Ok(session);
        }
        self.check_conflict(&entry.sid, None)?;
        self.starting.lock().unwrap().insert(uid.into());
        let _starting = StartingGuard {
            set: &self.starting,
            uid: uid.into(),
        };
        if let Some(key) = entry.host_key.as_deref() {
            let dir = self
                .host
                .as_deref()
                .ok_or("kiro_sdk_recovery_unknown: no host directory")?;
            let session = Session::attach_existing(
                dir,
                key,
                uid,
                &entry.sid,
                &entry.cwd,
                self.shutdown.clone(),
            )
            .await?;
            if !session.dead.load(Ordering::Acquire) {
                self.sessions
                    .lock()
                    .unwrap()
                    .insert(uid.into(), session.clone());
                self.finish_open_setup(&session, uid, &entry.sid, &entry.cwd)
                    .await?;
                return Ok(session);
            }
            self.catalog.bind_host(uid, None)?;
        }
        let key = format!("kiro_open_{}", random_key()?);
        self.catalog.bind_host(uid, Some(key.clone()))?;
        let session = match Session::spawn_or_attach(
            self.host.as_deref(),
            Some(&key),
            uid,
            &entry.sid,
            &entry.cwd,
            self.command(&entry.cwd),
            self.shutdown.clone(),
        )
        .await?
        {
            SpawnedSession::Fresh(session) | SpawnedSession::Reused(session) => session,
        };
        // Keep the actor if setup fails, so a retry cannot introduce a second reader.
        self.sessions
            .lock()
            .unwrap()
            .insert(uid.into(), session.clone());
        self.finish_open_setup(&session, uid, &entry.sid, &entry.cwd)
            .await?;
        Ok(session)
    }

    pub(super) fn get_session(&self, uid: &str) -> Result<Arc<Session>, String> {
        self.sessions
            .lock()
            .unwrap()
            .get(uid)
            .cloned()
            .ok_or_else(|| format!("kiro session {uid} is not open"))
    }

    async fn turn(
        &self,
        uid: &str,
        cwd: &str,
        target: &Value,
        payload: &Value,
    ) -> Result<Value, String> {
        let session = if uid.is_empty() {
            let snap = self.create_session(cwd, payload).await?;
            let new_uid = snap["uid"].as_str().unwrap_or("").to_string();
            self.get_session(&new_uid)?
        } else {
            self.ensure_open(uid, cwd, target).await?
        };

        let lock = self.lock_for(&session.uid());
        let _guard = lock.lock().await;
        let text = payload["text"].as_str().unwrap_or("").trim().to_string();
        let attachments = payload["attachments"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let quotes = payload["quotes"].as_array().cloned().unwrap_or_default();
        if text.is_empty() && attachments.is_empty() && quotes.is_empty() {
            return Err("prompt text is empty".into());
        }

        if session.prompt_starting.swap(true, Ordering::AcqRel) {
            return Err("kiro_sdk_session_busy: session is already starting a turn".into());
        }

        let sid = session.session_id.lock().unwrap().clone();
        if let Err(err) = self.check_conflict(&sid, Some(session.pid)) {
            session.prompt_starting.store(false, Ordering::Release);
            return Err(err);
        }

        let request_id = payload["request_id"].as_str();
        let (content, display_text) = build_prompt_blocks(&text, &attachments, &quotes);
        let prompt_params = json!({ "sessionId": sid, "prompt": content });
        let result = session
            .begin_prompt(prompt_params, &display_text, request_id)
            .await;
        session.prompt_starting.store(false, Ordering::Release);
        result
    }
}
