//! OpenCode v2 native API bridge. Contract and protocol evidence:
//! docs/opencode-sdk.md. No HTTP facade or native file writer lives here.
mod actions;
mod project;
mod transport;

use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use transport::{Transport, encoded};

pub struct Bridge {
    transport: Transport,
    root: PathBuf,
    proc_root: PathBuf,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    creates: tokio::sync::Mutex<HashMap<String, Value>>,
}

struct Session {
    uid: String,
    sid: String,
    cwd: PathBuf,
    state: Mutex<Live>,
    operation: Arc<tokio::sync::Mutex<()>>,
    wake: tokio::sync::Notify,
}

struct Live {
    seq: u64,
    raw: Value,
    records: HashMap<String, Value>,
    items: Vec<Value>,
    requests: Vec<Value>,
    turns: HashMap<String, String>,
    sending: usize,
    catalog: Vec<Value>,
    controls: HashMap<String, Value>,
    tx: broadcast::Sender<Arc<str>>,
}

impl Live {
    fn snapshot(&self) -> Value {
        json!({"seq":self.seq,"type":"snapshot","raw":self.raw})
    }
    fn publish(&mut self) {
        self.raw["items"] = json!(self.items);
        self.raw["requests"] = json!(self.requests);
        self.seq += 1;
        let _ = self.tx.send(Arc::from(self.snapshot().to_string()));
    }
    fn error(&mut self, error: &str) {
        self.raw["error"] = json!(error);
        self.publish();
    }
}

/// Configuration only: neither the native CLI nor an API process is started.
pub fn new(
    config: &crate::config::Config,
    shutdown: CancellationToken,
) -> io::Result<Option<Arc<Bridge>>> {
    let (Some(root), Some(path)) = (&config.roots.opencode, &config.launcher_config) else {
        return Ok(None);
    };
    let launcher = crate::lifecycle::launcher::read_config(path)
        .map_err(|e| io::Error::other(format!("OpenCode SDK launcher: {e:?}")))?;
    let mut profiles = launcher
        .profiles
        .into_iter()
        .filter(|p| p.source == crate::lifecycle::model::Source::Opencode);
    let Some(profile) = profiles.next() else {
        return Ok(None);
    };
    if profiles.next().is_some() {
        return Err(io::Error::other(
            "OpenCode SDK requires exactly one profile",
        ));
    }
    // The API must use the default shared service. A private CLI runtime cannot
    // be shared safely with an independently running TUI.
    if profile
        .args
        .iter()
        .any(|a| a == "--standalone" || a == "--server" || a.starts_with("--server="))
    {
        return Err(io::Error::other(
            "OpenCode SDK profile must use the shared native service",
        ));
    }
    Ok(Some(Arc::new(Bridge {
        transport: Transport { profile, shutdown },
        root: root.clone(),
        proc_root: config.proc_root.clone(),
        sessions: Mutex::new(HashMap::new()),
        creates: tokio::sync::Mutex::new(HashMap::new()),
    })))
}

