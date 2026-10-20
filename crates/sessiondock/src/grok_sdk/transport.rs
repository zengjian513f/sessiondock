//! ACP interpretation remains in the web service; sdkhost only owns byte pipes.
#[path = "legacy_transport.rs"]
mod legacy;
use super::{fresh_id, live::Live};
use sdkhost_client::{Client, Stream};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;
type Reply = Result<Value, String>;
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Entry {
    id: String,
    frame: Value,
    #[serde(default)]
    accepted: Option<Value>,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Journal {
    next: u64,
    entries: Vec<Entry>,
}
pub(super) fn save(path: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let result = (|| -> io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        serde_json::to_writer(&mut file, value)?;
        file.flush()?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
pub(super) fn private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub(super) struct Connection {
    legacy: Option<Arc<legacy::Connection>>,
    journal: Mutex<Journal>,
    journal_path: PathBuf,
    wake: Notify,
    pending: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    live: Arc<Mutex<Live>>,
    pub alive: Arc<AtomicBool>,
    pub stop: CancellationToken,
    pub done: CancellationToken,
    pub pid: u32,
    pub generation: u64,
    pub initialized: Mutex<Value>,
    pub resumed: bool,
}
impl Connection {
    pub fn hosted(&self) -> bool {
        self.legacy.is_none()
    }
    pub fn spawn_legacy(
        command: tokio::process::Command,
        live: Arc<Mutex<Live>>,
        shutdown: CancellationToken,
    ) -> Result<Arc<Self>, String> {
        let legacy = legacy::Connection::spawn(command, live.clone(), shutdown)?;
        Ok(Arc::new(Self {
            journal: Mutex::new(Journal {
                next: 1,
                entries: vec![],
            }),
            journal_path: PathBuf::new(),
            wake: Notify::new(),
            pending: Mutex::new(HashMap::new()),
            live,
            alive: legacy.alive.clone(),
            stop: legacy.stop.clone(),
            done: legacy.done.clone(),
            pid: legacy.pid,
            generation: legacy.generation,
            initialized: Mutex::new(Value::Null),
            resumed: false,
            legacy: Some(legacy),
        }))
    }

    pub async fn spawn(
        command: tokio::process::Command,
        live: Arc<Mutex<Live>>,
        shutdown: CancellationToken,
        root: &Path,
        checkpoint: PathBuf,
    ) -> Result<Arc<Self>, String> {
        let host_dir = root.join("sdkhost");
        private_dir(&host_dir).map_err(error)?;
        let existing = live.lock().unwrap().host_key.clone();
        let resumed = existing.is_some();
        let key = existing.unwrap_or_else(|| format!("grok-{}", fresh_id()));
        {
            let mut state = live.lock().unwrap();
            state.checkpoint = Some(checkpoint.clone());
            if resumed && state.setup_pending {
                state.setup_pending = false;
                state.running = false;
                state.prompt_started = false;
                state.error=Some("grok_sdk_setup_interrupted: service stopped before prompt dispatch; input was not sent".into());
                state.error_kind = Some("turn");
            }
            if !resumed {
                if state.idle_reclaimed {
                    state.items.clear();
                    state.idle_reclaimed = false;
                }
                state.idle_since = None;
                state.native_tasks.clear();
                state.incomplete_tasks = false;
                state.generation += 1;
                state.config = Value::Null;
                state.models = Value::Null;
                state.modes = Value::Null;
                state.usage = Value::Null;
                state.context = Value::Null;
                state.extensions.clear();
                state.initialized = Value::Null;
                state.host_key = Some(key.clone());
                state.stdout_offset = 0;
                state.completed_rpcs.clear();
            }
            state.save_checkpoint().map_err(error)?;
        }
        let client = if resumed {
            Client::attach(&host_dir, &key).await
        } else {
            Client::launch_or_attach(&host_dir, &key, &command).await
        }
        .map_err(error)?;
        let status = client.status().await.map_err(error)?;
        let journal_path = checkpoint.with_file_name(format!("{key}-transport.json"));
        let journal = match std::fs::read(&journal_path) {
            Ok(bytes) => serde_json::from_slice::<Journal>(&bytes).map_err(error)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound && !resumed => Journal {
                next: 1,
                entries: vec![],
            },
            Err(e) => return Err(error(e)),
        };
        save(&journal_path, &journal).map_err(error)?;
        // A crash between durable enqueue and checkpoint publication can lose
        // the receipt projection, but cannot lose the accepted prompt itself.
        {
            let mut state = live.lock().unwrap();
            for entry in &journal.entries {
                if let Some(accepted) = &entry.accepted {
                    let id = accepted["receipt"]["request_id"].as_str().unwrap_or("");
                    if !state.receipts.contains_key(id) {
                        state
                            .receipts
                            .insert(id.into(), accepted["receipt"].clone());
                        state.items.push(accepted["item"].clone());
                        state.turn = state.turn.max(accepted["turn"].as_u64().unwrap_or(0));
                        state.running = true;
                        state.prompt_started = true;
                        state.error = None;
                        state.error_kind = None;
                        state.setup_pending = false;
                    }
                }
            }
            state.save_checkpoint().map_err(error)?;
        }
        let (generation, initialized) = {
            let state = live.lock().unwrap();
            (state.generation, state.initialized.clone())
        };
        let connection = Arc::new(Self {
            legacy: None,
            journal: Mutex::new(journal),
            journal_path,
            wake: Notify::new(),
            pending: Mutex::new(HashMap::new()),
            live: live.clone(),
            alive: Arc::new(AtomicBool::new(true)),
            stop: CancellationToken::new(),
            done: CancellationToken::new(),
            pid: status.pid,
            generation,
            initialized: Mutex::new(initialized),
            resumed,
        });
        let (caught_up, wait) = oneshot::channel();
        tokio::spawn(run(connection.clone(), client, shutdown, caught_up));
        wait.await.map_err(error)??;
        Ok(connection)
    }
    pub fn has_pending(&self, state: &Live) -> bool {
        if let Some(legacy) = &self.legacy {
            return legacy.has_pending();
        }
        self.journal.lock().unwrap().entries.iter().any(|e| {
            e.frame.get("method").is_some()
                && e.frame["id"]
                    .as_u64()
                    .is_some_and(|id| !state.completed_rpcs.contains(&id))
        })
    }
    pub fn send(&self, frame: Value) -> Result<(), String> {
        if let Some(legacy) = &self.legacy {
            return legacy.send(frame);
        }
        if !self.alive.load(Ordering::Acquire) {
            return Err("grok_sdk_disconnected".into());
        }
        let mut journal = self.journal.lock().unwrap();
        journal.entries.push(Entry {
            id: fresh_id(),
            frame,
            accepted: None,
        });
        save(&self.journal_path, &*journal).map_err(error)?;
        self.wake.notify_one();
        Ok(())
    }
    pub async fn request(&self, method: &str, params: Value) -> Reply {
        self.begin_request(method, params)?
            .await
            .map_err(|_| "grok_sdk_disconnected".to_owned())?
    }
    pub fn begin_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<oneshot::Receiver<Reply>, String> {
        self.enqueue(method, params, None)
    }
    pub fn begin_prompt(
        &self,
        params: Value,
        accepted: Value,
    ) -> Result<oneshot::Receiver<Reply>, String> {
        self.enqueue("session/prompt", params, Some(accepted))
    }
    fn enqueue(
        &self,
        method: &str,
        params: Value,
        accepted: Option<Value>,
    ) -> Result<oneshot::Receiver<Reply>, String> {
        if let Some(legacy) = &self.legacy {
            return legacy.begin_request(method, params);
        }
        if !self.alive.load(Ordering::Acquire) {
            return Err("grok_sdk_disconnected".into());
        }
        let method = if method.starts_with("x.ai/") {
            format!("_{method}")
        } else {
            method.into()
        };
        let mut journal = self.journal.lock().unwrap();
        let id = journal.next;
        journal.next += 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        journal.entries.push(Entry {
            id: fresh_id(),
            frame: json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            accepted,
        });
        if let Err(e) = save(&self.journal_path, &*journal) {
            self.pending.lock().unwrap().remove(&id);
            return Err(error(e));
        }
        self.wake.notify_one();
        Ok(rx)
    }
    fn observe(&self, message: &Value, next: u64) -> Result<(), String> {
        let association = message["id"].as_u64().and_then(|id| {
            self.journal
                .lock()
                .unwrap()
                .entries
                .iter()
                .find(|e| e.frame["id"].as_u64() == Some(id) && e.frame.get("method").is_some())
                .cloned()
        });
        let mut state = self.live.lock().unwrap();
        state.stdout_offset = next;
        if message.get("method").is_some() {
            let is_chunk = matches!(
                message["params"]["update"]["sessionUpdate"].as_str(),
                Some("agent_message_chunk" | "agent_thought_chunk" | "user_message_chunk")
            );
            let prev_seq = state.seq;
            if let Some(response) = state.receive(message) {
                self.send(response)?;
            }
            if state.seq == prev_seq {
                if is_chunk {
                    state.save_checkpoint_throttled().map_err(error)?;
                } else {
                    state.save_checkpoint().map_err(error)?;
                }
            } else if let Some(err) = state.last_checkpoint_error.clone() {
                return Err(err);
            }
            return Ok(());
        }
        let Some(id) = message["id"].as_u64() else {
            state.save_checkpoint().map_err(error)?;
            return Ok(());
        };
        let result = if message.get("error").is_some() {
            if message["error"]["message"] == "Authentication required" {
                Err("grok_sdk_auth_required: Grok 原生认证不可用；请先在 CLI 配置认证，本桥接不启动登录流程".into())
            } else {
                Err(format!(
                    "grok_sdk_native_error: code {}",
                    message["error"]["code"]
                ))
            }
        } else {
            Ok(message["result"].clone())
        };
        if let Some(entry) = association {
            let method = entry.frame["method"].as_str().unwrap_or("");
            match method {
                "initialize" => {
                    if let Ok(result) = &result {
                        state.initialized = result.clone();
                        *self.initialized.lock().unwrap() = result.clone();
                    }
                }
                "session/load"
                | "session/new"
                | "session/set_config_option"
                | "session/set_mode" => {
                    if let Ok(result) = &result {
                        state.settings(result);
                    }
                }
                "session/prompt" => {
                    state.finish_prompt(&result);
                    state.running = false;
                    state.prompt_started = false;
                    state.requests.clear();
                    match &result {
                        Ok(result) => {
                            if let Some(usage) = result["_meta"].get("usage") {
                                state.usage["last_turn"] = super::live::scrub_usage(usage);
                            }
                        }
                        Err(error) => {
                            state.error = Some(error.clone());
                            state.error_kind = Some("turn");
                        }
                    }
                }
                "_x.ai/compact_conversation" | "_x.ai/rewind/execute" | "_x.ai/session/fork" => {
                    state.running = false;
                    state.prompt_started = false;
                    if result.is_ok() {
                        state.context = Value::Null;
                        if method == "_x.ai/rewind/execute" {
                            state.items.clear();
                        }
                    } else {
                        state.error = result.as_ref().err().cloned();
                        state.error_kind = Some("turn");
                    }
                }
                "_x.ai/interject" => {
                    if let Some(request_id) = entry.frame["params"]["interjectionId"].as_str()
                        && let Some(receipt) = state.steers.get_mut(request_id)
                    {
                        match &result {
                            Ok(result)
                                if result.get("result").unwrap_or(result)["status"] == "queued" =>
                            {
                                receipt["accepted"] = json!(true);
                                if receipt["native_received"] != true {
                                    receipt["status"] = json!("native_queued");
                                }
                            }
                            _ => {
                                receipt["accepted"] = Value::Null;
                                receipt["status"] = json!("uncertain");
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        state.completed_rpcs.insert(id);
        state.publish();
        if let Some(err) = state.last_checkpoint_error.clone() {
            return Err(err);
        }
        drop(state);
        if let Some(reply) = self.pending.lock().unwrap().remove(&id) {
            let _ = reply.send(result);
        }
        Ok(())
    }
}
fn error(error: impl std::fmt::Display) -> String {
    format!("grok_sdk_host: {error}")
}
async fn run(
    connection: Arc<Connection>,
    client: Client,
    shutdown: CancellationToken,
    caught_up: oneshot::Sender<Result<(), String>>,
) {
    let mut caught_up = Some(caught_up);
    let mut sent = 0;
    let mut read_offset = connection.live.lock().unwrap().stdout_offset;
    let mut buffer = Vec::new();
    let mut detached = false;
    let mut deliberate = false;
    let mut signal_sent = false;
    let outcome:Result<(),String>=async{loop{
        if shutdown.is_cancelled(){detached=true;return Ok(());}
        if connection.stop.is_cancelled()&&!signal_sent{
            #[cfg(unix)]
            {client.signal(libc::SIGKILL).await.map_err(error)?;}
            signal_sent=true;deliberate=true;
        }
        let entries=connection.journal.lock().unwrap().entries.clone();
        for entry in &entries[sent..]{
            connection.live.lock().unwrap().save_checkpoint().map_err(error)?;
            let mut bytes=serde_json::to_vec(&entry.frame).map_err(error)?;bytes.push(b'\n');
            let receipt=match client.receipt(&entry.id).await.map_err(error)?{Some(receipt)=>receipt,None=>client.write(&entry.id,&bytes).await.map_err(error)?};
            if receipt.state!="written"{return Err("grok_sdk_write_uncertain".into());}
            if entry.frame.get("method").is_none()&&entry.frame.get("id").is_some(){let mut state=connection.live.lock().unwrap();state.requests.retain(|_,request|request["rpc_id"]!=entry.frame["id"]);state.save_checkpoint().map_err(error)?;}
            sent+=1;
        }
        let output=client.read(Stream::Stdout,read_offset).await.map_err(error)?;read_offset=output.next_offset;buffer.extend_from_slice(&output.bytes);
        while let Some(length)=buffer.iter().position(|b|*b==b'\n').map(|p|p+1).or_else(||(output.eof&&!buffer.is_empty()).then_some(buffer.len())){
            let line:Vec<_>=buffer.drain(..length).collect();let next=read_offset-buffer.len()as u64;
            if line.iter().all(u8::is_ascii_whitespace){let mut state=connection.live.lock().unwrap();state.stdout_offset=next;state.save_checkpoint_throttled().map_err(error)?;continue;}
            let message:Value=serde_json::from_slice(&line).map_err(|_|"grok_sdk_invalid_protocol".to_owned())?;connection.observe(&message,next)?;
        }
        if output.eof{connection.live.lock().unwrap().save_checkpoint().map_err(error)?;}
        let status=client.status().await.map_err(error)?;
        if read_offset>=status.stdout_len && let Some(reply)=caught_up.take(){connection.live.lock().unwrap().save_checkpoint().map_err(error)?;let _=reply.send(Ok(()));}
        if output.eof && status.exit.is_some(){return Err("grok_sdk_disconnected".into());}
        if read_offset<status.stdout_len{continue;}
        tokio::select!{_=shutdown.cancelled()=>{detached=true;return Ok(());},_=connection.wake.notified()=>{},_=tokio::time::sleep(std::time::Duration::from_millis(20))=>{}}
    }}.await;
    if let Some(reply) = caught_up {
        let _ = reply.send(outcome.clone());
    }
    connection.alive.store(false, Ordering::Release);
    let exited = client.status().await.ok().is_some_and(|s| s.exit.is_some());
    let reason = outcome.err().unwrap_or_else(|| "grok_sdk_shutdown".into());
    for (_, reply) in connection.pending.lock().unwrap().drain() {
        let _ = reply.send(Err(reason.clone()));
    }
    let mut state = connection.live.lock().unwrap();
    if !detached {
        if exited || deliberate {
            state.host_key = None;
            state.running = false;
            state.prompt_started = false;
            state.requests.clear();
            state.native_tasks.clear();
            state.incomplete_tasks = false;
            let generation = state.generation;
            for receipt in state.steers.values_mut() {
                if receipt["generation"] == generation && receipt["status"] != "rejected" {
                    receipt["status"] = json!("connection_closed");
                    receipt["consumed"] = Value::Null;
                }
            }
        }
        if state.idle_reclaimed {
            state.error = None;
            state.error_kind = None;
        } else {
            state.runtime_error = Some(reason.clone());
            if state.last_turn.is_null() && state.error.is_none() {
                state.last_turn = json!({"status":"incomplete","error":reason});
                state.error = Some(reason);
                state.error_kind = Some("transport");
            }
        }
        state.publish();
    } else {
        let _ = state.save_checkpoint();
    }
    connection.done.cancel();
}
