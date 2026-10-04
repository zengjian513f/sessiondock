//! Initialize the single sidebar parent from an observed local CLI launch.
//! Native file ownership identifies the child; inherited session IDs or an
//! owning CLI ancestor identify its initiator, including detached dispatchers.
//! Explicit attachment/detachment wins permanently. No native history is edited.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use indexmap::IndexMap;

use super::procscan::{ANCESTRY_DEPTH, SPAWN_ENV, Scan, SessionRow, parse_created, resolve_path};
use crate::{
    metadata::{MetadataError, MetadataStore, NestParent},
    state::AppState,
};

/// Every possible spawner uid along the chain.
fn spawn_candidates(
    scan: &Scan,
    pid: u32,
    uid: &str,
    by_key: &HashMap<(String, String), String>,
    pid_owner: &HashMap<u32, String>,
) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut current = pid;
    for _ in 0..ANCESTRY_DEPTH {
        if current != pid
            && let Some(owner) = pid_owner.get(&current)
            && owner != uid
        {
            found.insert(owner.clone());
        }
        let Some(parent) = scan.tree.parent(current) else {
            break;
        };
        // The tmux server is shared by every session: its environment says
        // nothing about one of them and nothing above it can either.
        if parent.name.starts_with("tmux") {
            break;
        }
        let env = scan.tree.spawn_env(current);
        for (key, family) in SPAWN_ENV {
            let value = env.get(key).map(|value| value.to_ascii_lowercase());
            if let Some(owner) = by_key.get(&(family.to_owned(), value.unwrap_or_default()))
                && owner != uid
            {
                found.insert(owner.clone());
            }
        }
        if let Some(claude_pid) = env
            .get("CLAUDE_PID")
            .and_then(|value| value.parse::<u32>().ok())
            && let Some(owner) = pid_owner.get(&claude_pid)
            && owner != uid
        {
            found.insert(owner.clone());
        }
        if parent.ppid <= 1 || parent.ppid == current {
            break;
        }
        current = parent.ppid;
    }
    found
}

// Process ancestry describes a launch/resume, not necessarily session creation.
// In particular an inherited identity must never make a newer session the
// creator of an older one. Existing display cycles are checked under the metadata writer lock.
fn newer_than_child(parent: &SessionRow, child: &SessionRow) -> bool {
    matches!((parse_created(&parent.created), parse_created(&child.created)),
        (Some(parent), Some(child)) if parent > child)
}

/// `{uid: {source, sid}}` without a per-scan memo
/// for every owned session whose chain names another listed session.
pub fn spawn_parents(
    scan: &Scan,
    sessions: &[SessionRow],
    owned: &IndexMap<String, Vec<i64>>,
) -> BTreeMap<String, NestParent> {
    let rows: HashMap<&str, &SessionRow> = sessions
        .iter()
        .map(|session| (session.uid.as_str(), session))
        .collect();
    let by_key: HashMap<(String, String), String> = sessions
        .iter()
        .filter(|session| !session.sid.is_empty())
        .map(|session| {
            (
                (session.source.clone(), session.sid.to_ascii_lowercase()),
                session.uid.clone(),
            )
        })
        .collect();
    let mut pid_owner: HashMap<u32, String> = HashMap::new();
    for (uid, pids) in owned {
        for pid in pids.iter().filter(|pid| **pid > 0) {
            pid_owner.insert(pid.unsigned_abs() as u32, uid.clone());
        }
    }
    let mut found = BTreeMap::new();
    for (uid, pids) in owned {
        if !rows.contains_key(uid.as_str())
            || sessions
                .iter()
                .any(|row| row.continued_in.as_deref() == Some(uid.as_str()))
        {
            continue;
        }
        // A later resume is a new process, not a newly created session.
        if matches!((parse_created(&rows[uid.as_str()].created), scan.started_at(pids)),
            (Some(created), Some(started)) if created + 1.0 < started)
        {
            continue;
        }
        let mut candidates = BTreeSet::new();
        for pid in pids.iter().filter(|pid| **pid > 0) {
            candidates.extend(spawn_candidates(
                scan,
                pid.unsigned_abs() as u32,
                uid,
                &by_key,
                &pid_owner,
            ));
        }
        candidates.remove(uid);
        let Some(parent) = latest_parent(&candidates, &rows, rows[uid.as_str()]) else {
            continue;
        };
        found.insert(
            uid.clone(),
            NestParent {
                node_id: None,
                source: parent.source.clone(),
                sid: parent.sid.clone(),
            },
        );
    }
    for child in sessions.iter().filter(|row| row.source == "opencode") {
        if found.contains_key(&child.uid) {
            continue;
        }
        if let Some(parent) = opencode_parent(scan, child, &rows, &by_key, &pid_owner) {
            found.insert(
                child.uid.clone(),
                NestParent {
                    node_id: None,
                    source: parent.source.clone(),
                    sid: parent.sid.clone(),
                },
            );
        }
    }
    found
}

/// The most recently created listed candidate that is not younger than `child`.
fn latest_parent<'a>(
    candidates: &BTreeSet<String>,
    rows: &HashMap<&str, &'a SessionRow>,
    child: &SessionRow,
) -> Option<&'a SessionRow> {
    candidates
        .iter()
        .filter_map(|candidate| rows.get(candidate.as_str()).copied())
        .filter(|parent| !newer_than_child(parent, child))
        .max_by(|a, b| a.created.cmp(&b.created).then_with(|| a.uid.cmp(&b.uid)))
}

