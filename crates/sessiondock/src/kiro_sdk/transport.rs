//! Durable ACP interpretation lives here; sdkhost owns only the process and bytes.
use super::checkpoint::{self, Rpc};
use super::live::{self, LiveState};
use sdkhost_client::{Client, Stream};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex as AsyncMutex, Notify, oneshot};
use tokio_util::sync::CancellationToken;

enum Transport {
    Host {
        client: Client,
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
    pub(super) state: Mutex<LiveState>,
    pub(super) stderr: Mutex<VecDeque<String>>,
    pub(super) dead: AtomicBool,
    pub(super) unavailable: AtomicBool,
    pub(super) prompt_starting: AtomicBool,
    pub(super) last_active: AtomicU64,
    projection: Mutex<Option<Arc<crate::sessions::kiro::Catalog>>>,
    writer: AsyncMutex<()>,
    changed: Notify,
    done: CancellationToken,
    stop_token: CancellationToken,
}

pub(super) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub(super) fn sanitize_host_key(raw: &str) -> String {
    let key: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if key.is_empty() {
        "kiro_default".into()
    } else {
        key
    }
}
pub(super) enum SpawnedSession {
    Fresh(Arc<Session>),
    Reused(Arc<Session>),
}

fn response(value: &Value) -> Result<Value, String> {
    if let Some(error) = value.get("error") {
        Err(error["message"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| error.to_string()))
    } else {
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }
}

impl Session {
    pub(super) fn uid(&self) -> String {
        self.uid.lock().unwrap().clone()
    }
    /// Whether the native process lives in sdkhost and survives a service restart.
    pub(super) fn hosted(&self) -> bool {
        matches!(self.transport, Transport::Host { .. })
    }
    pub(super) fn bind_identity(&self, uid: &str, sid: &str) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        if !st.session_id.is_empty() && st.session_id != sid {
            return Err("kiro_sdk_identity_mismatch".into());
        }
        st.uid = uid.into();
        st.session_id = sid.into();
        st.persist()?;
        *self.uid.lock().unwrap() = uid.into();
        *self.session_id.lock().unwrap() = sid.into();
        st.publish();
        Ok(())
    }
    pub(super) fn set_projection(
        &self,
        catalog: Arc<crate::sessions::kiro::Catalog>,
    ) -> Result<(), String> {
        // Projecting the full transcript to disk is only needed the first time a
        // session's projection is established. Afterwards every live RPC/update
        // calls `project(&mut st)` (see the reader loop), so the message store
        // stays current. Re-projecting the entire transcript on every `open`
        // (reached through `finish_open_setup`) re-serialized and rewrote the
        // whole history for a warm session that had not changed — the dominant
        // cost of switching back to a Kiro session. Do it once; later opens just
        // keep the catalog reference.
        let already = self.projection.lock().unwrap().is_some();
        let st = self.state.lock().unwrap();
        if !already && !st.uid.is_empty() {
            catalog.project(&st.uid, &st.snapshot(&st.uid, &st.cwd, false)["raw"])?;
        }
        *self.projection.lock().unwrap() = Some(catalog);
        Ok(())
    }
    pub(super) fn project(&self, st: &mut LiveState) {
        if let Some(catalog) = self.projection.lock().unwrap().as_ref() {
            if !st.uid.is_empty() {
                if let Err(error) =
                    catalog.project(&st.uid, &st.snapshot(&st.uid, &st.cwd, false)["raw"])
                {
                    st.runtime_error = Some(format!("kiro_sdk_projection: {error}"));
                }
            }
        }
    }
    pub(super) fn creation_result(&self) -> Option<Result<Value, String>> {
        self.state
            .lock()
            .unwrap()
            .journal
            .iter()
            .find(|r| r.frame["method"] == "session/new")
            .and_then(|r| r.response.as_ref())
            .map(response)
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
        if let (Some(dir), Some(key)) = (host_dir, host_key) {
            let key = sanitize_host_key(key);
            let path = checkpoint::path(dir, &key);
            if path.exists() || Client::attach(dir, &key).await.is_ok() {
                return Self::attach_existing(dir, &key, uid, sid, cwd, shutdown)
                    .await
                    .map(SpawnedSession::Reused);
            }
            let mut state = LiveState::new();
            state.uid = uid.into();
            state.session_id = sid.into();
            state.cwd = cwd.into();
            state.checkpoint = Some(path);
            state.persist()?;
            let client = Client::launch_or_attach(dir, &key, &command)
                .await
                .map_err(|e| format!("kiro_sdk_host_launch: {e}"))?;
            return Self::from_host_client(client, state, shutdown)
                .await
                .map(SpawnedSession::Fresh);
        }
        Self::spawn_child(uid, sid, cwd, command, shutdown).await
    }

