use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sdkhost_client::{Client, Stream};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex as AsyncMutex, oneshot};
use tokio_util::sync::CancellationToken;

use super::live::{self, LiveState};

enum Transport {
    Host {
        client: Client,
        write_seq: AtomicU64,
    },
    Child {
        stdin: AsyncMutex<ChildStdin>,
        child: AsyncMutex<Child>,
    },
}

pub(crate) struct Session {
    uid: Mutex<String>,
    pub(super) session_id: Mutex<String>,
    pub(super) cwd: String,
    pub(super) pid: u32,
    transport: Transport,
    next_id: AtomicU64,
    pub(super) pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    pub(super) state: Mutex<LiveState>,
    pub(super) stderr: Mutex<VecDeque<String>>,
    pub(super) dead: AtomicBool,
    pub(super) prompt_starting: AtomicBool,
    pub(super) last_active: AtomicU64,
    stop_token: CancellationToken,
}

pub(super) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn sanitize_host_key(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 5);
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "kiro_default".to_string()
    } else {
        out
    }
}

pub(super) enum SpawnedSession {
    Fresh(Arc<Session>),
    Reused(Arc<Session>),
}

impl Session {
    pub(super) fn uid(&self) -> String {
        self.uid.lock().unwrap().clone()
    }

    pub(super) fn set_uid(&self, uid: &str) {
        *self.uid.lock().unwrap() = uid.to_string();
        let mut st = self.state.lock().unwrap();
        st.uid = uid.to_string();
    }

    pub(super) async fn spawn_or_attach(
        host_dir: Option<&Path>,
        host_key: Option<&str>,
        uid: &str,
        sid: &str,
        cwd: &str,
        command: Command,
        shutdown: CancellationToken,
    ) -> Result<SpawnedSession, String> {
        if let (Some(dir), Some(key)) = (host_dir, host_key)
            && cfg!(unix)
        {
            let key = sanitize_host_key(key);
            // Try attaching to an existing live sdkhost first
            if let Ok(client) = Client::attach(dir, &key).await {
                if let Ok(status) = client.status().await
                    && status.exit.is_none()
                {
                    let session = Self::from_host_client(
                        client,
                        status.pid,
                        status.stdout_len,
                        uid,
                        cwd,
                        shutdown,
                    );
                    let resolved_sid = if sid.is_empty() {
                        uid.strip_prefix("kiro:").unwrap_or(uid).to_string()
                    } else {
                        sid.to_string()
                    };
                    *session.session_id.lock().unwrap() = resolved_sid.clone();
                    {
                        let mut st = session.state.lock().unwrap();
                        st.session_id = resolved_sid;
                    }
                    return Ok(SpawnedSession::Reused(session));
                } else {
                    let _ = client.remove().await;
                }
            }

            // Try launching a new sdkhost instance; if sdkhost binary is unavailable, fall back to direct child spawn
            match Client::launch_or_attach(dir, &key, &command).await {
                Ok(client) => {
                    let pid = client.status().await.map(|s| s.pid).unwrap_or(0);
                    let session = Self::from_host_client(client, pid, 0, uid, cwd, shutdown);
                    return Ok(SpawnedSession::Fresh(session));
                }
                Err(_) => {
                    // Fall through to direct child spawn below
                }
            }
        }

        Self::spawn_child(uid, cwd, command, shutdown).await
    }

