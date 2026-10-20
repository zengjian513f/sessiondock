//! Native SDK bridge for Kiro CLI via ACP (`kiro-cli acp`) over JSON-RPC 2.0.
//! Supports V3 (`--agent-engine=v3`, `configOptions`) primary and V2 (`modes`/`models`)
//! fallback through `sdkhost` (with legacy stdio fallback when `state_dir` is `None`).

mod actions;
mod live;
mod transport;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::process::Command;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::lifecycle::launcher::{CliProfile, DENIED_ENV};
use crate::lifecycle::model::Source;

use actions::{build_prompt_blocks, read_disk_session_metadata};
use transport::{Session, SpawnedSession, now_secs, sanitize_host_key};

const IDLE_TIMEOUT_SECS: u64 = 30 * 60;
const REAPER_INTERVAL_SECS: u64 = 60;

pub(crate) struct CachedModels {
    pub(crate) at: Instant,
    pub(crate) models: Vec<Value>,
    pub(crate) default_model: Option<String>,
}

pub struct Bridge {
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

fn home_for_root(root: &Path) -> Option<PathBuf> {
    if root.file_name().and_then(|n| n.to_str()) == Some("cli")
        && let Some(sessions_dir) = root.parent()
        && sessions_dir.file_name().and_then(|n| n.to_str()) == Some("sessions")
        && let Some(dot_kiro) = sessions_dir.parent()
        && dot_kiro.file_name().and_then(|n| n.to_str()) == Some(".kiro")
    {
        return dot_kiro.parent().map(Path::to_path_buf);
    }
    if root.file_name().and_then(|n| n.to_str()) == Some("sessions")
        && let Some(dot_kiro) = root.parent()
        && dot_kiro.file_name().and_then(|n| n.to_str()) == Some(".kiro")
    {
        return dot_kiro.parent().map(Path::to_path_buf);
    }
    if root.file_name().and_then(|n| n.to_str()) == Some(".kiro") {
        return root.parent().map(Path::to_path_buf);
    }
    None
}

pub fn new(config: &Config, shutdown: CancellationToken) -> io::Result<Option<Arc<Bridge>>> {
    if !config.native_sdk_enabled {
        return Ok(None);
    }
    let (Some(root), Some(launcher_path)) = (&config.roots.kiro, &config.launcher_config) else {
        return Ok(None);
    };
    let launcher = crate::lifecycle::launcher::read_config(launcher_path).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "kiro_sdk: launcher configuration unavailable",
        )
    })?;
    let mut iter = launcher
        .profiles
        .into_iter()
        .filter(|p| p.source == Source::Kiro);
    let Some(profile) = iter.next() else {
        return Ok(None);
    };
    if iter.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "kiro_sdk: ambiguous Kiro launcher profiles",
        ));
    }
    let host = config
        .state_dir
        .as_ref()
        .filter(|_| cfg!(unix))
        .map(|dir| dir.join("sdkhost"));
    let canonical_root = root
        .canonicalize()
        .unwrap_or_else(|_| std::path::absolute(root).unwrap_or_else(|_| root.clone()));
    let bridge = Arc::new(Bridge {
        profile,
        root: canonical_root,
        proc_root: config.proc_root.clone(),
        host,
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
            guard
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };

        for (uid, session) in candidates {
            if session.dead.load(Ordering::Acquire) {
                self.sessions.lock().unwrap().remove(&uid);
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
            self.sessions.lock().unwrap().remove(&uid);
            session.kill().await;
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
        let removed = self.sessions.lock().unwrap().remove(uid);
        if let Some(session) = removed {
            {
                let st = session.state.lock().unwrap();
                let _ = st
                    .events
                    .send(Arc::from(json!({"method": "bridge/stopped"}).to_string()));
            }
            session.kill().await;
            return Ok(true);
        }
        if let Some(ref dir) = self.host
            && cfg!(unix)
        {
            let key = sanitize_host_key(uid);
            if let Ok(client) = sdkhost_client::Client::attach(dir, &key).await {
                #[cfg(unix)]
                {
                    let _ = client.signal(libc::SIGKILL).await;
                }
                let _ = client.wait().await;
                let _ = client.remove().await;
                return Ok(true);
            }
        }
        Ok(false)
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

    pub(super) fn command(&self, cwd: &str) -> Command {
        let mut args = self.profile.args.clone();
        if !args.iter().any(|a| a == "acp") {
            args.push("acp".to_string());
        }
        let has_engine = args
            .iter()
            .any(|a| a == "--agent-engine" || a.starts_with("--agent-engine="));
        if !has_engine {
            if let Some(custom_engine) = self
                .profile
                .new_args
                .iter()
                .find(|a| a.starts_with("--agent-engine="))
            {
                args.push(custom_engine.clone());
            } else {
                args.push("--agent-engine=v3".to_string());
            }
        }
        let has_auth = args
            .iter()
            .any(|a| a == "--auth-method" || a.starts_with("--auth-method="));
        if !has_auth {
            if let Some(custom_auth) = self
                .profile
                .new_args
                .iter()
                .find(|a| a.starts_with("--auth-method="))
            {
                args.push(custom_auth.clone());
            } else {
                args.push("--auth-method=cli".to_string());
            }
        }

        let mut command = Command::new(&self.profile.executable);
        command.args(&args).current_dir(cwd);
        for name in &self.profile.env_remove {
            command.env_remove(name);
        }
        for name in DENIED_ENV {
            command.env_remove(name);
        }
        command.envs(&self.profile.env);
        if !self.profile.env.contains_key("HOME")
            && let Some(home) = home_for_root(&self.root)
        {
            command.env("HOME", home);
        }
        if !self.profile.env.contains_key("SESSIONDOCK_KIRO_ROOT") {
            command.env("SESSIONDOCK_KIRO_ROOT", &self.root);
        }
        command
    }

    pub(super) fn resolve_sid(&self, uid: &str, target: &Value, payload: &Value) -> String {
        for val in [target, payload] {
            for key in ["session_id", "sid", "sessionId"] {
                if let Some(s) = val[key].as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    return s.to_string();
                }
            }
        }
        if !uid.is_empty()
            && let Some(session) = self.sessions.lock().unwrap().get(uid).cloned()
        {
            let sid = session.session_id.lock().unwrap().clone();
            if !sid.is_empty() {
                return sid;
            }
        }
        for val in [target, payload] {
            if let Some(path_str) = val["path"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
                let path = Path::new(path_str);
                if let Some(sid) = crate::sessions::index::summary::kiro::sid_for_path(path) {
                    return sid;
                }
            }
        }
        if !uid.is_empty()
            && let Some((_, sid)) =
                crate::sessions::index::summary::kiro::locate_session_file_by_uid_or_sid(
                    &self.root, uid,
                )
            && !sid.is_empty()
        {
            return sid;
        }
        uid.strip_prefix("kiro:").unwrap_or(uid).to_string()
    }

    fn resolve_cwd(&self, target: &Value) -> String {
        for key in ["cwd", "folder", "project"] {
            if let Some(s) = target[key].as_str() {
                let t = s.trim();
                if !t.is_empty() && Path::new(t).is_dir() {
                    return t.to_string();
                }
            }
        }
        if let Some(uid) = target["uid"].as_str() {
            let sid = self.resolve_sid(uid, target, &Value::Null);
            if let Some(disk) = read_disk_session_metadata(&self.root, &sid) {
                for key in ["cwd", "workspaceDirectory", "workingDirectory"] {
                    if let Some(s) = disk[key].as_str() {
                        let t = s.trim();
                        if !t.is_empty() && Path::new(t).is_dir() {
                            return t.to_string();
                        }
                    }
                }
            }
        }
        std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/tmp".to_string())
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
            "reattach" => {
                if !uid.starts_with("kiro:") {
                    return Ok(json!({ "attached": false }));
                }
                let Some(ref dir) = self.host else {
                    return Ok(json!({ "attached": false }));
                };
                let key = sanitize_host_key(uid);
                if sdkhost_client::Client::attach(dir, &key).await.is_err() {
                    return Ok(json!({ "attached": false }));
                }
                let session = self.ensure_open(uid, &cwd, &target).await?;
                let mut snap = session.snapshot();
                snap["attached"] = json!(true);
                Ok(snap)
            }
            "recover" => {
                let session = self.ensure_open(uid, &cwd, &target).await?;
                let mut snap = session.snapshot();
                snap["accepted"] = json!(true);
                Ok(snap)
            }
            "create" => self.create_session(&cwd, &payload).await,
            "open" => {
                let session = self.ensure_open(uid, &cwd, &target).await?;
                Ok(session.snapshot())
            }
            "turn" => self.turn(uid, &cwd, &target, &payload).await,
            "interrupt" => {
                let session = self.get_session(uid)?;
                let sid = session.session_id.lock().unwrap().clone();
                let _ = session
                    .notify("session/cancel", json!({ "sessionId": sid }))
                    .await;
                session.state.lock().unwrap().mark_interrupted();
                let mut snap = session.snapshot();
                snap["accepted"] = json!(true);
                Ok(snap)
            }
            "steer" => self.steer(uid, &payload).await,
            "input_queue" => self.handle_queue_action(uid, &payload).await,
            "respond" => self.respond(uid, &payload).await,
            "models" => self.models_action(uid, &cwd, &target).await,
            "settings" => {
                let session = self.ensure_open(uid, &cwd, &target).await?;
                if let (Some(cid), Some(val)) = (
                    payload["configId"].as_str(),
                    payload["value"].as_str(),
                ) {
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
                    self.apply_setting(
                        &session,
                        "autopilot",
                        if ap { "true" } else { "false" },
                    )
                    .await?;
                }
                Ok(session.snapshot())
            }
            "usage" | "billing" => {
                let existing_session = (!uid.is_empty())
                    .then(|| self.sessions.lock().unwrap().get(uid).cloned())
                    .flatten();
                if let Some(session) = existing_session {
                    let sid = self.resolve_sid(uid, &target, &payload);
                    let need_rpc = {
                        let st = session.state.lock().unwrap();
                        st.context_usage.is_none() || st.billing.is_none()
                    };
                    if need_rpc && !session.dead.load(Ordering::Acquire) {
                        if let Ok(ctx_res) = session
                            .request("_kiro/session/context", json!({ "sessionId": sid }))
                            .await
                        {
                            session.state.lock().unwrap().apply_metadata(&ctx_res);
                        }
                        if let Ok(acct_res) = session
                            .request("_kiro/account/getUsage", json!({}))
                            .await
                        {
                            session.state.lock().unwrap().apply_metadata(&acct_res);
                        }
                    }
                    {
                        let mut st = session.state.lock().unwrap();
                        if st.context_usage.is_none() && st.billing.is_none() {
                            if let Some(disk) = read_disk_session_metadata(&self.root, &sid) {
                                st.apply_disk_metadata(&disk);
                            }
                        }
                    }
                    let snap = session.snapshot();
                    let raw = &snap["raw"];
                    return Ok(json!({
                        "usage": raw["usage"],
                        "billing": raw["billing"],
                        "context_usage": raw["context_usage"],
                        "capabilities": snap["capabilities"],
                        "raw": raw,
                    }));
                }
                if !uid.is_empty() {
                    let sid = self.resolve_sid(uid, &target, &payload);
                    if let Some(disk) = read_disk_session_metadata(&self.root, &sid) {
                        let mut temp = live::LiveState::new();
                        temp.uid = uid.to_string();
                        temp.cwd = cwd.clone();
                        temp.session_id = sid;
                        temp.apply_disk_metadata(&disk);
                        let snap = temp.snapshot(uid, &cwd);
                        let raw = &snap["raw"];
                        return Ok(json!({
                            "usage": raw["usage"],
                            "billing": raw["billing"],
                            "context_usage": raw["context_usage"],
                            "capabilities": snap["capabilities"],
                            "raw": raw,
                        }));
                    }
                }
                Ok(json!({
                    "usage": Value::Null,
                    "billing": Value::Null,
                    "context_usage": Value::Null,
                }))
            }
            "compact" => self.compact(uid, &cwd, &target).await,
            "rename" => self.rename_session(uid, &target, &payload).await,
            "fork" => self.fork_session(uid, &cwd, &target, &payload).await,
            "stop" | "close" | "discard" => {
                let _ = self.stop(uid).await;
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

    async fn create_session(&self, cwd: &str, payload: &Value) -> Result<Value, String> {
        let cmd = self.command(cwd);
        let host_key = sanitize_host_key(&format!("kiro_new_{}", actions::generate_uuid_v4()));
        let spawned = Session::spawn_or_attach(
            self.host.as_deref(),
            Some(&host_key),
            "",
            "",
            cwd,
            cmd,
            self.shutdown.clone(),
        )
        .await?;
        let session = match spawned {
            SpawnedSession::Fresh(s) | SpawnedSession::Reused(s) => s,
        };
        let init_res = match session
            .request(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientInfo": { "name": "sessiondock", "version": env!("CARGO_PKG_VERSION") },
                    "clientCapabilities": {
                        "_meta": {
                            "kiro": {
                                "userInput": true
                            }
                        }
                    }
                }),
            )
            .await
        {
            Ok(v) => v,
            Err(err) => {
                session.kill().await;
                return Err(err);
            }
        };

        let new_res = match session
            .request(
                "session/new",
                json!({
                    "cwd": cwd,
                    "mcpServers": [],
                }),
            )
            .await
        {
            Ok(v) => v,
            Err(err) => {
                session.kill().await;
                return Err(err);
            }
        };

        let sid = new_res["sessionId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("kiro session/new did not return sessionId")?
            .to_string();
        let path = crate::sessions::index::summary::kiro::locate_session_file(&self.root, &sid)
            .map(|p| p.canonicalize().unwrap_or(p))
            .unwrap_or_else(|| self.root.join("cli").join(format!("{sid}.json")));
        let uid = crate::sessions::uid_for("kiro", &path);
        session.set_uid(&uid);
        *session.session_id.lock().unwrap() = sid.clone();
        {
            let mut st = session.state.lock().unwrap();
            st.apply_initialize_result(&init_res);
            st.apply_open_result(&sid, &new_res);
        }

        if let Some(m) = payload["model"].as_str().filter(|s| !s.is_empty()) {
            let _ = self.apply_setting(&session, "model", m).await;
        }
        if let Some(e) = payload["effort"]
            .as_str()
            .or_else(|| payload["effortLevel"].as_str())
            .filter(|s| !s.is_empty())
        {
            let _ = self.apply_setting(&session, "effort", e).await;
        }
        if let Some(mode) = payload["mode"].as_str().filter(|s| !s.is_empty()) {
            let _ = self.apply_setting(&session, "mode", mode).await;
        }
        if let Some(pm) = payload["permission_mode"]
            .as_str()
            .or_else(|| payload["autopilot"].as_str())
            .filter(|s| !s.is_empty())
        {
            let _ = self.apply_setting(&session, "permission_mode", pm).await;
        } else if let Some(ap) = payload["autopilot"].as_bool() {
            let _ = self
                .apply_setting(&session, "autopilot", if ap { "true" } else { "false" })
                .await;
        }

        self.sessions
            .lock()
            .unwrap()
            .insert(uid.clone(), session.clone());
        let mut snap = session.snapshot();
        snap["session"] = json!({
            "uid": uid,
            "source": "kiro",
            "session_id": sid,
            "sid": sid,
            "cwd": cwd,
            "path": path,
            "title": "新建 Kiro 会话",
            "sdk": true,
        });
        Ok(snap)
    }

    pub(super) async fn ensure_open(
        &self,
        uid: &str,
        cwd: &str,
        target: &Value,
    ) -> Result<Arc<Session>, String> {
        if uid.is_empty() {
            return Err("missing kiro session uid".into());
        }
        let existing = self.sessions.lock().unwrap().get(uid).cloned();
        if let Some(existing) = existing {
            if !existing.dead.load(Ordering::Acquire) {
                existing.touch();
                return Ok(existing);
            }
            self.sessions.lock().unwrap().remove(uid);
        }

        let sid = self.resolve_sid(uid, target, &Value::Null);
        self.check_conflict(&sid, None)?;

        self.starting.lock().unwrap().insert(uid.to_string());
        let _starting_guard = StartingGuard {
            set: &self.starting,
            uid: uid.to_string(),
        };

        let cmd = self.command(cwd);
        let host_key = sanitize_host_key(uid);
        let spawned = Session::spawn_or_attach(
            self.host.as_deref(),
            Some(&host_key),
            uid,
            &sid,
            cwd,
            cmd,
            self.shutdown.clone(),
        )
        .await?;

        let session = match spawned {
            SpawnedSession::Reused(session) => {
                if let Some(disk) = read_disk_session_metadata(&self.root, &sid) {
                    session.state.lock().unwrap().apply_disk_metadata(&disk);
                }
                if let Some(obs) = target["observed_model"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    let mut st = session.state.lock().unwrap();
                    if st.model.is_none() {
                        st.model = Some(obs.to_string());
                        st.model_observed = true;
                    }
                }
                self.sessions
                    .lock()
                    .unwrap()
                    .insert(uid.to_string(), session.clone());
                return Ok(session);
            }
            SpawnedSession::Fresh(session) => session,
        };

        if let Some(disk) = read_disk_session_metadata(&self.root, &sid) {
            session.state.lock().unwrap().apply_disk_metadata(&disk);
        }
        if let Some(obs) = target["observed_model"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let mut st = session.state.lock().unwrap();
            if st.model.is_none() {
                st.model = Some(obs.to_string());
                st.model_observed = true;
            }
        }

        let init_res = match session
            .request(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientInfo": { "name": "sessiondock", "version": env!("CARGO_PKG_VERSION") },
                    "clientCapabilities": {
                        "_meta": {
                            "kiro": {
                                "userInput": true
                            }
                        }
                    }
                }),
            )
            .await
        {
            Ok(v) => v,
            Err(err) => {
                session.kill().await;
                return Err(err);
            }
        };

        {
            let mut st = session.state.lock().unwrap();
            st.apply_initialize_result(&init_res);
            st.loading = true;
        }
        let load_res = session
            .request(
                "session/load",
                json!({
                    "sessionId": sid,
                    "cwd": cwd,
                    "mcpServers": [],
                }),
            )
            .await;
        session.state.lock().unwrap().loading = false;

        let res = match load_res {
            Ok(v) => v,
            Err(err) => {
                session.kill().await;
                return Err(format!("kiro session/load failed: {err}"));
            }
        };

        *session.session_id.lock().unwrap() = sid.clone();
        {
            let mut st = session.state.lock().unwrap();
            st.apply_open_result(&sid, &res);
        }

        self.sessions
            .lock()
            .unwrap()
            .insert(uid.to_string(), session.clone());
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
        {
            let mut st = session.state.lock().unwrap();
            if st.running {
                session.prompt_starting.store(false, Ordering::Release);
                return Err("kiro_sdk_session_busy: session is already running a turn".into());
            }
            // Do NOT pass picker model/effort overrides here so turn never overwrites
            // the session's active model/effort. Model/effort changes use `settings`.
            st.begin_turn(&display_text, request_id, None, None);
        }
        session.prompt_starting.store(false, Ordering::Release);

        let prompt_params = json!({
            "sessionId": sid,
            "prompt": content,
        });

        let s = session.clone();
        tokio::spawn(async move {
            let res = s.request("session/prompt", prompt_params).await;
            let mut st = s.state.lock().unwrap();
            match res {
                Ok(val) => st.finish_turn(&val),
                Err(err) => st.fail_turn(&err),
            }
        });

        let mut snap = session.snapshot();
        snap["accepted"] = json!(true);
        if let Some(rid) = request_id {
            snap["request_id"] = json!(rid);
        }
        Ok(snap)
    }
}