/// Seconds a session's recorded birth may precede its process's start tick.
const OPENCODE_BIRTH_SLACK: f64 = 1.0;

/// OpenCode (`opencode run` from a tool shell) exposes no session id on its
/// command line or in its open files, so a top-level session is paired with the
/// OpenCode processes that were already running in its directory when it was
/// created. Every such process must lead to the same spawner; a process with
/// none (a user's own TUI there) or a different one leaves the row a root. A
/// native subagent child (`parent_id`) is not an external top-level launch.
fn opencode_parent<'a>(
    scan: &Scan,
    child: &SessionRow,
    rows: &HashMap<&str, &'a SessionRow>,
    by_key: &HashMap<(String, String), String>,
    pid_owner: &HashMap<u32, String>,
) -> Option<&'a SessionRow> {
    if scan.opencode.is_empty() {
        return None;
    }
    let created = parse_created(&child.created)?;
    let cwd = resolve_path(child.cwd.as_deref()?);
    let mut chosen: Option<&SessionRow> = None;
    let mut matched = false;
    for (pid, process) in &scan.opencode {
        if process.cwd != cwd || process.started > created + OPENCODE_BIRTH_SLACK {
            continue;
        }
        let candidates = spawn_candidates(scan, *pid, &child.uid, by_key, pid_owner);
        let parent = latest_parent(&candidates, rows, child)?;
        if chosen.is_some_and(|known| known.uid != parent.uid) {
            return None;
        }
        chosen = Some(parent);
        matched = true;
    }
    if !matched || opencode_subagent(child) {
        return None;
    }
    chosen
}

fn opencode_subagent(child: &SessionRow) -> bool {
    let Ok(text) = std::fs::read_to_string(std::path::Path::new(&child.path).join("summary.json"))
    else {
        return true;
    };
    let Ok(summary) = serde_json::from_str::<serde_json::Value>(&text) else {
        return true;
    };
    summary["session"]["parent_id"]
        .as_str()
        .is_some_and(|parent| !parent.is_empty())
}

/// Save only previously undecided attachments. The metadata lock rechecks the
/// decision so a concurrent user detach cannot be overwritten by this scan.
pub fn record(
    metadata: &MetadataStore,
    scan: &Scan,
    sessions: &[SessionRow],
    owned: &IndexMap<String, Vec<i64>>,
) -> Result<(), MetadataError> {
    let snapshot = metadata.snapshot()?;
    let by_key: HashMap<_, _> = sessions
        .iter()
        .map(|row| ((row.source.as_str(), row.sid.as_str()), row))
        .collect();
    let invalid: Vec<_> = sessions
        .iter()
        .filter_map(|child| {
            let saved = snapshot.row(&child.uid);
            if saved["nest_initialized"] != serde_json::json!(false) {
                return None;
            }
            let parent = snapshot.nest_parent(&child.uid)?;
            if parent.node_id.is_some() {
                return None;
            }
            let row = by_key.get(&(parent.source.as_str(), parent.sid.as_str()))?;
            (newer_than_child(row, child)
                || row.continued_in.as_deref() == Some(child.uid.as_str()))
            .then(|| (child.uid.clone(), parent.clone()))
        })
        .collect();
    if !invalid.is_empty() {
        metadata.discard_invalid_initial_nest_parents(&invalid)?;
    }
    let found: Vec<_> = spawn_parents(scan, sessions, owned).into_iter().collect();
    if !found.is_empty() {
        let identities = sessions
            .iter()
            .map(|row| ((row.source.clone(), row.sid.clone()), row.uid.clone()))
            .collect();
        metadata.initialize_nest_parents(&found, &identities)?;
    }
    Ok(())
}

/// The first HTTP publication of new identities must use current process
/// evidence, even if /api/live cached a scan before their CLI was launched.
/// Called on the blocking reader with the inventory that will be signed.
pub fn prepare_list(
    scanner: &super::procscan::ProcScanner,
    metadata: &MetadataStore,
    rows: &[serde_json::Value],
) -> Result<(), MetadataError> {
    let sessions: Vec<_> = rows.iter().filter_map(SessionRow::from_value).collect();
    let scan = scanner.scan_blocking();
    let active = scan.active_processes(&sessions);
    record(metadata, &scan, &sessions, &active.owned)
}

async fn tick(state: &AppState) -> Result<(), String> {
    let (Some(scanner), Some(metadata)) = (&state.proc_scan, &state.metadata) else {
        return Ok(());
    };
    let document = state
        .reader
        .run_wait(&state.shutdown, |store| store.list_recent())
        .await
        .map_err(|error| error.message)?;
    let snapshot = scanner
        .snapshot(false)
        .await
        .map_err(|error| error.to_string())?;
    let metadata = metadata.clone();
    tokio::task::spawn_blocking(move || {
        let sessions = SessionRow::from_list(&document);
        let active = snapshot.scan.active_processes(&sessions);
        record(&metadata, &snapshot.scan, &sessions, &active.owned).map_err(|error| error.message)
    })
    .await
    .map_err(|_| "nest discovery failed".to_owned())?
}

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = state.shutdown.cancelled() => return,
                result = tick(&state) => {
                    if let Err(error) = result { eprintln!("nest discovery failed: {error}"); }
                }
            }
            tokio::select! {
                _ = state.shutdown.cancelled() => return,
                _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {}
            }
        }
    });
}
