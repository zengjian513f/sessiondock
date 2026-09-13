//! Read-only live status; never upgrades observations into CLI authority.
//! Managed host observations and Python-shaped native process discovery are
//! merged into the legacy `uids` / `tmux_uids` / `started_at` envelope.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use axum::{
    Json,
    extract::{RawQuery, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use crate::{
    error::ApiError,
    lifecycle::model::{BindingState, State as LaunchState},
    runtime::{
        ExitReceipt, ManagedRuntime, RuntimeSnapshot, SharedError, SharedObservation,
        procscan::{Scan, ScanError, ScanSnapshot, SessionRow},
    },
    state::AppState,
};

/// Python `_pane_for_session` recursion bound: continued-in hops examined.
const CONTINUED_HOPS: usize = 8;

const UNCONFIGURED: &str =
    "尚未实现外部 CLI 进程探测；受控 host 的部分观察不能据此接管或停止会话。";
const PARTIAL: &str = "仅观察显式配置的受控 host 实例；未列出的会话运行状态未知，不表示已停止，也不能据此接管或停止会话。";
const SCAN_FAILED: &str =
    "进程表扫描失败；本次只有受控 host 实例的观察，未列出的会话运行状态未知。";

fn external_scan_result(
    result: Result<ScanSnapshot, ScanError>,
) -> Result<Option<Arc<Scan>>, ApiError> {
    match result {
        Ok(snapshot) => Ok(Some(snapshot.scan)),
        // Python falls back to psutil off /proc and returns an empty observation
        // when that provider is unavailable. Absence of evidence never grants
        // PID control; managed host observation remains independently usable.
        Err(ScanError::UnsupportedPlatform) => Ok(None),
        Err(error @ ScanError::Failed) => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "process_scan_unavailable",
            format!("无法核对会话进程：{error}"),
        )),
    }
}

async fn external_scan(state: &AppState) -> Result<Option<Arc<Scan>>, ApiError> {
    let scanner = state.proc_scan.clone().unwrap_or_else(|| {
        Arc::new(crate::runtime::procscan::ProcScanner::new(
            "/proc".into(),
            None,
            crate::runtime::procscan::SessionRoots::default(),
        ))
    });
    external_scan_result(scanner.snapshot(true).await)
}

/// One process-table snapshot over a frozen session-list document. The map is
/// Python `live.is_live(row, force=True)` for every row, including Grok's
/// active-session evidence when it exposes no PID. Callers can protect a whole
/// trash batch without rescanning once per UID.
pub(crate) async fn observe_external_all(
    state: &AppState,
    document: &Value,
) -> Result<BTreeMap<String, bool>, ApiError> {
    let sessions = SessionRow::from_list(document);
    let scan = external_scan(state).await?;
    Ok(sessions
        .iter()
        .map(|session| {
            (
                session.uid.clone(),
                scan.as_ref().is_some_and(|scan| scan.is_live(session)),
            )
        })
        .collect())
}

/// The scan merged with the managed observations (Python `/api/live` body).
#[derive(Default)]
struct Merged {
    uids: Vec<String>,
    tmux_uids: Vec<String>,
    started: BTreeMap<String, f64>,
    recorded: Option<Result<usize, crate::metadata::MetadataError>>,
}

fn seed_managed_status(response: &mut Value, running: &[String], started: &BTreeMap<String, f64>) {
    response["uids"] = json!(running);
    // A running managed instance owns an attachable ptyhost console even when
    // this platform has no native process scanner (notably Windows). A
    // successful scan later rebuilds this list in session order and adds
    // native tmux/inherited-pane matches.
    response["tmux_uids"] = json!(running);
    response["started_at"] = json!(started);
}

