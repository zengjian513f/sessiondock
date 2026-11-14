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
        let _admission = self.writer_admission.lock().await;
        self.ensure_hosted(session).await
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