    pub(super) async fn attach_existing(
        dir: &Path,
        key: &str,
        uid: &str,
        sid: &str,
        cwd: &str,
        shutdown: CancellationToken,
    ) -> Result<Arc<Self>, String> {
        let path = checkpoint::path(dir, &sanitize_host_key(key));
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("kiro_sdk_recovery_unknown: checkpoint: {e}"))?;
        let mut state: LiveState = serde_json::from_slice(&bytes)
            .map_err(|e| format!("kiro_sdk_recovery_unknown: checkpoint: {e}"))?;
        if (!uid.is_empty() && !state.uid.is_empty() && uid != state.uid)
            || (!sid.is_empty() && !state.session_id.is_empty() && sid != state.session_id)
        {
            return Err("kiro_sdk_identity_mismatch".into());
        }
        if state.cwd.is_empty() {
            state.cwd = cwd.into();
        }
        let stdout = dir.join(sanitize_host_key(key)).join("stdout");
        let mut state = tokio::task::spawn_blocking(move || {
            state.upgrade_transcript(&stdout)?;
            Ok::<_, String>(state)
        })
        .await
        .map_err(|e| format!("kiro_sdk_projection_upgrade: {e}"))??;
        state.checkpoint = Some(path);
        let client = Client::attach(dir, &sanitize_host_key(key))
            .await
            .map_err(|e| format!("kiro_sdk_recovery_unknown: host: {e}"))?;
        Self::from_host_client(client, state, shutdown).await
    }

    fn make(state: LiveState, pid: u32, transport: Transport) -> Arc<Self> {
        Arc::new(Self {
            uid: Mutex::new(state.uid.clone()),
            session_id: Mutex::new(state.session_id.clone()),
            cwd: state.cwd.clone(),
            pid,
            transport,
            stderr: Mutex::new(state.stderr_tail.iter().cloned().collect()),
            state: Mutex::new(state),
            dead: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
            prompt_starting: AtomicBool::new(false),
            last_active: AtomicU64::new(now_secs()),
            projection: Mutex::new(None),
            writer: AsyncMutex::new(()),
            changed: Notify::new(),
            done: CancellationToken::new(),
            stop_token: CancellationToken::new(),
        })
    }

    async fn from_host_client(
        client: Client,
        state: LiveState,
        shutdown: CancellationToken,
    ) -> Result<Arc<Self>, String> {
        let status = client
            .status()
            .await
            .map_err(|e| format!("kiro_sdk_host_status: {e}"))?;
        let target = status.stdout_len;
        let session = Self::make(
            state,
            status.pid,
            Transport::Host {
                client: client.clone(),
            },
        );
        let (caught_tx, caught_rx) = oneshot::channel();
        let s = session.clone();
        tokio::spawn(async move {
            let mut caught = Some(caught_tx);
            let result = s.read_host(&client, target, &mut caught, shutdown).await;
            if let Err(error) = result {
                // Losing observation does not prove that the native process exited.
                s.unavailable.store(true, Ordering::Release);
                {
                    let mut st = s.state.lock().unwrap();
                    st.runtime_error = Some(error.clone());
                    st.publish();
                }
                if let Some(tx) = caught.take() {
                    let _ = tx.send(Err(error));
                }
            }
            s.done.cancel();
            s.changed.notify_waiters();
        });
        // Keep the actor and its ownership on an observation failure. Recovery
        // callers see unavailable evidence instead of silently replacing it.
        let _ = caught_rx.await;
        // Reattachment must not revive a timer from historical failed output.
        {
            let mut st = session.state.lock().unwrap();
            st.model_retry = Default::default();
            st.retry = None;
            st.publish();
        }
        // A crash before host.write is safe to resume only with the original byte ID.
        let queued: Vec<String> = session
            .state
            .lock()
            .unwrap()
            .journal
            .iter()
            .filter(|r| r.delivery.as_deref() != Some("written") && r.response.is_none())
            .map(|r| r.key.clone())
            .collect();
        for key in queued {
            let _ = session.deliver(&key).await;
        }
        Ok(session)
    }

    async fn read_host(
        self: &Arc<Self>,
        client: &Client,
        target: u64,
        caught: &mut Option<oneshot::Sender<Result<(), String>>>,
        shutdown: CancellationToken,
    ) -> Result<(), String> {
        let (mut out_pos, mut err_pos) = {
            let st = self.state.lock().unwrap();
            (st.stdout_offset, st.stderr_offset)
        };
        let mut out_buffer = Vec::new();
        let mut err_buffer = Vec::new();
        loop {
            if shutdown.is_cancelled() {
                return Ok(());
            } // detach; never stop the hosted process
            let out = client
                .read(Stream::Stdout, out_pos)
                .await
                .map_err(|e| format!("kiro_sdk_stdout: {e}"))?;
            out_pos = out.next_offset;
            out_buffer.extend_from_slice(&out.bytes);
            // Commit all complete frames from this host read together. The durable
            // cursor never passes an incomplete frame, and no snapshot or RPC waiter
            // sees the batch before its state and cursor have reached disk.
            if let Some(end) = out_buffer.iter().rposition(|b| *b == b'\n') {
                let bytes: Vec<u8> = out_buffer.drain(..=end).collect();
                let committed = out_pos - out_buffer.len() as u64;
                self.observe_line(&String::from_utf8_lossy(&bytes), Some(committed))?;
            }
            if out.eof && !out_buffer.is_empty() {
                self.observe_line(&String::from_utf8_lossy(&out_buffer), Some(out_pos))?;
                out_buffer.clear();
            }
            let status = client
                .status()
                .await
                .map_err(|e| format!("kiro_sdk_host_disconnected: {e}"))?;
            // Empty host reads long-poll for 250 ms. Do not put that wait after
            // every stdout chunk while replaying history; stderr is independent.
            let mut err_eof = false;
            if err_pos < status.stderr_len || status.stderr_eof || status.output_error.is_some() {
                let err = client
                    .read(Stream::Stderr, err_pos)
                    .await
                    .map_err(|e| format!("kiro_sdk_stderr: {e}"))?;
                err_eof = err.eof;
                err_pos = err.next_offset;
                err_buffer.extend_from_slice(&err.bytes);
                while let Some(end) = err_buffer.iter().position(|b| *b == b'\n') {
                    let bytes: Vec<u8> = err_buffer.drain(..=end).collect();
                    self.record_stderr(
                        &String::from_utf8_lossy(&bytes),
                        err_pos - err_buffer.len() as u64,
                    )?;
                }
                if err.eof && !err_buffer.is_empty() {
                    self.record_stderr(&String::from_utf8_lossy(&err_buffer), err_pos)?;
                    err_buffer.clear();
                }
            }
            if let Some(exit) = status.exit.as_ref() {
                if out.eof
                    && err_eof
                    && out_pos >= status.stdout_len
                    && err_pos >= status.stderr_len
                {
                    self.dead.store(true, Ordering::Release);
                    live::mark_exit(
                        self,
                        &self.exit_error(&format!(
                            "kiro process exited (code={:?}, signal={:?})",
                            exit.code, exit.signal
                        )),
                    );
                    if let Some(tx) = caught.take() {
                        let _ = tx.send(Ok(()));
                    }
                    return Ok(());
                }
            }
            if out_pos >= target && out_buffer.is_empty() {
                if let Some(tx) = caught.take() {
                    let _ = tx.send(Ok(()));
                }
            }
            if out_pos < status.stdout_len {
                continue;
            }
            tokio::select! {
                _ = shutdown.cancelled() => return Ok(()),
                _ = tokio::time::sleep(Duration::from_millis(20)) => {},
            }
        }
    }

    fn record_stderr(&self, line: &str, cursor: u64) -> Result<(), String> {
        let text = line.trim();
        let mut st = self.state.lock().unwrap();
        if !text.is_empty() {
            st.stderr_tail.push(text.into());
            if st.stderr_tail.len() > 24 {
                st.stderr_tail.remove(0);
            }
        }
        st.stderr_offset = cursor;
        st.persist()?;
        *self.stderr.lock().unwrap() = st.stderr_tail.iter().cloned().collect();
        Ok(())
    }

    fn observe_line(self: &Arc<Self>, line: &str, cursor: Option<u64>) -> Result<(), String> {
        self.touch();
        let mut auto = Vec::new();
        let mut sid = None;
        {
            let mut st = self.state.lock().unwrap();
            st.observing = true;
            for line in line.lines() {
                if let Ok(value) = serde_json::from_str::<Value>(line.trim()) {
                    if value.get("method").is_none() {
                        if let Some(id) = value["id"].as_u64() {
                            if let Some(index) = st.journal.iter().position(|r| r.id == Some(id)) {
                                if st.journal[index].response.is_none() {
                                    let rpc = st.journal[index].clone();
                                    st.journal[index].response = Some(value.clone());
                                    match response(&value) {
                                        Ok(result) => {
                                            match rpc.frame["method"].as_str().unwrap_or("") {
                                                "initialize" => st.apply_initialize_result(&result),
                                                "session/new" | "session/load" => {
                                                    let resolved = result["sessionId"]
                                                        .as_str()
                                                        .or_else(|| {
                                                            rpc.frame["params"]["sessionId"]
                                                                .as_str()
                                                        })
                                                        .unwrap_or("");
                                                    if !resolved.is_empty() {
                                                        st.apply_open_result(resolved, &result);
                                                        sid = Some(resolved.to_string());
                                                    }
                                                    st.loading = false;
                                                }
                                                "session/prompt"
                                                    if st.active_prompt == Some(id) =>
                                                {
                                                    st.finish_turn(&result);
                                                    st.active_prompt = None;
                                                    st.requests.clear();
                                                }
                                                "session/set_config_option"
                                                | "session/set_model"
                                                | "session/set_mode" => {
                                                    if let Some(opts) =
                                                        result["configOptions"].as_array()
                                                    {
                                                        st.apply_config_options(opts);
                                                    }
                                                    st.apply_metadata(&result);
                                                }
                                                _ => {}
                                            }
                                        }
                                        Err(error) => {
                                            if rpc.frame["method"] == "session/prompt"
                                                && st.active_prompt == Some(id)
                                            {
                                                st.fail_turn(&error);
                                                st.schedule_model_retry(&value["error"]);
                                                st.active_prompt = None;
                                                st.requests.clear();
                                            } else {
                                                st.runtime_error = Some(error);
                                            }
                                            if rpc.frame["method"] == "session/load" {
                                                st.loading = false;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else if let Some(method) = value["method"].as_str() {
                        let params = value.get("params").cloned().unwrap_or(Value::Null);
                        if let Some(id) = value.get("id").cloned() {
                            if let Some(reply) =
                                st.handle_incoming_request(id.clone(), method, &params)
                            {
                                let frame = if reply.is_null() {
                                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})
                                } else {
                                    json!({"jsonrpc":"2.0","id":id,"result":reply})
                                };
                                let key = format!(
                                    "auto:{}:{}",
                                    st.next_request_id,
                                    serde_json::to_string(&id).unwrap_or_default()
                                );
                                Self::enqueue(&mut st, &key, frame, None, None)?;
                                auto.push(key);
                            }
                        } else {
                            st.handle_notification(method, &params);
                        }
                    }
                }
            }
            if let Some(cursor) = cursor {
                st.stdout_offset = cursor;
            }
            st.observing = false;
            st.seq += 1;
            st.persist()?;
            self.project(&mut st);
            st.broadcast();
        }
        if let Some(sid) = sid {
            *self.session_id.lock().unwrap() = sid;
        }
        self.changed.notify_waiters();
        for key in auto {
            let s = self.clone();
            tokio::spawn(async move {
                let _ = s.deliver(&key).await;
            });
        }
        Ok(())
    }

    async fn spawn_child(
        uid: &str,
        sid: &str,
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
        let mut state = LiveState::new();
        state.uid = uid.into();
        state.session_id = sid.into();
        state.cwd = cwd.into();
        let session = Self::make(
            state,
            pid,
            Transport::Child {
                stdin: AsyncMutex::new(stdin),
                child: AsyncMutex::new(child),
            },
        );
        let s = session.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = s.record_stderr(&line, 0);
            }
        });
        let s = session.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if s.observe_line(&line, None).is_err() {
                    break;
                }
            }
            s.dead.store(true, Ordering::Release);
            live::mark_exit(&s, &s.exit_error("kiro acp process exited"));
            s.done.cancel();
            s.changed.notify_waiters();
        });
        let s = session.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = shutdown.cancelled() => { let _ = s.kill().await; },
                _ = s.stop_token.cancelled() => {},
            }
        });
        Ok(SpawnedSession::Fresh(session))
    }

    pub(super) fn touch(&self) {
        self.last_active.store(now_secs(), Ordering::Release);
    }
    fn enqueue(
        st: &mut LiveState,
        key: &str,
        frame: Value,
        turn: Option<u64>,
        request_key: Option<String>,
    ) -> Result<(), String> {
        if let Some(existing) = st.journal.iter().find(|r| r.key == key) {
            if existing.frame != frame {
                return Err("kiro_sdk_operation_conflict: operation payload changed".into());
            }
            return Ok(());
        }
        st.journal.push(Rpc {
            key: key.into(),
            id: frame["id"]
                .as_u64()
                .filter(|_| frame.get("method").is_some()),
            write_id: format!("kiro-{}", super::actions::generate_uuid_v4()),
            frame,
            delivery: None,
            response: None,
            turn,
            request_key,
        });
        Ok(())
    }

    async fn deliver(&self, key: &str) -> Result<(), String> {
        let _writer = self.writer.lock().await;
        let rpc = {
            let mut st = self.state.lock().unwrap();
            // A prior persistence failure can leave an in-memory entry. Never
            // let a retry send those bytes until the complete intent is durable.
            st.persist()?;
            st.journal
                .iter()
                .find(|rpc| rpc.key == key)
                .cloned()
                .ok_or("kiro_sdk_journal_missing")?
        };
        if rpc.response.is_some() || rpc.delivery.as_deref() == Some("written") {
            return Ok(());
        }

        let mut bytes = serde_json::to_vec(&rpc.frame).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        let result: Result<String, String> = match &self.transport {
            Transport::Host { client } => match client.receipt(&rpc.write_id).await {
                Ok(Some(receipt)) => Ok(receipt.state),
                Ok(None)
                    if rpc.delivery.is_none()
                        && !self.dead.load(Ordering::Acquire)
                        && !self.unavailable.load(Ordering::Acquire) =>
                {
                    client
                        .write(&rpc.write_id, &bytes)
                        .await
                        .map(|r| r.state)
                        .map_err(|e| e.to_string())
                }
                Ok(None) => Err("original write outcome unknown; will not resend".into()),
                Err(error) => Err(error.to_string()),
            },
            Transport::Child { stdin, .. } => {
                if rpc.delivery.is_some()
                    || self.dead.load(Ordering::Acquire)
                    || self.unavailable.load(Ordering::Acquire)
                {
                    return Err("kiro_sdk_write_unknown".into());
                }
                let mut stdin = stdin.lock().await;
                match stdin.write_all(&bytes).await {
                    Ok(()) => stdin
                        .flush()
                        .await
                        .map(|_| "written".into())
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                }
            }
        };
        let delivery = result.unwrap_or_else(|error| format!("unknown: {error}"));
        let mut st = self.state.lock().unwrap();
        if let Some(entry) = st.journal.iter_mut().find(|r| r.key == key) {
            entry.delivery = Some(delivery.clone());
        }
        if delivery == "written" {
            if let Some(request_key) = rpc.request_key {
                st.requests.remove(&request_key);
            }
        } else {
            st.runtime_error = Some(format!("kiro_sdk_write_uncertain: {delivery}"));
        }
        st.seq += 1;
        st.persist()?;
        st.broadcast();
        self.changed.notify_waiters();
        if delivery == "written"
            || st
                .journal
                .iter()
                .any(|r| r.key == key && r.response.is_some())
        {
            Ok(())
        } else {
            Err(format!("kiro_sdk_write_uncertain: {delivery}"))
        }
    }

    pub(super) async fn request_once(
        &self,
        key: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        {
            let mut st = self.state.lock().unwrap();
            if let Some(existing) = st.journal.iter().find(|r| r.key == key) {
                if existing.frame["method"] != method || existing.frame["params"] != params {
                    return Err("kiro_sdk_operation_conflict".into());
                }
            } else {
                let id = st.next_rpc;
                st.next_rpc += 1;
                let frame = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
                Self::enqueue(&mut st, key, frame, None, None)?;
                if method == "session/load" {
                    st.loading = true;
                }
                st.persist()?;
            }
        }
        self.deliver(key).await?;
        let wait = async {
            loop {
                let changed = self.changed.notified();
                if let Some(reply) = self
                    .state
                    .lock()
                    .unwrap()
                    .journal
                    .iter()
                    .find(|r| r.key == key)
                    .and_then(|r| r.response.clone())
                {
                    return response(&reply);
                }
                if self.dead.load(Ordering::Acquire) || self.unavailable.load(Ordering::Acquire) {
                    return Err(self.exit_error("kiro acp transport unavailable"));
                }
                changed.await;
            }
        };
        tokio::time::timeout(
            Duration::from_secs(if method == "session/prompt" { 600 } else { 60 }),
            wait,
        )
        .await
        .map_err(|_| format!("kiro acp request {method} timed out; original operation retained"))?
    }
    pub(super) async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.request_once(
            &format!("rpc-{}", super::actions::generate_uuid_v4()),
            method,
            params,
        )
        .await
    }
    pub(super) async fn begin_prompt(
        &self,
        params: Value,
        text: &str,
        delivery_id: Option<&str>,
    ) -> Result<Value, String> {
        let key = delivery_id
            .map(|id| format!("prompt:{id}"))
            .unwrap_or_else(|| format!("prompt:{}", super::actions::generate_uuid_v4()));
        {
            let mut st = self.state.lock().unwrap();
            if let Some(existing) = st.journal.iter().find(|r| r.key == key) {
                if existing.frame["params"] != params {
                    return Err("kiro_sdk_operation_conflict".into());
                }
            } else {
                if st.running {
                    return Err("kiro_sdk_session_busy".into());
                }
                if self.dead.load(Ordering::Acquire) || self.unavailable.load(Ordering::Acquire) {
                    return Err("kiro_sdk_transport_unavailable".into());
                }
                let id = st.next_rpc;
                st.next_rpc += 1;
                Self::enqueue(
                    &mut st,
                    &key,
                    json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":params}),
                    Some(id),
                    None,
                )?;
                st.observing = true;
                st.active_prompt = Some(id);
                st.model_retry = Default::default();
                st.begin_turn(text, delivery_id, None, None);
                st.observing = false;
                st.persist()?;
                st.broadcast();
            }
        }
        self.deliver(&key).await?;
        let mut snap = self.snapshot();
        snap["accepted"] = json!(true);
        snap["sdkhost"] = json!(self.hosted());
        if let Some(id) = delivery_id {
            snap["request_id"] = json!(id);
        }
        Ok(snap)
    }
    pub(super) async fn continue_model_retry(&self) -> Result<(), String> {
        let key;
        {
            let mut st = self.state.lock().unwrap();
            if !st
                .model_retry
                .deadline
                .is_some_and(|at| std::time::Instant::now() >= at)
                || st.running
                || st.loading
                || !st.requests.is_empty()
            {
                return Ok(());
            }
            if self.dead.load(Ordering::Acquire) || self.unavailable.load(Ordering::Acquire) {
                st.cancel_model_retry();
                st.publish();
                return Ok(());
            }
            let id = st.next_rpc;
            st.next_rpc += 1;
            key = format!("model-retry:{id}");
            let sid = st.session_id.clone();
            Self::enqueue(
                &mut st,
                &key,
                json!({"jsonrpc":"2.0","id":id,"method":"session/prompt",
                    "params":{"sessionId":sid,"prompt":[{"type":"text","text":super::retry::CONTINUE}]}}),
                Some(id),
                None,
            )?;
            st.model_retry.deadline = None;
            st.observing = true;
            st.active_prompt = Some(id);
            st.begin_turn(super::retry::CONTINUE, None, None, None);
            st.observing = false;
            st.persist()?;
            st.broadcast();
        }
        self.deliver(&key).await
    }

    pub(super) fn recover_delivery(&self, id: &str) -> Result<Value, String> {
        let st = self.state.lock().unwrap();
        let entry = st
            .journal
            .iter()
            .find(|r| r.key == format!("prompt:{id}"))
            .ok_or("kiro_sdk_recovery_unknown: no original prompt")?;
        if self.unavailable.load(Ordering::Acquire) && entry.response.is_none() {
            return Err("kiro_sdk_recovery_unknown: original host observation unavailable".into());
        }
        if entry.response.is_none() && entry.delivery.as_deref() != Some("written") {
            return Err("kiro_sdk_recovery_unknown: native delivery not confirmed".into());
        }
        let mut snap = st.snapshot(&st.uid, &st.cwd, true);
        snap["accepted"] = json!(true);
        snap["sdkhost"] = json!(self.hosted());
        snap["request_id"] = json!(id);
        Ok(snap)
    }
    pub(super) async fn cancel_turn(&self) -> Result<Value, String> {
        let key;
        {
            let mut st = self.state.lock().unwrap();
            let waiting = st.retry.is_some();
            st.cancel_model_retry();
            let Some(turn) = st.active_prompt else {
                if waiting {
                    st.error = None;
                    st.error_kind = Some("interrupted".into());
                    st.last_turn = Some(json!({"status":"interrupted"}));
                    st.publish();
                }
                return Ok(st.snapshot(&st.uid, &st.cwd, true));
            };
            key = format!("cancel:{turn}");
            let sid = st.session_id.clone();
            Self::enqueue(
                &mut st,
                &key,
                json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":sid}}),
                Some(turn),
                None,
            )?;
            st.observing = true;
            st.mark_interrupted();
            st.observing = false;
            st.persist()?;
            st.broadcast();
        }
        self.deliver(&key).await?;
        let mut snap = self.snapshot();
        snap["accepted"] = json!(true);
        Ok(snap)
    }
    pub(super) async fn respond_pending(&self, key: &str, result: Value) -> Result<(), String> {
        let operation = format!("answer:{key}");
        {
            let mut st = self.state.lock().unwrap();
            if !st.requests.contains_key(key) {
                if let Some(prior) = st.journal.iter().find(|rpc| rpc.key == operation) {
                    if st
                        .requests
                        .values()
                        .any(|pending| pending.rpc_id == prior.frame["id"])
                        || (st.active_prompt.is_some() && prior.turn != st.active_prompt)
                    {
                        return Err(
                            "kiro_sdk_request_expired: request belongs to an earlier turn".into(),
                        );
                    }
                }
            }
            if let Some(pending) = st.requests.get(key) {
                if key.starts_with("perm-") && result["outcome"]["outcome"] == "selected" {
                    let option = result["outcome"]["optionId"]
                        .as_str()
                        .ok_or("Kiro permission response requires a native optionId")?;
                    if !pending.card["options"].as_array().is_some_and(|options| {
                        options
                            .iter()
                            .any(|entry| entry["optionId"].as_str() == Some(option))
                    }) {
                        return Err(
                            "Kiro permission option was not offered by the native request".into(),
                        );
                    }
                }
            }
            let rpc_id = st
                .requests
                .get(key)
                .map(|r| r.rpc_id.clone())
                .or_else(|| {
                    st.journal
                        .iter()
                        .find(|r| r.key == operation)
                        .map(|r| r.frame["id"].clone())
                })
                .ok_or_else(|| format!("request {key} not found"))?;
            let turn = st.active_prompt;
            Self::enqueue(
                &mut st,
                &operation,
                json!({"jsonrpc":"2.0","id":rpc_id,"result":result}),
                turn,
                Some(key.into()),
            )?;
            st.persist()?;
        }
        self.deliver(&operation).await
    }
    pub(super) fn exit_error(&self, prefix: &str) -> String {
        let tail: Vec<_> = self.stderr.lock().unwrap().iter().cloned().collect();
        if tail.is_empty() {
            prefix.into()
        } else {
            format!("{prefix}: {}", tail.join(" | "))
        }
    }
    pub(super) fn snapshot(&self) -> Value {
        let st = self.state.lock().unwrap();
        // The public API/SSE shape: windowed items, subtask metadata only.
        st.snapshot(&st.uid, &st.cwd, true)
    }
    pub(super) async fn kill(self: &Arc<Self>) -> Result<(), String> {
        self.cancel_model_retry();
        if !self.dead.load(Ordering::Acquire) {
            match &self.transport {
                Transport::Host { client } => {
                    #[cfg(unix)]
                    client
                        .signal(libc::SIGKILL)
                        .await
                        .map_err(|e| format!("kiro_sdk_stop: {e}"))?;
                    client
                        .wait()
                        .await
                        .map_err(|e| format!("kiro_sdk_stop_wait: {e}"))?;
                }
                Transport::Child { child, .. } => child
                    .lock()
                    .await
                    .kill()
                    .await
                    .map_err(|e| format!("kiro_sdk_stop: {e}"))?,
            }
        }
        if self.unavailable.load(Ordering::Acquire) {
            if let Transport::Host { client } = &self.transport {
                // A stopped reader must still drain the exact host after stop.
                let mut caught = None;
                self.read_host(client, 0, &mut caught, CancellationToken::new())
                    .await?;
                self.unavailable.store(false, Ordering::Release);
            }
        }
        tokio::time::timeout(Duration::from_secs(15), self.done.cancelled())
            .await
            .map_err(|_| "kiro_sdk_stop_drain_timeout".to_string())?;
        self.stop_token.cancel();
        Ok(())
    }
}
