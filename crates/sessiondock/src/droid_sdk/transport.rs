use super::*;
use crate::sdk_common::write_input;
pub(super) use crate::sdk_common::{diagnostic_secrets, redact};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Clone)]
pub(super) enum Outbound {
    Direct(mpsc::UnboundedSender<Value>),
    Hosted(super::hosted::Sender),
}
impl Outbound {
    pub fn send(&self, frame: Value) -> Result<(), String> {
        match self {
            Self::Direct(sender) => sender
                .send(frame)
                .map_err(|_| "droid_sdk_disconnected".into()),
            Self::Hosted(sender) => sender.send(frame),
        }
    }
}

impl Bridge {
    pub(super) async fn ensure(self: &Arc<Self>, session: &Arc<Session>) -> Result<(), String> {
        let _admission = self.writer_admission.lock().await;
        if self.state_dir.is_some() {
            return self.ensure_hosted(session).await;
        }
        if lock(&session.live).ready {
            return Ok(());
        }
        self.native_home()?;
        self.check_conflict(session).await?;
        let needs_spawn = lock(&session.live).outbound.is_none();
        if needs_spawn {
            let mut command = tokio::process::Command::new(&self.profile.executable);
            command.args(&self.profile.args).args([
                "exec",
                "--input-format",
                "stream-jsonrpc",
                "--output-format",
                "stream-jsonrpc",
            ]);
            for name in &self.profile.env_remove {
                command.env_remove(name);
            }
            command.envs(&self.profile.env);
            for name in launcher::DENIED_ENV {
                command.env_remove(name);
            }
            command
                .current_dir(&session.cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            command.process_group(0);
            #[cfg(windows)]
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
            let mut child = command.spawn().map_err(|e| {
                format!(
                    "droid_sdk_unavailable: configured Droid CLI could not start ({})",
                    e.kind()
                )
            })?;
            let stdin = child.stdin.take().ok_or("droid_sdk: stdin unavailable")?;
            let stdout = child.stdout.take().ok_or("droid_sdk: stdout unavailable")?;
            let stderr = child.stderr.take().ok_or("droid_sdk: stderr unavailable")?;
            let pid = child.id();
            let failed = CancellationToken::new();
            let (done, done_rx) = watch::channel(false);
            let (tx, rx) = mpsc::unbounded_channel();
            {
                let mut live = lock(&session.live);
                live.outbound = Some(Outbound::Direct(tx));
                live.pid = pid;
                live.ready = false;
                live.state_observed = false;
                live.native_running = false;
                live.runtime_stop = Some(failed.clone());
                live.runtime_done = Some(done_rx);
                live.replaying = session.path.is_file();
                live.error = None;
                live.error_kind = None;
                live.background_tasks.clear();
                live.streaming_blocks.clear();
                live.stderr.clear();
            }
            let session = session.clone();
            let shutdown = self.shutdown.clone();
            let secrets = diagnostic_secrets(&self.profile);
            // This task owns the subprocess. No request future or SSE receiver
            // is its parent; accepted work survives either one's cancellation.
            tokio::spawn(async move {
                let writer = tokio::spawn(write_input(stdin, rx, failed.clone()));
                let reader_session = session.clone();
                let reader_failed = failed.clone();
                let reader = tokio::spawn(async move {
                    let mut lines = BufReader::new(stdout).lines();
                    loop {
                        match lines.next_line().await {
                            Ok(Some(line)) if line.trim().is_empty() => {}
                            Ok(Some(line)) => match parse_json(&line) {
                                Ok(frame) => {
                                    protocol::observe(&reader_session, frame);
                                }
                                Err(_) => {
                                    let mut live = lock(&reader_session.live);
                                    live.error =
                                        Some("droid_sdk: invalid native JSON-RPC output".into());
                                    live.error_kind = Some("transport");
                                    reader_session.publish(&mut live);
                                    reader_failed.cancel();
                                    break;
                                }
                            },
                            Ok(None) | Err(_) => {
                                reader_failed.cancel();
                                break;
                            }
                        }
                    }
                });
                let stderr_session = session.clone();
                let diagnostic = tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let mut live = lock(&stderr_session.live);
                        live.stderr.push_back(redact(line, &secrets));
                        while live.stderr.len() > 100 {
                            live.stderr.pop_front();
                        }
                    }
                });
                let mut deliberate = false;
                let mut status = tokio::select! {
                    result=child.wait()=>result.ok(),
                    _=shutdown.cancelled()=>{deliberate=true; None},
                    _=session.stop.cancelled()=>{deliberate=true; None},
                    _=failed.cancelled()=>None,
                };
                if status.is_none() && !deliberate {
                    // stdout EOF can race a normal process exit. Collect the
                    // real exit status before classifying it as a transport
                    // failure, while still bounding a broken pipe owner.
                    if let Ok(Ok(exit)) =
                        tokio::time::timeout(Duration::from_secs(1), child.wait()).await
                    {
                        status = Some(exit);
                    }
                }
                // Only the isolated process group that this bridge spawned.
                // The child has not been reaped on the signal paths, so its PID
                // cannot have been reused. No external TUI is ever signalled.
                if status.is_none() {
                    #[cfg(unix)]
                    if let Some(pid) = pid {
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                    }
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
                writer.abort();
                // Drain already delivered final result frames before declaring
                // exit. A descendant retaining a pipe cannot block shutdown.
                let mut reader = reader;
                if tokio::time::timeout(Duration::from_secs(1), &mut reader)
                    .await
                    .is_err()
                {
                    reader.abort();
                }
                let mut diagnostic = diagnostic;
                if tokio::time::timeout(Duration::from_secs(1), &mut diagnostic)
                    .await
                    .is_err()
                {
                    diagnostic.abort();
                }
                let mut live = lock(&session.live);
                let unfinished = !live.turns.is_empty();
                live.outbound = None;
                live.pid = None;
                live.ready = false;
                live.native_running = false;
                live.runtime_stop = None;
                live.runtime_done = None;
                live.background_tasks.clear();
                live.turns.clear();
                live.requests.clear();
                if !deliberate && (unfinished || status.is_none_or(|s| !s.success())) {
                    let diagnostic = live.stderr.back().cloned().unwrap_or_default();
                    let error = format!(
                        "droid_sdk_disconnected: native CLI exited; accepted input is not replayed. {diagnostic}"
                    );
                    live.runtime_error = Some(error.clone());
                    if live.last_turn.is_null() && live.error.is_none() {
                        live.last_turn = json!({"status":"incomplete","error":error});
                        live.error = Some(error);
                        live.error_kind = Some("transport");
                    }
                }
                // initialize is a control request too. Preserve the already
                // redacted exit diagnostic when its waiter reports failure;
                // otherwise ensure would replace it with a generic error.
                let control_error = live.error.clone().unwrap_or_else(|| {
                    "droid_sdk_disconnected: native CLI exited; request outcome unknown".into()
                });
                for (_, response) in std::mem::take(&mut live.controls) {
                    let _ = response.send(Err(control_error.clone()));
                }
                session.publish(&mut live);
                let _ = done.send(true);
            });
        }
        self.initialize_session(session).await
    }

    pub(super) async fn initialize_session(&self, session: &Arc<Session>) -> Result<(), String> {
        let initialized = if session.uid.is_empty() {
            session
                .control(json!({"method":"droid.list_models","params":{}}))
                .await
        } else if session.path.is_file() {
            session
                .control(json!({"method":"droid.load_session","params":{"sessionId":session.sid}}))
                .await
        } else {
            let mut params =
                json!({"machineId":"sessiondock","sessionId":session.sid,"cwd":session.cwd});
            {
                let live = lock(&session.live);
                if let Some(model) = &live.model {
                    params["modelId"] = json!(model);
                }
                if let Some(effort) = &live.effort {
                    params["reasoningEffort"] = json!(effort);
                }
            }
            session
                .control(json!({"method":"droid.initialize_session","params":params}))
                .await
        };
        match initialized {
            Ok(initialized) => {
                let mut live = lock(&session.live);
                if live.outbound.is_none() {
                    return Err("droid_sdk: CLI exited during initialize".into());
                }
                if !initialized["settings"].is_null() {
                    protocol::settings(&mut live, &initialized["settings"]);
                }
                live.models = initialized
                    .get("models")
                    .or_else(|| initialized.get("availableModels"))
                    .cloned()
                    .unwrap_or(Value::Null);
                normalize_models(&mut live.models);
                live.ready = true;
                session.publish(&mut live);
                Ok(())
            }
            Err(error) => {
                // A startup failure must not leave a headless runtime orphaned.
                // Cancel this runtime rather than session.stop, so a
                // later open may retry this same selected session.
                let mut live = lock(&session.live);
                live.error = Some(error.clone());
                live.error_kind = Some("transport");
                if let Some(stop) = &live.runtime_stop {
                    stop.cancel();
                }
                session.publish(&mut live);
                Err(error)
            }
        }
    }

    pub(super) async fn check_conflict(&self, session: &Arc<Session>) -> Result<(), String> {
        // Idle parent transports may retain delegated/background writers. Until
        // native task ownership is terminal, stop that transport before opening
        // the child. Also prevent the parent from reopening over a child writer.
        {
            let state = lock(&self.state);
            for other in state.sessions.values() {
                if other.uid == session.uid {
                    continue;
                }
                let owner = state.owners.get(&session.uid);
                let other_owner = state.owners.get(&other.uid);
                let related = owner
                    .is_some_and(|owner| owner == &other.uid || other_owner == Some(owner))
                    || other_owner == Some(&session.uid);
                if related && lock(&other.live).outbound.is_some() {
                    return Err("droid_sdk_session_busy: 父会话或关联子代理仍有独立运行实例；请先停止该 SDK 实例，再打开此子代理，避免后台任务产生第二写者".into());
                }
            }
        }

        if self.proc_root != Path::new("/proc")
            && self.proc_root.join("droid-processes.json").is_file()
        {
            return self.portable_conflict(session).await;
        }
        #[cfg(target_os = "linux")]
        {
            use crate::runtime::procscan::{ProcTree, SessionRoots, scan};
            let root = self.proc_root.clone();
            let native_root = self.root.clone();
            let sid = session.sid.to_ascii_lowercase();
            let ancestors = self.ancestor_sids(session)?;
            let cwd = crate::runtime::procscan::resolve_path(&session.cwd.to_string_lossy());
            let path = session.path.to_string_lossy().into_owned();
            // Only this selected runtime owns this SID. Another SDK session's
            // descendants may be delegated child writers, never shared transports.
            let own_pids: std::collections::BTreeSet<_> =
                lock(&session.live).pid.into_iter().collect();
            // An explicitly owned, different SID is not an unidentified writer
            // merely because stream RPC omits its SID from argv. This exception
            // applies only to that exact root PID, never its delegated children.
            let other_roots: std::collections::BTreeSet<_> = lock(&self.state)
                .sessions
                .values()
                .filter(|other| other.sid != session.sid)
                .filter_map(|other| {
                    let live = lock(&other.live);
                    live.outbound.as_ref().and(live.pid)
                })
                .collect();
            let conflict = tokio::task::spawn_blocking(move || {
                let scan = scan(
                    Arc::new(ProcTree::open(root)),
                    None,
                    &SessionRoots::new([native_root.as_path()]),
                );
                let own = |pid| crate::sdk_common::process_owned(&scan.tree, pid, &own_pids);
                let holders = scan
                    .sids
                    .get(&sid)
                    .into_iter()
                    .flatten()
                    .chain(scan.paths.get(&path).into_iter().flatten())
                    .chain(
                        ancestors
                            .iter()
                            .flat_map(|sid| scan.sids.get(sid).into_iter().flatten()),
                    );
                holders
                    .filter_map(|pid| u32::try_from(pid.unsigned_abs()).ok())
                    .any(|pid| !own(pid))
                    || scan.bare_droid.iter().any(|(pid, process)| {
                        process.cwd == cwd && !own(*pid) && !other_roots.contains(pid)
                    })
            })
            .await
            .map_err(|_| "droid_sdk: native writer observation failed")?;
            if conflict {
                return Err("droid_sdk_session_busy: an independent Droid process owns this SID/path or has unresolved identity in this cwd; this subprocess cannot share that runtime (shared_tui:false). Stop it explicitly before SDK resume; no takeover was attempted.".into());
            }
        }
        #[cfg(any(target_os = "macos", windows))]
        self.portable_conflict(session).await?;
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        if session.path.is_file() && lock(&session.live).outbound.is_none() {
            return Err("droid_sdk_unsupported: independent Droid writer detection is unavailable on this platform; existing-session SDK resume cannot safely share a TUI (shared_tui:false)".into());
        }
        Ok(())
    }
}
