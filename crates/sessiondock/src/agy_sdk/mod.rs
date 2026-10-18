//! Native Agy headless NDJSON bridge. No PTY, mirror writes or approval emulation.
//! See docs/agy-sdk.md for the verified protocol and coexistence boundary.
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
}

struct Data {
    seq: u64,
    items: Vec<Value>,
    receipts: HashMap<String, Value>,
    outstanding: usize,
    active: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    error: Option<String>,
    diagnostics: String,
}

#[derive(Clone)]
struct Turn {
    id: String,
    text: String,
    model: Option<String>,
    effort: Option<String>,
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
    let process = ProcessConfig::new(profile, home.clone(), config.proc_root.clone())?;
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
        "models":true,"model_per_turn":true,"shared_tui":false,
        "limitations":["Agy headless 由原生策略允许或软拒绝审批；stream-json 不支持 control_request/control_response。",
        "独立 TUI 与 SDK 没有共享运行时；原生数据库被外部进程打开时拒绝恢复/发送。",
        "每轮原生 headless 恢复同一 SID；实时快照仅保存在当前服务进程。"]})
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
            "open" => {
                let live = self.open(&target).await?;
                self.process.check_available(live.sid.clone()).await?;
                let data = live.data.lock().unwrap();
                Ok(json!({"session_id":live.sid,"cwd":live.cwd,"capabilities":capabilities(),"raw":live.raw(&data)}))
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
                    effort: choice(&input,"effort").or_else(|| data.effort.clone()) };
                let receipt = json!({"accepted":true,"request_id":request,"session_id":live.sid,"capabilities":capabilities()});
                live.commands.send(Command::Turn(turn)).map_err(|_| "agy_sdk_shutdown")?;
                data.items.push(json!({"id":format!("agy:request:{request}"),"role":"user","content":content,"request_id":request,"status":"queued"}));
                data.receipts.insert(request, receipt.clone());
                data.outstanding += 1;
                data.error = None;
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
        self.process.check_available(sid.clone()).await?;
        if !self.process.native_path(&sid).is_file() {
            return Err(
                "agy_sdk_native_missing: 原生 conversation 数据库不存在，不能从只读镜像恢复。"
                    .into(),
            );
        }
        self.install(key, sid, cwd, None, None)
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
            events,
            data: Mutex::new(Data {
                seq: 0,
                items: vec![],
                receipts: HashMap::new(),
                outstanding: 0,
                active: None,
                model,
                effort,
                error: None,
                diagnostics: String::new(),
            }),
        });
        sessions.native.insert(sid, live.clone());
        sessions.keys.insert(key, live.clone());
        tokio::spawn(actor(
            self.process.clone(),
            live.clone(),
            receiver,
            self.shutdown.clone(),
        ));
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
            self.shutdown.clone(),
        )
        .await?;
        let sid = native.sid.clone();
        let native_cwd = native.cwd.clone();
        let diagnostics = native.close(self.shutdown.clone()).await?;
        if !self.process.native_path(&sid).is_file() {
            return Err("agy_sdk_native_missing_after_create".into());
        }
        // Inventory's established Agy UID is rooted in the read-only mirror *directory*.
        // It is determined by this native SID, without creating any mirror files here.
        let path = self.mirror.join("cli").join(&sid);
        let uid = crate::sessions::uid_for("agy", &path);
        let live = self.install(uid.clone(), sid.clone(), native_cwd.clone(), model, effort)?;
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
    fn raw(&self, data: &Data) -> Value {
        json!({"session_id":self.sid,"cwd":self.cwd,"running":data.outstanding>0,
            "items":data.items,"requests":[],"model":data.model,"effort":data.effort,
            "error":data.error,"diagnostics":data.diagnostics,"capabilities":capabilities()})
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

async fn actor(
    process: Arc<ProcessConfig>,
    live: Arc<Live>,
    mut receiver: mpsc::UnboundedReceiver<Command>,
    shutdown: CancellationToken,
) {
    let mut pending = std::collections::VecDeque::new();
    loop {
        let command = if let Some(turn) = pending.pop_front() {
            Some(Command::Turn(turn))
        } else {
            tokio::select! { _=shutdown.cancelled()=>None, command=receiver.recv()=>command }
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
                    shutdown.clone(),
                )
                .await;
                let mut data = live.data.lock().unwrap();
                data.active = None;
                data.outstanding = data.outstanding.saturating_sub(1);
                if let Some(item) = data
                    .items
                    .iter_mut()
                    .find(|item| item["request_id"] == turn.id && item["role"] == "user")
                {
                    item["status"] = json!(if outcome.is_ok() {
                        "completed"
                    } else {
                        "failed"
                    });
                }
                if let Err(error) = outcome {
                    data.error = Some(error.clone());
                    data.items.push(json!({"id":format!("agy:error:{}",turn.id),"role":"system","content":error,"error":true}));
                }
                live.publish(&mut data);
            }
        }
        if shutdown.is_cancelled() {
            break;
        }
    }
    let mut data = live.data.lock().unwrap();
    if data.outstanding > 0 {
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
    shutdown: CancellationToken,
) -> Result<(), String> {
    process.check_available(live.sid.clone()).await?;
    let mut native = Native::start(
        process.clone(),
        Some(&live.sid),
        std::path::Path::new(&live.cwd),
        turn.model.as_deref(),
        turn.effort.as_deref(),
        shutdown.clone(),
    )
    .await?;
    let outcome=async {
    tokio::select! {
        _=shutdown.cancelled()=>{native.terminate().await;return Err("agy_sdk_shutdown".into());},
        sent=native.send(&turn.text)=>sent?,
    }
    // EOF means finish this turn and exit, rather than cancel. Next turn resumes the same SID.
    native.input.take();
    let mut result = None;
    let mut interrupted = false;
    let mut interrupt_deadline = None;
    let mut saw_assistant = false;
    loop {
        tokio::select! {
            _=shutdown.cancelled()=>{ native.terminate().await; return Err("agy_sdk_shutdown".into()); }
            _=async { match interrupt_deadline { Some(deadline)=>tokio::time::sleep_until(deadline).await, None=>std::future::pending().await } }=>{
                native.terminate().await;return Err("agy_sdk_interrupted: 原生 SIGINT 未及时退出，已结束本桥接进程。".into());
            }
            command=receiver.recv()=>match command {
                Some(Command::Turn(turn))=>pending.push_back(turn),
                Some(Command::Interrupt(reply))=>{
                    let answer=native.interrupt();
                    if answer.is_ok() { interrupted=true;interrupt_deadline=Some(tokio::time::Instant::now()+std::time::Duration::from_secs(5)); }
                    let _=reply.send(answer.map(|()|json!({"interrupted":true,"session_id":live.sid})));
                }
                None=>{ native.terminate().await;return Err("agy_sdk_shutdown".into()); }
            },
            line=native.output.next_line()=>{
                let line=match line { Ok(Some(line))=>line, Ok(None)=>break, Err(error)=>{ native.terminate().await;return Err(format!("agy_sdk_read: {error}")); } };
                let event:Value=match serde_json::from_str(&line) {Ok(event)=>event,Err(_)=>{native.terminate().await;return Err("agy_sdk_invalid_ndjson".into());}};
                if let Err(error)=native.check_event(&event) {native.terminate().await;return Err(error);}
                match event["event"].as_str() {
                    Some("step_update")=>{ saw_assistant |= event["step_update"]["step_type"]=="agent_response" && event["step_update"]["text_delta"].as_str().is_some_and(|s|!s.is_empty()); project(live,turn,&event["step_update"]); }
                    Some("result")=>{
                        let value=&event["result"];
                        if !saw_assistant && value["response"].as_str().is_some_and(|text|!text.is_empty()) {
                            let mut data=live.data.lock().unwrap();
                            data.items.push(json!({"id":format!("agy:result:{}",turn.id),"role":"assistant","content":value["response"],"phase":"final","native":value}));live.publish(&mut data);
                        }
                        let status=value["status"].as_str().unwrap_or("INVALID");
                        result=Some(if status=="SUCCESS" {Ok(())} else {Err(format!("agy_sdk_native_{status}: {}",process.redact(value["error"].as_str().unwrap_or(status))))});
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
    let status = native.finish(shutdown).await?;
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
    if outcome.is_err() {
        native.terminate().await;
    }
    live.diagnostic(native.diagnostics().await);
    outcome
}

fn project(live: &Live, turn: &Turn, step: &Value) {
    let mut data = live.data.lock().unwrap();
    let kind = step["step_type"].as_str().unwrap_or("unknown");
    let native_id = format!("agy:{}:step:{}", live.sid, step["step_index"]);
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
        if let Some(delta) = step["text_delta"].as_str() {
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
        } else if kind != "agent_response" {
            item["content"] = json!(step.to_string());
        }
    }
    live.publish(&mut data);
}
