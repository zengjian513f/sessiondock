use super::*;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

impl Bridge {
    pub(super) async fn ensure(self: &Arc<Self>, session: &Arc<Session>) -> Result<(), String> {
        if lock(&session.live).ready {
            return Ok(());
        }
        self.native_home()?;
        self.check_conflict(session).await?;
        let needs_spawn = lock(&session.live).outbound.is_none();
        if needs_spawn {
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
            // Resume by exact inventory path: SID searches may select another
            // project with the same UUID. Native CLI owns all transcript writes.
            if session.path.is_file() {
                command.arg(format!("--resume={}", session.path.to_string_lossy()));
            } else if let Some((source, at)) = &session.fork {
                command
                    .arg(format!("--resume={}", source.to_string_lossy()))
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
            command.env("CLAUDE_CONFIG_DIR", self.native_home()?);
            // The official SDK opts into these native state notifications so
            // a result does not hide a still-running background follow-up.
            command.env("CLAUDE_CODE_SDK_READS_SESSION_STATE", "1");
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
                    "claude_sdk_unavailable: configured Claude CLI could not start ({})",
                    e.kind()
                )
            })?;
            let stdin = child.stdin.take().ok_or("claude_sdk: stdin unavailable")?;
            let stdout = child
                .stdout
                .take()
                .ok_or("claude_sdk: stdout unavailable")?;
            let stderr = child
                .stderr
                .take()
                .ok_or("claude_sdk: stderr unavailable")?;
            let pid = child.id();
            let failed = CancellationToken::new();
            let idle_stop = CancellationToken::new();
            let (done, done_rx) = watch::channel(false);
            let (tx, rx) = mpsc::unbounded_channel();
            {
                let mut live = lock(&session.live);
                live.outbound = Some(tx);
                live.pid = pid;
                live.ready = false;
                live.state_observed = false;
                live.native_running = false;
                live.runtime_stop = Some(failed.clone());
                live.idle_stop = Some(idle_stop.clone());
                live.runtime_done = Some(done_rx);
                live.idle_since = None;
                live.error = None;
                live.error_kind = None;
                live.background_tasks.clear();
                if let Some(settings) = live.settings.as_object_mut() {
                    settings.remove("fast_mode_state");
                    settings.remove("fast_mode_disabled_reason");
                }
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
                                    let refresh = frame["type"] == "result"
                                        || (frame["type"] == "system"
                                            && frame["subtype"] == "compact_boundary");
                                    protocol::observe(&reader_session, frame);
                                    if refresh {
                                        reader_session.schedule_usage();
                                    }
                                }
                                Err(_) => {
                                    let mut live = lock(&reader_session.live);
                                    live.error = Some(
                                        "claude_sdk: invalid native stream-json output".into(),
                                    );
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
                    _=idle_stop.cancelled()=>{deliberate=true; None},
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
                live.idle_stop = None;
                live.runtime_done = None;
                live.idle_since = None;
                live.background_tasks.clear();
                live.turns.clear();
                live.requests.clear();
                if !deliberate
                    && live.error.is_none()
                    && (unfinished || status.is_none_or(|s| !s.success()))
                {
                    let diagnostic = live.stderr.back().cloned().unwrap_or_default();
                    live.error = Some(format!(
                        "claude_sdk_disconnected: native CLI exited; accepted input is not replayed. {diagnostic}"
                    ));
                    live.error_kind = Some("transport");
                }
                for (_, response) in std::mem::take(&mut live.controls) {
                    let _ = response.send(Err(
                        "claude_sdk_disconnected: native CLI exited; request outcome unknown"
                            .into(),
                    ));
                }
                session.publish(&mut live);
                let _ = done.send(true);
            });
        }
        let initialized = session
            .control(json!({"subtype":"initialize","hooks":null,"supportedDialogKinds":["refusal_fallback_prompt"]}))
            .await;
        let initialized = match initialized {
            Ok(mut initialized) => {
                let settings = lock(&session.live).settings.clone();
                async {
                    // An idle child is resumed into a fresh native runtime.
                    // Preserve this session's previously confirmed choices;
                    // the CLI still applies its current policy restrictions.
                    if let Some(mode) = settings["permission_mode"].as_str() {
                        session.control(json!({"subtype":"set_permission_mode","mode":mode})).await?;
                    }
                    if let Some(fast) = settings["fast_mode"].as_bool() {
                        session.control(json!({"subtype":"apply_flag_settings","settings":{"fastMode":fast}})).await?;
                        // initialize predates this update; the next native
                        // event supplies the new effective Fast state.
                        if let Some(fields) = initialized.as_object_mut() {
                            fields.remove("fast_mode_state");
                            fields.remove("fast_mode_disabled_reason");
                        }
                    }
                    Ok(initialized)
                }.await
            }
            Err(error) => Err(error),
        };
        match initialized {
            Ok(initialized) => {
                let mut live = lock(&session.live);
                if live.outbound.is_none() {
                    return Err("claude_sdk: CLI exited during initialize".into());
                }
                live.models = initialized["models"].clone();
                for field in ["fast_mode_state", "fast_mode_disabled_reason"] {
                    if let Some(value) = initialized.get(field) {
                        live.settings[field] = value.clone();
                    }
                }
                if let Some(models) = live.models.as_array_mut() {
                    for model in models {
                        if model["id"].is_null() {
                            model["id"] = model["value"].clone();
                        }
                        if model["name"].is_null() {
                            model["name"] = model["displayName"].clone();
                        }
                        if model["efforts"].is_null() {
                            model["efforts"] = model["supportedEffortLevels"].clone();
                        }
                    }
                }
                live.ready = true;
                session.publish(&mut live);
                drop(live);
                for field in [
                    "pending_permission_requests",
                    "pending_user_dialog_requests",
                ] {
                    if let Some(requests) = initialized[field].as_array() {
                        for request in requests {
                            protocol::observe(session, request.clone());
                        }
                    }
                }
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
        #[cfg(target_os = "linux")]
        {
            use crate::runtime::procscan::{ProcTree, SessionRoots, scan};
            let root = self.proc_root.clone();
            let native_root = self.root.clone();
            let sid = session.sid.to_ascii_lowercase();
            let cwd = session.cwd.to_string_lossy().into_owned();
            let path = session.path.to_string_lossy().into_owned();
            let own_pids: std::collections::BTreeSet<_> = lock(&self.state)
                .sessions
                .values()
                .filter_map(|session| lock(&session.live).pid)
                .collect();
            let conflict = tokio::task::spawn_blocking(move || {
                let scan = scan(
                    Arc::new(ProcTree::open(root)),
                    None,
                    &SessionRoots::new([native_root.as_path()]),
                );
                let own = |pid: u32| {
                    let mut current = pid;
                    let mut visited = std::collections::BTreeSet::new();
                    while current > 1 && visited.insert(current) {
                        if own_pids.contains(&current) {
                            return true;
                        }
                        let Some(parent) = scan.tree.parent(current) else {
                            break;
                        };
                        current = parent.ppid;
                    }
                    false
                };
                let holders = scan
                    .sids
                    .get(&sid)
                    .into_iter()
                    .flatten()
                    .chain(scan.paths.get(&path).into_iter().flatten());
                holders
                    .filter_map(|pid| u32::try_from(pid.unsigned_abs()).ok())
                    .any(|pid| !own(pid))
                    || scan
                        .bare_claude
                        .iter()
                        .any(|(pid, process)| process.cwd == cwd && !own(*pid))
            })
            .await
            .map_err(|_| "claude_sdk: native writer observation failed")?;
            if conflict {
                return Err("claude_sdk_session_busy: an independent Claude process owns this SID/path or has unresolved identity in this cwd; stream-json cannot share that runtime (shared_tui:false). Stop it explicitly before SDK resume; no takeover was attempted.".into());
            }
        }
        #[cfg(not(target_os = "linux"))]
        if session.path.is_file() && lock(&session.live).outbound.is_none() {
            return Err("claude_sdk_unsupported: independent Claude writer detection is unavailable on this platform; existing-session SDK resume cannot safely share a TUI (shared_tui:false)".into());
        }
        Ok(())
    }
}

