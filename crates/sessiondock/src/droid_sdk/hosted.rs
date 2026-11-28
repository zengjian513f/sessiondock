//! Restartable stream transport. Only this adapter interprets Droid frames.
use super::*;
use crate::sdk_common::hosted::{
    DeltaBatch, Observation, Projection, Runtime, Stdout, Termination, uncertain_write,
};
use crate::sdk_common::private_dir;
use sdkhost_client::Client;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::sdk_common::outbox::{Entry, Sender};
fn frame_id(frame: &Value) -> Option<String> {
    frame["id"]
        .as_str()
        .map(|id| format!("{}:{id}", frame["type"].as_str().unwrap_or("frame")))
}
fn error(error: impl std::fmt::Display) -> String {
    format!("droid_sdk_host: {error}")
}

impl Bridge {
    pub(super) fn checkpoint_path(&self, session: &Session) -> Result<PathBuf, String> {
        let root = self
            .state_dir
            .as_ref()
            .ok_or("droid_sdk_host: state directory unavailable")?
            .join("sdk-adapters/droid");
        private_dir(&root).map_err(error)?;
        Ok(crate::sdk_common::checkpoint_file(
            &root,
            &session.uid,
            &session.sid,
        ))
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
            .ok_or("droid_sdk_recovery: request identity missing")?;
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
                    .ok_or("droid_sdk_recovery: input association unavailable; not replayed")?,
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
            let entry = entries.iter().find(|entry| {
                entry.frame["method"] == "droid.add_user_message"
                    && entry.frame["params"]["messageId"] == receipt["message_id"]
            });
            if active && entry.is_none() {
                let mut live = lock(&session.live);
                live.turns.retain(|turn| turn != id);
                live.error = Some(
                    "droid_sdk_write_uncertain: input was not durably enqueued; not replayed"
                        .into(),
                );
                live.error_kind = Some("transport");
                session.publish(&mut live);
                return Err(live.error.clone().unwrap());
            }
            let client = Client::attach(&self.state_dir.as_ref().unwrap().join("sdkhost"), &key)
                .await
                .map_err(error)?;
            self.attach_hosted(session, false).await?;
            if active && let Some(entry) = entry {
                let receipt = client.receipt(&entry.id).await.map_err(error)?;
                let state = receipt
                    .as_ref()
                    .map_or("missing", |receipt| receipt.state.as_str());
                if state != "written" {
                    return Err(uncertain_write("droid", state));
                }
            }
        } else if active {
            return Err("droid_sdk_recovery: runtime unavailable; not replayed".into());
        }
        let receipt = lock(&session.live)
            .accepted
            .get(id)
            .cloned()
            .unwrap_or(receipt);
        if receipt["native_accepted"] != true {
            return Err(
                "droid_sdk_write_uncertain: native admission not confirmed; input not replayed"
                    .into(),
            );
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
            let key = format!("droid-{}", uuid()?);
            {
                let mut live = lock(&session.live);
                live.host_key = Some(key.clone());
                live.stdout_offset = 0;
                live.ready = false;
                live.state_observed = false;
                live.native_running = false;
                live.replaying = session.path.is_file();
                // Reopening the channel must not erase the previous turn's
                // failure. The next explicit turn clears its own error state.
                live.background_tasks.clear();
                live.streaming_blocks.clear();
                live.control_requests.clear();
                live.save_checkpoint().map_err(error)?;
            }
            let mut command = crate::sdk_common::host::command(&self.profile);
            command.args([
                "exec",
                "--input-format",
                "stream-jsonrpc",
                "--output-format",
                "stream-jsonrpc",
            ]);

            command.current_dir(&session.cwd);
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
            identity: frame_id,
            source: "droid",
            entries: Arc::new(Mutex::new(entries)),
            path: outbox_path,
            wake: Arc::new(tokio::sync::Notify::new()),
            closed: Arc::new(AtomicBool::new(false)),
        };
        let stop = session.stop.child_token();
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
            let pending_initialize = lock(&session.live).control_requests.values().any(|r| {
                matches!(
                    r["method"].as_str(),
                    Some("droid.initialize_session" | "droid.load_session" | "droid.list_models")
                )
            });
            if pending_initialize {
                return tokio::time::timeout(Duration::from_secs(60), async {
                    loop {
                        {
                            let live = lock(&session.live);
                            if live.ready {
                                return Ok(());
                            }
                            if live.outbound.is_none() {
                                return Err("droid_sdk_disconnected".into());
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                })
                .await
                .map_err(|_| "droid_sdk_initialize_unknown".to_owned())?;
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
    let mut end = Termination::default();
    let outcome = async {
        let stdout_offset = lock(&session.live).stdout_offset;
        Runtime {
            client: &client,
            sender: &sender,
            shutdown: &shutdown,
            stop: &stop,
            secrets: &secrets,
        }
        .run(
            &mut SessionProjection {
                session: &session,
                deltas: DeltaBatch::new(),
            },
            stdout_offset,
            &mut caught_up,
            &mut end,
        )
        .await
    }
    .await;
    sender.closed.store(true, Ordering::Release);
    let mut live = lock(&session.live);
    live.outbound = None;
    live.pid = None;
    live.runtime_stop = None;
    live.runtime_done = None;
    if !end.detached && (end.retired || end.deliberate) {
        live.host_key = None;
        live.ready = false;
        if let Err(error) = &outcome {
            live.runtime_error = Some(error.clone());
            if live.last_turn.is_null() && live.error.is_none() {
                live.last_turn = json!({"status":"incomplete","error":error});
                live.error = Some(error.clone());
                live.error_kind = Some("transport");
            }
        } else if !end.deliberate && (!live.turns.is_empty() || live.native_running) {
            let error = "droid_sdk_disconnected: native CLI exited with unfinished work; accepted input is not replayed";
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
    if !end.detached
        && !end.retired
        && !end.deliberate
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
        .unwrap_or_else(|| "droid_sdk_disconnected: request outcome unknown".into());
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

struct SessionProjection<'a> {
    session: &'a Session,
    deltas: DeltaBatch,
}

impl Projection for SessionProjection<'_> {
    fn checkpoint(&self) -> io::Result<()> {
        lock(&self.session.live).save_checkpoint()
    }

    fn written(&self, frame: &Value) -> io::Result<()> {
        if frame["type"] == "response"
            && let Some(id) = frame["id"].as_str()
        {
            let mut live = lock(&self.session.live);
            live.requests.remove(id);
            live.save_checkpoint()?;
        }
        Ok(())
    }

    fn write_failed(&self, error: &str) {
        let mut live = lock(&self.session.live);
        live.runtime_error = Some(error.into());
        if live.error.is_none() && live.last_turn.is_null() {
            live.error = Some(error.into());
            live.error_kind = Some("transport");
        }
        for (_, reply) in std::mem::take(&mut live.controls) {
            let _ = reply.send(Err(error.into()));
        }
        self.session.publish(&mut live);
    }

    fn stdout(&mut self, batch: Stdout<'_>) -> Result<(), String> {
        let session = self.session;
        let mut offset_dirty = false;
        for (next, line) in batch.lines() {
            if line.iter().all(u8::is_ascii_whitespace) {
                lock(&session.live).stdout_offset = next;
                offset_dirty = true;
                continue;
            }
            let frame: Value = serde_json::from_slice(line)
                .map_err(|_| "droid_sdk: invalid native JSON-RPC output")?;
            let observation =
                protocol::observe_at(session, frame, Some(next), batch.received_at(next));
            offset_dirty = !matches!(observation, Observation::Published);
            self.deltas.record(observation);
            if let Some(error) = lock(&session.live).last_checkpoint_error.clone() {
                return Err(error);
            }
        }
        if self.deltas.due(&batch) {
            let mut live = lock(&session.live);
            if batch.eof {
                session.publish(&mut live);
            } else {
                session.publish_throttled(&mut live);
            }
            if let Some(error) = &live.last_checkpoint_error {
                return Err(error.clone());
            }
            self.deltas.record(Observation::Published);
        } else if offset_dirty {
            let mut live = lock(&session.live);
            if batch.eof {
                live.save_checkpoint().map_err(error)?;
            } else {
                live.save_checkpoint_throttled().map_err(error)?;
            }
        }
        Ok(())
    }

    fn stderr(&self, line: String) {
        let mut live = lock(&self.session.live);
        live.stderr.push_back(line);
        if live.stderr.len() > 100 {
            live.stderr.pop_front();
        }
    }

    fn last_diagnostic(&self) -> String {
        lock(&self.session.live)
            .stderr
            .back()
            .cloned()
            .unwrap_or_default()
    }
}
