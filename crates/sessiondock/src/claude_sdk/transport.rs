use super::*;
pub(super) use crate::sdk_common::{diagnostic_secrets, redact};

#[derive(Clone)]
pub(super) enum Outbound {
    Hosted(crate::sdk_common::outbox::Sender),
}
impl Outbound {
    pub fn send(&self, frame: Value) -> Result<(), String> {
        match self {
            Self::Hosted(sender) => sender.send(frame),
        }
    }
}

impl Bridge {
    pub(super) async fn ensure(self: &Arc<Self>, session: &Arc<Session>) -> Result<(), String> {
        self.ensure_hosted(session).await
    }

    pub(super) async fn initialize_session(&self, session: &Arc<Session>) -> Result<(), String> {
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
                let own = |pid| crate::sdk_common::process_owned(&scan.tree, pid, &own_pids);
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
        if cfg!(not(target_os = "linux")) || self.proc_root.join("sdk-writers.json").is_file() {
            let root = self.proc_root.clone();
            let sid = session.sid.clone();
            let path = session.path.clone();
            let cwd = session.cwd.clone();
            let own_pids: std::collections::BTreeSet<_> = lock(&self.state)
                .sessions
                .values()
                .filter_map(|session| lock(&session.live).pid)
                .collect();
            let conflict = tokio::task::spawn_blocking(move || {
                let snapshot =
                    crate::sdk_common::writers::snapshot(&root, std::slice::from_ref(&path))?;
                Ok::<_, String>(snapshot.processes.iter().any(|process| {
                    !snapshot.owns(process.pid, &own_pids)
                        && (process.holds(&path)
                            || (process.is_cli("claude") && process.session_matches(&sid, &path))
                            || (process.is_cli("claude")
                                && process
                                    .option(&["--session-id", "--resume-id", "--resume", "-r"])
                                    .is_none()
                                && process.same_cwd(&cwd)))
                }))
            })
            .await
            .map_err(|e| format!("claude_sdk: writer observation failed: {e}"))??;
            if conflict {
                return Err("claude_sdk_session_busy: an independent Claude process owns this SID/path or has unresolved identity in this cwd; stop it explicitly before SDK resume (shared_tui:false). No takeover was attempted.".into());
            }
        }
        Ok(())
    }
}