async fn write_input(
    mut stdin: tokio::process::ChildStdin,
    mut rx: mpsc::UnboundedReceiver<Value>,
    failed: CancellationToken,
) {
    while let Some(frame) = rx.recv().await {
        let mut line = frame.to_string();
        line.push('\n');
        if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
            failed.cancel();
            return;
        }
    }
    let _ = stdin.shutdown().await;
    failed.cancel();
}

fn diagnostic_secrets(profile: &CliProfile) -> Vec<String> {
    std::env::vars()
        .chain(profile.env.clone())
        .filter_map(|(name, value)| {
            let name = name.to_ascii_uppercase();
            (value.len() >= 4
                && ["TOKEN", "KEY", "SECRET", "PASSWORD", "CREDENTIAL", "AUTH"]
                    .iter()
                    .any(|key| name.contains(key)))
            .then_some(value)
        })
        .collect()
}

fn redact(mut text: String, secrets: &[String]) -> String {
    for secret in secrets {
        text = text.replace(secret, "[redacted]");
    }
    // Unknown secret-shaped values in SDK/HTTP diagnostics are redacted too.
    for pattern in [
        r#"(?i)\bBearer\s+[^\s"']+"#,
        r"\bsk-[A-Za-z0-9_-]+",
        r"(?i)(?:api[_-]?key|token|password|secret)\s*[:=]\s*[^\s,;]+",
    ] {
        if let Ok(regex) = regex::Regex::new(pattern) {
            text = regex.replace_all(&text, "[redacted]").into_owned();
        }
    }
    text
}
