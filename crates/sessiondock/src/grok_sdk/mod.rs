//! Grok's native ACP bridge. Contract and protocol evidence: docs/grok-sdk.md.
//! No HTTP, PTY keystrokes, native transcript writes or implicit CLI discovery.
mod actions;
mod idle;
mod live;
mod steering;
mod transport;

use crate::{
    config::Config,
    lifecycle::{
        launcher::{CliProfile, DENIED_ENV},
        model::Source,
    },
};
use live::Live;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::Ordering},
};
use tokio::sync::{Mutex as AsyncMutex, broadcast};
use tokio_util::sync::CancellationToken;
use transport::Connection;

struct Session {
    uid: String,
    sid: String,
    cwd: String,
    path: PathBuf,
    live: Arc<Mutex<Live>>,
    connection: Arc<Connection>,
    turns: Arc<AsyncMutex<()>>,
    steer_gate: AsyncMutex<()>,
}

pub struct Bridge {
    root: PathBuf,
    state_dir: Option<PathBuf>,
    home: PathBuf,
    profile: CliProfile,
    proc_root: PathBuf,
    active: PathBuf,
    shutdown: CancellationToken,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    // Serialize native attach/create, including retries of lost HTTP receipts.
    attach: AsyncMutex<()>,
    creates: AsyncMutex<HashMap<String, Result<Value, String>>>,
    mutations: AsyncMutex<HashMap<String, Result<Value, String>>>,
}

/// Configuration only: a read root without a Grok launcher profile never
/// starts a process. Native storage remains owned by Grok.
pub fn new(config: &Config, shutdown: CancellationToken) -> io::Result<Option<Arc<Bridge>>> {
    let (Some(root), Some(launcher_path)) = (&config.roots.grok, &config.launcher_config) else {
        return Ok(None);
    };
    let launcher = crate::lifecycle::launcher::read_config(launcher_path).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "grok_sdk: launcher configuration unavailable",
        )
    })?;
    let mut profiles = launcher
        .profiles
        .into_iter()
        .filter(|p| p.source == Source::Grok);
    let Some(profile) = profiles.next() else {
        return Ok(None);
    };
    if profiles.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "grok_sdk: ambiguous Grok launcher profiles",
        ));
    }
    let absolute = |path: &Path| -> io::Result<PathBuf> {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        Ok(path.canonicalize().unwrap_or(path))
    };
    let root = absolute(root)?;
    // Grok offers GROK_HOME, not a free-form sessions-root override. Do not
    // silently launch into another home than the inventory we were given.
    let home = if root.file_name().is_some_and(|name| name == "sessions") {
        root.parent().unwrap().to_path_buf()
    } else if let Some(home) = profile.env.get("GROK_HOME") {
        absolute(Path::new(home))?
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "grok_sdk: root must resolve to GROK_HOME/sessions",
        ));
    };
    if absolute(&home.join("sessions"))? != root {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "grok_sdk: launcher GROK_HOME and session root differ",
        ));
    }
    if let Some(configured_home) = profile.env.get("GROK_HOME")
        && absolute(Path::new(configured_home))? != home
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "grok_sdk: launcher GROK_HOME and session root differ",
        ));
    }
    let active = config
        .grok_active
        .clone()
        .unwrap_or_else(|| home.join("active_sessions.json"));
    let saved_attempts = |name: &str| -> io::Result<HashMap<String, Result<Value, String>>> {
        let Some(root) = &config.state_dir else {
            return Ok(HashMap::new());
        };
        match std::fs::read(root.join("sdk-adapters/grok").join(name)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e),
        }
    };
    let bridge = Arc::new(Bridge {
        root,
        state_dir: config.state_dir.clone().filter(|_| cfg!(unix)),
        home,
        profile,
        proc_root: config.proc_root.clone(),
        active,
        shutdown,
        sessions: Mutex::new(HashMap::new()),
        attach: AsyncMutex::new(()),
        creates: AsyncMutex::new(saved_attempts("creates.json")?),
        mutations: AsyncMutex::new(saved_attempts("mutations.json")?),
    });
    bridge.start_idle_reaper();
    Ok(Some(bridge))
}

