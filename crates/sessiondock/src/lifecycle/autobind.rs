//! Process-evidence binding of pending Codex/Grok launches (WP-E).
//!
//! Python's `new-status` resolution associates a fresh `codex`/`grok` pane
//! with the native record that appeared after the first prompt: same cwd,
//! not in the `before` set and — for Codex — the rollout held open by a
//! process inside the pane's tree (`term.process_belongs_to`). Rust never
//! guesses by cwd, time or file name; it keeps only the process evidence:
//!
//! 1. a receipt in `Running` with launch kind `new_pending` and no binding;
//! 2. one fresh guarded host observation naming that exact instance and its
//!    child process (`summary.pid`);
//! 3. the `/proc` scan of [`crate::runtime::procscan`] (the same one
//!    `/api/live` uses): every indexed session of the receipt's source one
//!    of whose owned processes — CLI main process or helper holding the
//!    native record, exactly Python's `pids_of` → `process_belongs_to` with
//!    `abs(pid)` — is, or descends from, that child with no other CLI main
//!    process in between (Python's `descendant_of` barrier);
//! 4. exactly one such session, whose native scope the index verifies.
//!
//! Then the ordinary `bind` path runs with `BindingMethod::Process`: the
//! durable intent carries the evidence note, the host publishes the
//! one-time binding, and only a matching guarded Info confirms it. Zero or
//! several candidates keep the receipt pending; the operator dialog remains
//! available. The task only runs when the lifecycle service, the managed
//! runtime and the process scan are all configured.

use std::time::Duration;

use serde_json::json;

use super::{
    model::{Launch, Record, State},
    service::{Error as ServiceError, VerifiedNativeBinding},
};
use crate::{
    runtime::{Liveness, procscan::SessionRow},
    state::AppState,
};

/// Poll cadence while unbound pending receipts exist (Python's page polled
/// `new-status` every 750 ms). Each pass forces a fresh scan (≈ 60–70 ms on
/// this machine) so a rollout the CLI just opened is seen at once.
pub const TICK: Duration = Duration::from_millis(1500);
/// Idle cadence when nothing is pending.
pub const IDLE: Duration = Duration::from_secs(6);
/// One candidate pairing computed off the reactor.
struct Match {
    record: Record,
    uid: String,
    evidence: String,
}

enum Pairing {
    Unique(Box<Match>),
    /// Several indexed sessions descend from the same host child.
    Ambiguous(String, usize),
}

pub fn enabled(state: &AppState) -> bool {
    state.lifecycle.is_some() && state.runtime.is_some() && state.proc_scan.is_some()
}

/// Spawn the loop on the current runtime; returns immediately when the
/// prerequisites are missing.
pub fn spawn(state: AppState) {
    if !enabled(&state) {
        return;
    }
    tokio::spawn(run(state));
}

async fn run(state: AppState) {
    let mut wait = IDLE;
    loop {
        tokio::select! {
            biased;
            _ = state.shutdown.cancelled() => break,
            _ = tokio::time::sleep(wait) => {}
        }
        wait = match tick(&state).await {
            Ok(Some(_)) => TICK,
            Ok(None) => IDLE,
            Err(_) => TICK,
        };
    }
}

