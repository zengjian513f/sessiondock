//! Native Agy headless NDJSON bridge. No PTY, mirror writes or approval emulation.
//! See docs/agy-sdk.md for the verified protocol and coexistence boundary.
mod transcript;
mod transport;

use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use transport::{Native, ProcessConfig};

pub struct Bridge {
    process: Arc<ProcessConfig>,
    mirror: PathBuf,
    shutdown: CancellationToken,
    sessions: Mutex<Sessions>,
    creates: tokio::sync::Mutex<HashMap<String, Result<Value, String>>>,
}

#[derive(Default)]
struct Sessions {
    keys: HashMap<String, Arc<Live>>,
    native: HashMap<String, Arc<Live>>,
}

struct Live {
    sid: String,
    cwd: String,
    data: Mutex<Data>,
    events: broadcast::Sender<Arc<str>>,
    commands: mpsc::UnboundedSender<Command>,
    stop: CancellationToken,
    done: CancellationToken,
}

struct Data {
    seq: u64,
    items: Vec<Value>,
    receipts: HashMap<String, Value>,
    outstanding: usize,
    active: Option<String>,
    model: Option<String>,
    // Observed native state is presentation evidence, not a launch override.
    observed_model: Option<String>,
    effort: Option<String>,
    error: Option<String>,
    error_kind: Option<&'static str>,
    diagnostics: String,
    usage: Value,
    step_usage: Value,
    mode: Option<String>,
    runtime: Option<Turn>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Turn {
    id: String,
    text: String,
    model: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
    #[serde(default)]
    recovering: bool,
    #[serde(default)]
    persistent: bool,
    #[serde(default)]
    runtime_id: String,
    #[serde(default)]
    stdout_offset: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Binding {
    sid: String,
    cwd: String,
    turn: Turn,
}

enum Command {
    Turn(Turn),
    Interrupt(oneshot::Sender<Result<Value, String>>),
}

pub fn new(
    config: &crate::config::Config,
    shutdown: CancellationToken,
) -> io::Result<Option<Arc<Bridge>>> {
    if !cfg!(target_os = "linux") {
        return Ok(None);
    }
    let (Some(mirror), Some(home), Some(launcher)) =
        (&config.roots.agy, &config.agy_home, &config.launcher_config)
    else {
        return Ok(None);
    };
    let launcher = crate::lifecycle::launcher::read_config(launcher)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "agy_sdk_launcher_config"))?;
    let Some(profile) = launcher
        .profiles
        .into_iter()
        .find(|profile| profile.source == crate::lifecycle::model::Source::Agy)
    else {
        return Ok(None);
    };
    let process = ProcessConfig::new(
        profile,
        home.clone(),
        config.proc_root.clone(),
        config.state_dir.as_ref(),
    )?;
    Ok(Some(Arc::new(Bridge {
        process: Arc::new(process),
        mirror: mirror.clone(),
        shutdown,
        sessions: Mutex::new(Sessions::default()),
        creates: tokio::sync::Mutex::new(HashMap::new()),
    })))
}

fn capabilities() -> Value {
    json!({"send":true,"create":true,"resume":true,"stream":true,
        "interrupt":cfg!(unix),"respond":false,"approvals":false,"questions":false,
        "models":true,"model_per_turn":true,"shared_tui":false,"usage":true,"settings":true,"mode":true,
        "limitations":["Agy headless 由原生策略允许或软拒绝审批；stream-json 不支持 control_request/control_response。",
        "独立 TUI 与 SDK 没有共享运行时；原生数据库被外部进程打开时拒绝恢复/发送。",
        "连续回合复用同一原生 headless 进程；显式更改模型、强度或模式在下一轮重建进程。"]})
}

fn text(value: &Value, field: &str) -> Result<String, String> {
    value[field]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("agy_sdk_missing_{field}"))
}

