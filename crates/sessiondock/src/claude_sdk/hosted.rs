//! Restartable stream transport. Only this adapter interprets Claude frames.
use super::*;
use sdkhost_client::{Client, Stream};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
pub(super) struct Sender {
    entries: Arc<Mutex<Vec<Entry>>>,
    path: PathBuf,
    wake: Arc<tokio::sync::Notify>,
    closed: Arc<AtomicBool>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Entry {
    id: String,
    frame: Value,
}
impl Sender {
    pub fn send(&self, frame: Value) -> Result<(), String> {
        if self.closed.load(Ordering::Acquire) {
            return Err("claude_sdk_disconnected".into());
        }
        let mut entries = lock(&self.entries);
        // Native IDs also make replayed automatic protocol replies idempotent.
        let native_id = frame["uuid"]
            .as_str()
            .or_else(|| frame["request_id"].as_str())
            .or_else(|| frame["response"]["request_id"].as_str());
        let id = match native_id {
            Some(id) => format!("{}:{id}", frame["type"].as_str().unwrap_or("frame")),
            None => uuid()?,
        };
        if entries.iter().any(|entry| entry.id == id) {
            return Ok(());
        }
        entries.push(Entry { id, frame });
        save_atomic(&self.path, &*entries).map_err(|e| format!("claude_sdk_outbox: {e}"))?;
        self.wake.notify_one();
        Ok(())
    }
}
fn private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}
fn error(error: impl std::fmt::Display) -> String {
    format!("claude_sdk_host: {error}")
}

