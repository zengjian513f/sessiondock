//! Initialize the single sidebar parent from an observed local CLI launch.
//! Native file ownership identifies the child; inherited session IDs or an
//! owning CLI ancestor identify its initiator, including detached dispatchers.
//! Explicit attachment/detachment wins permanently. No native history is edited.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use indexmap::IndexMap;

use super::procscan::{ANCESTRY_DEPTH, SPAWN_ENV, Scan, SessionRow, parse_created};
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
        if !rows.contains_key(uid.as_str()) {
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

/// Save only previously undecided attachments. The metadata lock rechecks the
/// decision so a concurrent user detach cannot be overwritten by this scan.
pub fn record(
    metadata: &MetadataStore,
    scan: &Scan,
    sessions: &[SessionRow],
    owned: &IndexMap<String, Vec<i64>>,
) -> Result<(), MetadataError> {
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