fn choice(value: &Value, field: &str) -> Option<String> {
    value[field]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn key(target: &Value) -> Result<String, String> {
    let uid = text(target, "uid")?;
    Ok(match choice(target, "agent") {
        Some(agent) => format!("{uid}:{agent}"),
        None => uid,
    })
}

impl Bridge {
    pub async fn stop(&self, uid: &str) -> Result<bool, String> {
        let live = {
            let mut sessions = self.sessions.lock().unwrap();
            let Some(live) = sessions.keys.remove(uid) else {
                return Ok(false);
            };
            sessions.native.remove(&live.sid);
            live
        };
        let _ = live
            .events
            .send(Arc::from(json!({"method":"bridge/stopped"}).to_string()));
        live.stop.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(10), live.done.cancelled())
            .await
            .map_err(|_| "Agy SDK 停止尚未确认")?;
        if let Some(path) = self.binding_path(&live.sid) {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("agy_sdk_binding: {error}")),
            }
        }
        Ok(true)
    }

    pub fn loaded_uids(&self) -> Vec<String> {
        self.sessions
            .lock()
            .unwrap()
            .keys
            .keys()
            .filter(|key| key.split(':').count() == 2)
            .cloned()
            .collect()
    }

    /// Detached from the HTTP request lifetime: accepted native work survives page/SSE closure.
    pub async fn call(
        self: &Arc<Self>,
        method: &str,
        target: Value,
        input: Value,
    ) -> Result<Value, String> {
        let this = self.clone();
        let method = method.to_owned();
        tokio::spawn(async move { this.dispatch(&method, target, input).await })
            .await
            .map_err(|_| "agy_sdk_worker_failed".to_owned())?
    }

    async fn dispatch(
        self: &Arc<Self>,
        method: &str,
        target: Value,
        input: Value,
    ) -> Result<Value, String> {
        if self.shutdown.is_cancelled() {
            return Err("agy_sdk_shutdown".into());
        }
        match method {
            "models" => self.process.models(self.shutdown.clone()).await,
            "create" => self.create(target, input).await,
            "usage" => { let live=self.open(&target).await?;let data=live.data.lock().unwrap();Ok(json!({"usage":data.usage,"step_usage":data.step_usage})) },
            "settings" => {
                let live=self.open(&target).await?;let mut data=live.data.lock().unwrap();
                if let Some(mode)=choice(&input,"mode") { data.mode=Some(mode); }
                if let Some(model)=choice(&input,"model") { data.model=Some(model); }
                if input.get("effort").is_some() { data.effort=choice(&input,"effort"); }
                live.publish(&mut data);
                Ok(json!({"settings":{"mode":data.mode,"model":data.model,"effort":data.effort},"applies":"next_turn"}))
            },
            "open" => {
                let live = self.open(&target).await?;
                self.process.check_available(live.sid.clone()).await?;
                let data = live.data.lock().unwrap();
                Ok(json!({"session_id":live.sid,"cwd":live.cwd,"capabilities":capabilities(),"raw":live.raw(&data)}))
            }
            "reattach" => {
                let sid = text(&target, "session_id")?;
                if self.process.host_dir.is_none() || self.read_binding(&sid)?.is_none() {
                    return Ok(json!({"attached":false}));
                }
                let live = self.open(&target).await?;
                Ok(json!({"attached":true,"session_id":live.sid}))
            }
            "recover" => {
                let live = self.open(&target).await?;
                let request = text(&input, "request_id")?;
                let data = live.data.lock().unwrap();
                data.receipts.get(&request).cloned().ok_or_else(|| "agy_sdk_recovery_missing: 未找到原宿主绑定，不会重发输入".into())
            }
            "turn" => {
                let live = self.open(&target).await?;
                if let Some(request)=choice(&input,"request_id")
                    && let Some(receipt)=live.data.lock().unwrap().receipts.get(&request).cloned()
                {return Ok(receipt);}
                // Check again for an independent writer even when this session was already opened.
                self.process.check_available(live.sid.clone()).await?;
                let mut data = live.data.lock().unwrap();
                let request = choice(&input, "request_id").unwrap_or_else(|| format!("local:{}", data.seq + 1));
                if let Some(receipt) = data.receipts.get(&request) { return Ok(receipt.clone()); }
                let content = input["text"].as_str().ok_or("agy_sdk_missing_text")?.to_owned();
                let turn = Turn { id: request.clone(), text: content.clone(),
                    model: choice(&input,"model").or_else(|| data.model.clone()),
                    effort: choice(&input,"effort").or_else(|| data.effort.clone()),
                    mode: choice(&input,"mode").or_else(|| data.mode.clone()), recovering: false,
                    persistent:true, runtime_id:request.clone(), stdout_offset:0 };
                let mut turn = turn;
                if let Some(runtime) = &data.runtime
                    && runtime.model == turn.model && runtime.effort == turn.effort && runtime.mode == turn.mode
                {
                    turn.runtime_id = runtime.runtime_id.clone();
                    turn.stdout_offset = runtime.stdout_offset;
                }
                let receipt = json!({"accepted":true,"sdkhost":self.process.host_dir.is_some(),"request_id":request,"session_id":live.sid,"capabilities":capabilities()});
                self.save_binding(&live, &turn)?;
                live.commands.send(Command::Turn(turn)).map_err(|_| "agy_sdk_shutdown")?;
                data.items.push(json!({"id":format!("agy:request:{request}"),"role":"user","content":content,"request_id":request,"status":"queued"}));
                data.receipts.insert(request, receipt.clone());
                data.outstanding += 1;
                data.error = None;
                data.error_kind = None;
                live.publish(&mut data);
                Ok(receipt)
            }
            "interrupt" => {
                let live = self.lookup(&key(&target)?)?;
                let (sender, receiver) = oneshot::channel();
                live.commands.send(Command::Interrupt(sender)).map_err(|_| "agy_sdk_shutdown")?;
                receiver.await.map_err(|_| "agy_sdk_shutdown".to_owned())?
            }
            "respond" => Err("agy_sdk_respond_unsupported: Agy stream-json 不支持控制请求或审批/问题回答；原生 headless 按已配置策略软拒绝需要交互批准的工具。".into()),
            "discard" => {
                // A create has already obtained a native SID; never delete that database or cancel work.
                let live = self.lookup(&key(&target)?)?;
                Ok(json!({"discarded":true,"native_preserved":true,"session_id":live.sid,"capabilities":capabilities()}))
            }
            _ => Err(format!("agy_sdk_method_unsupported: {method}")),
        }
    }

    fn lookup(&self, key: &str) -> Result<Arc<Live>, String> {
        self.sessions
            .lock()
            .unwrap()
            .keys
            .get(key)
            .cloned()
            .ok_or_else(|| "agy_sdk_not_open".into())
    }

    async fn open(self: &Arc<Self>, target: &Value) -> Result<Arc<Live>, String> {
        let key = key(target)?;
        let sid = text(target, "session_id")?;
        let cwd = text(target, "cwd")?;
        transport::validate_sid(&sid)?;
        if let Ok(live) = self.lookup(&key) {
            if live.sid != sid || live.cwd != cwd {
                return Err("agy_sdk_identity_changed".into());
            }
            return Ok(live);
        }
        let binding = self.read_binding(&sid)?;
        if let Some(binding) = &binding {
            if binding.sid != sid || binding.cwd != cwd {
                return Err("agy_sdk_identity_changed".into());
            }
            if let Some(dir) = &self.process.host_dir {
                let runtime = if binding.turn.runtime_id.is_empty() {
                    &binding.turn.id
                } else {
                    &binding.turn.runtime_id
                };
                let host = sdkhost_client::Client::attach(dir, &host_key(&sid, runtime))
                    .await
                    .map_err(|e| format!("agy_sdk_recovery_missing: {e}"))?;
                let status = host
                    .status()
                    .await
                    .map_err(|e| format!("agy_sdk_recovery: {e}"))?;
                if status.exit.is_none() {
                    self.process.adopt(status.pid);
                }
            }
        }
        self.process.check_available(sid.clone()).await?;
        if !self.process.native_path(&sid).is_file() {
            return Err(
                "agy_sdk_native_missing: 原生 conversation 数据库不存在，不能从只读镜像恢复。"
                    .into(),
            );
        }
        let observed_model = transcript::model(&self.process, &sid).await;
        let live = self.install(key, sid, cwd, None, None)?;
        live.observe_model(observed_model);
        if let Some(mut binding) = binding {
            let mut data = live.data.lock().unwrap();
            // Concurrent opens share the installed actor and its receipt.
            if !data.receipts.contains_key(&binding.turn.id) {
                binding.turn.recovering = true;
                let request = binding.turn.id.clone();
                data.model = binding.turn.model.clone();
                data.effort = binding.turn.effort.clone();
                data.mode = binding.turn.mode.clone();
                data.items.push(json!({"id":format!("agy:request:{request}"),"role":"user","content":binding.turn.text,"request_id":request,"status":"running"}));
                data.receipts.insert(request.clone(), json!({"accepted":true,"sdkhost":self.process.host_dir.is_some(),"request_id":request,"session_id":live.sid,"capabilities":capabilities()}));
                data.outstanding += 1;
                live.commands
                    .send(Command::Turn(binding.turn))
                    .map_err(|_| "agy_sdk_shutdown")?;
                live.publish(&mut data);
            }
        }
        Ok(live)
    }

    fn binding_path(&self, sid: &str) -> Option<PathBuf> {
        self.process
            .adapter_dir
            .as_ref()
            .map(|dir| dir.join(format!("{sid}.json")))
    }

    fn read_binding(&self, sid: &str) -> Result<Option<Binding>, String> {
        let Some(path) = self.binding_path(sid) else {
            return Ok(None);
        };
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| format!("agy_sdk_binding: {e}")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("agy_sdk_binding: {error}")),
        }
    }

    fn save_binding(&self, live: &Live, turn: &Turn) -> Result<(), String> {
        use std::io::Write;
        let Some(path) = self.binding_path(&live.sid) else {
            return Ok(());
        };
        let result = (|| -> io::Result<()> {
            let parent = path.parent().unwrap();
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
            let mut options = std::fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let temporary = path.with_extension("tmp");
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec(&Binding {
                sid: live.sid.clone(),
                cwd: live.cwd.clone(),
                turn: turn.clone(),
            })?)?;
            file.sync_all()?;
            std::fs::rename(temporary, path)
        })();
        result.map_err(|e| format!("agy_sdk_binding: {e}"))
    }

    fn install(
        &self,
        key: String,
        sid: String,
        cwd: String,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<Arc<Live>, String> {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(live) = sessions.native.get(&sid).cloned() {
            if live.cwd != cwd {
                return Err("agy_sdk_identity_changed".into());
            }
            sessions.keys.insert(key, live.clone());
            return Ok(live);
        }
        let (commands, receiver) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(256);
        let live = Arc::new(Live {
            sid: sid.clone(),
            cwd,
            commands,
            stop: CancellationToken::new(),
            done: CancellationToken::new(),
            events,
            data: Mutex::new(Data {
                seq: 0,
                items: vec![],
                receipts: HashMap::new(),
                outstanding: 0,
                active: None,
                model,
                observed_model: None,
                effort,
                error: None,
                error_kind: None,
                diagnostics: String::new(),
                usage: Value::Null,
                step_usage: Value::Null,
                mode: None,
                runtime: None,
            }),
        });
        sessions.native.insert(sid, live.clone());
        sessions.keys.insert(key, live.clone());
        let process = self.process.clone();
        let running = live.clone();
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            actor(process, running.clone(), receiver, shutdown).await;
            running.done.cancel();
        });
        Ok(live)
    }

    async fn create(self: &Arc<Self>, target: Value, input: Value) -> Result<Value, String> {
        if target["source"]
            .as_str()
            .is_some_and(|source| source != "agy")
        {
            return Err("agy_sdk_wrong_source".into());
        }
        let request = text(&input, "request_id")?;
        let mut creates = self.creates.lock().await;
        if let Some(receipt) = creates.get(&request) {
            return receipt.clone();
        }
        let result = self.create_native(target, input).await;
        creates.insert(request, result.clone());
        result
    }

    async fn create_native(self: &Arc<Self>, target: Value, input: Value) -> Result<Value, String> {
        let cwd = text(&target, "cwd")?;
        let cwd = std::fs::canonicalize(&cwd).map_err(|error| format!("agy_sdk_cwd: {error}"))?;
        if !cwd.is_dir() {
            return Err("agy_sdk_cwd_not_directory".into());
        }
        let model = choice(&input, "model");
        let effort = choice(&input, "effort");
        // init allocates the native identity without submitting a model prompt.
        let native = Native::start(
            self.process.clone(),
            None,
            &cwd,
            model.as_deref(),
            effort.as_deref(),
            choice(&input, "mode").as_deref(),
            self.shutdown.clone(),
            None,
        )
        .await?;
        let sid = native.sid.clone();
        let native_cwd = native.cwd.clone();
        let observed_model = native.model.clone();
        let diagnostics = native.close(self.shutdown.clone()).await?;
        if !self.process.native_path(&sid).is_file() {
            return Err("agy_sdk_native_missing_after_create".into());
        }
        // Inventory's established Agy UID is rooted in the read-only mirror *directory*.
        // It is determined by this native SID, without creating any mirror files here.
        let path = self.mirror.join("cli").join(&sid);
        let uid = crate::sessions::uid_for("agy", &path);
        let live = self.install(uid.clone(), sid.clone(), native_cwd.clone(), model, effort)?;
        live.observe_model(observed_model);
        live.diagnostic(diagnostics);
        let data = live.data.lock().unwrap();
        let receipt = json!({"uid":uid,"session_id":sid,"cwd":native_cwd,
            "session":{"uid":uid,"source":"agy","sid":sid,"session_id":sid,"cwd":native_cwd,
                "path":path,"native_path":self.process.native_path(&sid),"title":"Agy 会话","pending":true},
            "capabilities":capabilities(),"raw":live.raw(&data)});
        Ok(receipt)
    }

    /// The caller must open the exact selected target first. agent keys are uid:agent.
    /// Both snapshot and receiver are captured under the publication lock.
    pub fn subscribe(&self, key: &str) -> Result<(Value, broadcast::Receiver<Arc<str>>), String> {
        let live = self.lookup(key)?;
        let data = live.data.lock().unwrap();
        let receiver = live.events.subscribe();
        Ok((live.snapshot(&data), receiver))
    }
}

