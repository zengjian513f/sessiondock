//! Droid's native, persistent JSON-RPC SDK transport. See docs/droid-sdk.md.
//! HTTP cancellation and broadcast receiver lifetimes never own a CLI process.

mod actions;
mod activity;
#[cfg(unix)]
mod hosted;
mod protocol;
mod transport;
mod writers;
use crate::sdk_common::save_atomic;
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
    writer_admission: AsyncMutex<()>,
}

#[derive(Default)]
struct BridgeState {
    sessions: BTreeMap<String, Arc<Session>>,
    creates: BTreeMap<String, (Value, Value)>,
    aliases: BTreeMap<String, String>,
    owners: BTreeMap<String, String>,
}

struct Session {
    uid: String,
    sid: String,
    cwd: PathBuf,
    path: PathBuf,
    observed_model: Option<String>,
    runtime: RwLock<()>,
    operations: AsyncMutex<()>,
    live: Mutex<Live>,
    stop: CancellationToken,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Live {
    #[serde(skip)]
    checkpoint: Option<PathBuf>,
    #[serde(skip)]
    last_checkpoint: Option<std::time::Instant>,
    #[serde(skip)]
    last_checkpoint_error: Option<String>,
    #[serde(default)]
    stdout_offset: u64,
    #[serde(default)]
    host_key: Option<String>,
    seq: u64,
    items: Vec<Value>,
    #[serde(skip)]
    message_time: crate::sdk_message_time::MessageTime,
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
    runtime_done: Option<watch::Receiver<bool>>,
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
    status: Value,
    retry: Value,
    compact: Value,
    background_tasks: Vec<Value>,
    #[serde(default)]
    activity: BTreeMap<String, Value>,
    error: Option<String>,
    #[serde(default)]
    runtime_error: Option<String>,
    #[serde(default)]
    last_turn: Value,
    #[serde(default)]
    interrupt_requested: bool,
    /// Kept with the transport checkpoint; native load history belongs to the read model.
    #[serde(default)]
    replaying: bool,
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
    let (Some(root), Some(path)) = (&config.roots.droid, &config.launcher_config) else {
        return Ok(None);
    };
    let launcher = launcher::read_config(path).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "droid_sdk: launcher configuration unavailable",
        )
    })?;
    let profiles: Vec<_> = launcher
        .profiles
        .into_iter()
        .filter(|p| p.source == Source::Droid)
        .collect();
    let mut profiles = profiles.into_iter();
    let Some(profile) = profiles.next() else {
        return Ok(None);
    };
    if profiles.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "droid_sdk: ambiguous Droid launcher profiles",
        ));
    }
    let bridge = Arc::new(Bridge {
        root: root.clone(),
        state_dir: config.state_dir.clone(),
        profile,
        proc_root: config.proc_root.clone(),
        shutdown,
        state: Mutex::default(),
        writer_admission: AsyncMutex::new(()),
    });
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
        .map_err(|_| "droid_sdk: invalid native JSON".into())
}

fn capabilities() -> Value {
    json!({"send":true,"interrupt":true,"respond":true,"approval":true,"question":true,
        "models":true,"create":true,"resume":cfg!(any(target_os="linux",target_os="macos",windows)),"shared_tui":false,
        "subagents":true,"effort":true,"settings":true,"permission_mode":true,"mode":true,"fast_mode":false,
        "usage":true,"context_usage":true,"compact":true,"compact_creates_session":true,"rename":true,
        "fork":true,"fork_before":true,"rewind":true,"rewind_creates_session":true,"steer":true,"mcp_elicitation":false,"user_dialog":false})
}
fn normalize_models(models: &mut Value) {
    let Some(rows) = models.as_array_mut() else {
        return;
    };
    for model in rows {
        *model = json!({"id":model["id"],"name":model.get("displayName").unwrap_or(&model["id"]),
            "efforts":model["supportedReasoningEfforts"],"default_effort":model["defaultReasoningEffort"]});
    }
}

fn uuid() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "droid_sdk: random identity unavailable".to_owned())?;
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