    fn from_host_client(
        client: Client,
        pid: u32,
        initial_offset: u64,
        uid: &str,
        cwd: &str,
        shutdown: CancellationToken,
    ) -> Arc<Self> {
        let stop_token = CancellationToken::new();
        let mut live_state = LiveState::new();
        live_state.uid = uid.to_string();
        live_state.cwd = cwd.to_string();
        let session = Arc::new(Self {
            uid: Mutex::new(uid.to_string()),
            session_id: Mutex::new(String::new()),
            cwd: cwd.to_string(),
            pid,
            transport: Transport::Host {
                client: client.clone(),
                write_seq: AtomicU64::new(1),
            },
            next_id: AtomicU64::new(100),
            pending: Mutex::new(HashMap::new()),
            state: Mutex::new(live_state),
            stderr: Mutex::new(VecDeque::with_capacity(24)),
            dead: AtomicBool::new(false),
            prompt_starting: AtomicBool::new(false),
            last_active: AtomicU64::new(now_secs()),
            stop_token: stop_token.clone(),
        });

        let s = session.clone();
        tokio::spawn(async move {
            let mut stdout_offset = initial_offset;
            let mut stderr_offset = 0u64;
            let mut stdout_buf = Vec::<u8>::new();
            let mut stderr_buf = Vec::<u8>::new();

            loop {
                if shutdown.is_cancelled() || stop_token.is_cancelled() {
                    break;
                }

                if let Ok(out) = client.read(Stream::Stdout, stdout_offset).await {
                    stdout_offset = out.next_offset;
                    if !out.bytes.is_empty() {
                        stdout_buf.extend_from_slice(&out.bytes);
                        while let Some(pos) = stdout_buf.iter().position(|&b| b == b'\n') {
                            let line_bytes: Vec<u8> = stdout_buf.drain(..=pos).collect();
                            let line = String::from_utf8_lossy(&line_bytes);
                            live::handle_line(&s, &line);
                        }
                    }
                    if out.eof && !stdout_buf.is_empty() {
                        let line = String::from_utf8_lossy(&stdout_buf).to_string();
                        stdout_buf.clear();
                        live::handle_line(&s, &line);
                    }
                }

                if let Ok(err_out) = client.read(Stream::Stderr, stderr_offset).await {
                    stderr_offset = err_out.next_offset;
                    if !err_out.bytes.is_empty() {
                        stderr_buf.extend_from_slice(&err_out.bytes);
                        while let Some(pos) = stderr_buf.iter().position(|&b| b == b'\n') {
                            let line_bytes: Vec<u8> = stderr_buf.drain(..=pos).collect();
                            let trimmed = String::from_utf8_lossy(&line_bytes).trim().to_string();
                            if !trimmed.is_empty() {
                                let mut guard = s.stderr.lock().unwrap();
                                if guard.len() >= 24 {
                                    guard.pop_front();
                                }
                                guard.push_back(trimmed);
                            }
                        }
                    }
                }

                match client.status().await {
                    Ok(status) => {
                        if let Some(exit) = status.exit
                            && stdout_offset >= status.stdout_len
                        {
                            s.dead.store(true, Ordering::Release);
                            let reason = format!(
                                "kiro process exited (code={:?}, signal={:?})",
                                exit.code, exit.signal
                            );
                            live::mark_exit(&s, &reason);
                            break;
                        }
                        if stdout_offset < status.stdout_len {
                            continue;
                        }
                    }
                    Err(_) => {
                        s.dead.store(true, Ordering::Release);
                        live::mark_exit(&s, "kiro sdkhost disconnected");
                        break;
                    }
                }

                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = stop_token.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            }
        });

        session
    }

