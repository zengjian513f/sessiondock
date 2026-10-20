//! Claude's native, persistent stream-json SDK transport. See docs/claude-sdk.md.
//! HTTP cancellation and broadcast receiver lifetimes never own a CLI process.

mod actions;
#[cfg(unix)]
mod hosted;
mod idle;
mod protocol;
mod transport;
mod usage;
use crate::sessions::native_json;

use std::{
    collections::{BTreeMap, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, RwLock, broadcast, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::lifecycle::{
    launcher::{self, CliProfile},
    model::Source,
};

type StaticErrorKind = Option<&'static str>;
type ControlReply = oneshot::Sender<Result<Value, String>>;

pub struct Bridge {
    root: PathBuf,
    state_dir: Option<PathBuf>,
    profile: CliProfile,
    proc_root: PathBuf,
    shutdown: CancellationToken,
    state: Mutex<BridgeState>,
}

#[derive(Default)]
struct BridgeState {
    sessions: BTreeMap<String, Arc<Session>>,
    creates: BTreeMap<String, (Value, Value)>,
    forks: BTreeMap<String, (Value, Option<Value>)>,
}

struct Session {
    uid: String,
    sid: String,
    cwd: PathBuf,
    path: PathBuf,
    observed_model: Option<String>,
    fork: Option<(PathBuf, String)>,
    fork_metadata: Value,
    runtime: RwLock<()>,
    operations: AsyncMutex<()>,
    live: Mutex<Live>,
    stop: CancellationToken,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Live {
    #[serde(skip)]
    checkpoint: Option<PathBuf>,
    #[serde(default)]
    stdout_offset: u64,
    #[serde(default)]
    host_key: Option<String>,
    seq: u64,
    items: Vec<Value>,
    requests: BTreeMap<String, Value>,
    #[serde(skip)]
    controls: BTreeMap<String, ControlReply>,
    #[serde(default)]
    control_requests: BTreeMap<String, Value>,
    #[serde(default)]
    initialization_requests: Vec<Value>,
    accepted: BTreeMap<String, Value>,
    actions: BTreeMap<String, Value>,
    turns: VecDeque<String>,
    #[serde(skip)]
    outbound: Option<transport::Outbound>,
    ready: bool,
    native_running: bool,
    state_observed: bool,
    #[serde(skip)]
    runtime_stop: Option<CancellationToken>,
    #[serde(skip)]
    idle_stop: Option<CancellationToken>,
    #[serde(skip)]
    runtime_done: Option<watch::Receiver<bool>>,
    #[serde(skip)]
    idle_since: Option<tokio::time::Instant>,
    pid: Option<u32>,
    model: Option<String>,
    effort: Option<String>,
    models: Value,
    settings: Value,
    usage: Value,
    message_usage: BTreeMap<String, Value>,
    model_usage: Value,
    context_usage: Value,
    rate_limits: Value,
    usage_errors: BTreeMap<String, String>,
    #[serde(skip)]
    refreshing_usage: bool,
    status: Value,
    retry: Value,
    compact: Value,
    background_tasks: Vec<Value>,
    error: Option<String>,
    #[serde(deserialize_with = "hosted_error_kind")]
    error_kind: StaticErrorKind,
    // Diagnostic tail is private and redacted, never returned in snapshots.
    stderr: VecDeque<String>,
    streaming_message: String,
    streaming_blocks: BTreeMap<u64, Value>,
    #[serde(skip, default = "live_events")]
    events: broadcast::Sender<Arc<str>>,
}

pub fn new(
    config: &crate::config::Config,
    shutdown: CancellationToken,
) -> io::Result<Option<Arc<Bridge>>> {
    let (Some(root), Some(path)) = (&config.roots.claude, &config.launcher_config) else {
        return Ok(None);
    };
    let launcher = launcher::read_config(path).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "claude_sdk: launcher configuration unavailable",
        )
    })?;
    let profiles: Vec<_> = launcher
        .profiles
        .into_iter()
        .filter(|p| p.source == Source::Claude)
        .collect();
    let mut profiles = profiles.into_iter();
    let Some(profile) = profiles.next() else {
        return Ok(None);
    };
    if profiles.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "claude_sdk: ambiguous Claude launcher profiles",
        ));
    }
    let bridge = Arc::new(Bridge {
        root: root.clone(),
        state_dir: config.state_dir.clone(),
        profile,
        proc_root: config.proc_root.clone(),
        shutdown,
        state: Mutex::default(),
    });
    bridge.start_reaper();
    Ok(Some(bridge))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn parse_json(text: &str) -> Result<Value, String> {
    serde_json::from_str(text)
        .or_else(|_| {
            native_json::scan_value(
                text.as_bytes(),
                native_json::Limits {
                    inline_string_bytes: text.len(),
                },
            )
        })
        .map_err(|_| "claude_sdk: invalid native JSON".into())
}