/// One pass. `Ok(None)` means no unbound pending receipt exists; `Ok(Some(n))`
/// is the number of bindings confirmed this pass.
pub async fn tick(state: &AppState) -> Result<Option<usize>, ServiceError> {
    let (Some(service), Some(scanner)) = (&state.lifecycle, &state.proc_scan) else {
        return Ok(None);
    };
    if state.runtime.is_none() {
        return Ok(None);
    }
    let candidates: Vec<Record> = service
        .list(0, usize::MAX)
        .await?
        .into_iter()
        .filter(|record| {
            record.state() == State::Running
                && !record.cancel_requested()
                && record.spec().launch() == &Launch::NewPending
                && record.binding().is_none()
        })
        .collect();
    if candidates.is_empty() {
        return Ok(None);
    }
    let observed = crate::api::runtime::observe(state)
        .await
        .map_err(|_| ServiceError::Busy)?
        .ok_or(ServiceError::Busy)?;
    let scan = scanner
        .snapshot(true)
        .await
        .map_err(|_| ServiceError::WorkerFailed)?
        .scan;
    let (document, catalog) = state
        .reader
        .run_wait(&state.shutdown, |store| {
            Ok((store.list(false)?, store.native_catalog()?))
        })
        .await
        .map_err(|_| ServiceError::Busy)?;
    let pairings = tokio::task::spawn_blocking(move || {
        let sessions = SessionRow::from_list(&document);
        let active = scan.active_processes(&sessions);
        let mut pairings = Vec::new();
        for record in candidates {
            let Some(child) = observed
                .hosts
                .iter()
                .find(|host| {
                    host.summary.name == record.host_name()
                        && host.instance_id.as_deref() == Some(record.instance_id())
                        && host.known
                        && host.liveness == Liveness::Running
                })
                .map(|host| host.summary.pid)
            else {
                continue;
            };
            let source = match record.spec().source() {
                super::model::Source::Claude => "claude",
                super::model::Source::Codex => "codex",
                super::model::Source::Grok => "grok",
            };
            let mut found: Vec<(&SessionRow, Vec<i64>)> = Vec::new();
            for session in sessions.iter().filter(|session| session.source == source) {
                let Some(pids) = active.owned.get(session.uid.as_str()) else {
                    continue;
                };
                if pids
                    .iter()
                    .any(|pid| descends_from(&scan.tree, *pid, child))
                {
                    found.push((session, pids.clone()));
                }
            }
            match found.as_slice() {
                [] => {}
                [(session, pids)] => {
                    let held: Vec<i64> = pids
                        .iter()
                        .copied()
                        .filter(|pid| descends_from(&scan.tree, *pid, child))
                        .collect();
                    let evidence = format!(
                        "cli_pids={held:?} under host child pid {child} ({}); native record {path}",
                        record.host_name(),
                        path = session.path.as_str()
                    );
                    pairings.push(Pairing::Unique(Box::new(Match {
                        record,
                        uid: session.uid.clone(),
                        evidence,
                    })));
                }
                many => {
                    pairings.push(Pairing::Ambiguous(
                        record.record_id().to_owned(),
                        many.len(),
                    ));
                }
            }
        }
        pairings
    })
    .await
    .map_err(|_| ServiceError::WorkerFailed)?;
    let mut bound = 0;
    for pairing in pairings {
        match pairing {
            Pairing::Ambiguous(record_id, count) => {
                audit(
                    state,
                    "lifecycle.autobind_ambiguous",
                    "warning",
                    json!({"record_id": record_id, "candidates": count}),
                );
            }
            Pairing::Unique(found) => {
                let scope = match catalog.verified_scope(&found.uid) {
                    Ok(scope) => scope,
                    Err(reason) => {
                        audit(
                            state,
                            "lifecycle.autobind_unverified",
                            "warning",
                            json!({"record_id": found.record.record_id(), "uid": found.uid,
                                "reason": format!("{reason:?}")}),
                        );
                        continue;
                    }
                };
                let verified = match VerifiedNativeBinding::from_process_evidence(
                    &scope,
                    &found.record,
                    found.evidence.clone(),
                ) {
                    Ok(verified) => verified,
                    Err(_) => continue,
                };
                match service.bind(verified).await {
                    Ok(record) => {
                        let state_text = record
                            .binding()
                            .map(|binding| format!("{:?}", binding.state()).to_ascii_lowercase());
                        if record.binding().is_some_and(|binding| {
                            binding.state() == super::model::BindingState::Confirmed
                        }) {
                            bound += 1;
                        }
                        audit(
                            state,
                            "lifecycle.autobind",
                            "info",
                            json!({"record_id": record.record_id(), "instance_id": record.instance_id(),
                                "uid": found.uid, "sid": scope.session_id, "source": scope.source,
                                "binding_state": state_text, "evidence": found.evidence,
                                "bound_at": record.binding().and_then(|binding| binding.bound_at())}),
                        );
                    }
                    Err(error) => {
                        audit(
                            state,
                            "lifecycle.autobind_failed",
                            "warning",
                            json!({"record_id": found.record.record_id(), "uid": found.uid,
                                "error": format!("{error:?}")}),
                        );
                    }
                }
            }
        }
    }
    Ok(Some(bound))
}

/// Python `procs.descendant_of(pid, root, barrier=live.is_cli_process)`:
/// `abs(pid)` is, or descends from, the host's child within 16 levels; an
/// intermediate CLI main process (not the start, not the root) cuts the
/// relation — a `grok -p` the pane's CLI spawned belongs to that CLI's console.
/// Helpers (negative scan pids) count exactly like Python's `pids_of` set.
fn descends_from(tree: &crate::runtime::procscan::ProcTree, pid: i64, root: u32) -> bool {
    let Ok(mut current) = u32::try_from(pid.unsigned_abs()) else {
        return false;
    };
    if root == 0 || current == 0 {
        return false;
    }
    for step in 0..crate::runtime::procscan::ANCESTRY_DEPTH {
        if current == root {
            return true;
        }
        if step > 0 && tree.is_cli_process(current) {
            return false;
        }
        match tree.parent(current) {
            Some(parent) if parent.ppid > 1 && parent.ppid != current => current = parent.ppid,
            _ => return false,
        }
    }
    false
}

fn audit(state: &AppState, event: &'static str, severity: &'static str, data: serde_json::Value) {
    if let Some(audit) = &state.audit {
        let uid = data["uid"].as_str().unwrap_or("").to_owned();
        audit.record(crate::audit::query::ServerEvent {
            event,
            category: "lifecycle",
            severity,
            uid: &uid,
            trace_id: "",
            page_id: "",
            build: "",
            data,
        });
    }
}