    async fn spawn_child(
        uid: &str,
        cwd: &str,
        mut command: Command,
        shutdown: CancellationToken,
    ) -> Result<SpawnedSession, String> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|e| format!("spawn kiro acp failed: {e}"))?;
        let pid = child.id().unwrap_or(0);
        let stdin = child.stdin.take().ok_or("missing stdin")?;
        let stdout = child.stdout.take().ok_or("missing stdout")?;
        let stderr = child.stderr.take().ok_or("missing stderr")?;

        let stop_token = CancellationToken::new();
        let mut live_state = LiveState::new();
        live_state.uid = uid.to_string();
        live_state.cwd = cwd.to_string();

        let session = Arc::new(Self {
            uid: Mutex::new(uid.to_string()),
            session_id: Mutex::new(String::new()),
            cwd: cwd.to_string(),
            pid,
            transport: Transport::Child {
                stdin: AsyncMutex::new(stdin),
                child: AsyncMutex::new(child),
            },
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            state: Mutex::new(live_state),
            stderr: Mutex::new(VecDeque::with_capacity(24)),
            dead: AtomicBool::new(false),
            prompt_starting: AtomicBool::new(false),
            last_active: AtomicU64::new(now_secs()),
            stop_token: stop_token.clone(),
        });

        let s_out = session.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                live::handle_line(&s_out, &line);
            }
            s_out.dead.store(true, Ordering::Release);
            let reason = s_out.exit_error("kiro acp process exited");
            live::mark_exit(&s_out, &reason);
        });

        let s_err = session.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim().to_string();
                if !trimmed.is_empty() {
                    let mut buf = s_err.stderr.lock().unwrap();
                    if buf.len() >= 24 {
                        buf.pop_front();
                    }
                    buf.push_back(trimmed);
                }
            }
        });

        let s_kill = session.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    s_kill.kill().await;
                }
                _ = stop_token.cancelled() => {}
            }
        });

        Ok(SpawnedSession::Fresh(session))
    }

    pub(super) fn touch(&self) {
        self.last_active.store(now_secs(), Ordering::Release);
    }

    pub(super) async fn send_line(&self, line: &str) -> Result<(), String> {
        self.touch();
        let mut bytes = line.as_bytes().to_vec();
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        match &self.transport {
            Transport::Host { client, write_seq } => {
                let id = format!(
                    "w-{}-{}",
                    self.pid,
                    write_seq.fetch_add(1, Ordering::Relaxed)
                );
                let receipt = client
                    .write(&id, &bytes)
                    .await
                    .map_err(|e| format!("write to kiro sdkhost failed: {e}"))?;
                if receipt.state != "written" {
                    return Err(format!("kiro sdkhost write uncertain: {}", receipt.state));
                }
                Ok(())
            }
            Transport::Child { stdin, .. } => {
                let mut guard = stdin.lock().await;
                guard
                    .write_all(&bytes)
                    .await
                    .map_err(|e| format!("write to kiro acp failed: {e}"))?;
                guard
                    .flush()
                    .await
                    .map_err(|e| format!("flush kiro acp failed: {e}"))
            }
        }
    }

    pub(super) async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        if self.dead.load(Ordering::Acquire) {
            return Err(self.exit_error("kiro process is not running"));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(err) = self.send_line(&msg.to_string()).await {
            self.pending.lock().unwrap().remove(&id);
            return Err(err);
        }
        let timeout_secs = if matches!(method, "session/prompt" | "_kiro.dev/commands/execute") {
            600
        } else {
            60
        };
        match tokio::time::timeout(Duration::from_secs(timeout_secs), rx).await {
            Ok(Ok(res)) => res,
            Ok(Err(_)) => Err(self.exit_error("kiro acp channel closed")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("kiro acp request {method} timed out"))
            }
        }
    }

    pub(super) async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.send_line(&msg.to_string()).await
    }

    pub(super) async fn respond_rpc(&self, id: Value, result: Value) -> Result<(), String> {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        });
        self.send_line(&msg.to_string()).await
    }

    pub(super) fn exit_error(&self, prefix: &str) -> String {
        let tail: Vec<String> = self.stderr.lock().unwrap().iter().cloned().collect();
        if tail.is_empty() {
            prefix.to_string()
        } else {
            format!("{prefix}: {}", tail.join(" | "))
        }
    }

    pub(super) fn snapshot(&self) -> Value {
        let st = self.state.lock().unwrap();
        st.snapshot(&self.uid(), &self.cwd)
    }

    pub(super) async fn kill(&self) {
        self.dead.store(true, Ordering::Release);
        self.stop_token.cancel();
        match &self.transport {
            Transport::Host { client, .. } => {
                #[cfg(unix)]
                {
                    let _ = client.signal(libc::SIGKILL).await;
                }
                let _ = client.wait().await;
                let _ = client.remove().await;
            }
            Transport::Child { child, .. } => {
                let mut guard = child.lock().await;
                let _ = guard.kill().await;
            }
        }
    }
}