impl Live {
    fn observe_model(&self, model: Option<String>) {
        if let Some(model) = model {
            let mut data = self.data.lock().unwrap();
            if data.observed_model.as_ref() != Some(&model) {
                data.observed_model = Some(model);
                self.publish(&mut data);
            }
        }
    }

    fn raw(&self, data: &Data) -> Value {
        json!({"session_id":self.sid,"cwd":self.cwd,"running":data.outstanding>0,
            "items":data.items,"requests":[],"model":data.model.as_ref().or(data.observed_model.as_ref()),"effort":data.effort,
            "error":data.error,"error_kind":data.error_kind,"diagnostics":data.diagnostics,"capabilities":capabilities(),
            "usage":data.usage,"step_usage":data.step_usage,"settings":{"mode":data.mode,"applies":"next_turn"},
            "modes":[{"id":"accept-edits","name":"接受编辑"},{"id":"auto","name":"自动"},{"id":"plan","name":"计划"}]})
    }
    fn snapshot(&self, data: &Data) -> Value {
        json!({"seq":data.seq,"type":"snapshot","raw":self.raw(data)})
    }
    fn publish(&self, data: &mut Data) {
        data.seq += 1;
        let _ = self.events.send(Arc::from(self.snapshot(data).to_string()));
    }
    fn diagnostic(&self, diagnostic: String) {
        if diagnostic.is_empty() {
            return;
        }
        let mut data = self.data.lock().unwrap();
        data.diagnostics.push_str(&diagnostic);
        self.publish(&mut data);
    }
}