impl Bridge {
    fn checkpoint_path(&self, session: &Session) -> Result<PathBuf, String> {
        let root = self
            .state_dir
            .as_ref()
            .ok_or("claude_sdk_host: state directory unavailable")?
            .join("sdk-adapters/claude");
        private_dir(&root).map_err(error)?;
        let identity = if session.uid.is_empty() {
            &session.sid
        } else {
            &session.uid
        };
        let key: String = identity
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(root.join(format!("{key}.json")))
    }
    pub(super) async fn reattach_hosted(
        self: &Arc<Self>,
        session: &Arc<Session>,
    ) -> Result<Value, String> {
        if self.state_dir.is_none() {
            return Ok(json!({"attached":false}));
        }
        if lock(&session.live).checkpoint.is_none() {
            let path = self.checkpoint_path(session)?;
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Ok(json!({"attached":false}));
                }
                Err(e) => return Err(error(e)),
            };
            let mut restored: Live = serde_json::from_slice(&bytes).map_err(error)?;
            restored.checkpoint = Some(path);
            *lock(&session.live) = restored;
        }
        let key = lock(&session.live).host_key.clone();
        let Some(key) = key else {
            return Ok(json!({"attached":false}));
        };
        Client::attach(&self.state_dir.as_ref().unwrap().join("sdkhost"), &key)
            .await
            .map_err(error)?;
        self.attach_hosted(session, false).await?;
        Ok(json!({"attached":lock(&session.live).host_key.is_some()}))
    }
    pub(super) async fn recover_hosted(
        self: &Arc<Self>,
        session: &Arc<Session>,
        input: &Value,
    ) -> Result<Value, String> {
        let id = input["request_id"]
            .as_str()
            .ok_or("claude_sdk_recovery: request identity missing")?;
        if lock(&session.live).checkpoint.is_none() {
            let path = self.checkpoint_path(session)?;
            let mut restored: Live =
                serde_json::from_slice(&std::fs::read(&path).map_err(error)?).map_err(error)?;
            restored.checkpoint = Some(path);
            *lock(&session.live) = restored;
        }
        let (receipt, active, key, path) = {
            let live = lock(&session.live);
            (
                live.accepted
                    .get(id)
                    .cloned()
                    .ok_or("claude_sdk_recovery: input association unavailable; not replayed")?,
                live.turns.iter().any(|turn| turn == id),
                live.host_key.clone(),
                live.checkpoint.clone().unwrap(),
            )
        };
        if let Some(key) = key {
            // The association is saved before enqueueing. A crash in that gap
            // must stay unknown rather than inventing a send or a running turn.
            let entries: Vec<Entry> =
                match std::fs::read(path.with_file_name(format!("{key}-outbox.json"))) {
                    Ok(bytes) => serde_json::from_slice(&bytes).map_err(error)?,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
                    Err(e) => return Err(error(e)),
                };
            if active
                && !entries.iter().any(|entry| {
                    entry.frame["type"] == "user" && entry.frame["uuid"] == receipt["message_id"]
                })
            {
                let mut live = lock(&session.live);
                live.turns.retain(|turn| turn != id);
                live.error = Some(
                    "claude_sdk_write_uncertain: input was not durably enqueued; not replayed"
                        .into(),
                );
                live.error_kind = Some("transport");
                session.publish(&mut live);
                return Err(live.error.clone().unwrap());
            }
            Client::attach(&self.state_dir.as_ref().unwrap().join("sdkhost"), &key)
                .await
                .map_err(error)?;
            self.attach_hosted(session, false).await?;
        } else if active {
            return Err("claude_sdk_recovery: runtime unavailable; not replayed".into());
        }
        Ok(receipt)
    }
    pub(super) async fn ensure_hosted(
        self: &Arc<Self>,
        session: &Arc<Session>,
    ) -> Result<(), String> {
        self.attach_hosted(session, true).await
    }
    async fn attach_hosted(
        self: &Arc<Self>,
        session: &Arc<Session>,
        allow_launch: bool,
    ) -> Result<(), String> {
        if lock(&session.live).outbound.is_some() {
            return Ok(());
        }
        self.native_home()?;
        let checkpoint = self.checkpoint_path(session)?;
        if lock(&session.live).checkpoint.is_none() {
            match std::fs::read(&checkpoint) {
                Ok(bytes) => {
                    let mut restored: Live = serde_json::from_slice(&bytes).map_err(error)?;
                    restored.checkpoint = Some(checkpoint.clone());
                    *lock(&session.live) = restored;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    lock(&session.live).checkpoint = Some(checkpoint.clone())
                }
                Err(e) => return Err(error(e)),
            }
        }
        let directory = self.state_dir.as_ref().unwrap().join("sdkhost");
        let existing_key = lock(&session.live).host_key.clone();
        let attached = match &existing_key {
            Some(key) => Some(Client::attach(&directory, key).await.map_err(error)?),
            None => None,
        };
        let resumed = attached.is_some();
        let client = if let Some(client) = attached {
            client
        } else {
            if !allow_launch {
                return Ok(());
            }
            self.check_conflict(session).await?;
            let key = format!("claude-{}", uuid()?);
            {
                let mut live = lock(&session.live);
                live.host_key = Some(key.clone());
                live.stdout_offset = 0;
                live.ready = false;
                live.state_observed = false;
                live.native_running = false;
                // Reopening the channel must not erase the previous turn's
                // failure. The next explicit turn clears its own error state.
                live.background_tasks.clear();
                live.streaming_blocks.clear();
                live.control_requests.clear();
                live.save_checkpoint().map_err(error)?;
            }
            let mut command = tokio::process::Command::new(&self.profile.executable);
            command.args(&self.profile.args).args([
                "--print",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--replay-user-messages",
                "--permission-prompt-tool=stdio",
            ]);
            if session.path.is_file() {
                command.arg(format!("--resume={}", session.path.display()));
            } else if let Some((source, at)) = &session.fork {
                command
                    .arg(format!("--resume={}", source.display()))
                    .arg("--fork-session")
                    .arg(format!("--session-id={}", session.sid))
                    .arg(format!("--resume-session-at={at}"));
            } else {
                command.arg(format!("--session-id={}", session.sid));
            }
            {
                let live = lock(&session.live);
                if let Some(model) = &live.model {
                    command.arg(format!("--model={model}"));
                }
                if let Some(effort) = &live.effort {
                    command.arg(format!("--effort={effort}"));
                }
            }
            for name in &self.profile.env_remove {
                command.env_remove(name);
            }
            command.envs(&self.profile.env);
            for name in launcher::DENIED_ENV {
                command.env_remove(name);
            }
            command
                .env("CLAUDE_CONFIG_DIR", self.native_home()?)
                .env("CLAUDE_CODE_SDK_READS_SESSION_STATE", "1")
                .current_dir(&session.cwd);
            Client::launch_or_attach(&directory, &key, &command)
                .await
                .map_err(error)?
        };
        let client = Arc::new(client);
        let status = client.status().await.map_err(error)?;
        let key = lock(&session.live).host_key.clone().unwrap();
        let outbox_path = checkpoint.with_file_name(format!("{key}-outbox.json"));
        let entries = match std::fs::read(&outbox_path) {
            Ok(bytes) => serde_json::from_slice::<Vec<Entry>>(&bytes).map_err(error)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => vec![],
            Err(e) => return Err(error(e)),
        };
        let sender = Sender {
            entries: Arc::new(Mutex::new(entries)),
            path: outbox_path,
            wake: Arc::new(tokio::sync::Notify::new()),
            closed: Arc::new(AtomicBool::new(false)),
        };
        let stop = CancellationToken::new();
        let (done, done_rx) = watch::channel(false);
        {
            let mut live = lock(&session.live);
            live.outbound = Some(transport::Outbound::Hosted(sender.clone()));
            live.pid = Some(status.pid);
            live.runtime_stop = Some(stop.clone());
            live.runtime_done = Some(done_rx);
        }
        // Catch up before returning open/recover: an old "idle" projection is not
        // evidence that the surviving process has finished its current turn.
        let (caught_up, wait_catchup) = oneshot::channel();
        tokio::spawn(run(
            client,
            session.clone(),
            sender,
            self.shutdown.clone(),
            stop,
            done,
            caught_up,
            transport::diagnostic_secrets(&self.profile),
        ));
        wait_catchup.await.map_err(error)??;
        protocol::restore_initial_requests(session);
        if lock(&session.live).outbound.is_none() {
            if allow_launch && lock(&session.live).host_key.is_none() {
                return Box::pin(self.attach_hosted(session, true)).await;
            }
            return Ok(());
        }
        if resumed {
            // A completed startup handshake is never repeated on the same CLI.
            if lock(&session.live).ready {
                return Ok(());
            }
            let pending_initialize = lock(&session.live)
                .control_requests
                .values()
                .any(|r| r["subtype"] == "initialize");
            if pending_initialize {
                return tokio::time::timeout(Duration::from_secs(60), async {
                    loop {
                        {
                            let live = lock(&session.live);
                            if live.ready {
                                return Ok(());
                            }
                            if live.outbound.is_none() {
                                return Err("claude_sdk_disconnected".into());
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                })
                .await
                .map_err(|_| "claude_sdk_initialize_unknown".to_owned())?;
            }
        }
        self.initialize_session(session).await
    }
}

// Keep the owned lifecycle handles explicit at this single task-spawn boundary.
#[allow(clippy::too_many_arguments)]
async fn run(
    client: Arc<Client>,
    session: Arc<Session>,
    sender: Sender,
    shutdown: CancellationToken,
    stop: CancellationToken,
    done: watch::Sender<bool>,
    caught_up: oneshot::Sender<Result<(), String>>,
    secrets: Vec<String>,
) {
    let mut caught_up = Some(caught_up);
    let mut sent = 0;
    let mut deliberate = false;
    let mut detached = false;
    let mut read_offset = lock(&session.live).stdout_offset;
    let mut buffer = Vec::new();
    let mut signal_sent = false;
    let mut stderr_offset = 0;
    let mut stderr_buffer = Vec::new();
    let mut write_error = None;
    let mut drained_exit = false;
    let outcome: Result<(), String> = async {
        loop {
            if shutdown.is_cancelled() {
                detached = true;
                return Ok(());
            }
            if !signal_sent && (stop.is_cancelled() || session.stop.is_cancelled()) {
                deliberate = true;
                client.signal(libc::SIGKILL).await.map_err(error)?;
                signal_sent = true;
            }
            // Entries are durable before acknowledgement; host receipts make a
            // repeated transport write harmless after a lost acknowledgement.
            let entries = lock(&sender.entries).clone();
            for entry in &entries[sent..] {
                if write_error.is_some() {
                    break;
                }
                // The submitter publishes its protocol association under this lock.
                lock(&session.live).save_checkpoint().map_err(error)?;
                let mut bytes = serde_json::to_vec(&entry.frame).map_err(error)?;
                bytes.push(b'\n');
                let written = async {
                    let receipt = match client.receipt(&entry.id).await.map_err(error)? {
                        Some(receipt) => receipt,
                        None => client.write(&entry.id, &bytes).await.map_err(error)?,
                    };
                    if receipt.state != "written" {
                        return Err(format!("claude_sdk_write_uncertain: {}", receipt.state));
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = written {
                    // A process may finish its turn and exit before a queued
                    // control write. Preserve its final output before reporting
                    // the write failure, and never retry the uncertain write.
                    sender.closed.store(true, Ordering::Release);
                    write_error = Some(error);
                    break;
                }
                if entry.frame["type"] == "control_response"
                    && let Some(id) = entry.frame["response"]["request_id"].as_str()
                {
                    let mut live = lock(&session.live);
                    if let Some(card) = live.requests.remove(id) {
                        protocol::retire_dialog(&mut live, &card);
                    }
                    live.save_checkpoint().map_err(error)?;
                }
                sent += 1;
            }
            let output = client
                .read(Stream::Stdout, read_offset)
                .await
                .map_err(error)?;
            read_offset = output.next_offset;
            buffer.extend_from_slice(&output.bytes);
            let mut offset_dirty = false;
            while let Some(length) = buffer
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|position| position + 1)
                .or_else(|| (output.eof && !buffer.is_empty()).then_some(buffer.len()))
            {
                let line: Vec<_> = buffer.drain(..length).collect();
                let next = read_offset - buffer.len() as u64;
                if line.iter().all(u8::is_ascii_whitespace) {
                    lock(&session.live).stdout_offset = next;
                    offset_dirty = true;
                    continue;
                }
                let frame: Value = serde_json::from_slice(&line)
                    .map_err(|_| "claude_sdk: invalid native stream-json output")?;
                let refresh = frame["type"] == "result"
                    || (frame["type"] == "system" && frame["subtype"] == "compact_boundary");
                let is_delta = frame["type"] == "stream_event"
                    && frame["event"]["type"] == "content_block_delta";
                protocol::observe_at(&session, frame, Some(next));
                if let Some(err) = lock(&session.live).last_checkpoint_error.clone() {
                    return Err(err);
                }
                offset_dirty = is_delta;
                if refresh {
                    session.schedule_usage();
                }
            }
            if offset_dirty {
                let mut live = lock(&session.live);
                if output.eof {
                    live.save_checkpoint().map_err(error)?;
                } else {
                    live.save_checkpoint_throttled().map_err(error)?;
                }
            }
            let diagnostic = client
                .read(Stream::Stderr, stderr_offset)
                .await
                .map_err(error)?;
            stderr_offset = diagnostic.next_offset;
            stderr_buffer.extend_from_slice(&diagnostic.bytes);
            while let Some(length) = stderr_buffer
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|position| position + 1)
                .or_else(|| {
                    (diagnostic.eof && !stderr_buffer.is_empty()).then_some(stderr_buffer.len())
                })
            {
                let line: Vec<_> = stderr_buffer.drain(..length).collect();
                let line = transport::redact(
                    String::from_utf8_lossy(&line).trim_end().to_owned(),
                    &secrets,
                );
                let mut live = lock(&session.live);
                live.stderr.push_back(line);
                while live.stderr.len() > 100 {
                    live.stderr.pop_front();
                }
            }
            let status = client.status().await.map_err(error)?;
            if read_offset >= status.stdout_len
                && !(status.stdout_eof && status.exit.is_some())
                && write_error.is_none()
                && let Some(reply) = caught_up.take()
            {
                lock(&session.live).save_checkpoint().map_err(error)?;
                let _ = reply.send(Ok(()));
            }
            if status.stdout_eof
                && read_offset >= status.stdout_len
                && buffer.is_empty()
                && status.stderr_eof
                && stderr_offset >= status.stderr_len
                && stderr_buffer.is_empty()
                && let Some(exit) = status.exit
            {
                drained_exit = true;
                if let Some(error) = &write_error {
                    return Err(error.clone());
                }
                if !deliberate && !exit.success() {
                    return Err(format!(
                        "claude_sdk_disconnected: native CLI exit code {:?}, signal {:?}; {}",
                        exit.code,
                        exit.signal,
                        lock(&session.live)
                            .stderr
                            .back()
                            .cloned()
                            .unwrap_or_default()
                    ));
                }
                return Ok(());
            }
            if read_offset < status.stdout_len || stderr_offset < status.stderr_len {
                continue;
            }
            if status.exit.is_none()
                && let Some(error) = &write_error
            {
                return Err(error.clone());
            }
            tokio::select! {
                _ = shutdown.cancelled() => { detached = true; return Ok(()); }
                _ = sender.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(25)) => {}
            }
        }
    }
    .await;
    sender.closed.store(true, Ordering::Release);
    // A child can exit between the read loop's status query and this one.
    // Keep the host association after a write failure until its output has
    // been drained, so a later attachment can still recover the final result.
    let exited = client.status().await.ok().is_some_and(|s| s.exit.is_some())
        && (write_error.is_none() || drained_exit);
    let mut live = lock(&session.live);
    live.outbound = None;
    live.pid = None;
    live.runtime_stop = None;
    live.runtime_done = None;
    if !detached && (exited || deliberate) {
        live.host_key = None;
        live.ready = false;
        if let Err(error) = &outcome {
            live.runtime_error = Some(error.clone());
            if live.last_turn.is_null() && live.error.is_none() {
                live.last_turn = json!({"status":"incomplete","error":error});
                live.error = Some(error.clone());
                live.error_kind = Some("transport");
            }
        } else if !deliberate && (!live.turns.is_empty() || live.native_running) {
            let error = "claude_sdk_disconnected: native CLI exited with unfinished work; accepted input is not replayed";
            live.runtime_error = Some(error.into());
            if live.last_turn.is_null() && live.error.is_none() {
                live.last_turn = json!({"status":"incomplete","error":error});
                live.error = Some(error.into());
                live.error_kind = Some("transport");
            }
        }
        live.turns.clear();
        live.native_running = false;
        live.requests.clear();
        live.background_tasks.clear();
    }
    if !detached
        && !exited
        && !deliberate
        && let Err(error) = &outcome
    {
        live.runtime_error = Some(error.clone());
        if live.last_turn.is_null() && live.error.is_none() {
            live.error = Some(error.clone());
            live.error_kind = Some("transport");
        }
    }
    let control_error = live
        .error
        .clone()
        .unwrap_or_else(|| "claude_sdk_disconnected: request outcome unknown".into());
    for (_, reply) in std::mem::take(&mut live.controls) {
        let _ = reply.send(Err(control_error.clone()));
    }
    session.publish(&mut live);
    drop(live);
    let _ = done.send(true);
    if let Some(reply) = caught_up {
        let _ = reply.send(outcome);
    }
}