/// Legacy envelope. Without the scan, `uids` lists only sessions whose managed
/// instance is verified running and everything else is unknown, never
/// stopped. With the scan (Linux, explicit switch) the answer is Python's:
/// `uids` = sessions with a live CLI process (list order), `tmux_uids` those
/// running under tmux or a managed host, `started_at[uid]` the earliest CLI
/// main-process start; spawners are recorded on the way. `?force=1` bypasses
/// both caches exactly like the Python endpoint.
pub async fn live(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let force = query
        .as_deref()
        .is_some_and(|query| query.split('&').any(|pair| pair == "force=1"));
    // Python filters the session list through the debug-run registry before
    // pairing processes: a hidden session is never a live uid of this view.
    let debug_run = crate::sessions::debug_run_of(query.as_deref());
    let runs = state.reader.store.debug_runs();
    let mut response = json!({"enabled":false,"known":false,"partial":true,
        "unavailable_reason":UNCONFIGURED,
        "uids":[],"tmux_uids":[],"started_at":{},"managed":null});
    // Managed running uids in list-independent order, their start times, the
    // host session roots (Python `term_host.hosts`) and the uids the host
    // records declare (Python's pane named for a session).
    let mut managed_running: Vec<String> = Vec::new();
    let mut managed_started: BTreeMap<String, f64> = BTreeMap::new();
    let mut host_roots: BTreeSet<u32> = BTreeSet::new();
    let mut host_uids: BTreeSet<String> = BTreeSet::new();
    if let Some(runtime) = &state.runtime {
        let shared = shared(&state, runtime, force).await?;
        let snapshot: &RuntimeSnapshot = &shared.snapshot;
        for uid in snapshot.running_uids() {
            managed_running.push(uid.to_owned());
            if let Some(at) = snapshot.sessions[uid].started_at {
                managed_started.insert(uid.to_owned(), at);
            }
        }
        host_roots.extend(snapshot.hosts.iter().map(|host| host.summary.pid));
        host_uids.extend(
            snapshot
                .hosts
                .iter()
                .filter_map(|host| host.native_uid().map(str::to_owned)),
        );
        response["enabled"] = json!(true);
        response["known"] = json!(snapshot.known);
        response["unavailable_reason"] = json!(PARTIAL);
        seed_managed_status(&mut response, &managed_running, &managed_started);
        let mut managed = serde_json::to_value(snapshot).map_err(|_| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "runtime_encoding",
                "受控进程观察无法编码",
            )
        })?;
        managed["cache"] = json!({
            "hit": shared.cached,
            "age_ms": u64::try_from(shared.age.as_millis()).unwrap_or(u64::MAX),
            "ttl_ms": u64::try_from(runtime.limits().cache_ttl.as_millis()).unwrap_or(u64::MAX),
        });
        response["managed"] = managed;
    }
    {
        let scanner = state.proc_scan.clone().unwrap_or_else(|| {
            Arc::new(crate::runtime::procscan::ProcScanner::new(
                "/proc".into(),
                None,
                crate::runtime::procscan::SessionRoots::default(),
            ))
        });
        let mut scan_report = json!({"enabled": true, "root": scanner.root().to_string_lossy()});
        match scanner.snapshot(force).await {
            Ok(snapshot) => {
                // Wait for a reader like the SSE coordinators: the legacy
                // poller must not see a spurious `reader_busy` from liveness.
                let mut document = state
                    .reader
                    .run_wait(&state.shutdown, |store| store.list_recent())
                    .await?;
                let hidden = runs.split_document(&mut document, &debug_run);
                managed_running.retain(|uid| !hidden.contains(uid));
                let scan = snapshot.scan.clone();
                let watcher = state.spawn_watch.clone();
                // Pairing, ancestry walks and the spawner write touch the
                // process table and the metadata file: off the reactor.
                let merged = tokio::task::spawn_blocking(move || {
                    let sessions = SessionRow::from_list(&document);
                    let active = scan.active_processes(&sessions);
                    let by_uid: HashMap<&str, &SessionRow> = sessions
                        .iter()
                        .map(|session| (session.uid.as_str(), session))
                        .collect();
                    let mut merged = Merged::default();
                    let live: BTreeSet<&str> = active
                        .uids
                        .iter()
                        .map(String::as_str)
                        .chain(managed_running.iter().map(String::as_str))
                        .collect();
                    merged.uids = sessions
                        .iter()
                        .filter(|session| live.contains(session.uid.as_str()))
                        .map(|session| session.uid.clone())
                        .collect();
                    for uid in &managed_running {
                        if !merged.uids.contains(uid) {
                            merged.uids.push(uid.clone());
                        }
                    }
                    for uid in &merged.uids {
                        let pids = active.owned.get(uid).map_or(&[][..], Vec::as_slice);
                        // Managed = it is the CLI of a pane itself (a managed
                        // instance runs under its host by construction), or a
                        // continued Claude session that inherited the origin's
                        // pane; a grandchild the pane's CLI spawned is not, it
                        // has no console of its own.
                        if managed_running.contains(uid)
                            || scan.tree.in_tmux(pids)
                            || scan.tree.hosted(pids, &host_roots)
                            || by_uid.get(uid.as_str()).is_some_and(|session| {
                                inherits_pane(&scan, &sessions, session, &host_uids, &host_roots)
                            })
                        {
                            merged.tmux_uids.push(uid.clone());
                        }
                        if let Some(at) = scan
                            .started_at(pids)
                            .or_else(|| managed_started.get(uid).copied())
                        {
                            merged.started.insert(uid.clone(), at);
                        }
                    }
                    // Python records spawners on every `/api/live` (趁每次判活顺手记下).
                    merged.recorded = watcher
                        .as_ref()
                        .map(|watcher| watcher.record(&scan, &sessions, &active.owned));
                    merged
                })
                .await
                .map_err(|_| {
                    ApiError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "live_failed",
                        "进程表配对失败",
                    )
                })?;
                response["enabled"] = json!(true);
                response["known"] = json!(true);
                response["partial"] = json!(false);
                response
                    .as_object_mut()
                    .expect("object")
                    .remove("unavailable_reason");
                response["uids"] = json!(merged.uids);
                response["tmux_uids"] = json!(merged.tmux_uids);
                response["started_at"] = json!(merged.started);
                if let Some(managed) = response["managed"].as_object_mut() {
                    managed.insert("external_detection".into(), json!("proc_scan"));
                }
                scan_report["stats"] = json!(snapshot.scan.stats);
                scan_report["cache"] = json!({
                    "hit": snapshot.cached,
                    "age_ms": u64::try_from(snapshot.age.as_millis()).unwrap_or(u64::MAX),
                    "ttl_ms": u64::try_from(scanner.ttl().as_millis()).unwrap_or(u64::MAX),
                });
                scan_report["spawned_recorded"] = match merged.recorded {
                    Some(Ok(count)) => json!(count),
                    Some(Err(error)) => json!({"error": error.code}),
                    None => Value::Null,
                };
            }
            Err(ScanError::UnsupportedPlatform) => {
                scan_report["status"] = json!("unsupported_platform");
            }
            Err(ScanError::Failed) => {
                scan_report["status"] = json!("failed");
                response["unavailable_reason"] = json!(SCAN_FAILED);
            }
        }
        response["scan"] = scan_report;
    }
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(response)).into_response())
}