#[derive(Default)]
struct Runtime {
    native: Option<Native>,
    transcript: transcript::Pending,
}

async fn actor(
    process: Arc<ProcessConfig>,
    live: Arc<Live>,
    mut receiver: mpsc::UnboundedReceiver<Command>,
    shutdown: CancellationToken,
) {
    let mut pending = std::collections::VecDeque::new();
    let mut runtime = Runtime::default();
    loop {
        if shutdown.is_cancelled() || live.stop.is_cancelled() {
            break;
        }
        let command = if let Some(turn) = pending.pop_front() {
            Some(Command::Turn(turn))
        } else {
            tokio::select! {
                _=shutdown.cancelled()=>None,
                _=live.stop.cancelled()=>None,
                command=receiver.recv()=>command,
                line=async {
                    match runtime.native.as_mut() {
                        Some(native) => native.next_line().await,
                        None => std::future::pending().await,
                    }
                }=>{
                    match line {
                        Ok(Some(line))=>{
                            let turn=live.data.lock().unwrap().runtime.clone();
                            if let Some(turn)=turn && let Ok(event)=serde_json::from_str::<Value>(&line)
                                && runtime.native.as_ref().unwrap().check_event(&event).is_ok()
                            {
                                if event["event"]=="step_update" {
                                    project(&live,&turn,&event["step_update"]);
                                    runtime.transcript.observe(&event["step_update"]);
                                    runtime.transcript.refresh(&process,&live,&turn).await;
                                } else if event["event"]=="result" {
                                    let mut data=live.data.lock().unwrap();
                                    if event["result"]["usage"].is_object() { data.usage=event["result"]["usage"].clone(); }
                                    if event["result"]["status"]!="SUCCESS" {
                                        data.error=Some(format!("agy_sdk_native_{}: {}",event["result"]["status"],process.redact(event["result"]["error"].as_str().unwrap_or("原生后台任务失败"))));
                                        data.error_kind=Some("turn");
                                    }
                                    live.publish(&mut data);
                                }
                                let runtime_native_offset=runtime.native.as_ref().unwrap().stdout_offset;
                                if let Some(runtime)=live.data.lock().unwrap().runtime.as_mut() {
                                    runtime.stdout_offset=runtime_native_offset;
                                }
                            }
                        }
                        Ok(None)|Err(_)=>{
                            runtime.native=None;
                            let mut data=live.data.lock().unwrap();
                            data.runtime=None;
                            data.error=Some("agy_sdk_runtime_closed: 原生输出已结束；下次发送将启动同一会话的新运行实例。".into());
                            data.error_kind=Some("transport");
                            live.publish(&mut data);
                        }
                    }
                    continue;
                },
                _=tokio::time::sleep(std::time::Duration::from_millis(350)), if !runtime.transcript.is_empty()=>{
                    let turn=live.data.lock().unwrap().runtime.clone();
                    if let Some(turn)=turn { runtime.transcript.refresh(&process,&live,&turn).await; }
                    continue;
                }
            }
        };
        let Some(command) = command else {
            break;
        };
        match command {
            Command::Interrupt(reply) => {
                let _ = reply.send(Err("agy_sdk_idle".into()));
            }
            Command::Turn(turn) => {
                {
                    let mut data = live.data.lock().unwrap();
                    data.active = Some(turn.id.clone());
                    data.error = None;
                    data.error_kind = None;
                    data.model = turn.model.clone();
                    data.effort = turn.effort.clone();
                    if let Some(item) = data
                        .items
                        .iter_mut()
                        .find(|item| item["request_id"] == turn.id && item["role"] == "user")
                    {
                        item["status"] = json!("running");
                    }
                    live.publish(&mut data);
                }
                let outcome = execute(
                    &process,
                    &live,
                    &turn,
                    &mut receiver,
                    &mut pending,
                    &mut runtime,
                    shutdown.clone(),
                )
                .await;
                let mut data = live.data.lock().unwrap();
                let conflict = outcome
                    .as_ref()
                    .is_err_and(|error| error.starts_with("agy_sdk_result_conflict:"));
                data.runtime = if (outcome.is_ok() || conflict) && turn.persistent {
                    runtime.native.as_ref().map(|native| {
                        let mut runtime = turn.clone();
                        runtime.stdout_offset = native.stdout_offset;
                        runtime
                    })
                } else {
                    None
                };
                data.active = None;
                data.outstanding = data.outstanding.saturating_sub(1);
                if let Some(item) = data
                    .items
                    .iter_mut()
                    .find(|item| item["request_id"] == turn.id && item["role"] == "user")
                {
                    item["status"] = json!(if outcome.is_ok() {
                        "completed"
                    } else if conflict {
                        "unknown"
                    } else {
                        "failed"
                    });
                }
                if let Err(error) = outcome {
                    data.error_kind = Some(if conflict {
                        "uncertain"
                    } else if error.starts_with("agy_sdk_interrupted")
                        || error.starts_with("agy_sdk_native_INTERRUPTED")
                    {
                        "interrupted"
                    } else if error.starts_with("agy_sdk_native_") {
                        "turn"
                    } else {
                        "transport"
                    });
                    data.error = Some(error.clone());
                    if !conflict {
                        data.items.push(json!({"id":format!("agy:error:{}",turn.id),"role":"system","content":error,"error":true}));
                    }
                }
                live.publish(&mut data);
            }
        }
        if shutdown.is_cancelled() || live.stop.is_cancelled() {
            break;
        }
    }
    if shutdown.is_cancelled() {
        return;
    }
    if let Some(mut native) = runtime.native {
        native.terminate().await;
    }
    let mut data = live.data.lock().unwrap();
    if data.outstanding > 0 {
        data.error_kind = Some("transport");
        data.error = Some("agy_sdk_shutdown: 已接受工作未完成，不会自动重放。".into());
    }
    data.outstanding = 0;
    data.active = None;
    live.publish(&mut data);
}