fn capabilities() -> Value {
    json!({"send":true,"interrupt":true,"respond":true,"approval":true,"question":true,
        "models":true,"create":true,"resume":cfg!(target_os="linux"),"shared_tui":false,
        "subagents":false,"effort":true,"settings":true,"permission_mode":true,"fast_mode":true,
        "usage":true,"context_usage":true,"compact":true,
        "fork":true,"fork_before":true,
        "mcp_elicitation":true,"user_dialog":true,
        "limitations":["Claude stream-json owns an independent CLI runtime; an active independent TUI cannot be attached or taken over.",
            "Claude subagent transcripts cannot be resumed as independent SDK sessions."]})
}

fn uuid() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| "claude_sdk: random identity unavailable".to_owned())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// Claude 2.1.302 sanitizePath: JS UTF-16 replacement, 200 characters then
/// Math.abs(simpleHash(original)).toString(36). This is native naming, no quota.
fn project_key(cwd: &Path) -> String {
    let text = cwd.to_string_lossy();
    let units: Vec<_> = text.encode_utf16().collect();
    let mut key: String = units
        .iter()
        .map(|c| {
            if *c <= 127 && (*c as u8).is_ascii_alphanumeric() {
                *c as u8 as char
            } else {
                '-'
            }
        })
        .collect();
    if units.len() > 200 {
        key.truncate(200);
        let hash = units
            .iter()
            .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(*c)));
        let mut number = i64::from(hash).unsigned_abs();
        let mut suffix = Vec::new();
        loop {
            suffix.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(number % 36) as usize] as char);
            number /= 36;
            if number == 0 {
                break;
            }
        }
        key.push('-');
        key.extend(suffix.into_iter().rev());
    }
    key
}

impl Session {
    fn new(uid: String, sid: String, cwd: PathBuf, path: PathBuf, input: &Value) -> Self {
        let (events, _) = broadcast::channel(128);
        Self {
            uid,
            sid,
            cwd,
            path,
            observed_model: None,
            fork: None,
            fork_metadata: Value::Null,
            runtime: RwLock::new(()),
            operations: AsyncMutex::new(()),
            stop: CancellationToken::new(),
            live: Mutex::new(Live {
                checkpoint: None,
                stdout_offset: 0,
                host_key: None,
                seq: 0,
                items: vec![],
                requests: BTreeMap::new(),
                controls: BTreeMap::new(),
                control_requests: BTreeMap::new(),
                initialization_requests: Vec::new(),
                accepted: BTreeMap::new(),
                actions: BTreeMap::new(),
                turns: VecDeque::new(),
                outbound: None,
                ready: false,
                native_running: false,
                state_observed: false,
                runtime_stop: None,
                idle_stop: None,
                runtime_done: None,
                idle_since: None,
                pid: None,
                model: input["model"].as_str().map(str::to_owned),
                effort: input["effort"].as_str().map(str::to_owned),
                models: Value::Null,
                settings: json!({}),
                usage: json!({}),
                message_usage: BTreeMap::new(),
                model_usage: Value::Null,
                context_usage: Value::Null,
                rate_limits: Value::Null,
                usage_errors: BTreeMap::new(),
                refreshing_usage: false,
                status: Value::Null,
                retry: Value::Null,
                compact: Value::Null,
                background_tasks: vec![],
                error: None,
                error_kind: None,
                stderr: VecDeque::new(),
                streaming_message: String::new(),
                streaming_blocks: BTreeMap::new(),
                events,
            }),
        }
    }

