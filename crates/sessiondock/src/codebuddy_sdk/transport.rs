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
        // `account` in the response carries a login token; only `models` is read.
        let initialized = session
            .control(
                json!({"subtype":"initialize","hooks":null,"systemPrompt":null,
                "appendSystemPrompt":null,"agents":null,"sdkMcpServers":null,
                "hasPrompt":false,"capabilities":{"askUserQuestion":true}}),
            )
            .await;
        let initialized = match initialized {
            Ok(initialized) => {
                let settings = lock(&session.live).settings.clone();
                async {
                    // A relaunched runtime keeps this session's confirmed
                    // permission mode; the CLI still applies its own policy.
                    if let Some(mode) = settings["permission_mode"].as_str() {
                        session
                            .control(json!({"subtype":"set_permission_mode",
                                "session_id":session.sid,"mode":mode}))
                            .await?;
                    }
                    Ok(initialized)
                }
                .await
            }
            Err(error) => Err(error),
        };
        match initialized {
            Ok(initialized) => {
                let mut live = lock(&session.live);
                if live.outbound.is_none() {
                    return Err("codebuddy_sdk: CLI exited during initialize".into());
                }
                live.models = initialized["models"].clone();
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
        #[cfg(target_os = "linux")]
        {
            use crate::runtime::procscan::{ProcTree, SessionRoots, scan};
            let root = self.proc_root.clone();
            let native_root = self.root.clone();
            let sid = session.sid.to_ascii_lowercase();
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
            })
            .await
            .map_err(|_| "codebuddy_sdk: native writer observation failed")?;
            if conflict {
                return Err("codebuddy_sdk_session_busy: an independent CodeBuddy process owns this SID/path; stream-json cannot share that runtime (shared_tui:false). Stop it explicitly before SDK resume; no takeover was attempted.".into());
            }
        }
        if cfg!(not(target_os = "linux")) || self.proc_root.join("sdk-writers.json").is_file() {
            let root = self.proc_root.clone();
            let sid = session.sid.clone();
            let path = session.path.clone();

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
                            || ((process.is_cli("codebuddy")
                                || process.is_cli("codebuddy-code")
                                || process.is_cli("cbc"))
                                && process.session_matches(&sid, &path)))
                }))
            })
            .await
            .map_err(|e| format!("codebuddy_sdk: writer observation failed: {e}"))??;
            if conflict {
                return Err("codebuddy_sdk_session_busy: an independent CodeBuddy process owns this SID/path; stop it explicitly before SDK resume (shared_tui:false). No takeover was attempted.".into());
            }
        }
        Ok(())
    }
}