impl Bridge {
    fn save_attempts(
        &self,
        name: &str,
        attempts: &HashMap<String, Result<Value, String>>,
    ) -> Result<(), String> {
        if self.state_dir.is_none() {
            return Ok(());
        }
        let root = self
            .state_dir
            .as_ref()
            .ok_or("grok_sdk_host: state directory unavailable")?
            .join("sdk-adapters/grok");
        transport::private_dir(&root).map_err(|e| format!("grok_sdk_checkpoint: {e}"))?;
        transport::save(&root.join(name), attempts).map_err(|e| format!("grok_sdk_checkpoint: {e}"))
    }
    fn checkpoint_path(&self, identity: &str) -> Result<PathBuf, String> {
        let root = self
            .state_dir
            .as_ref()
            .ok_or("grok_sdk_host: state directory unavailable")?
            .join("sdk-adapters/grok");
        transport::private_dir(&root).map_err(|e| format!("grok_sdk_checkpoint: {e}"))?;
        let key: String = identity
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(root.join(format!("{key}.json")))
    }
    async fn reattach(&self, target: &Value) -> Result<Value, String> {
        if self.state_dir.is_none() {
            return Ok(json!({"attached":false}));
        }
        let key = target_key(target)?;
        let bytes = match std::fs::read(self.checkpoint_path(&key)?) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(json!({"attached":false})),
            Err(e) => return Err(format!("grok_sdk_checkpoint: {e}")),
        };
        let saved: Live =
            serde_json::from_slice(&bytes).map_err(|_| "grok_sdk_recovery_invalid")?;
        if saved.host_key.is_none() {
            return Ok(json!({"attached":false}));
        }
        let session = match self.open_with_spawn(target, false).await {
            Ok(session) => session,
            Err(error) if error == "grok_sdk_recovery_missing" => {
                return Ok(json!({"attached":false}));
            }
            Err(error) => return Err(error),
        };
        let mut receipt = self.receipt(&session);
        receipt["attached"] = json!(true);
        Ok(receipt)
    }
    async fn recover(&self, target: &Value, input: &Value) -> Result<Value, String> {
        if self.state_dir.is_none() {
            return Err("grok_sdk_recovery_missing".into());
        }
        let key = target_key(target)?;
        let request_id = required(input, "request_id")?;
        let bytes =
            std::fs::read(self.checkpoint_path(&key)?).map_err(|_| "grok_sdk_recovery_missing")?;
        let saved: Live =
            serde_json::from_slice(&bytes).map_err(|_| "grok_sdk_recovery_invalid")?;
        if saved.sid != required(target, "session_id")? {
            return Err("grok_sdk_identity_mismatch".into());
        }
        if saved.host_key.is_none() {
            return saved
                .receipts
                .get(request_id)
                .cloned()
                .ok_or_else(|| "grok_sdk_recovery_missing".into());
        }
        // open recognizes the saved host identity and only attaches, with no
        // initialize/load or prompt replay. Missing/dead hosts remain errors.
        let session = self.open_with_spawn(target, false).await?;
        let state = session.live.lock().unwrap();
        state
            .receipts
            .get(request_id)
            .cloned()
            .ok_or_else(|| "grok_sdk_recovery_missing".into())
    }
    pub async fn stop(&self, uid: &str) -> Result<bool, String> {
        let _gate = self.attach.lock().await;
        let session = self.sessions.lock().unwrap().remove(uid);
        let Some(session) = session else {
            return Ok(false);
        };
        {
            let live = session.live.lock().unwrap();
            let _ = live
                .events
                .send(Arc::from(json!({"method":"bridge/stopped"}).to_string()));
        }
        session.connection.stop.cancel();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            session.connection.done.cancelled(),
        )
        .await
        .map_err(|_| "Grok SDK 停止尚未确认")?;
        Ok(true)
    }

    pub fn loaded_uids(&self) -> Vec<String> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|(key, session)| {
                key.split(':').count() == 2 && session.connection.alive.load(Ordering::Acquire)
            })
            .map(|(_, session)| session.uid.clone())
            .collect()
    }

    /// The detached owner task makes accepted work independent of HTTP/SSE
    /// cancellation, including a lost create response.
    pub async fn call(
        self: &Arc<Self>,
        method: &str,
        target: Value,
        input: Value,
    ) -> Result<Value, String> {
        let bridge = self.clone();
        let method = method.to_owned();
        tokio::spawn(async move { bridge.dispatch(&method, target, input).await })
            .await
            .map_err(|_| "grok_sdk_internal_task_failed".to_owned())?
    }

    pub fn subscribe(&self, key: &str) -> Result<(Value, broadcast::Receiver<Arc<str>>), String> {
        let session = self
            .sessions
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or("grok_sdk_not_open")?;
        let mut live = session.live.lock().unwrap();
        live.idle_since = None;
        // snapshot and receiver are established under the publication lock.
        Ok((live.snapshot(), live.events.subscribe()))
    }

    async fn dispatch(
        self: &Arc<Self>,
        method: &str,
        target: Value,
        input: Value,
    ) -> Result<Value, String> {
        if self.shutdown.is_cancelled() {
            return Err("grok_sdk_shutdown".into());
        }
        if method == "reattach" {
            return self.reattach(&target).await;
        }
        if method == "recover" {
            return self.recover(&target, &input).await;
        }
        if method == "create" {
            return self.create(&target, &input).await;
        }
        if method == "models" && target["uid"].as_str().is_none() {
            let cwd = required(&target, "cwd")?;
            let live = Arc::new(Mutex::new(Live::new(String::new())));
            let connection = self.connection(cwd, live, None).await?;
            let initialized = connection.initialized.lock().unwrap().clone();
            connection.stop.cancel();
            let models = &initialized["_meta"]["modelState"];
            if models.is_null() {
                return Err(
                    "grok_sdk_models_unsupported: initialize did not advertise modelState".into(),
                );
            }
            return Ok(
                json!({"models":models["availableModels"],"native":models,"capabilities":{"models":true}}),
            );
        }
        if !matches!(
            method,
            "open"
                | "turn"
                | "interrupt"
                | "respond"
                | "models"
                | "discard"
                | "usage"
                | "billing"
                | "settings"
                | "compact"
                | "fork"
                | "rewind_points"
                | "rewind"
                | "shell"
                | "steer"
        ) {
            return Err(format!("grok_sdk_unsupported_method: {method}"));
        }
        if method == "discard" {
            let _gate = self.attach.lock().await;
            return self.discard(&target);
        }
        let session = self.open(&target).await?;
        match method {
            "open" => Ok(self.receipt(&session)),
            "models" => {
                let live = session.live.lock().unwrap();
                if live.models.is_null() && live.config.is_null() {
                    return Err("grok_sdk_models_unsupported".into());
                }
                Ok(
                    json!({"models":live.models["availableModels"],"native":live.models,"configOptions":live.config,"capabilities":capabilities(&live)}),
                )
            }
            "turn" => self.turn(session, &input, false).await,
            "shell" => self.turn(session, &input, true).await,
            "steer" => self.steer(&session, &input).await,
            "usage" | "billing" | "rewind_points" => self.read_extension(&session, method).await,
            "settings" | "compact" | "fork" | "rewind" => {
                self.mutate(session, method, &input).await
            }
            "interrupt" => {
                let mut live = session.live.lock().unwrap();
                if live.generation != session.connection.generation {
                    return Err("grok_sdk_disconnected".into());
                }
                if !live.running {
                    return Err("grok_sdk_idle".into());
                }
                live.cancel_requested = true;
                if live.prompt_started {
                    session.connection.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session.sid}}))?;
                }
                Ok(json!({"accepted":true,"session_id":session.sid}))
            }
            "respond" => self.respond(&session, &input),
            _ => unreachable!(),
        }
    }

    fn command(&self, cwd: &str) -> Result<tokio::process::Command, String> {
        // Follow an updated executable symlink at each launch. Prefix argv is
        // server-owned (including login-shell wrappers), never browser input.
        let executable = self
            .profile
            .executable
            .canonicalize()
            .map_err(|e| format!("grok_sdk_executable: {}", e.kind()))?;
        let mut command = tokio::process::Command::new(executable);
        command
            .args(&self.profile.args)
            .args(["agent", "--no-leader", "stdio"]);
        for name in &self.profile.env_remove {
            command.env_remove(name);
        }
        command.envs(&self.profile.env);
        for name in DENIED_ENV {
            command.env_remove(name);
        }
        command.env("GROK_HOME", &self.home).current_dir(cwd);
        Ok(command)
    }

    async fn connection(
        &self,
        cwd: &str,
        live: Arc<Mutex<Live>>,
        identity: Option<&str>,
    ) -> Result<Arc<Connection>, String> {
        let connection = if let Some(root) = &self.state_dir {
            let checkpoint = self.checkpoint_path(identity.unwrap_or(&fresh_id()))?;
            Connection::spawn(
                self.command(cwd)?,
                live.clone(),
                self.shutdown.clone(),
                root,
                checkpoint,
            )
            .await?
        } else {
            Connection::spawn_legacy(
                self.command(cwd)?,
                live.clone(),
                self.shutdown.child_token(),
            )?
        };
        if connection.resumed {
            let initialized = live.lock().unwrap().initialized.clone();
            if initialized["protocolVersion"] != 1 {
                return Err("grok_sdk_initialize_unknown".into());
            }
            return Ok(connection);
        }
        let result = connection
            .request(
                "initialize",
                json!({"protocolVersion":1,
            "clientInfo":{"name":"sessiondock","version":env!("CARGO_PKG_VERSION")},
            "clientCapabilities":{},"_meta":{"clientIdentifier":"sessiondock"}}),
            )
            .await;
        match result {
            Ok(result) if result["protocolVersion"] == 1 => {
                *connection.initialized.lock().unwrap() = result;
                Ok(connection)
            }
            Ok(_) => {
                connection.stop.cancel();
                Err("grok_sdk_unsupported_protocol".into())
            }
            Err(error) => {
                connection.stop.cancel();
                Err(error)
            }
        }
    }

    async fn open(&self, target: &Value) -> Result<Arc<Session>, String> {
        self.open_with_spawn(target, true).await
    }
    async fn open_with_spawn(
        &self,
        target: &Value,
        allow_spawn: bool,
    ) -> Result<Arc<Session>, String> {
        let key = target_key(target)?;
        let sid = required(target, "session_id")?;
        let cwd = required(target, "cwd")?;
        let _gate = self.attach.lock().await;
        if let Some(session) = self.sessions.lock().unwrap().get(&key).cloned() {
            if session.sid != sid || session.cwd != cwd {
                return Err("grok_sdk_identity_mismatch".into());
            }
            if session.connection.alive.load(Ordering::Acquire) {
                session.live.lock().unwrap().idle_since = None;
                return Ok(session);
            }
            // Keep the same live projection and receipt cache on transport
            // recovery, so a retried request_id cannot replay accepted work.
        }
        let path = self.native_path(sid)?;
        if target["agent"].as_str().unwrap_or("").is_empty()
            && crate::sessions::uid_for("grok", &path) != required(target, "uid")?
        {
            return Err("grok_sdk_identity_mismatch".into());
        }
        let old = self.sessions.lock().unwrap().get(&key).cloned();
        if let Some(previous) = &old
            && !previous.connection.alive.load(Ordering::Acquire)
        {
            previous.connection.done.cancelled().await;
        }
        let live = old
            .as_ref()
            .map(|s| s.live.clone())
            .unwrap_or_else(|| Arc::new(Mutex::new(Live::new(sid.to_owned()))));
        if old.is_none() && self.state_dir.is_some() {
            let checkpoint = self.checkpoint_path(&key)?;
            match std::fs::read(&checkpoint) {
                Ok(bytes) => {
                    let mut restored: Live = serde_json::from_slice(&bytes)
                        .map_err(|e| format!("grok_sdk_checkpoint: {e}"))?;
                    if restored.sid != sid {
                        return Err("grok_sdk_identity_mismatch".into());
                    }
                    restored.checkpoint = Some(checkpoint);
                    *live.lock().unwrap() = restored;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("grok_sdk_checkpoint: {e}")),
            }
        }
        let hosted = live.lock().unwrap().host_key.is_some();
        if !hosted && !allow_spawn {
            return Err("grok_sdk_recovery_missing".into());
        }
        if !hosted {
            self.check_conflict(sid, &path, None).await?;
        }
        let connection = self.connection(cwd, live.clone(), Some(&key)).await?;
        let result = if connection.resumed {
            Value::Null
        } else {
            let loaded = connection
                .request(
                    "session/load",
                    json!({"sessionId":sid,"cwd":cwd,"mcpServers":[],
            "_meta":{"yoloMode":false,"autoMode":false}}),
                )
                .await;
            match loaded {
                Ok(result) => result,
                Err(error) => {
                    connection.stop.cancel();
                    return Err(error);
                }
            }
        };
        if result["_meta"]["currentWorkingDirectory"]
            .as_str()
            .is_some_and(|native| native != cwd)
        {
            connection.stop.cancel();
            return Err("grok_sdk_native_cwd_mismatch".into());
        }
        {
            let mut state = live.lock().unwrap();
            state.settings(&result);
            state.error = None;
            state.error_kind = None;
            state.publish();
        }
        let session = Arc::new(Session {
            uid: required(target, "uid")?.into(),
            sid: sid.into(),
            cwd: cwd.into(),
            path,
            live,
            connection,
            turns: Arc::new(AsyncMutex::new(())),
            steer_gate: AsyncMutex::new(()),
        });
        self.sessions.lock().unwrap().insert(key, session.clone());
        Ok(session)
    }

    async fn create(&self, target: &Value, input: &Value) -> Result<Value, String> {
        if target["source"]
            .as_str()
            .is_some_and(|source| source != "grok")
        {
            return Err("grok_sdk_source_mismatch".into());
        }
        let request_id = input["request_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(fresh_id);
        let cwd = required(target, "cwd")?;
        let mut attempts = self.creates.lock().await;
        if let Some(result) = attempts.get(&request_id) {
            return result.clone();
        }
        // The full outcome, including an uncertain native error, is cached:
        // retrying a lost response never creates a second native session.
        attempts.insert(
            request_id.clone(),
            Err("grok_sdk_create_unknown: operation dispatched before service restart".into()),
        );
        self.save_attempts("creates.json", &attempts)?;
        let result = self.create_once(cwd, input).await;
        attempts.insert(request_id, result.clone());
        self.save_attempts("creates.json", &attempts)?;
        result
    }

    async fn create_once(&self, cwd: &str, input: &Value) -> Result<Value, String> {
        let _gate = self.attach.lock().await;
        let live = Arc::new(Mutex::new(Live::new(String::new())));
        let connection = self.connection(cwd, live.clone(), None).await?;
        let result = connection
            .request(
                "session/new",
                json!({"cwd":cwd,"mcpServers":[],"_meta":{"yoloMode":false,"autoMode":false}}),
            )
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                connection.stop.cancel();
                return Err(error);
            }
        };
        let outcome = async {
            let sid = required(&result,"sessionId")?.to_owned();
            let actual_cwd = result["_meta"]["currentWorkingDirectory"].as_str().unwrap_or(cwd).to_owned();
            if actual_cwd != cwd { return Err("grok_sdk_native_cwd_mismatch".into()); }
            let path = self.native_path(&sid)?;
            let uid = crate::sessions::uid_for("grok", &path);
            { let mut state = live.lock().unwrap(); state.sid = sid.clone(); state.settings(&result); }
            self.configure(&connection, &sid, input, &live).await?;
            let session = Arc::new(Session { uid:uid.clone(), sid, cwd:actual_cwd, path, live:live.clone(), connection:connection.clone(), turns:Arc::new(AsyncMutex::new(())), steer_gate:AsyncMutex::new(()) });
            self.sessions.lock().unwrap().insert(uid.clone(), session.clone());
            {let mut state=session.live.lock().unwrap();if self.state_dir.is_some(){state.checkpoint=Some(self.checkpoint_path(&uid)?);}state.publish();}
            let mut receipt = self.receipt(&session);
            receipt["session"] = json!({"uid":session.uid,"source":"grok","session_id":session.sid,"sid":session.sid,
                "cwd":session.cwd,"title":"新建 Grok 会话","path":session.path,"sdk":true});
            Ok(receipt)
        }.await;
        if outcome.is_err() {
            connection.stop.cancel();
        }
        outcome
    }

    fn native_path(&self, sid: &str) -> Result<PathBuf, String> {
        // Resolve the directory created by the native runtime, including its
        // long-cwd slug/hash scheme. Never synthesize files or guess its hash.
        let groups = std::fs::read_dir(&self.root)
            .map_err(|e| format!("grok_sdk_native_root: {}", e.kind()))?;
        let mut found = None;
        for group in groups {
            let group = group.map_err(|e| format!("grok_sdk_native_root: {}", e.kind()))?;
            if !group.path().is_dir() {
                continue;
            }
            // SID is resolved by the facade/native response; compare directory
            // entries rather than using an untrusted path component in join.
            let Ok(entries) = std::fs::read_dir(group.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.file_name() == sid && entry.path().is_dir() {
                    if found.is_some() {
                        return Err("grok_sdk_ambiguous_native_session".into());
                    }
                    found = Some(entry.path());
                }
            }
        }
        found.ok_or_else(|| "grok_sdk_native_identity_unavailable: Grok did not publish a local session directory".into())
    }

    async fn configure(
        &self,
        connection: &Connection,
        sid: &str,
        input: &Value,
        live: &Mutex<Live>,
    ) -> Result<(), String> {
        if let Some(mode) = input.get("mode").filter(|v| !v.is_null()) {
            let mode = mode.as_str().ok_or("grok_sdk_invalid_mode")?;
            {
                let state = live.lock().unwrap();
                if !state.modes["availableModes"]
                    .as_array()
                    .is_some_and(|modes| modes.iter().any(|m| m["id"] == mode))
                {
                    return Err("grok_sdk_mode_unsupported".into());
                }
            }
            let result = connection
                .request("session/set_mode", json!({"sessionId":sid,"modeId":mode}))
                .await?;
            let mut state = live.lock().unwrap();
            state.settings(&result);
            state.modes["currentModeId"] = json!(mode);
            state.publish();
        }
        for (field, config_id) in [("model", "model"), ("effort", "reasoning_effort")] {
            if let Some(value) = input.get(field).filter(|v| !v.is_null()) {
                let value = value
                    .as_str()
                    .ok_or_else(|| format!("grok_sdk_invalid_{field}"))?;
                let result = connection
                    .request(
                        "session/set_config_option",
                        json!({"sessionId":sid,"configId":config_id,"value":value}),
                    )
                    .await?;
                let mut state = live.lock().unwrap();
                if state.generation != connection.generation {
                    return Err("grok_sdk_disconnected".into());
                }
                state.settings(&result);
                state.publish();
            }
        }
        Ok(())
    }

    async fn turn(
        self: &Arc<Self>,
        session: Arc<Session>,
        input: &Value,
        shell: bool,
    ) -> Result<Value, String> {
        let text = if shell {
            required(input, "command").or_else(|_| required(input, "text"))?
        } else {
            required(input, "text")?
        }
        .to_owned();
        let request_id = input["request_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(fresh_id);
        if let Some(receipt) = session.live.lock().unwrap().receipts.get(&request_id) {
            return Ok(receipt.clone());
        }
        let operation = session.turns.clone().lock_owned().await;
        // A surviving prompt has no old HTTP future to hold this mutex after
        // restart; its durable projection is the admission gate until ACP ends it.
        loop {
            {
                let state = session.live.lock().unwrap();
                if let Some(receipt) = state.receipts.get(&request_id) {
                    return Ok(receipt.clone());
                }
                if !state.running {
                    break;
                }
            }
            if self.shutdown.is_cancelled() || !session.connection.alive.load(Ordering::Acquire) {
                return Err("grok_sdk_disconnected".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        self.check_conflict(&session.sid, &session.path, Some(session.connection.pid))
            .await?;
        // Configuration is visible and interruptible before prompt acceptance.
        // Persist that boundary so recovery never invents an accepted prompt.
        {
            let mut state = session.live.lock().unwrap();
            state.running = true;
            state.prompt_started = false;
            state.cancel_requested = false;
            state.setup_pending = true;
            state.error = None;
            state.error_kind = None;
            state.publish();
            state
                .save_checkpoint()
                .map_err(|e| format!("grok_sdk_checkpoint: {e}"))?;
        }
        if let Err(error) = self
            .configure(&session.connection, &session.sid, input, &session.live)
            .await
        {
            if self.shutdown.is_cancelled() {
                return Err(error);
            }
            let mut state = session.live.lock().unwrap();
            let receipt = json!({"accepted":true,"sdkhost":session.connection.hosted(),"request_id":request_id,"session_id":session.sid});
            state.receipts.insert(request_id.clone(), receipt.clone());
            state.turn += 1;
            state.error = Some(error);
            state.error_kind = Some("turn");
            state.running = false;
            state.prompt_started = false;
            state.setup_pending = false;
            state.items.push(json!({"id":format!("grok:{}:user:{}",session.sid,request_id),"role":"user","content":text,"request_id":request_id}));
            state.publish();
            state
                .save_checkpoint()
                .map_err(|e| format!("grok_sdk_checkpoint: {e}"))?;
            return Ok(receipt);
        }
        let mut prompt = vec![json!({"type":"text","text":text})];
        if shell {
            prompt[0]["_meta"] = json!({"bash_command":text});
        }
        let (receipt, response) = {
            let mut state = session.live.lock().unwrap();
            if state.generation != session.connection.generation
                || !session.connection.alive.load(Ordering::Acquire)
            {
                return Err("grok_sdk_disconnected".into());
            }
            let receipt = json!({"accepted":true,"sdkhost":session.connection.hosted(),"request_id":request_id,"session_id":session.sid});
            state.receipts.insert(request_id.clone(), receipt.clone());
            state.setup_pending = false;
            if state.cancel_requested {
                state.running = false;
                state.prompt_started = false;
                state.publish();
                state
                    .save_checkpoint()
                    .map_err(|e| format!("grok_sdk_checkpoint: {e}"))?;
                return Ok(receipt);
            }
            state.running = true;
            state.cancel_requested = false;
            state.prompt_started = true;
            state.error = None;
            state.error_kind = None;
            state.turn += 1;
            let item = json!({"id":format!("grok:{}:user:{}",session.sid,request_id),"role":if shell{"tool"}else{"user"},"content":text,"tool_name":if shell{json!("shell")}else{Value::Null},"request_id":request_id});
            state.items.push(item.clone());
            let response = session.connection.begin_prompt(
                json!({"sessionId":session.sid,"prompt":prompt}),
                json!({"receipt":receipt,"item":item,"turn":state.turn}),
            )?;
            state.publish();
            state
                .save_checkpoint()
                .map_err(|e| format!("grok_sdk_checkpoint: {e}"))?;
            (receipt, response)
        };
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let _operation = operation;
            let result = response
                .await
                .unwrap_or_else(|_| Err("grok_sdk_disconnected".into()));
            if shutdown.is_cancelled() {
                return;
            }
            if !session.connection.hosted() {
                let mut state = session.live.lock().unwrap();
                if state.generation == session.connection.generation {
                    state.running = false;
                    state.prompt_started = false;
                    state.requests.clear();
                    if let Ok(result) = &result
                        && let Some(usage) = result["_meta"].get("usage")
                    {
                        state.usage["last_turn"] = live::scrub_usage(usage);
                    }
                    state.publish();
                }
            }
            if let Err(error) = result {
                let mut state = session.live.lock().unwrap();
                if state.generation == session.connection.generation {
                    state.running = false;
                    state.prompt_started = false;
                    if state.error_kind.is_none() {
                        state.error_kind = Some("transport");
                    }
                    state.error = Some(error);
                    state.publish();
                }
            }
        });
        Ok(receipt)
    }

    fn respond(&self, session: &Session, input: &Value) -> Result<Value, String> {
        let key = required(input, "key")?;
        let mut state = session.live.lock().unwrap();
        if state.generation != session.connection.generation {
            return Err("grok_sdk_request_gone".into());
        }
        let request = state.requests.get(key).ok_or("grok_sdk_request_gone")?;
        let result = &input["result"];
        let response = actions::response_result(request, result)?;
        session
            .connection
            .send(json!({"jsonrpc":"2.0","id":request["rpc_id"],"result":response}))?;
        state.requests.remove(key);
        state.publish();
        Ok(json!({"accepted":true}))
    }

    fn discard(&self, target: &Value) -> Result<Value, String> {
        let key = target_key(target)?;
        let mut sessions = self.sessions.lock().unwrap();
        let Some(session) = sessions.get(&key) else {
            return Ok(json!({"discarded":true,"native_preserved":true}));
        };
        let state = session.live.lock().unwrap();
        if state.running
            || !state.requests.is_empty()
            || state.has_pending_steers()
            || !state.native_tasks.is_empty()
            || state.incomplete_tasks
        {
            return Err("grok_sdk_busy".into());
        }
        session.connection.stop.cancel();
        drop(state);
        sessions.remove(&key);
        Ok(json!({"discarded":true,"native_preserved":true}))
    }

    fn receipt(&self, session: &Session) -> Value {
        let live = session.live.lock().unwrap();
        json!({"sdkhost":session.connection.hosted(),"uid":session.uid,"session_id":session.sid,"cwd":session.cwd,
            "capabilities":capabilities(&live),"raw":live.snapshot()["raw"]})
    }

    async fn check_conflict(&self, sid: &str, path: &Path, own: Option<u32>) -> Result<(), String> {
        let proc_root = self.proc_root.clone();
        let active = self.active.clone();
        let root = self.root.clone();
        let sid = sid.to_owned();
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            use crate::runtime::procscan::{self, ProcTree};
            let tree = Arc::new(ProcTree::open(proc_root));
            let scan = procscan::scan(tree.clone(), None, &procscan::SessionRoots::new([root.as_path()]));
            let is_ours = |pid: i64| -> bool {
                let pid = pid.unsigned_abs() as u32;
                own.is_some_and(|own| pid == own || tree.cli_ancestor(pid) == Some(own))
            };
            let sid_pids = scan.sids.get(&sid.to_ascii_lowercase());
            let prefix = format!("{}/",path.display());
            if sid_pids.is_some_and(|pids| pids.iter().any(|pid| !is_ours(*pid))) || scan.paths.iter().any(|(file,pids)| file.starts_with(&prefix) && pids.iter().any(|pid| !is_ours(*pid))) {
                return Err("grok_sdk_session_busy: 原生会话正在由独立 Grok 进程使用；本桥接不接管该进程".into());
            }
            if let Ok(bytes) = std::fs::read(active) && let Ok(data) = serde_json::from_slice::<Value>(&bytes) {
                let entries = data.as_array().or_else(||data["sessions"].as_array());
                for entry in entries.into_iter().flatten() {
                    let entry_sid = entry["id"].as_str().or_else(||entry["session_id"].as_str()).or_else(||entry.as_str());
                    if entry_sid != Some(sid.as_str()) { continue; }
                    let pid = entry["pid"].as_u64().and_then(|pid|u32::try_from(pid).ok());
                    if pid.is_some_and(|pid| is_ours(i64::from(pid))) { continue; }
                    if pid.is_none() || !tree.root().is_dir() || pid.is_some_and(|pid| {
                        std::fs::read_to_string(tree.root().join(pid.to_string()).join("stat")).ok()
                            .and_then(|s| s.rsplit_once(") ").map(|(_,tail)| !tail.starts_with('Z'))).unwrap_or(false)
                    }) {
                        return Err("grok_sdk_session_busy: Grok active_sessions 标记该会话仍由独立 TUI 使用".into());
                    }
                }
            }
            Ok(())
        }).await.map_err(|_|"grok_sdk_process_scan_failed".to_owned())?
    }
}