    fn snapshot(&self, live: &Live) -> Value {
        json!({"seq":live.seq,"type":"snapshot","raw":{
            "session_id":self.sid,"running":!live.turns.is_empty() || live.native_running,"items":live.items,
            "requests":live.requests.values().collect::<Vec<_>>(),"model":live.model.as_ref().or(self.observed_model.as_ref()),"effort":live.effort,
            "model_observed":live.model.is_none() && self.observed_model.is_some(),
            "settings":live.settings,"usage":live.usage,"model_usage":live.model_usage,
            "context_usage":live.context_usage,"rate_limits":live.rate_limits,
            "usage_errors":live.usage_errors,"status":live.status,"retry":live.retry,"compact":live.compact,
            "background_tasks":live.background_tasks,"error":live.error,"error_kind":live.error_kind,
            "capabilities":capabilities()}})
    }

    fn publish(&self, live: &mut Live) {
        live.seq += 1;
        if let Err(error) = live.save_checkpoint() {
            live.error = Some(format!("claude_sdk_checkpoint: {error}"));
            live.error_kind = Some("transport");
        }
        let _ = live.events.send(Arc::from(self.snapshot(live).to_string()));
    }

    fn receipt(&self) -> Value {
        let live = lock(&self.live);
        json!({"uid":self.uid,"session_id":self.sid,"cwd":self.cwd,
            "capabilities":capabilities(),"raw":self.snapshot(&live)["raw"]})
    }

    async fn control(&self, request: Value) -> Result<Value, String> {
        let id = uuid()?;
        let (tx, rx) = oneshot::channel();
        {
            let mut live = lock(&self.live);
            live.idle_since = None;
            let outbound = live
                .outbound
                .clone()
                .ok_or("claude_sdk: native transport disconnected")?;
            live.controls.insert(id.clone(), tx);
            live.control_requests.insert(id.clone(), request.clone());
            live.save_checkpoint()
                .map_err(|e| format!("claude_sdk_checkpoint: {e}"))?;
            if outbound
                .send(json!({"type":"control_request","request_id":id,"request":request}))
                .is_err()
            {
                live.controls.remove(&id);
                return Err("claude_sdk: native transport disconnected".into());
            }
        }
        match tokio::time::timeout(Duration::from_secs(60), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                Err("claude_sdk: native transport disconnected; request outcome unknown".into())
            }
            Err(_) => {
                lock(&self.live).controls.remove(&id);
                Err("claude_sdk: native control response timed out; request outcome unknown".into())
            }
        }
    }
}