async fn execute(
    process: &Arc<ProcessConfig>,
    live: &Arc<Live>,
    turn: &Turn,
    receiver: &mut mpsc::UnboundedReceiver<Command>,
    pending: &mut std::collections::VecDeque<Turn>,
    runtime: &mut Runtime,
    shutdown: CancellationToken,
) -> Result<(), String> {
    let Runtime {
        native: slot,
        transcript,
    } = runtime;
    process.check_available(live.sid.clone()).await?;
    let runtime = if turn.runtime_id.is_empty() {
        &turn.id
    } else {
        &turn.runtime_id
    };
    let key = host_key(&live.sid, runtime);
    if slot
        .as_ref()
        .is_some_and(|native| native.key.as_deref() != Some(&key))
    {
        let mut previous = slot.take().unwrap();
        previous.close_input().await?;
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            previous.finish(shutdown.clone()),
        )
        .await
        {
            Ok(result) => {
                result?;
            }
            Err(_) => {
                previous.terminate().await;
                return Err("agy_sdk_settings_close_timeout".into());
            }
        }
    }
    if slot.is_none() {
        *slot = Some(
            Native::start(
                process.clone(),
                Some(&live.sid),
                std::path::Path::new(&live.cwd),
                turn.model.as_deref(),
                turn.effort.as_deref(),
                turn.mode.as_deref(),
                shutdown.clone(),
                Some((&key, turn.recovering, turn.stdout_offset)),
            )
            .await?,
        );
        live.observe_model(slot.as_ref().unwrap().model.clone());
    }
    let native = slot.as_mut().unwrap();
    let outcome=async {
    tokio::select! {
        _=shutdown.cancelled()=>{return Err("agy_sdk_shutdown".into());},
        sent=native.send(&turn.text, if turn.persistent { &turn.id } else { "input" })=>sent?,
    }
    // Preserve EOF completion only for bindings created before persistent runtimes.
    if !turn.persistent { native.close_input().await?; }
    let mut result = None;
    let mut interrupted = false;
    let mut interrupt_deadline = None;
    let mut saw_assistant = false;
    loop {
        tokio::select! {
            _=tokio::time::sleep(std::time::Duration::from_millis(350)), if !transcript.is_empty()=>{
                saw_assistant |= transcript.refresh(process,live,turn).await;
            }
            _=shutdown.cancelled()=>{ return Err("agy_sdk_shutdown".into()); }
            _=live.stop.cancelled()=>{ native.terminate().await; return Err("agy_sdk_stopped".into()); }
            _=async { match interrupt_deadline { Some(deadline)=>tokio::time::sleep_until(deadline).await, None=>std::future::pending().await } }=>{
                native.terminate().await;return Err("agy_sdk_interrupted: 原生 SIGINT 未及时退出，已结束本桥接进程。".into());
            }
            command=receiver.recv()=>match command {
                Some(Command::Turn(turn))=>pending.push_back(turn),
                Some(Command::Interrupt(reply))=>{
                    let answer=native.interrupt().await;
                    if answer.is_ok() { interrupted=true;interrupt_deadline=Some(tokio::time::Instant::now()+std::time::Duration::from_secs(5)); }
                    let _=reply.send(answer.map(|()|json!({"interrupted":true,"session_id":live.sid})));
                }
                None=>{ native.terminate().await;return Err("agy_sdk_shutdown".into()); }
            },
            line=native.next_line()=>{
                let line=match line { Ok(Some(line))=>line, Ok(None)=>break, Err(error)=>{ native.terminate().await;return Err(format!("agy_sdk_read: {error}")); } };
                let event:Value=match serde_json::from_str(&line) {Ok(event)=>event,Err(_)=>{native.terminate().await;return Err("agy_sdk_invalid_ndjson".into());}};
                if let Err(error)=native.check_event(&event) {native.terminate().await;return Err(error);}
                match event["event"].as_str() {
                    Some("step_update")=>{
                        let step=&event["step_update"];
                        saw_assistant |= step["step_type"]=="agent_response" && step["text_delta"].as_str().is_some_and(|s|!s.is_empty());
                        project(live,turn,step);
                        transcript.observe(step);
                        saw_assistant |= transcript.refresh(process,live,turn).await;
                    }
                    Some("result")=>{
                        saw_assistant |= transcript.refresh(process,live,turn).await;
                        let value=&event["result"];
                        if value["usage"].is_object() { let mut data=live.data.lock().unwrap();data.usage=value["usage"].clone();live.publish(&mut data); }
                        if !saw_assistant && value["response"].as_str().is_some_and(|text|!text.is_empty()) {
                            let mut data=live.data.lock().unwrap();
                            data.items.push(json!({"id":format!("agy:result:{}",turn.id),"role":"assistant","content":value["response"],"phase":"final","native":value}));live.publish(&mut data);
                        }
                        let status=value["status"].as_str().unwrap_or("INVALID");
                        if !interrupted && transcript::conflicting_result(process,live,turn,value).await {
                            live.diagnostic(format!("\nAgy result.status=ERROR: {}\n",process.redact(value["error"].as_str().unwrap_or_default())));
                            return Err("agy_sdk_result_conflict: 输入已被原生读取，中断后已有完整回复，但原生最终回执仍报告旧中断；请核对会话结果，不要直接重发。".into());
                        }
                        result=Some(if status=="SUCCESS" {Ok(())} else {Err(format!("agy_sdk_native_{status}: {}",process.redact(value["error"].as_str().unwrap_or(status))))});
                        if turn.persistent {
                            if interrupted { return Err("agy_sdk_interrupted: 已中断当前 Agy 回合。".into()); }
                            return result.take().unwrap();
                        }
                    }
                    _=>{
                        let mut data=live.data.lock().unwrap();
                        let id=format!("agy:event:{}:{}",turn.id,data.seq);
                        data.items.push(json!({"id":id,"role":"system","content":event.to_string(),"native":event}));live.publish(&mut data);
                    }
                }
            }
        }
    }
    let status = native.finish(shutdown.clone()).await?;
    if interrupted {
        return Err(
            "agy_sdk_interrupted: 已向本桥接启动的 Agy 发送 SIGINT；下一轮恢复同一 SID。".into(),
        );
    }
    match result {
        Some(outcome) => outcome.and_then(|()| {
            if status.success() {
                Ok(())
            } else {
                Err(format!("agy_sdk_exit: {status}"))
            }
        }),
        None => Err(format!(
            "agy_sdk_no_result: {status}; 已接受输入不会自动重发。"
        )),
    }
    }.await;
    if turn.persistent
        && (outcome.is_ok()
            || outcome
                .as_ref()
                .is_err_and(|error| error.starts_with("agy_sdk_result_conflict:")))
    {
        return outcome;
    }
    if outcome.is_err() && !shutdown.is_cancelled() {
        native.terminate().await;
    }
    live.diagnostic(native.diagnostics().await);
    if !shutdown.is_cancelled() {
        *slot = None;
    }
    outcome
}