fn capabilities(live: &Live) -> Value {
    let supported = |name: &str| live.extensions.get(name).copied().unwrap_or(true);
    json!({"send":true,"interrupt":true,"respond":true,"approval":true,"question":true,
        "usage":supported("usage"),"billing":supported("billing"),
        "settings":!live.modes.is_null() || !live.config.is_null(),"mode":!live.modes.is_null(),
        "compact":supported("compact"),"fork":supported("fork"),"rewind_points":supported("rewind_points"),
        "rewind":supported("rewind"),"shell":true,"steer":supported("steer"),
        "models":!live.models.is_null() || !live.config.is_null(),"effort":live.config.as_array().is_some_and(|options|options.iter().any(|o|o["id"] == "reasoning_effort")),
        "shared_tui":false,"transport":"grok-acp-stdio","limitations":["独立 TUI 无法共享本桥接的 ACP 进程；外部占用会话拒绝发送", "扩展按原生返回判断支持；未认证安装版只验证过握手", "连接丢失后不重放已接受的 turn", "原生 shell 按 CLI 的直接执行权限运行"]})
}
fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("grok_sdk_missing_{key}"))
}
fn target_key(target: &Value) -> Result<String, String> {
    let uid = required(target, "uid")?;
    Ok(match target["agent"].as_str().filter(|s| !s.is_empty()) {
        Some(agent) => format!("{uid}:{agent}"),
        None => uid.to_owned(),
    })
}
fn fresh_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("OS randomness unavailable");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
