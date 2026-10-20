//! Native ACP NDJSON transport. Only processes spawned here are terminated.
use super::super::live::Live;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

type Reply = Result<Value, String>;
pub(crate) struct Connection {
    outgoing: mpsc::UnboundedSender<Value>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    next: AtomicU64,
    pub alive: Arc<AtomicBool>,
    pub stop: CancellationToken,
    pub done: CancellationToken,
    pub pid: u32,
    pub generation: u64,
    // Diagnostics never enter HTTP responses or logs. Keep redacted stderr for
    // local debugging without forwarding credential-bearing CLI error bodies.
    _diagnostics: Arc<Mutex<Vec<String>>>,
}

impl Connection {
    pub fn spawn(
        mut command: tokio::process::Command,
        live: Arc<Mutex<Live>>,
        stop: CancellationToken,
    ) -> Result<Arc<Self>, String> {
        let generation = {
            let mut state = live.lock().unwrap();
            if state.idle_reclaimed {
                state.items.clear();
                state.idle_reclaimed = false;
            }
            state.idle_since = None;
            state.native_tasks.clear();
            state.incomplete_tasks = false;
            state.generation += 1;
            // A resumed ACP process must report its actual native settings;
            // do not retain selectors from a runtime that has been released.
            state.config = Value::Null;
            state.models = Value::Null;
            state.modes = Value::Null;
            state.usage = Value::Null;
            state.context = Value::Null;
            state.extensions.clear();
            state.generation
        };
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("grok_sdk_spawn: {}", e.kind()))?;
        let pid = child.id().ok_or("grok_sdk_spawn: missing PID")?;
        let mut stdin = child.stdin.take().ok_or("grok_sdk_spawn: missing stdin")?;
        let mut stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or("grok_sdk_spawn: missing stdout")?,
        )
        .lines();
        let mut stderr = BufReader::new(
            child
                .stderr
                .take()
                .ok_or("grok_sdk_spawn: missing stderr")?,
        )
        .lines();
        let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
        let pending = Arc::new(Mutex::new(HashMap::<u64, oneshot::Sender<Reply>>::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let done = CancellationToken::new();
        let connection = Arc::new(Self {
            outgoing: tx.clone(),
            pending: pending.clone(),
            next: AtomicU64::new(1),
            alive: alive.clone(),
            stop: stop.clone(),
            done: done.clone(),
            pid,
            generation,
            _diagnostics: diagnostics.clone(),
        });
        tokio::spawn(async move {
            let mut stderr_open = true;
            let reason = loop {
                tokio::select! {
                    _ = stop.cancelled() => break "grok_sdk_shutdown",
                    line = stderr.next_line(), if stderr_open => match line {
                        Ok(Some(line)) => diagnostics.lock().unwrap().push(redact(&line)),
                        _ => stderr_open = false,
                    },
                    line = stdout.next_line() => match line {
                        Ok(Some(line)) => {
                            let Ok(message) = serde_json::from_str::<Value>(&line) else { break "grok_sdk_invalid_protocol"; };
                            if message.get("method").is_some() {
                                let response = {
                                    let mut state = live.lock().unwrap();
                                    if state.generation == generation { state.receive(&message) } else { None }
                                };
                                if let Some(response) = response { let _ = tx.send(response); }
                            } else if let Some(id) = message["id"].as_u64()
                                && let Some(reply) = pending.lock().unwrap().remove(&id) {
                                    let result = if message.get("error").is_some() {
                                        // Native text/data can contain provider headers or credentials.
                                        // Keep its diagnostic after redaction; expose the numeric code.
                                        diagnostics.lock().unwrap().push(redact(&message["error"].to_string()));
                                        if message["error"]["message"] == "Authentication required" {
                                            Err("grok_sdk_auth_required: Grok 原生认证不可用；请先在 CLI 配置认证，本桥接不启动登录流程".into())
                                        } else {
                                            Err(format!("grok_sdk_native_error: code {}", message["error"]["code"]))
                                        }
                                    } else { Ok(message["result"].clone()) };
                                    let _ = reply.send(result);
                            }
                        }
                        Ok(None) => break "grok_sdk_disconnected",
                        Err(_) => break "grok_sdk_read_failed",
                    },
                    message = rx.recv() => {
                        let Some(message) = message else { break "grok_sdk_closed"; };
                        let mut bytes = message.to_string().into_bytes(); bytes.push(b'\n');
                        if stdin.write_all(&bytes).await.is_err() { break "grok_sdk_write_failed"; }
                    }
                }
            };
            alive.store(false, Ordering::Release);
            diagnostics.lock().unwrap().push(format!(
                "exit: {:?}",
                child.try_wait().map(|s| s.and_then(|s| s.code()))
            ));
            let _ = child.start_kill();
            let _ = child.wait().await;
            done.cancel();
            for (_, reply) in pending.lock().unwrap().drain() {
                let _ = reply.send(Err(reason.into()));
            }
            let mut state = live.lock().unwrap();
            if state.generation != generation {
                return;
            }
            state.running = false;
            state.prompt_started = false;
            state.requests.clear();
            for receipt in state.steers.values_mut() {
                if receipt["generation"] == generation && receipt["status"] != "rejected" {
                    receipt["status"] = json!("connection_closed");
                    receipt["consumed"] = Value::Null;
                }
            }
            if state.idle_reclaimed {
                state.error = None;
                state.error_kind = None;
                state.publish();
                return;
            }
            state.error = Some(reason.into());
            state.error_kind = Some("transport");
            let sid = state.sid.clone();
            state.items.push(json!({"id":format!("grok:{sid}:transport:{generation}"),"role":"system",
                "content":format!("{reason}；连接结束不会重放已接受的输入。运行中的原生请求可能已产生结果，请查看原生历史。"),"error":true}));
            state.publish();
        });
        Ok(connection)
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.lock().unwrap().is_empty()
    }

    pub fn send(&self, message: Value) -> Result<(), String> {
        if !self.alive.load(Ordering::Acquire) {
            return Err("grok_sdk_disconnected".into());
        }
        self.outgoing
            .send(message)
            .map_err(|_| "grok_sdk_disconnected".into())
    }
    pub fn begin_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<oneshot::Receiver<Result<Value, String>>, String> {
        // ACP reserves leading-underscore methods for extensions; the Grok
        // handler API strips that prefix before matching logical x.ai names.
        let method = if method.starts_with("x.ai/") {
            format!("_{method}")
        } else {
            method.to_owned()
        };
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if let Err(error) =
            self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
        {
            self.pending.lock().unwrap().remove(&id);
            return Err(error);
        }
        Ok(rx)
    }
}

fn redact(line: &str) -> String {
    // Do not try to enumerate every credential spelling in arbitrary stderr.
    // Preserve diagnostic category/location; suppress values on sensitive lines.
    let lower = line.to_ascii_lowercase();
    if [
        "token",
        "secret",
        "password",
        "authorization",
        "api_key",
        "api-key",
        "bearer",
        "credential",
        "sk-",
        "xai-",
        "key=",
    ]
    .iter()
    .any(|word| lower.contains(word))
    {
        "[credential-bearing diagnostic redacted]".into()
    } else {
        line.to_owned()
    }
}