/// Native Factory project key (SDK 0.10.0 session-discovery).
fn project_key(cwd: &Path) -> String {
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let value = canonical.to_string_lossy();
    #[cfg(windows)]
    {
        // Rust canonicalization returns extended-length paths; Node realpath
        // (the official SDK's source) returns ordinary drive/UNC paths.
        let value = if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else {
            value.strip_prefix(r"\\?\").unwrap_or(&value).to_owned()
        };
        let value = value.trim_end_matches(['/', '\\']);
        let drive = regex::Regex::new(r"^([A-Za-z]):").expect("static drive regex");
        let value = drive.replace(value, "$1");
        let separators = regex::Regex::new(r"[\\/]+").expect("static path regex");
        format!("-{}", separators.replace_all(&value, "-"))
    }
    #[cfg(not(windows))]
    {
        let value = value.trim_end_matches(['/', '\\']).trim_start_matches('/');
        let separators = regex::Regex::new(r"/+").expect("static path regex");
        format!("-{}", separators.replace_all(value, "-"))
    }
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
            runtime: RwLock::new(()),
            operations: AsyncMutex::new(()),
            stop: CancellationToken::new(),
            live: Mutex::new(Live {
                checkpoint: None,
                last_checkpoint: None,
                last_checkpoint_error: None,
                stdout_offset: 0,
                host_key: None,
                seq: 0,
                items: vec![],
                message_time: Default::default(),
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
                runtime_done: None,
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
                status: Value::Null,
                retry: Value::Null,
                compact: Value::Null,
                background_tasks: vec![],
                activity: BTreeMap::new(),
                error: None,
                runtime_error: None,
                last_turn: Value::Null,
                interrupt_requested: false,
                replaying: false,
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
            "steers":live.accepted.values().filter(|r|r["operation"]=="steer" && r["consumed"]!=true).collect::<Vec<_>>(),
            "activity":live.activity,"background_tasks":live.background_tasks,"error":live.error,"error_kind":live.error_kind,
            "last_turn":live.last_turn,"runtime_error":live.runtime_error,
            "capabilities":capabilities()}})
    }

    fn publish(&self, live: &mut Live) {
        self.publish_with(live, true);
    }

    fn publish_throttled(&self, live: &mut Live) {
        self.publish_with(live, false);
    }

    fn publish_with(&self, live: &mut Live, force_checkpoint: bool) {
        live.seq += 1;
        let res = if force_checkpoint {
            live.save_checkpoint()
        } else {
            live.save_checkpoint_throttled()
        };
        if let Err(error) = res {
            let msg = format!("droid_sdk_checkpoint: {error}");
            live.last_checkpoint_error = Some(msg.clone());
            live.error = Some(msg);
            live.error_kind = Some("transport");
        } else {
            live.last_checkpoint_error = None;
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
            let outbound = live
                .outbound
                .clone()
                .ok_or("droid_sdk: native transport disconnected")?;
            live.controls.insert(id.clone(), tx);
            live.control_requests.insert(id.clone(), request.clone());
            live.save_checkpoint()
                .map_err(|e| format!("droid_sdk_checkpoint: {e}"))?;
            if outbound.send(protocol::request(&id, &request)).is_err() {
                live.controls.remove(&id);
                return Err("droid_sdk: native transport disconnected".into());
            }
        }
        let timeout = if request["method"] == "droid.compact_session" {
            240
        } else {
            60
        };
        match tokio::time::timeout(Duration::from_secs(timeout), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                Err("droid_sdk: native transport disconnected; request outcome unknown".into())
            }
            Err(_) => {
                lock(&self.live).controls.remove(&id);
                Err("droid_sdk: native control response timed out; request outcome unknown".into())
            }
        }
    }
}

impl Bridge {
    pub(crate) async fn resource_process_owners(
        &self,
        identities: &[(String, String)],
    ) -> Vec<(String, process_links::Process)> {
        let Some(state) = &self.state_dir else {
            return Vec::new();
        };
        let mut owners = Vec::new();
        for (uid, sid) in identities {
            if let Some(key) =
                crate::native_sdk::resources::checkpoint_host(state, "droid", uid, sid)
                && let Some(process) = crate::native_sdk::resources::hosted_process(
                    &self.proc_root,
                    &state.join("sdkhost"),
                    &key,
                )
                .await
            {
                owners.push((uid.clone(), process));
            }
        }
        owners
    }