fn project(live: &Live, turn: &Turn, step: &Value) {
    let mut data = live.data.lock().unwrap();
    if step["usage"].is_object() {
        data.step_usage = step["usage"].clone();
    }
    let kind = step["step_type"].as_str().unwrap_or("unknown");
    let native_id = format!("agy:{}:step:{}", live.sid, step["step_index"]);
    if let Some(thinking) = step["transcript"]["thinking"]
        .as_str()
        .filter(|text| !text.is_empty())
    {
        let id = format!("{native_id}:thinking");
        if !data.items.iter().any(|item| item["id"] == id) {
            data.items.push(json!({"id":id,"role":"thinking","content":thinking,"request_id":turn.id,"native_type":"PLANNER_RESPONSE"}));
        }
    }
    // A completed step notification need not carry any visible body. Its
    // transcript is authoritative; metadata alone is not an assistant message.
    if matches!(kind, "agent_response" | "system_message")
        && !step["text"].as_str().is_some_and(|text| !text.is_empty())
        && !step["text_delta"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
        && !data.items.iter().any(|item| item["id"] == native_id)
    {
        live.publish(&mut data);
        return;
    }
    if kind == "user_input" {
        if let Some(item) = data
            .items
            .iter_mut()
            .find(|item| item["request_id"] == turn.id && item["role"] == "user")
        {
            item["native_id"] = json!(native_id);
            item["native"] = step.clone();
            item["status"] = json!("accepted");
        }
    } else {
        let index = if let Some(index) = data.items.iter().position(|item| item["id"] == native_id)
        {
            index
        } else {
            let role = match kind {
                "agent_response" => "assistant",
                "tool" => "tool",
                _ => "system",
            };
            data.items.push(json!({"id":native_id,"role":role,"content":"","native_type":kind,"request_id":turn.id}));
            data.items.len() - 1
        };
        let item = &mut data.items[index];
        let mut body = item["content"].as_str().unwrap_or("").to_owned();
        if let Some(text) = step["text"].as_str() {
            body = text.to_owned();
        } else if let Some(delta) = step["text_delta"].as_str() {
            body.push_str(delta);
        }
        item["content"] = json!(body);
        item["native"] = step.clone();
        item["status"] = step["state"].clone();
        if kind == "agent_response" {
            item["phase"] = json!(if step["state"] == "DONE" {
                "final"
            } else {
                "progress"
            });
        }
        if kind == "tool" {
            let tool = &step["tool_info"];
            if let Some(name) = tool["name"].as_str().or_else(|| step["tool_name"].as_str()) {
                item["name"] = json!(name);
            }
            if let Some(value) = tool.get("parameters") {
                item["input"] = value.clone();
            }
            if let Some(value) = tool.get("output") {
                item["output"] = value.clone();
                item["result"] = value.clone();
            }
            if let Some(value) = tool.get("error") {
                item["tool_error"] = value.clone();
                item["error"] = json!(!value.is_null());
                if !value.is_null() && tool.get("output").is_none() {
                    item["result"] = value.clone();
                }
            }
            item["content"] = json!(serde_json::to_string_pretty(&json!({"name":item["name"],"parameters":item["input"],"output":item["output"],"error":item["tool_error"]})).unwrap_or_default());
        } else if kind == "system_message" {
            item["native_type"] = json!("SYSTEM_MESSAGE");
            if let Some(fields) = step["system_fields"].as_object() {
                item.as_object_mut().unwrap().extend(fields.clone());
            }
        } else if kind != "agent_response" {
            item["content"] = json!(step.to_string());
        }
    }
    live.publish(&mut data);
}

fn host_key(sid: &str, request: &str) -> String {
    format!("agy:{sid}:{request}")
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