/// Python `_pane_for_session`'s continued-in fallback: a Claude session no
/// pane owns inherits the pane of the listed row whose `continued_in` names
/// it. The continued JSONL's process runs under the origin TUI's daemon child,
/// so by process tree it is not the pane's CLI, but the list shows only the
/// continued session and the console follows it (up to eight hops). The
/// origin owns a pane when a host record declares its uid (Python's pane
/// named for the session) or one of its CLI processes descends from a host's
/// session root; a spawned grandchild is not a continuation and never inherits.
fn inherits_pane(
    scan: &Scan,
    sessions: &[SessionRow],
    session: &SessionRow,
    host_uids: &BTreeSet<String>,
    host_roots: &BTreeSet<u32>,
) -> bool {
    if host_uids.is_empty() && host_roots.is_empty() {
        return false;
    }
    let mut current = session;
    for _ in 0..CONTINUED_HOPS {
        let Some(origin) = continued_origin(sessions, current) else {
            return false;
        };
        if host_uids.contains(&origin.uid) {
            return true;
        }
        let pids: Vec<i64> = scan.pids_of(origin).into_iter().collect();
        if scan.tree.hosted(&pids, host_roots) {
            return true;
        }
        current = origin;
    }
    false
}

/// Python `_continued_origin`: the first listed row whose `continued_in`
/// names this Claude session; other sources have none.
fn continued_origin<'a>(
    sessions: &'a [SessionRow],
    session: &SessionRow,
) -> Option<&'a SessionRow> {
    if session.source != "claude" {
        return None;
    }
    sessions.iter().find(|row| {
        row.continued_in.as_deref() == Some(session.uid.as_str()) && row.uid != session.uid
    })
}

/// Shared, single-flight observation for display. Admission and the frozen
/// native catalog are only taken when a refresh really runs; waiters reuse it.
/// Also the Codex approval poller's instance lookup (`bridge::live`).
pub(crate) async fn shared(
    state: &AppState,
    runtime: &ManagedRuntime,
    force: bool,
) -> Result<SharedObservation, ApiError> {
    tokio::select! {
        biased;
        _ = state.shutdown.cancelled() => Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "shutdown", "服务正在关闭")),
        result = runtime.observe_shared(force, async || {
            let permit = admit(state).await?;
            let catalog = state.reader.run(|store| store.native_catalog()).await?;
            let receipts = exit_receipts(state).await?;
            Ok::<_, ApiError>((permit, catalog, receipts))
        }) => result.map_err(|error| match error {
            SharedError::Prepare(error) => error,
            SharedError::Runtime(_) => unavailable(),
        }),
    }
}