    pub async fn stop(&self, uid: &str) -> Result<bool, String> {
        let session = {
            let mut state = lock(&self.state);
            let native_uid = state
                .aliases
                .get(uid)
                .cloned()
                .unwrap_or_else(|| uid.to_owned());
            state.sessions.remove(&native_uid)
        };
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
            .map_err(|_| "Droid SDK 停止尚未确认")?
            .map_err(|_| "Droid SDK 停止确认通道已关闭")?;
        }
        Ok(true)
    }

    pub fn loaded_uids(&self) -> Vec<String> {
        let state = lock(&self.state);
        state
            .sessions
            .values()
            .filter(|session| {
                let live = lock(&session.live);
                live.ready && live.outbound.is_some()
            })
            .flat_map(|session| {
                let aliases: Vec<String> = state
                    .aliases
                    .iter()
                    .filter(|(_, uid)| *uid == &session.uid)
                    .map(|(key, _)| key.clone())
                    .collect();
                if aliases.is_empty() {
                    vec![session.uid.clone()]
                } else {
                    aliases
                }
            })
            .collect()
    }

    /// Snapshot and receiver are acquired under the same lock as every update.
    /// Public owner:agent scope aliases the proven child native UID.
    pub fn subscribe(&self, key: &str) -> Result<(Value, broadcast::Receiver<Arc<str>>), String> {
        let session = {
            let state = lock(&self.state);
            let native_key = state.aliases.get(key).map(String::as_str).unwrap_or(key);
            state
                .sessions
                .get(native_key)
                .cloned()
                .ok_or("droid_sdk: session not open")?
        };
        // Snapshot and subscription share the live-state lock.
        let live = lock(&session.live);
        if !live.ready || live.outbound.is_none() {
            return Err("droid_sdk: native runtime is not ready; reopen this session".into());
        }
        Ok((session.snapshot(&live), live.events.subscribe()))
    }

    pub(crate) async fn child_handoff(
        &self,
        target: &Value,
        result: &Value,
    ) -> Result<Option<Value>, String> {
        let owner = target["uid"].as_str().ok_or("droid_sdk: owner missing")?;
        let native_uid = result["uid"]
            .as_str()
            .ok_or("droid_sdk: child identity missing")?;
        let sid = result["session_id"]
            .as_str()
            .ok_or("droid_sdk: child SID missing")?;
        let source = target["native_uid"]
            .as_str()
            .ok_or("droid_sdk: source child missing")?;
        let child = lock(&self.state)
            .sessions
            .get(native_uid)
            .cloned()
            .ok_or("droid_sdk: new child missing")?;
        use std::io::BufRead;
        let file = std::fs::File::open(&child.path)
            .map_err(|e| format!("droid_sdk: child provenance unavailable ({})", e.kind()))?;
        let first = std::io::BufReader::new(file)
            .lines()
            .next()
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        let start: Value =
            serde_json::from_str(&first).map_err(|_| "droid_sdk: child provenance invalid")?;
        if !start["callingSessionId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
        {
            return Ok(None);
        }
        let session = lock(&self.state).sessions.get(source).cloned();
        if let Some(session) = session {
            self.retire_action_runtime(&session).await?;
        }
        let mut binding = target.clone();
        binding["agent"] = json!(sid);
        binding["session_id"] = json!(sid);
        binding["native_uid"] = json!(native_uid);
        let mut state = lock(&self.state);
        state
            .aliases
            .insert(format!("{owner}:{sid}"), native_uid.to_owned());
        state.owners.insert(native_uid.to_owned(), owner.to_owned());
        Ok(Some(binding))
    }

    fn native_home(&self) -> Result<PathBuf, String> {
        let home = self
            .profile
            .env
            .get("FACTORY_HOME_OVERRIDE")
            .or_else(|| self.profile.env.get("HOME"))
            .or_else(|| self.profile.env.get("USERPROFILE"))
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("FACTORY_HOME_OVERRIDE")
                    .or_else(|| std::env::var_os("HOME"))
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(PathBuf::from)
            })
            .ok_or("droid_sdk: native HOME unavailable")?;
        let expected = home.join(".factory/sessions");
        let actual = expected.canonicalize().unwrap_or(expected);
        let root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        if actual != root {
            return Err(
                "droid_sdk: launcher native HOME does not match configured sessions root".into(),
            );
        }
        Ok(home)
    }

    fn select(&self, target: &Value) -> Result<Arc<Session>, String> {
        let owner = target["uid"].as_str().ok_or("droid_sdk: uid is required")?;
        let agent = target["agent"].as_str().filter(|s| !s.is_empty());
        let uid = if agent.is_some() {
            target["native_uid"]
                .as_str()
                .ok_or("droid_sdk: resolved child identity is required")?
        } else {
            owner
        };
        let scope_key = agent.map_or_else(|| owner.to_owned(), |agent| format!("{owner}:{agent}"));
        let sid = target["session_id"]
            .as_str()
            .ok_or("droid_sdk: resolved session_id is required")?;
        let cwd = PathBuf::from(
            target["cwd"]
                .as_str()
                .ok_or("droid_sdk: resolved cwd is required")?,
        );
        let mut state = lock(&self.state);
        state.aliases.insert(scope_key, uid.to_owned());
        if agent.is_some() {
            state.owners.insert(uid.to_owned(), owner.to_owned());
        }
        if let Some(session) = state.sessions.get(uid) {
            if session.sid != sid || session.cwd != cwd {
                return Err("droid_sdk: native session identity changed".into());
            }
            return Ok(session.clone());
        }
        self.native_home()?;
        // Resolve the inventory UID, never reinterpret an arbitrary SID as a path.
        let legacy = self.root.join(format!("{sid}.jsonl"));
        let mut path = (crate::sessions::uid_for("droid", &legacy) == uid && legacy.is_file())
            .then_some(legacy);
        for entry in std::fs::read_dir(&self.root)
            .map_err(|e| format!("droid_sdk: native root unavailable ({})", e.kind()))?
        {
            let entry = entry
                .map_err(|e| format!("droid_sdk: native project unavailable ({})", e.kind()))?;
            let candidate = entry.path().join(format!("{sid}.jsonl"));
            if crate::sessions::uid_for("droid", &candidate) == uid && candidate.is_file() {
                path = Some(candidate);
                break;
            }
        }
        let path = path.ok_or(
            "droid_sdk: inventory identity no longer exists under the configured native root",
        )?;
        let mut session = Session::new(uid.into(), sid.into(), cwd, path, &Value::Null);
        // Historical evidence is display-only until JSON-RPC reports the
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
            .ok_or("droid_sdk: create request_id is required")?;
        let identity = json!({"target":target,"model":input["model"],"effort":input["effort"]});
        let mut state = lock(&self.state);
        if let Some((previous, receipt)) = state.creates.get(request) {
            return if *previous == identity {
                Ok(receipt.clone())
            } else {
                Err("droid_sdk: create request_id already used with different input".into())
            };
        }
        let cwd = PathBuf::from(
            target["cwd"]
                .as_str()
                .ok_or("droid_sdk: create cwd is required")?,
        );
        if !cwd.is_absolute() {
            return Err("droid_sdk: cwd must be an absolute native working directory".into());
        }
        let cwd = cwd
            .canonicalize()
            .map_err(|e| format!("droid_sdk: cwd unavailable ({})", e.kind()))?;
        if !cwd.is_dir() {
            return Err("droid_sdk: cwd is not a directory".into());
        }
        let sid = uuid()?;
        let path = self
            .root
            .join(project_key(&cwd))
            .join(format!("{sid}.jsonl"));
        let uid = crate::sessions::uid_for("droid", &path);
        let session = Arc::new(Session::new(
            uid.clone(),
            sid.clone(),
            cwd.clone(),
            path,
            input,
        ));
        let mut receipt = session.receipt();
        receipt["session"] = json!({"uid":uid,"source":"droid","session_id":sid,"cwd":cwd,
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
            return Err("droid_sdk: node shutting down".into());
        }
        if method == "create" {
            let receipt = self.create(&target, &input)?;
            let session = self.select(&receipt)?;
            let _operation = session.operations.lock().await;
            self.ensure(&session).await?;
            return Ok(receipt);
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
        if method == "respond" {
            return self.respond(&session, &input);
        }
        if method == "interrupt" {
            let idle = {
                let live = lock(&session.live);
                live.turns.is_empty() && !live.native_running
            };
            if idle {
                return Err("droid_sdk_idle: no accepted native turn is running".into());
            }
            // Explicit interruption uses the native control protocol, never a key or signal.
            let result = session
                .control(json!({"method":"droid.interrupt_session","params":{}}))
                .await?;
            return Ok(json!({"interrupted":true,"session_id":session.sid,"raw":result}));
        }
        let _operation = session.operations.lock().await;
        match method {
            "rename" | "fork" | "compact" | "rewind" => self.action(&session, method, &input).await,
            "rewind_points" => {
                self.ensure(&session).await?;
                self.check_conflict(&session).await?;
                let before = input["before"]
                    .as_str()
                    .ok_or("droid_sdk: before must be a native message identity")?;
                let result = session.control(json!({"method":"droid.get_rewind_info", "params":{"sessionId":session.sid,"messageId":before}})).await?;
                Ok(json!({"uid":session.uid,"rewind_info":result}))
            }
            "usage" => {
                // Read only the owned transport; a quota read must never take a TUI.
                if lock(&session.live).outbound.is_some() {
                    let result = session
                        .control(json!({"method":"droid.get_context_stats","params":{}}))
                        .await;
                    let mut live = lock(&session.live);
                    match result {
                        Ok(value)
                            if value["used"].is_number()
                                && value["limit"].as_f64().is_some_and(|n| n > 0.) =>
                        {
                            live.context_usage = json!({"used_tokens":value["used"],"max_tokens":value["limit"],
                                "remaining_tokens":value["remaining"],"percent":value["used"].as_f64().unwrap()*100./value["limit"].as_f64().unwrap(),
                                "basis":if value["accuracy"] == "estimated" {"原生估计"} else {"原生统计"},
                                "accuracy":value["accuracy"],"updated_at":value["updatedAt"]});
                            live.usage_errors.remove("context");
                        }
                        _ => {
                            live.usage_errors
                                .insert("context".into(), "上下文查询失败，保留上次结果".into());
                        }
                    }
                    session.publish(&mut live);
                }
                let live = lock(&session.live);
                Ok(
                    json!({"usage":live.usage,"context_usage":live.context_usage,"usage_errors":live.usage_errors}),
                )
            }
            "discard" => {
                let active = {
                    let live = lock(&session.live);
                    !live.turns.is_empty() || live.native_running
                };
                if active {
                    return Err(
                        "droid_sdk: active work must be explicitly interrupted before discard"
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
                        return Err("droid_sdk_recovery: host unavailable".into());
                    }
                }
                self.ensure(&session).await?;
                if method == "models" {
                    return Ok(
                        json!({"models":lock(&session.live).models,"capabilities":capabilities()}),
                    );
                }
                Ok(session.receipt())
            }
            "settings" => {
                self.ensure(&session).await?;
                let request_id = input["request_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or("droid_sdk: settings request_id is required")?;
                {
                    let live = lock(&session.live);
                    if let Some(previous) = live.actions.get(request_id) {
                        if previous["input"] != input {
                            return Err(
                                "droid_sdk: settings request_id already used with different input"
                                    .into(),
                            );
                        }
                        return previous.get("receipt").cloned().ok_or_else(||
                            "droid_sdk: settings already attempted; native outcome may be unknown, inspect current settings before retrying".into());
                    }
                }
                self.ensure(&session).await?;
                // Reserve before the first native write. A failed or uncertain
                // control must not be replayed by an HTTP retry.
                lock(&session.live)
                    .actions
                    .insert(request_id.into(), json!({"input":input}));
                let mut params = json!({});
                if let Some(model) = input["model"].as_str() {
                    params["modelId"] = json!(model);
                }
                if let Some(effort) = input["effort"].as_str() {
                    params["reasoningEffort"] = json!(effort);
                }
                if let Some(mode) = input.get("mode") {
                    params["interactionMode"] = mode.clone();
                }
                if let Some(level) = input.get("autonomy_level") {
                    params["autonomyLevel"] = level.clone();
                }
                lock(&session.live)
                    .save_checkpoint()
                    .map_err(|e| format!("droid_sdk_checkpoint: {e}"))?;
                session
                    .control(json!({"method":"droid.update_session_settings","params":params}))
                    .await?;
                let mut live = lock(&session.live);
                session.publish(&mut live);
                drop(live);
                let receipt = json!({"configured":true,"request_id":request_id,"session_id":session.sid,
                    "settings":lock(&session.live).settings});
                let mut live = lock(&session.live);
                live.actions.get_mut(request_id).unwrap()["receipt"] = receipt.clone();
                live.save_checkpoint()
                    .map_err(|e| format!("droid_sdk_checkpoint: {e}"))?;
                Ok(receipt)
            }
            "turn" | "steer" => {
                self.ensure(&session).await?;
                let request_id = input["request_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or(uuid()?);
                let dedup_key = request_id.clone();
                if let Some(receipt) = lock(&session.live).accepted.get(&dedup_key).cloned() {
                    if receipt
                        .get("text")
                        .is_some_and(|text| *text != input["text"])
                        || receipt["operation"].as_str().unwrap_or("turn") != method
                    {
                        return Err(
                            "droid_sdk: input request_id already used with different input".into(),
                        );
                    }
                    if let Some(error) = receipt["error"].as_str() {
                        return Err(error.into());
                    }
                    return if receipt["native_accepted"] == true {
                        Ok(receipt)
                    } else {
                        Err("droid_sdk_write_uncertain: previous native admission is unknown; input not replayed".into())
                    };
                }
                self.check_conflict(&session).await?;
                let text = input["text"]
                    .as_str()
                    .ok_or("droid_sdk: text must be a string")?;
                let message_id = uuid()?;
                let (response, wait_response) = oneshot::channel();
                {
                    let mut live = lock(&session.live);
                    if method == "steer" && live.turns.is_empty() && !live.native_running {
                        return Err(
                            "droid_sdk_idle: no active turn to steer; input not sent".into()
                        );
                    }
                    let sender = live
                        .outbound
                        .clone()
                        .ok_or("droid_sdk: native transport disconnected")?;
                    let frame = protocol::request(
                        &request_id,
                        &json!({"method":"droid.add_user_message",
                    "params":{"messageId":message_id,"text":text,"queuePlacement":"end_of_turn"}}),
                    );
                    let receipt = json!({"accepted":true,"request_id":request_id,"session_id":session.sid,"message_id":message_id,"sdkhost":live.host_key.is_some(),"operation":method,"text":text,"status":"dispatching"});
                    live.accepted.insert(dedup_key, receipt.clone());
                    live.controls.insert(request_id.clone(), response);
                    if method == "turn" {
                        live.turns.push_back(request_id.clone());
                    }
                    live.items.push(json!({"id":message_id,"native_id":message_id,"role":"user","content":text,"text":text,"request_id":request_id,"received_at":crate::sdk_message_time::now()}));
                    if method == "turn" {
                        live.compact = Value::Null;
                        live.retry = Value::Null;
                        live.error = None;
                        live.error_kind = None;
                        live.runtime_error = None;
                        live.last_turn = Value::Null;
                        live.interrupt_requested = false;
                    }
                    session.publish(&mut live);
                    if let Some(err) = live.last_checkpoint_error.clone() {
                        return Err(err);
                    }
                    sender
                        .send(frame)
                        .map_err(|_| "droid_sdk: native transport disconnected")?;
                }
                match tokio::time::timeout(Duration::from_secs(60), wait_response).await {
                    Ok(Ok(Ok(_))) => Ok(lock(&session.live).accepted[&request_id].clone()),
                    Ok(Ok(Err(error))) => Err(error),
                    _ => {
                        let error = "droid_sdk_write_uncertain: native admission unavailable; input not replayed";
                        let mut live = lock(&session.live);
                        if let Some(receipt) = live.accepted.get_mut(&request_id) {
                            receipt["status"] = json!("uncertain");
                            receipt["error"] = json!(error);
                        }
                        session.publish(&mut live);
                        Err(error.into())
                    }
                }
            }
            _ => Err(format!(
                "droid_sdk_unsupported: {method} (capabilities: {})",
                capabilities()
            )),
        }
    }

    fn respond(&self, session: &Session, input: &Value) -> Result<Value, String> {
        let key = input["key"]
            .as_str()
            .ok_or("droid_sdk: response key is required")?;
        let mut live = lock(&session.live);
        let card = live
            .requests
            .get(key)
            .ok_or("droid_sdk_request_gone: request no longer pending")?;
        let result = protocol::permission_result(card, &input["result"])?;
        let sender = live
            .outbound
            .as_ref()
            .ok_or("droid_sdk: native transport disconnected")?;
        sender.send(json!({"jsonrpc":"2.0","type":"response","factoryApiVersion":"1.0.0","id":key,"result":result}))
            .map_err(|_| "droid_sdk: native transport disconnected")?;
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
    fn save_checkpoint(&mut self) -> io::Result<()> {
        let Some(path) = &self.checkpoint else {
            return Ok(());
        };
        save_atomic(path, self)?;
        self.last_checkpoint = Some(std::time::Instant::now());
        Ok(())
    }

    fn save_checkpoint_throttled(&mut self) -> io::Result<()> {
        if self
            .last_checkpoint
            .is_none_or(|t| t.elapsed() >= Duration::from_millis(250))
        {
            self.save_checkpoint()?;
        }
        Ok(())
    }
}