impl Bridge {
    pub fn loaded_uids(&self) -> Vec<String> {
        self.sessions
            .lock()
            .map(|sessions| {
                sessions
                    .iter()
                    .filter(|(key, session)| {
                        key.split(':').count() == 2
                            && session
                                .state
                                .lock()
                                .map(|state| state.raw["error"].is_null())
                                .unwrap_or(false)
                    })
                    .map(|(_, session)| session.uid.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn subscribe(&self, key: &str) -> Result<(Value, broadcast::Receiver<Arc<str>>), String> {
        let session = self
            .sessions
            .lock()
            .map_err(|_| "OpenCode SDK state poisoned")?
            .get(key)
            .cloned()
            .ok_or("请先打开此 OpenCode SDK 会话")?;
        let state = session
            .state
            .lock()
            .map_err(|_| "OpenCode SDK state poisoned")?;
        // One lock covers both operations: an update cannot land between them.
        session.wake.notify_one();
        Ok((state.snapshot(), state.tx.subscribe()))
    }

    pub async fn call(
        self: &Arc<Self>,
        method: &str,
        target: Value,
        input: Value,
    ) -> Result<Value, String> {
        // Accepted operations run independently of the HTTP request/SSE lifetime.
        let bridge = self.clone();
        let method = method.to_owned();
        tokio::spawn(async move { bridge.dispatch(&method, target, input).await })
            .await
            .map_err(|e| format!("OpenCode SDK worker: {e}"))?
    }

    async fn dispatch(
        self: &Arc<Self>,
        method: &str,
        target: Value,
        input: Value,
    ) -> Result<Value, String> {
        match method {
            "create" => self.create(&target, &input).await,
            "open" => {
                let (session, key) = self.open(&target).await?;
                Ok(
                    json!({"uid":session.uid,"session_id":session.sid,"cwd":session.cwd,"key":key,"capabilities":capabilities()}),
                )
            }
            "turn" | "interrupt" | "respond" | "models" | "discard" | "queue_cancel"
            | "queue_update" | "fork" | "revert" | "unrevert" | "revert_commit" | "compact"
            | "shell" | "command" | "settings" | "agents" | "commands" | "usage" => {
                let (session, _) = self.open(&target).await?;
                match method {
                    "turn" => self.turn(session, input).await,
                    "interrupt" => {
                        self.check_runtime(&session.sid, &session.cwd)?;
                        let _guard = session.operation.lock().await;
                        self.transport.api(&session.cwd,"POST",&format!("/api/session/{}/interrupt?resume=false",encoded(&session.sid)),None).await
                    }
                    "respond" => self.respond(&session, input).await,
                    "models" => self.models(&session.cwd).await,
                    // Native remove is irreversible. Discard only releases the
                    // browser draft; ordinary delete owns native destruction.
                    "discard" => Err("OpenCode 原生会话没有可撤销 discard；请使用已确认的原生删除操作 (capability discard=false)".into()),
                    _ => self.action(&session, method, input).await,
                }
            }
            _ => Err(format!("OpenCode SDK 不支持 {method}")),
        }
    }

    async fn create(self: &Arc<Self>, target: &Value, input: &Value) -> Result<Value, String> {
        let request = string(input, "request_id")?;
        let cwd = PathBuf::from(string(target, "cwd")?);
        let cwd = cwd.canonicalize().map_err(|e| e.to_string())?;
        if !cwd.is_dir() {
            return Err("OpenCode cwd 不是目录".into());
        }
        self.check_runtime("", &cwd)?;
        let mut creates = self.creates.lock().await;
        if let Some(value) = creates.get(request) {
            if value["cwd"] != json!(cwd) {
                return Err("request_id 已用于另一个目录".into());
            }
            if value.get("session").is_some() {
                return Ok(value.clone());
            }
        }
        let native_id = match creates.get(request).and_then(|v| v["session_id"].as_str()) {
            Some(sid) => sid.to_owned(),
            None => random_id("ses_")?,
        };
        creates.insert(request.into(), json!({"cwd":cwd,"session_id":native_id}));
        let mut body = json!({"id":native_id,"location":{"directory":cwd}});
        if let Some(model) = model(input, None)? {
            body["model"] = model;
        }
        // Retry an ambiguous HTTP result against the same native identity.
        let response = match self
            .transport
            .api(
                &cwd,
                "GET",
                &format!("/api/session/{}", encoded(&native_id)),
                None,
            )
            .await
        {
            Ok(existing) => existing,
            Err(_) => {
                self.transport
                    .api(&cwd, "POST", "/api/session", Some(body))
                    .await?
            }
        };
        let info = &response["data"];
        let target = self.identity(info)?;
        let (session, key) = self.install(&target, info).await?;
        let value = json!({"uid":session.uid,"session_id":session.sid,"cwd":session.cwd,"key":key,
            "session":{"uid":session.uid,"source":"opencode","session_id":session.sid,"sid":session.sid,
                "cwd":session.cwd,"title":info["title"],"model":info["model"]},"capabilities":capabilities()});
        creates.insert(request.into(), value.clone());
        Ok(value)
    }

    fn identity(&self, info: &Value) -> Result<Value, String> {
        let sid = string(info, "id")?;
        let project = string(info, "projectID")?;
        if !crate::sessions::opencode::safe_id(sid) || !crate::sessions::opencode::safe_id(project)
        {
            return Err("OpenCode 原生路径身份无效".into());
        }
        let path = self.root.join(project).join(sid);
        Ok(
            json!({"uid":crate::sessions::uid_for("opencode",&path),"session_id":sid,
            "cwd":info["location"]["directory"]}),
        )
    }

    async fn open(self: &Arc<Self>, target: &Value) -> Result<(Arc<Session>, String), String> {
        let uid = string(target, "uid")?;
        let agent = target["agent"].as_str().unwrap_or_default();
        let key = if agent.is_empty() {
            uid.to_owned()
        } else {
            format!("{uid}:{agent}")
        };
        let existing = self
            .sessions
            .lock()
            .map_err(|_| "OpenCode SDK state poisoned")?
            .get(&key)
            .cloned();
        if let Some(session) = existing {
            if session.sid != string(target, "session_id")?
                || session.cwd != Path::new(string(target, "cwd")?)
            {
                return Err("OpenCode SDK target 与已打开的原生身份不一致".into());
            }
            return Ok((session, key));
        }
        let sid = string(target, "session_id")?;
        let cwd = Path::new(string(target, "cwd")?);
        self.check_runtime(sid, cwd)?;
        let info = self
            .transport
            .api(cwd, "GET", &format!("/api/session/{}", encoded(sid)), None)
            .await?;
        let identity = self.identity(&info["data"])?;
        if identity["uid"] != target["uid"]
            || identity["session_id"] != target["session_id"]
            || identity["cwd"] != target["cwd"]
        {
            return Err("OpenCode SDK 原生 session/cwd/uid 与 inventory 不一致".into());
        }
        self.install(target, &info["data"]).await
    }

    async fn install(
        self: &Arc<Self>,
        target: &Value,
        info: &Value,
    ) -> Result<(Arc<Session>, String), String> {
        let uid = string(target, "uid")?.to_owned();
        let sid = string(target, "session_id")?.to_owned();
        let cwd = PathBuf::from(string(target, "cwd")?);
        let agent = target["agent"].as_str().unwrap_or_default();
        let key = if agent.is_empty() {
            uid.clone()
        } else {
            format!("{uid}:{agent}")
        };
        let records = self.read_messages(&sid, &cwd, None).await?;
        let items = records
            .iter()
            .filter(|record| {
                record["type"] == "assistant"
                    && record["time"]["completed"].is_null()
                    && record["finish"].is_null()
            })
            .flat_map(project::messages)
            .collect();
        // Model catalog is credential-free and supplies the native context limit.
        // A failed catalog request must not make an otherwise usable session fail.
        let catalog = self
            .models(&cwd)
            .await
            .ok()
            .and_then(|v| v["models"].as_array().cloned())
            .unwrap_or_default();
        let (tx, _) = broadcast::channel(128);
        let session = Arc::new(Session {
            uid,
            sid: sid.clone(),
            cwd,
            state: Mutex::new(Live {
                seq: 0,
                raw: json!({"session_id":sid,"running":false,"items":[],"requests":[],"model":info["model"],
                "effort":info["model"]["variant"]}),
                records: records
                    .into_iter()
                    .filter_map(|v| Some((v["id"].as_str()?.to_owned(), v)))
                    .collect(),
                items,
                requests: Vec::new(),
                turns: HashMap::new(),
                sending: 0,
                catalog,
                controls: HashMap::new(),
                tx,
            }),
            operation: Arc::new(tokio::sync::Mutex::new(())),
            wake: tokio::sync::Notify::new(),
        });
        {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| "OpenCode SDK state poisoned")?;
            if let Some(existing) = sessions.get(&key) {
                return Ok((existing.clone(), key));
            }
            // Aliases for agent views use the same selected native SID/state,
            // rather than creating competing workers for that session.
            if let Some(existing) = sessions
                .values()
                .find(|s| s.uid == session.uid && s.sid == session.sid)
                .cloned()
            {
                sessions.insert(key.clone(), existing.clone());
                return Ok((existing, key));
            }
            sessions.insert(key.clone(), session.clone());
        }
        let bridge = self.clone();
        let worker = session.clone();
        // Hydrate native activity and outstanding requests before returning the
        // first subscribable snapshot. Failure remains visible and recoverable.
        if let Err(error) = self.refresh(&session).await
            && let Ok(mut state) = session.state.lock()
        {
            state.error(&error);
        }
        tokio::spawn(async move {
            bridge.observe(worker).await;
        });
        Ok((session, key))
    }

    async fn turn(self: &Arc<Self>, session: Arc<Session>, input: Value) -> Result<Value, String> {
        let request = string(&input, "request_id")?.to_owned();
        let text = input["text"]
            .as_str()
            .ok_or("OpenCode SDK 缺少 text 字符串")?
            .to_owned();
        let delivery = actions::delivery(&input, "queue")?;
        self.check_runtime(&session.sid, &session.cwd)?;
        // Reserve native operation order before acknowledging. An interrupt
        // following an accepted turn cannot overtake its prompt admission.
        let operation = session.operation.clone().lock_owned().await;
        let native_id;
        {
            let mut state = session
                .state
                .lock()
                .map_err(|_| "OpenCode SDK state poisoned")?;
            if let Some(id) = state.turns.get(&request) {
                return Ok(
                    json!({"accepted":true,"duplicate":true,"request_id":request,"message_id":id}),
                );
            }
            // Validate an override before accepting the request.
            model(&input, Some(&state.raw["model"]))?;
            native_id = random_id("msg_")?;
            state.turns.insert(request.clone(), native_id.clone());
            state.sending += 1;
            state.raw["running"] = json!(true);
            state.raw["error"] = Value::Null;
            state.items.push(json!({"id":native_id,"native_id":native_id,"role":"user","content":text,"text":text,"request_id":request,"delivery_state":"admitting"}));
            state.publish();
        }
        session.wake.notify_one();
        let bridge = self.clone();
        let task_session = session.clone();
        let task_id = native_id.clone();
        tokio::spawn(async move {
            let _operation = operation;
            let result = async {
                let current = task_session
                    .state
                    .lock()
                    .map_err(|_| "OpenCode SDK state poisoned")?
                    .raw["model"]
                    .clone();
                if let Some(model) = model(&input, Some(&current))? {
                    bridge
                        .transport
                        .api(
                            &task_session.cwd,
                            "POST",
                            &format!("/api/session/{}/model", encoded(&task_session.sid)),
                            Some(json!({"model":model})),
                        )
                        .await?;
                }
                bridge
                    .transport
                    .api(
                        &task_session.cwd,
                        "POST",
                        &format!("/api/session/{}/prompt", encoded(&task_session.sid)),
                        Some(json!({"id":task_id,"text":text,"delivery":delivery})),
                    )
                    .await
            }
            .await;
            if let Ok(mut state) = task_session.state.lock() {
                state.sending = state.sending.saturating_sub(1);
                match result {
                    Ok(_) => {
                        // This is the native POST response, distinct from the
                        // local asynchronous admission returned by turn(). A
                        // concurrent poll can still contain pre-submit history.
                        if let Some(item) = state
                            .items
                            .iter_mut()
                            .find(|item| item["native_id"] == task_id)
                        {
                            item["native_admitted"] = json!(true);
                        }
                        state.publish();
                    }
                    Err(error) => state.error(&error),
                }
            }
        });
        Ok(json!({"accepted":true,"request_id":request,"message_id":native_id}))
    }

    async fn respond(&self, session: &Session, input: Value) -> Result<Value, String> {
        self.check_runtime(&session.sid, &session.cwd)?;
        let key = string(&input, "key")?;
        let request = {
            session
                .state
                .lock()
                .map_err(|_| "OpenCode SDK state poisoned")?
                .requests
                .iter()
                .find(|r| r["key"] == key)
                .cloned()
                .ok_or("OpenCode 请求已结束或不属于此会话")?
        };
        let result = &input["result"];
        let (path, body) = if let Some(id) = key.strip_prefix("permission:") {
            let decision = result
                .as_str()
                .or_else(|| result["decision"].as_str())
                .ok_or("审批回答需要 once/always/reject")?;
            if !["once", "always", "reject"].contains(&decision) {
                return Err("原生审批回答需要 once/always/reject".into());
            }
            (
                format!("permission/{}/reply", encoded(id)),
                json!({"decision":decision}),
            )
        } else if let Some(id) = key.strip_prefix("form:") {
            let mut answer = result
                .get("answer")
                .cloned()
                .unwrap_or_else(|| result.clone());
            if !answer.is_object() {
                let fields = request["fields"]
                    .as_array()
                    .ok_or("原生 form 缺少 fields")?;
                if fields.len() != 1 {
                    return Err("多字段 OpenCode 问题需要按原生字段 key 提交对象".into());
                }
                let field = string(&fields[0], "key")?;
                answer = json!({(field):answer});
            }
            (
                format!("form/{}/reply", encoded(id)),
                json!({"answer":answer}),
            )
        } else {
            return Err("未知 OpenCode 原生请求".into());
        };
        let _guard = session.operation.lock().await;
        self.transport
            .api(
                &session.cwd,
                "POST",
                &format!("/api/session/{}/{path}", encoded(&session.sid)),
                Some(body),
            )
            .await?;
        if let Ok(mut state) = session.state.lock() {
            state.requests.retain(|r| r["key"] != key);
            state.publish();
        }
        Ok(json!({"ok":true}))
    }

    async fn models(&self, cwd: &Path) -> Result<Value, String> {
        let response = self
            .transport
            .api(
                cwd,
                "GET",
                &format!(
                    "/api/model?location%5Bdirectory%5D={}",
                    encoded(&cwd.to_string_lossy())
                ),
                None,
            )
            .await?;
        let models:Vec<_>=response["data"].as_array().ok_or("OpenCode model.list 返回格式未知")?.iter().map(|m|json!({
            "id":format!("{}/{}",m["providerID"].as_str().unwrap_or_default(),m["id"].as_str().unwrap_or_default()),
            "model":m["id"],"providerID":m["providerID"],"name":m["name"],"variants":m["variants"],"enabled":m["enabled"],
            "limit":m["limit"],"capabilities":m["capabilities"]})).collect();
        Ok(json!({"models":models,"capabilities":capabilities()}))
    }

    async fn read_messages(
        &self,
        sid: &str,
        cwd: &Path,
        known: Option<HashSet<String>>,
    ) -> Result<Vec<Value>, String> {
        let mut path = format!("/api/session/{}/message?order=desc&limit=100", encoded(sid));
        let mut records = Vec::new();
        loop {
            let response = self.transport.api(cwd, "GET", &path, None).await?;
            records.extend(
                response["data"]
                    .as_array()
                    .ok_or("OpenCode message.list 返回格式未知")?
                    .iter()
                    .cloned(),
            );
            // Recover the current native turn, then stop at the known tail.
            // Ordinary history pagination owns earlier persisted messages.
            let reached = records.iter().any(|record| {
                known.as_ref().map_or_else(
                    || record["type"] == "user",
                    |known| record["id"].as_str().is_some_and(|id| known.contains(id)),
                )
            });
            if reached {
                break;
            }
            if let Some(cursor) = response["cursor"]["next"].as_str() {
                path = format!(
                    "/api/session/{}/message?cursor={}",
                    encoded(sid),
                    encoded(cursor)
                );
            } else {
                break;
            }
        }
        records.reverse();
        Ok(records)
    }

    async fn observe(self: Arc<Self>, session: Arc<Session>) {
        loop {
            if self.transport.shutdown.is_cancelled() {
                break;
            }
            let active = session
                .state
                .lock()
                .map(|s| s.raw["running"] == true || s.tx.receiver_count() > 0)
                .unwrap_or(false);
            if !active {
                tokio::select! { _ = self.transport.shutdown.cancelled()=>break, _ = session.wake.notified()=>{} }
                continue;
            }
            let result = self.refresh(&session).await;
            if let Err(error) = result
                && let Ok(mut state) = session.state.lock()
                && state.raw["error"] != error
            {
                state.error(&error);
            }
            let active = session
                .state
                .lock()
                .map(|s| s.raw["running"] == true || s.tx.receiver_count() > 0)
                .unwrap_or(false);
            tokio::select! {_ = self.transport.shutdown.cancelled()=>break,
            _ = tokio::time::sleep(Duration::from_millis(if active {350}else{2000}))=>{}}
        }
    }

    async fn refresh(&self, session: &Session) -> Result<(), String> {
        let sid = encoded(&session.sid);
        let known = session
            .state
            .lock()
            .map_err(|_| "OpenCode SDK state poisoned")?
            .records
            .keys()
            .cloned()
            .collect();
        let messages = self
            .read_messages(&session.sid, &session.cwd, Some(known))
            .await?;
        let permissions = self
            .transport
            .api(
                &session.cwd,
                "GET",
                &format!("/api/session/{sid}/permission"),
                None,
            )
            .await?;
        let forms = self
            .transport
            .api(
                &session.cwd,
                "GET",
                &format!("/api/session/{sid}/form"),
                None,
            )
            .await?;
        let active = self
            .transport
            .api(&session.cwd, "GET", "/api/session/active", None)
            .await?;
        let inbox = self
            .transport
            .api(
                &session.cwd,
                "GET",
                &format!("/api/session/{sid}/inbox"),
                None,
            )
            .await?;
        let info = self
            .transport
            .api(&session.cwd, "GET", &format!("/api/session/{sid}"), None)
            .await?;
        let mut state = session
            .state
            .lock()
            .map_err(|_| "OpenCode SDK state poisoned")?;
        let before = state.raw.clone();
        for record in messages {
            let Some(id) = record["id"].as_str() else {
                continue;
            };
            if state.records.get(id) == Some(&record) {
                continue;
            }
            let projected = project::messages(&record);
            for item in projected {
                if let Some(old) = state.items.iter_mut().find(|m| m["id"] == item["id"]) {
                    let native_admitted = old["native_admitted"] == true;
                    *old = item;
                    if native_admitted {
                        old["native_admitted"] = json!(true);
                    }
                } else {
                    state.items.push(item);
                }
            }
            if record["type"] == "assistant" && !record["model"].is_null() {
                state.raw["model"] = record["model"].clone();
                state.raw["effort"] = record["model"]["variant"].clone();
            }
            state.records.insert(id.to_owned(), record);
        }
        state.requests = project::requests(&permissions, &forms);
        state.raw["queue"] = project::queue(&inbox);
        for entry in inbox["data"].as_array().into_iter().flatten() {
            if let Some(item) = state
                .items
                .iter_mut()
                .find(|item| item["native_id"] == entry["id"])
            {
                item["delivery_state"] = json!("queued");
                item["delivery"] = entry["delivery"].clone();
            }
        }
        if info["data"]["model"].is_object() {
            state.raw["model"] = info["data"]["model"].clone();
            state.raw["effort"] = info["data"]["model"]["variant"].clone();
        }
        state.raw["agent"] = info["data"]["agent"].clone();
        state.raw["permissions"] = info["data"]["permissions"].clone();
        state.raw["revert"] = info["data"]["revert"].clone();
        state.raw["usage"] = project::usage(&info["data"], &state.records, &state.catalog);
        state.raw["running"] = json!(
            state.sending > 0
                || active["data"].get(&session.sid).is_some()
                || inbox["data"].as_array().is_some_and(|v| !v.is_empty())
                || !state.requests.is_empty()
        );
        if state.raw["items"] != json!(state.items)
            || state.raw["requests"] != json!(state.requests)
            || state.raw != before
        {
            state.publish();
        }
        Ok(())
    }

    /// A default TUI joins the shared service. Private runtimes cannot be
    /// interrupted/controlled through it. Never kill or take over those writers.
    fn check_runtime(&self, sid: &str, cwd: &Path) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            let entries = std::fs::read_dir(&self.proc_root)
                .map_err(|e| format!("无法核实 OpenCode 原生 writer: {e}"))?;
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
                    continue;
                }
                let path = entry.path();
                let Ok(bytes) = std::fs::read(path.join("cmdline")) else {
                    continue;
                };
                let args: Vec<_> = bytes
                    .split(|b| *b == 0)
                    .filter_map(|b| std::str::from_utf8(b).ok())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !args.iter().take(3).any(|a| {
                    Path::new(a)
                        .file_name()
                        .is_some_and(|n| n == "opencode" || n == "opencode.exe")
                }) {
                    continue;
                }
                let independent = args.iter().any(|a| {
                    *a == "--standalone" || *a == "--server" || a.starts_with("--server=")
                });
                if !independent {
                    continue;
                }
                let same_sid = !sid.is_empty()
                    && (args
                        .windows(2)
                        .any(|a| (a[0] == "--session" || a[0] == "-s") && a[1] == sid)
                        || args
                            .iter()
                            .any(|a| a.strip_prefix("--session=") == Some(sid)));
                let same_cwd = std::fs::read_link(path.join("cwd"))
                    .ok()
                    .is_some_and(|p| p == cwd);
                if same_sid || same_cwd {
                    return Err("此 OpenCode 会话可能由独立 --standalone/--server runtime 写入；SDK 仅共享默认原生服务，不接管或终止独立 TUI".into());
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (sid, cwd);
        Ok(())
    }
}

fn capabilities() -> Value {
    json!({"send":true,"interrupt":true,"respond":true,"models":true,"create":true,
    "discard":false,"queue_cancel":true,"queue_update":true,"fork":true,
    "revert":true,"unrevert":true,"revert_commit":true,"compact":true,"shell":true,
    "command":true,"settings":true,"agents":true,"commands":true,"usage":true,
    "effort":true,"effort_kind":"native model variant","stream":"native API projection polling",
        "terminal_coexistence":"default shared native service only","autostart":false})
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("OpenCode SDK 缺少 {key}"))
}

fn model(input: &Value, current: Option<&Value>) -> Result<Option<Value>, String> {
    if input.get("model").is_none_or(Value::is_null)
        && input.get("effort").is_none_or(Value::is_null)
    {
        return Ok(None);
    }
    let mut model = match &input["model"] {
        Value::Object(_) => input["model"].clone(),
        Value::String(s) => {
            let (provider, id) = s
                .split_once('/')
                .ok_or("OpenCode model 需要 provider/model 或 {providerID,id}")?;
            json!({"providerID":provider,"id":id})
        }
        Value::Null => current
            .cloned()
            .filter(|v| v.is_object())
            .ok_or("设置 OpenCode variant 需要原生 model")?,
        _ => return Err("OpenCode model 格式错误".into()),
    };
    string(&model, "providerID")?;
    string(&model, "id")?;
    if let Some(effort) = input.get("effort").filter(|v| !v.is_null()) {
        model["variant"] = json!(
            effort
                .as_str()
                .ok_or("OpenCode effort 必须是原生 variant 名称")?
        );
    }
    Ok(Some(model))
}

fn random_id(prefix: &str) -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(format!(
        "{prefix}{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}