/// Durable lifecycle exit receipts for operator-bound instances. A receipt
/// names one exact instance; the runtime never lets it outrank a newer
/// reachable instance of the same session.
async fn exit_receipts(state: &AppState) -> Result<Vec<ExitReceipt>, ApiError> {
    let Some(service) = &state.lifecycle else {
        return Ok(Vec::new());
    };
    let records = service
        .list(0, usize::MAX)
        .await
        .map_err(super::lifecycle::failure)?;
    Ok(records
        .iter()
        .filter(|record| record.state() == LaunchState::Exited)
        .filter_map(|record| {
            let binding = record.binding()?;
            (binding.state() == BindingState::Confirmed).then(|| ExitReceipt {
                uid: binding.spec().uid().to_owned(),
                host: record.host_name().to_owned(),
                instance_id: record.instance_id().to_owned(),
            })
        })
        .collect())
}

/// One of `Pools::runtime_probes` permits, queued until available.
async fn admit(state: &AppState) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    crate::state::admit(&state.runtime_probes, "runtime_busy").await
}

fn unavailable() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "runtime_unavailable",
        "受控 host 目录不可用或超出观察预算",
    )
}

/// Fresh admission for claim/list. No stale cross-request cache is used to
/// authorize a new ownership claim.
pub(crate) async fn observe(state: &AppState) -> Result<Option<RuntimeSnapshot>, ApiError> {
    if let Some(runtime) = &state.runtime {
        let _permit = admit(state).await?;
        let observed = tokio::select! {
            biased;
            _ = state.shutdown.cancelled() => return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "shutdown", "服务正在关闭")),
            result = async {
                let catalog = state.reader.run_wait(&state.shutdown, |store| store.native_catalog()).await?;
                runtime.observe(&catalog).await.map_err(|_| unavailable())
            } => result?,
        };
        return Ok(Some(observed));
    }
    Ok(None)
}

#[cfg(test)]
mod external_scan_tests {
    use super::*;

    #[test]
    fn unsupported_platform_is_an_empty_external_observation() {
        assert!(
            external_scan_result(Err(ScanError::UnsupportedPlatform))
                .expect("unsupported process discovery is not a request failure")
                .is_none()
        );
        assert!(external_scan_result(Err(ScanError::Failed)).is_err());
    }

    #[test]
    fn managed_console_is_tmux_shaped_without_an_external_scan() {
        let uid = "claude:managed".to_owned();
        let started = BTreeMap::from([(uid.clone(), 42.0)]);
        let mut response = json!({"uids":[],"tmux_uids":[],"started_at":{}});

        seed_managed_status(&mut response, std::slice::from_ref(&uid), &started);

        assert_eq!(response["uids"], json!([uid]));
        assert_eq!(response["tmux_uids"], response["uids"]);
        assert_eq!(response["started_at"], json!({"claude:managed":42.0}));
    }
}