impl Bridge {
    pub async fn stop(&self, uid: &str) -> Result<bool, String> {
        let session = lock(&self.state).sessions.remove(uid);
        let Some(session) = session else {
            return Ok(false);
        };
        let mut done = {
            let live = lock(&session.live);
            let _ = live
                .events
                .send(Arc::from(json!({"method":"bridge/stopped"}).to_string()));
            live.runtime_done.clone()
        };
        session.stop.cancel();
        if let Some(done) = done.as_mut() {
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                done.wait_for(|done| *done),
            )
            .await
            .map_err(|_| "Claude SDK 停止尚未确认")?
            .map_err(|_| "Claude SDK 停止确认通道已关闭")?;
        }
        Ok(true)
    }

    pub fn loaded_uids(&self) -> Vec<String> {
        lock(&self.state)
            .sessions
            .values()
            .filter(|session| {
                let live = lock(&session.live);
                live.ready && live.outbound.is_some()
            })
            .map(|session| session.uid.clone())
            .collect()
    }

    /// Snapshot and receiver are acquired under the same lock as every update.
    /// Main sessions use uid. Subagent uid:agent is reserved but unsupported.
    pub fn subscribe(&self, key: &str) -> Result<(Value, broadcast::Receiver<Arc<str>>), String> {
        let session = lock(&self.state)
            .sessions
            .get(key)
            .cloned()
            .ok_or("claude_sdk: session not open")?;
        // A new subscriber cancels the idle interval before the reaper can
        // observe it. Both operations use this same state lock.
        let mut live = lock(&session.live);
        if !live.ready || live.outbound.is_none() {
            return Err("claude_sdk: native runtime is not ready; reopen this session".into());
        }
        live.idle_since = None;
        Ok((session.snapshot(&live), live.events.subscribe()))
    }

    fn native_home(&self) -> Result<PathBuf, String> {
        let home = self
            .profile
            .env
            .get("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                (self.root.file_name().is_some_and(|name| name == "projects"))
                    .then(|| self.root.parent().map(Path::to_path_buf))
                    .flatten()
            })
            .ok_or(
                "claude_sdk: configure the native CLAUDE_CONFIG_DIR/projects root, not a mirror",
            )?;
        let expected = home.join("projects");
        let actual = expected
            .canonicalize()
            .or_else(|_| std::path::absolute(&expected))
            .map_err(|_| "claude_sdk: native config directory unavailable")?;
        let root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        if actual != root {
            return Err(
                "claude_sdk: launcher CLAUDE_CONFIG_DIR does not match the configured native root"
                    .into(),
            );
        }
        Ok(home)
    }

    fn select(&self, target: &Value) -> Result<Arc<Session>, String> {
        if target["agent"]
            .as_str()
            .is_some_and(|agent| !agent.is_empty())
        {
            return Err("claude_sdk_unsupported: native Claude subagent transcripts cannot be independently resumed (subagents:false)".into());
        }
        let uid = target["uid"]
            .as_str()
            .ok_or("claude_sdk: uid is required")?;
        let sid = target["session_id"]
            .as_str()
            .ok_or("claude_sdk: resolved session_id is required")?;
        let cwd = PathBuf::from(
            target["cwd"]
                .as_str()
                .ok_or("claude_sdk: resolved cwd is required")?,
        );
        let mut state = lock(&self.state);
        if let Some(session) = state.sessions.get(uid) {
            if session.sid != sid || session.cwd != cwd {
                return Err("claude_sdk: native session identity changed".into());
            }
            return Ok(session.clone());
        }
        self.native_home()?;
        if target["sdk_fork"].is_object() {
            let session = self.restore_fork(target)?;
            state.sessions.insert(uid.into(), session.clone());
            return Ok(session);
        }
        // Resolve the inventory UID, never reinterpret an arbitrary SID as a path.
        let mut path = None;
        for entry in std::fs::read_dir(&self.root)
            .map_err(|e| format!("claude_sdk: native root unavailable ({})", e.kind()))?
        {
            let entry = entry
                .map_err(|e| format!("claude_sdk: native project unavailable ({})", e.kind()))?;
            let candidate = entry.path().join(format!("{sid}.jsonl"));
            if crate::sessions::uid_for("claude", &candidate) == uid && candidate.is_file() {
                path = Some(candidate);
                break;
            }
        }
        let path = path.ok_or(
            "claude_sdk: inventory identity no longer exists under the configured native root",
        )?;
        let mut session = Session::new(uid.into(), sid.into(), cwd, path, &Value::Null);
        // Historical evidence is display-only until stream-json reports the
        // runtime's model. Never turn a resumed transcript into a CLI override.
        session.observed_model = target["observed_model"]
            .as_str()
            .filter(|model| !model.is_empty() && *model != "<synthetic>")
            .map(str::to_owned);
        let session = Arc::new(session);
        state.sessions.insert(uid.into(), session.clone());
        Ok(session)
    }

    fn create(&self, target: &Value, input: &Value) -> Result<Value, String> {
        self.native_home()?;
        let request = input["request_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("claude_sdk: create request_id is required")?;
        let identity = json!({"target":target,"model":input["model"],"effort":input["effort"]});
        let mut state = lock(&self.state);
        if let Some((previous, receipt)) = state.creates.get(request) {
            return if *previous == identity {
                Ok(receipt.clone())
            } else {
                Err("claude_sdk: create request_id already used with different input".into())
            };
        }
        let cwd = PathBuf::from(
            target["cwd"]
                .as_str()
                .ok_or("claude_sdk: create cwd is required")?,
        );
        if !cwd.is_absolute() {
            return Err("claude_sdk: cwd must be an absolute native working directory".into());
        }
        let cwd = cwd
            .canonicalize()
            .map_err(|e| format!("claude_sdk: cwd unavailable ({})", e.kind()))?;
        if !cwd.is_dir() {
            return Err("claude_sdk: cwd is not a directory".into());
        }
        let sid = uuid()?;
        let path = self
            .root
            .join(project_key(&cwd))
            .join(format!("{sid}.jsonl"));
        let uid = crate::sessions::uid_for("claude", &path);
        let session = Arc::new(Session::new(
            uid.clone(),
            sid.clone(),
            cwd.clone(),
            path,
            input,
        ));
        let mut receipt = session.receipt();
        receipt["session"] = json!({"uid":uid,"source":"claude","session_id":sid,"cwd":cwd,
            "title":"新会话","model":input["model"],"started":chrono::DateTime::from_timestamp(
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64,0)
                .unwrap_or_default().to_rfc3339(),
            "pending":true});
        state.sessions.insert(uid, session);
        state
            .creates
            .insert(request.into(), (identity, receipt.clone()));
        Ok(receipt)
    }

    pub async fn call(
        self: &Arc<Self>,
        method: &str,
        target: Value,
        input: Value,
    ) -> Result<Value, String> {
        if self.shutdown.is_cancelled() {
            return Err("claude_sdk: node shutting down".into());
        }
        if method == "create" {
            return self.create(&target, &input);
        }
        if method == "models" && target["uid"].as_str().is_none() {
            self.native_home()?;
            let cwd = target["cwd"]
                .as_str()
                .map(PathBuf::from)
                .or_else(|| self.profile.env.get("HOME").map(PathBuf::from))
                .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?);
            let sid = uuid()?;
            let session = Arc::new(Session::new(
                String::new(),
                sid.clone(),
                cwd.clone(),
                self.root
                    .join(project_key(&cwd))
                    .join(format!("{sid}.jsonl")),
                &Value::Null,
            ));
            let result = self.ensure(&session).await.map(
                |_| json!({"models":lock(&session.live).models,"capabilities":capabilities()}),
            );
            session.stop.cancel();
            return result;
        }
        let session = self.select(&target)?;
        let _runtime = session.runtime.read().await;
        lock(&session.live).idle_since = None;
        if method == "respond" {
            return self.respond(&session, &input);
        }
        if method == "interrupt" {
            let idle = {
                let live = lock(&session.live);
                live.turns.is_empty() && !live.native_running
            };
            if idle {
                return Err("claude_sdk_idle: no accepted native turn is running".into());
            }
            // Explicit interruption uses the native control protocol, never a key or signal.
            let result = session.control(json!({"subtype":"interrupt"})).await?;
            return Ok(json!({"interrupted":true,"session_id":session.sid,"raw":result}));
        }
        let _operation = session.operations.lock().await;
        match method {
            "fork" => self.fork_session(&session, &input).await,
            "discard" => {
                let active = {
                    let live = lock(&session.live);
                    !live.turns.is_empty() || live.native_running
                };
                if active {
                    return Err(
                        "claude_sdk: active work must be explicitly interrupted before discard"
                            .into(),
                    );
                }
                session.stop.cancel();
                let mut state = lock(&self.state);
                state.sessions.remove(&session.uid);
                state
                    .creates
                    .retain(|_, (_, receipt)| receipt["uid"] != session.uid);
                Ok(json!({"discarded":true,"native_history_preserved":true}))
            }
            "open" | "models" | "recover" | "reattach" => {
                if method == "reattach" {
                    #[cfg(unix)]
                    {
                        return self.reattach_hosted(&session).await;
                    }
                    #[cfg(not(unix))]
                    {
                        return Ok(json!({"attached":false}));
                    }
                }
                if method == "recover" {
                    #[cfg(unix)]
                    {
                        return self.recover_hosted(&session, &input).await;
                    }
                    #[cfg(not(unix))]
                    {
                        return Err("claude_sdk_recovery: host unavailable".into());
                    }
                }
                self.ensure(&session).await?;
                session.schedule_usage();
                if method == "models" {
                    return Ok(
                        json!({"models":lock(&session.live).models,"capabilities":capabilities()}),
                    );
                }
                Ok(session.receipt())
            }
            "usage" => {
                self.ensure(&session).await?;
                session.refresh_usage().await;
                Ok(session.receipt())
            }
            "settings" => {
                let request_id = input["request_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or("claude_sdk: settings request_id is required")?;
                {
                    let live = lock(&session.live);
                    if let Some(previous) = live.actions.get(request_id) {
                        if previous["input"] != input {
                            return Err(
                                "claude_sdk: settings request_id already used with different input"
                                    .into(),
                            );
                        }
                        return previous.get("receipt").cloned().ok_or_else(||
                            "claude_sdk: settings already attempted; native outcome may be unknown, inspect current settings before retrying".into());
                    }
                }
                self.ensure(&session).await?;
                // Reserve before the first native write. A failed or uncertain
                // control must not be replayed by an HTTP retry.
                lock(&session.live)
                    .actions
                    .insert(request_id.into(), json!({"input":input}));
                if let Some(mode) = input["permission_mode"].as_str() {
                    session
                        .control(json!({"subtype":"set_permission_mode","mode":mode}))
                        .await?;
                    let mut live = lock(&session.live);
                    live.settings["permission_mode"] = json!(mode);
                    session.publish(&mut live);
                }
                if let Some(model) = input["model"].as_str() {
                    session
                        .control(json!({"subtype":"set_model","model":model}))
                        .await?;
                    let mut live = lock(&session.live);
                    live.model = Some(model.into());
                    session.publish(&mut live);
                }
                let mut flags = json!({});
                if let Some(fast) = input["fast_mode"].as_bool() {
                    flags["fastMode"] = json!(fast);
                }
                if let Some(effort) = input["effort"].as_str() {
                    flags["effortLevel"] = json!(effort);
                }
                if !flags.as_object().unwrap().is_empty() {
                    session
                        .control(json!({"subtype":"apply_flag_settings","settings":flags}))
                        .await?;
                    let mut live = lock(&session.live);
                    if let Some(fast) = input["fast_mode"].as_bool() {
                        live.settings["fast_mode"] = json!(fast);
                    }
                    if let Some(effort) = input["effort"].as_str() {
                        live.effort = Some(effort.into());
                    }
                    session.publish(&mut live);
                }
                let receipt = json!({"configured":true,"request_id":request_id,"session_id":session.sid,
                    "settings":lock(&session.live).settings});
                lock(&session.live).actions.get_mut(request_id).unwrap()["receipt"] =
                    receipt.clone();
                Ok(receipt)
            }
            "turn" | "compact" => {
                if method == "compact" && input["request_id"].as_str().is_none_or(str::is_empty) {
                    return Err("claude_sdk: compact request_id is required".into());
                }
                let request_id = input["request_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or(uuid()?);
                let dedup_key = if method == "compact" {
                    format!("compact:{request_id}")
                } else {
                    request_id.clone()
                };
                if let Some(receipt) = lock(&session.live).accepted.get(&dedup_key).cloned() {
                    return Ok(receipt);
                }
                self.ensure(&session).await?;
                self.check_conflict(&session).await?;
                let text = if method == "compact" {
                    "/compact"
                } else {
                    input["text"]
                        .as_str()
                        .ok_or("claude_sdk: text must be a string")?
                };
                let content = json!(text);
                let (model, effort) = {
                    let live = lock(&session.live);
                    (live.model.clone(), live.effort.clone())
                };
                if let Some(next) = input["model"]
                    .as_str()
                    .filter(|next| model.as_deref() != Some(*next))
                {
                    session
                        .control(json!({"subtype":"set_model","model":next}))
                        .await?;
                    lock(&session.live).model = Some(next.into());
                }
                if let Some(next) = input["effort"]
                    .as_str()
                    .filter(|next| effort.as_deref() != Some(*next))
                {
                    session.control(json!({"subtype":"apply_flag_settings","settings":{"effortLevel":next}})).await?;
                    lock(&session.live).effort = Some(next.into());
                }
                let message_id = uuid()?;
                let mut live = lock(&session.live);
                let sender = live
                    .outbound
                    .clone()
                    .ok_or("claude_sdk: native transport disconnected")?;
                let frame = json!({"type":"user","session_id":session.sid,"uuid":message_id,
                    "parent_tool_use_id":null,"message":{"role":"user","content":content}});
                let receipt = json!({"accepted":true,"request_id":request_id,"session_id":session.sid,"message_id":message_id,"sdkhost":live.host_key.is_some()});
                live.accepted.insert(dedup_key, receipt.clone());
                live.turns.push_back(request_id.clone());
                live.items.push(json!({"id":message_id,"native_id":message_id,"role":"user","content":text,"text":text,"request_id":request_id}));
                if method == "compact" {
                    live.compact = json!({"state":"requested","request_id":request_id});
                }
                live.error = None;
                live.error_kind = None;
                session.publish(&mut live);
                live.save_checkpoint()
                    .map_err(|e| format!("claude_sdk_checkpoint: {e}"))?;
                sender
                    .send(frame)
                    .map_err(|_| "claude_sdk: native transport disconnected")?;
                Ok(receipt)
            }
            _ => Err(format!(
                "claude_sdk_unsupported: {method} (capabilities: {})",
                capabilities()
            )),
        }
    }

    fn respond(&self, session: &Session, input: &Value) -> Result<Value, String> {
        let key = input["key"]
            .as_str()
            .ok_or("claude_sdk: response key is required")?;
        let mut live = lock(&session.live);
        let card = live
            .requests
            .get(key)
            .ok_or("claude_sdk_request_gone: request no longer pending")?;
        let result = protocol::permission_result(card, &input["result"])?;
        let sender = live
            .outbound
            .as_ref()
            .ok_or("claude_sdk: native transport disconnected")?;
        sender.send(json!({"type":"control_response","response":{"subtype":"success","request_id":key,"response":result}}))
            .map_err(|_| "claude_sdk: native transport disconnected")?;
        let card = card.clone();
        protocol::retire_dialog(&mut live, &card);
        live.requests.remove(key);
        session.publish(&mut live);
        Ok(json!({"responded":true,"key":key}))
    }
}

fn live_events() -> broadcast::Sender<Arc<str>> {
    broadcast::channel(128).0
}
fn hosted_error_kind<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<&'static str>, D::Error> {
    let value = <Option<String> as serde::Deserialize>::deserialize(d)?;
    Ok(match value.as_deref() {
        Some("transport") => Some("transport"),
        Some("turn") => Some("turn"),
        Some("interrupted") => Some("interrupted"),
        _ => None,
    })
}
impl Live {
    fn save_checkpoint(&self) -> io::Result<()> {
        let Some(path) = &self.checkpoint else {
            return Ok(());
        };
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        let temporary = path.with_extension("tmp");
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(temporary, path)
    }
}
