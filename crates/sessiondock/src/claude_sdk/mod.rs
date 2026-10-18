//! Claude's native, persistent stream-json SDK transport. See docs/claude-sdk.md.
//! HTTP cancellation and broadcast receiver lifetimes never own a CLI process.

mod protocol;
mod transport;
use crate::sessions::native_json;

use std::{
    collections::{BTreeMap, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::lifecycle::{
    launcher::{self, CliProfile},
    model::Source,
};

type ControlReply = oneshot::Sender<Result<Value, String>>;

pub struct Bridge {
    root: PathBuf,
    profile: CliProfile,
    proc_root: PathBuf,
    shutdown: CancellationToken,
    state: Mutex<BridgeState>,
}

#[derive(Default)]
struct BridgeState {
    sessions: BTreeMap<String, Arc<Session>>,
    creates: BTreeMap<String, (Value, Value)>,
}

struct Session {
    uid: String,
    sid: String,
    cwd: PathBuf,
    path: PathBuf,
    operations: AsyncMutex<()>,
    live: Mutex<Live>,
    stop: CancellationToken,
}

struct Live {
    seq: u64,
    items: Vec<Value>,
    requests: BTreeMap<String, Value>,
    controls: BTreeMap<String, ControlReply>,
    accepted: BTreeMap<String, Value>,
    turns: VecDeque<String>,
    outbound: Option<mpsc::UnboundedSender<Value>>,
    ready: bool,
    native_running: bool,
    state_observed: bool,
    runtime_stop: Option<CancellationToken>,
    pid: Option<u32>,
    model: Option<String>,
    effort: Option<String>,
    models: Value,
    error: Option<String>,
    // Diagnostic tail is private and redacted, never returned in snapshots.
    stderr: VecDeque<String>,
    streaming_message: String,
    streaming_blocks: BTreeMap<u64, Value>,
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
    Ok(Some(Arc::new(Bridge {
        root: root.clone(),
        profile,
        proc_root: config.proc_root.clone(),
        shutdown,
        state: Mutex::default(),
    })))
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
        "subagents":false,"effort":true,"mcp_elicitation":false,"user_dialog":false,
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
            operations: AsyncMutex::new(()),
            stop: CancellationToken::new(),
            live: Mutex::new(Live {
                seq: 0,
                items: vec![],
                requests: BTreeMap::new(),
                controls: BTreeMap::new(),
                accepted: BTreeMap::new(),
                turns: VecDeque::new(),
                outbound: None,
                ready: false,
                native_running: false,
                state_observed: false,
                runtime_stop: None,
                pid: None,
                model: input["model"].as_str().map(str::to_owned),
                effort: input["effort"].as_str().map(str::to_owned),
                models: Value::Null,
                error: None,
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
            "requests":live.requests.values().collect::<Vec<_>>(),"model":live.model,"effort":live.effort,
            "error":live.error,"capabilities":capabilities()}})
    }

    fn publish(&self, live: &mut Live) {
        live.seq += 1;
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
                .ok_or("claude_sdk: native transport disconnected")?;
            live.controls.insert(id.clone(), tx);
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
        let live = lock(&session.live);
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
        let session = Arc::new(Session::new(
            uid.into(),
            sid.into(),
            cwd,
            path,
            &Value::Null,
        ));
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
            "open" | "models" => {
                self.ensure(&session).await?;
                if method == "models" {
                    return Ok(
                        json!({"models":lock(&session.live).models,"capabilities":capabilities()}),
                    );
                }
                Ok(session.receipt())
            }
            "turn" => {
                let request_id = input["request_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or(uuid()?);
                if let Some(receipt) = lock(&session.live).accepted.get(&request_id).cloned() {
                    return Ok(receipt);
                }
                self.ensure(&session).await?;
                self.check_conflict(&session).await?;
                let text = input["text"]
                    .as_str()
                    .ok_or("claude_sdk: text must be a string")?;
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
                    "parent_tool_use_id":null,"message":{"role":"user","content":text}});
                sender
                    .send(frame)
                    .map_err(|_| "claude_sdk: native transport disconnected")?;
                let receipt = json!({"accepted":true,"request_id":request_id,"session_id":session.sid,"message_id":message_id});
                live.accepted.insert(request_id.clone(), receipt.clone());
                live.turns.push_back(request_id.clone());
                live.items.push(json!({"id":message_id,"native_id":message_id,"role":"user","content":text,"text":text,"request_id":request_id}));
                live.error = None;
                session.publish(&mut live);
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
        live.requests.remove(key);
        session.publish(&mut live);
        Ok(json!({"responded":true,"key":key}))
    }
}