/// Python 16cc89c `tests/test_server.py` PaneLinkingTests over a synthetic
/// tree: pane root 10 → claude 11 (origin) → daemon 12 → claude 13 (the
/// continued session's process); 11 also spawned grok 14. A CLI in between
/// cuts pane ownership, so 13 and 14 do not belong to 10 directly.
#[cfg(all(test, unix))]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::runtime::procscan::{
        ProcTree, SessionRoots as ScanRoots, scan,
        tests::{FakeProc, session},
    };

    const ORIGIN_SID: &str = "0aaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const NEXT_SID: &str = "0bbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const CHILD_SID: &str = "0ccccccc-cccc-4ccc-8ccc-cccccccccccc";

    fn fixture() -> (tempfile::TempDir, Scan, Vec<SessionRow>) {
        let temp = tempfile::tempdir().unwrap();
        let proc = FakeProc::new(temp.path());
        proc.add(1, "systemd", 0, "/sbin/init", &[], &[]);
        proc.add(10, "sh", 1, "sh -c claude", &[], &[]);
        proc.add(
            11,
            "claude",
            10,
            &format!("claude --session-id {ORIGIN_SID}"),
            &[],
            &[],
        );
        proc.add(12, "node", 11, "node daemon.js", &[], &[]);
        proc.add(
            13,
            "claude",
            12,
            &format!("claude --resume {NEXT_SID}"),
            &[],
            &[],
        );
        proc.add(
            14,
            "grok",
            11,
            "grok -p do the task",
            &[("GROK_SESSION_ID", CHILD_SID)],
            &[],
        );
        let scan = scan(
            Arc::new(ProcTree::open(proc.root.clone())),
            None,
            &ScanRoots::default(),
        );
        let next = session("claude", NEXT_SID, "2026-09-12T10:00:00Z");
        let origin = SessionRow {
            continued_in: Some(next.uid.clone()),
            ..session("claude", ORIGIN_SID, "2026-09-12T09:00:00Z")
        };
        let spawned = session("grok", CHILD_SID, "2026-09-12T10:30:00Z");
        (temp, scan, vec![origin, next, spawned])
    }

    #[test]
    fn origin_keeps_its_own_pane_and_the_continued_session_inherits_it() {
        let (_temp, scan, rows) = fixture();
        let (origin, next, spawned) = (&rows[0], &rows[1], &rows[2]);
        let root = BTreeSet::from([10]);
        let declared = BTreeSet::from([origin.uid.clone()]);
        assert_eq!(scan.pids_of(origin).into_iter().collect::<Vec<_>>(), [11]);
        assert_eq!(scan.pids_of(next).into_iter().collect::<Vec<_>>(), [13]);
        assert_eq!(scan.pids_of(spawned).into_iter().collect::<Vec<_>>(), [14]);
        // test_origin_keeps_its_own_pane: by process tree, no inheritance needed.
        assert!(scan.tree.hosted(&[11], &root));
        assert!(!inherits_pane(&scan, &rows, origin, &declared, &root));
        // test_continued_session_inherits_the_origin_pane: not the pane's CLI
        // by process tree (11 is in between), inherited through the origin —
        // through the host record naming the origin, or through the origin's
        // own CLI process under the pane root.
        assert!(!scan.tree.hosted(&[13], &root));
        assert!(inherits_pane(
            &scan,
            &rows,
            next,
            &declared,
            &BTreeSet::new()
        ));
        assert!(inherits_pane(&scan, &rows, next, &BTreeSet::new(), &root));
        assert!(!inherits_pane(
            &scan,
            &rows,
            next,
            &BTreeSet::new(),
            &BTreeSet::new()
        ));
        assert!(!inherits_pane(
            &scan,
            &rows,
            next,
            &BTreeSet::from(["claude:x".to_owned()]),
            &BTreeSet::from([99])
        ));
        // test_spawned_child_in_the_pane_has_no_console.
        assert!(!scan.tree.hosted(&[14], &root));
        assert!(!inherits_pane(&scan, &rows, spawned, &declared, &root));
        // Only a Claude session looks for an origin (`_continued_origin`), and
        // a row naming itself is none.
        let mut foreign = rows.clone();
        foreign[1].source = "codex".into();
        assert!(!inherits_pane(
            &scan,
            &foreign,
            &foreign[1],
            &declared,
            &root
        ));
        let mut own = rows.clone();
        own[1].continued_in = Some(own[1].uid.clone());
        own[0].continued_in = None;
        assert!(!inherits_pane(&scan, &own, &own[1], &declared, &root));
    }

    /// `_hops < 8`: the eighth origin up the chain still lends its pane, the ninth does not.
    #[test]
    fn continued_in_inheritance_stops_after_eight_hops() {
        let (_temp, scan, _) = fixture();
        let mut chain: Vec<SessionRow> = (0..10)
            .map(|index| {
                session(
                    "claude",
                    &format!("{index:08x}-0000-4000-8000-000000000000"),
                    "",
                )
            })
            .collect();
        for index in 0..9 {
            let target = chain[index + 1].uid.clone();
            chain[index].continued_in = Some(target);
        }
        let declared = BTreeSet::from([chain[0].uid.clone()]);
        assert!(inherits_pane(
            &scan,
            &chain,
            &chain[8],
            &declared,
            &BTreeSet::new()
        ));
        assert!(!inherits_pane(
            &scan,
            &chain,
            &chain[9],
            &declared,
            &BTreeSet::new()
        ));
        // The first listed row naming the session wins, like Python's `next(...)`.
        let mut twice = chain.clone();
        let shadow = SessionRow {
            uid: "claude:shadow".into(),
            continued_in: Some(chain[8].uid.clone()),
            ..session("claude", "shadow-sid", "")
        };
        twice.insert(0, shadow);
        assert!(!inherits_pane(
            &scan,
            &twice,
            &twice[9],
            &declared,
            &BTreeSet::new()
        ));
        assert!(inherits_pane(
            &scan,
            &twice,
            &twice[9],
            &BTreeSet::from(["claude:shadow".to_owned()]),
            &BTreeSet::new()
        ));
    }
}
