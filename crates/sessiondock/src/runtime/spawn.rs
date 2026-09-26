//! Which session started a running session.
//!
//! A CLI main process's ancestor chain carries the clues: an ancestor is itself
//! another session's CLI main process, or some level's environment names another
//! session (`SPAWN_ENV`, `CLAUDE_PID`). ptyhost strips the identity from the CLI
//! child's environment and systemd adopts the host away from the launcher's
//! tree, but the host itself still carries the spawner's environment, so every
//! level is examined; a `tmux` server is shared by every session and ends the
//! walk. When a chain names both a grandparent and a parent (Claude → Codex →
//! Grok) the later-created one wins: a child is born after its parent. The
//! relation is only visible while both processes exist, so it is persisted at
//! once (write once) by a 10 s background tick and by every `/api/live`.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{Arc, Mutex},
    time::Duration,
};

use indexmap::IndexMap;

use super::procscan::{ANCESTRY_DEPTH, ProcScanner, SPAWN_ENV, Scan, SessionRow, parse_created};
use crate::metadata::{MetadataError, MetadataStore, SpawnedBy};

/// Ten-second background tick.
pub const WATCH_INTERVAL: Duration = Duration::from_secs(10);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
// creator of an older one. Unknown dates retain the existing discovery behavior.
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
) -> BTreeMap<String, SpawnedBy> {
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
        let Some(parent) = candidates
            .iter()
            .filter_map(|candidate| rows.get(candidate.as_str()))
            .filter(|parent| !newer_than_child(parent, rows[uid.as_str()]))
            .max_by(|a, b| a.created.cmp(&b.created).then_with(|| a.uid.cmp(&b.uid)))
        else {
            continue;
        };
        found.insert(
            uid.clone(),
            SpawnedBy {
                source: parent.source.clone(),
                sid: parent.sid.clone(),
            },
        );
    }
    found
}

struct Memo {
    scan: Arc<Scan>,
    found: BTreeMap<String, SpawnedBy>,
}

/// Records spawners into the metadata store: from the background tick and from
/// `/api/live`. Results are memoised per scan and
/// sessions already recorded are skipped before writing.
pub struct SpawnWatcher {
    scanner: Arc<ProcScanner>,
    metadata: Arc<MetadataStore>,
    memo: Mutex<Option<Memo>>,
}

impl SpawnWatcher {
    pub fn new(scanner: Arc<ProcScanner>, metadata: Arc<MetadataStore>) -> Self {
        Self {
            scanner,
            metadata,
            memo: Mutex::new(None),
        }
    }

    pub fn scanner(&self) -> &Arc<ProcScanner> {
        &self.scanner
    }

    /// Spawners for this scan, computed once per scan snapshot.
    fn parents(
        &self,
        scan: &Arc<Scan>,
        sessions: &[SessionRow],
        owned: &IndexMap<String, Vec<i64>>,
    ) -> BTreeMap<String, SpawnedBy> {
        if let Some(memo) = lock(&self.memo).as_ref()
            && Arc::ptr_eq(&memo.scan, scan)
        {
            return memo.found.clone();
        }
        let found = spawn_parents(scan, sessions, owned);
        *lock(&self.memo) = Some(Memo {
            scan: scan.clone(),
            found: found.clone(),
        });
        found
    }

    /// Returns how many sessions were newly recorded.
    pub fn record(
        &self,
        scan: &Arc<Scan>,
        sessions: &[SessionRow],
        owned: &IndexMap<String, Vec<i64>>,
    ) -> Result<usize, MetadataError> {
        // Repair only relationships disproved by native creation timestamps.
        // Keep the rejected value on disk for diagnosis and preserve overrides.
        let snapshot = self.metadata.snapshot()?;
        let by_key: HashMap<_, _> = sessions
            .iter()
            .map(|row| ((row.source.as_str(), row.sid.as_str()), row))
            .collect();
        let invalid: Vec<_> = sessions
            .iter()
            .filter_map(|child| {
                let parent = snapshot.spawned_by(&child.uid)?;
                let row = by_key.get(&(parent.source.as_str(), parent.sid.as_str()))?;
                newer_than_child(row, child).then(|| (child.uid.clone(), parent.clone()))
            })
            .collect();
        self.metadata.invalidate_spawn_parents(&invalid)?;
        let found = self.parents(scan, sessions, owned);
        if found.is_empty() {
            return Ok(0);
        }
        let skip = self.metadata.snapshot()?.spawned_uids();
        let fresh: Vec<(String, SpawnedBy)> = found
            .into_iter()
            .filter(|(uid, _)| !skip.contains(uid))
            .collect();
        if fresh.is_empty() {
            return Ok(0);
        }
        self.metadata.record_spawn_parents(&fresh)
    }

    /// List, scan (shared 3 s cache), record.
    pub async fn tick(
        self: &Arc<Self>,
        reader: &crate::state::Reader,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<usize, String> {
        let document = reader
            .run_wait(cancel, |store| store.list_recent())
            .await
            .map_err(|error| error.message)?;
        let snapshot = self
            .scanner
            .snapshot(false)
            .await
            .map_err(|error| error.to_string())?;
        let watcher = self.clone();
        // Ancestry walks read the process table and the record writes the
        // metadata file: off the reactor.
        tokio::task::spawn_blocking(move || {
            let sessions = SessionRow::from_list(&document);
            let active = snapshot.scan.active_processes(&sessions);
            watcher
                .record(&snapshot.scan, &sessions, &active.owned)
                .map_err(|error| error.message)
        })
        .await
        .map_err(|_| "spawn watch tick failed".to_owned())?
    }

    /// Headless sessions an agent fans out often
    /// live and die while no page is open, so the service looks on its own at
    /// a fixed cadence; the scan shares the `/api/live` cache, so an open page
    /// costs almost nothing extra. Diagnostic only: a failed tick is dropped.
    pub fn spawn_loop(
        self: Arc<Self>,
        reader: crate::state::Reader,
        shutdown: tokio_util::sync::CancellationToken,
    ) {
        tokio::spawn(async move {
            loop {
                let _ = self.tick(&reader, &shutdown).await;
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => return,
                    _ = tokio::time::sleep(WATCH_INTERVAL) => {}
                }
            }
        });
    }
}

#[cfg(all(test, unix))]
mod tests;
