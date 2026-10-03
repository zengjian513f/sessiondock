//! Response caches of the two liveness polls every open tab repeats every
//! 3 s: `/api/live` and `/api/term/list` (docs/liveness.md "Response caches").
//! Both answers are pure functions of a few source snapshots that already
//! have their own freshness windows — the `/proc` scan (3 s), the shared
//! managed observation (2 s), the published session list (`OPEN_TTL`, 3 s)
//! and the lifecycle receipt list — so a hot request only checks that those
//! sources are the ones the entry was assembled from and hands back the
//! same document. The predecessor kept the same caches (`_live_views`,
//! `_panes`); the shapes and orderings are unchanged.
//!
//! Invalidation: every source identity is part of the key, and the
//! lifecycle mutation counter ([`crate::lifecycle::service::LifecycleService::generation`])
//! turns over after every create, kill, takeover, bind, stop or discard, so
//! an API mutation misses at once while a change made behind the server's
//! back (a host started by another backend, a session file appended) shows
//! up when its source refreshes, within one poll interval. `?force=1`
//! bypasses both caches and re-populates them.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::body::Bytes;
use serde_json::Value;

use crate::{
    lifecycle::model::Record,
    runtime::{
        Generation, RuntimeSnapshot,
        procscan::{Scan, SessionRow},
    },
};

/// How long an assembled `/api/term/list` is served: the receipt list and
/// the host discovery behind it carry no version of their own, so the
/// entry expires like the predecessor's `PANES_TTL`.
pub const TERM_LIST_TTL: Duration = Duration::from_secs(2);
/// How long one refreshed receipt list (`LifecycleService::list`, every live
/// receipt probed through its host) serves both the exit receipts of the
/// shared managed observation and `/api/term/list.pending`, so the two
/// polls of one 2 s window cost one list instead of two.
pub const RECEIPTS_TTL: Duration = Duration::from_secs(2);

/// The process-table half of a `/api/live` key.
#[derive(Clone)]
pub enum ScanState {
    /// One completed scan, by identity (the scanner republishes the same
    /// `Arc` within its TTL).
    Scan(Arc<Scan>),
    Unsupported,
    Failed,
}

impl ScanState {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Scan(a), Self::Scan(b)) => Arc::ptr_eq(a, b),
            (Self::Unsupported, Self::Unsupported) | (Self::Failed, Self::Failed) => true,
            _ => false,
        }
    }
}

/// Everything a `/api/live` answer is a function of. `rows`
/// are the list topology — the list-row fields the scan
/// pairs processes with — so a session file that only grew (a new
/// published list with the same topology) keeps the entry.
pub struct LiveKey {
    pub scan: ScanState,
    /// The shared managed observation, by identity; `None` without a host
    /// directory.
    pub runtime: Option<Arc<RuntimeSnapshot>>,
    pub generation: Generation,
    pub rows: Vec<SessionRow>,
}

impl LiveKey {
    fn same(&self, other: &Self) -> bool {
        self.scan.same(&other.scan)
            && match (&self.runtime, &other.runtime) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
            && self.generation == other.generation
            && self.rows == other.rows
    }
}

struct LiveEntry {
    key: LiveKey,
    /// The assembled body without its per-request fields (`managed.cache`,
    /// `scan.cache`, `scan.spawned_recorded`), which the handler fills in.
    response: Arc<Value>,
}

struct TermEntry {
    built: Instant,
    generation: Generation,
    bytes: Bytes,
}

struct ReceiptsEntry {
    built: Instant,
    generation: Generation,
    records: Arc<Vec<Record>>,
}

#[derive(Default)]
pub struct PollCache {
    live: Mutex<Option<LiveEntry>>,
    term_list: Mutex<Option<TermEntry>>,
    receipts: Mutex<Option<ReceiptsEntry>>,
    /// Single flight for the receipt list refresh: a second poll arriving
    /// during one waits for it instead of asking the coordinator again.
    pub receipts_flight: tokio::sync::Mutex<()>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl PollCache {
    /// The `/api/live` body assembled from exactly these sources.
    pub fn live(&self, key: &LiveKey) -> Option<Arc<Value>> {
        let cache = lock(&self.live);
        let entry = cache.as_ref()?;
        entry.key.same(key).then(|| entry.response.clone())
    }

    pub fn store_live(&self, key: LiveKey, response: Arc<Value>) {
        *lock(&self.live) = Some(LiveEntry { key, response });
    }

    /// The `/api/term/list` bytes assembled under `generation`, younger than [`TERM_LIST_TTL`].
    pub fn term_list(&self, generation: Generation) -> Option<Bytes> {
        self.term_list_at(generation, Instant::now())
    }

    fn term_list_at(&self, generation: Generation, now: Instant) -> Option<Bytes> {
        let cache = lock(&self.term_list);
        let entry = cache.as_ref()?;
        (entry.generation == generation
            && now.saturating_duration_since(entry.built) < TERM_LIST_TTL)
            .then(|| entry.bytes.clone())
    }

    pub fn store_term_list(&self, generation: Generation, bytes: Bytes) {
        self.store_term_list_at(generation, bytes, Instant::now());
    }

    fn store_term_list_at(&self, generation: Generation, bytes: Bytes, built: Instant) {
        *lock(&self.term_list) = Some(TermEntry {
            built,
            generation,
            bytes,
        });
    }
}

impl PollCache {
    /// The receipt list refreshed under `generation` less than
    /// [`RECEIPTS_TTL`] ago.
    pub fn receipts(&self, generation: Generation) -> Option<Arc<Vec<Record>>> {
        self.receipts_at(generation, Instant::now())
    }

    fn receipts_at(&self, generation: Generation, now: Instant) -> Option<Arc<Vec<Record>>> {
        let cache = lock(&self.receipts);
        let entry = cache.as_ref()?;
        (entry.generation == generation
            && now.saturating_duration_since(entry.built) < RECEIPTS_TTL)
            .then(|| entry.records.clone())
    }

    pub fn store_receipts(&self, generation: Generation, records: Arc<Vec<Record>>) {
        self.store_receipts_at(generation, records, Instant::now());
    }

    fn store_receipts_at(&self, generation: Generation, records: Arc<Vec<Record>>, built: Instant) {
        *lock(&self.receipts) = Some(ReceiptsEntry {
            built,
            generation,
            records,
        });
    }
}

/// The list rows paired with processes, in published order.
pub fn topology(document: &Value) -> Vec<SessionRow> {
    document["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(SessionRow::from_value)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(scan: ScanState, generation: Generation, rows: Vec<SessionRow>) -> LiveKey {
        LiveKey {
            scan,
            runtime: None,
            generation,
            rows,
        }
    }

    fn row(uid: &str) -> SessionRow {
        SessionRow::from_value(&json!({"uid": uid, "source": "claude", "sid": uid})).unwrap()
    }

    #[test]
    fn live_entries_answer_the_same_sources_only() {
        let cache = PollCache::default();
        let body = Arc::new(json!({"uids": ["claude:a"]}));
        cache.store_live(
            key(ScanState::Unsupported, 1, vec![row("claude:a")]),
            body.clone(),
        );
        let hit = cache
            .live(&key(ScanState::Unsupported, 1, vec![row("claude:a")]))
            .expect("same sources hit");
        assert!(Arc::ptr_eq(&hit, &body));
        // A mutation (generation), a changed topology, another scan state
        // all miss.
        assert!(
            cache
                .live(&key(ScanState::Unsupported, 2, vec![row("claude:a")]))
                .is_none()
        );
        assert!(
            cache
                .live(&key(ScanState::Unsupported, 1, vec![row("claude:b")]))
                .is_none()
        );
        assert!(
            cache
                .live(&key(ScanState::Failed, 1, vec![row("claude:a")]))
                .is_none()
        );
    }

    #[test]
    fn term_list_entries_expire_and_turn_over_with_the_generation() {
        let cache = PollCache::default();
        let built = Instant::now();
        cache.store_term_list_at(3, Bytes::from_static(b"{}"), built);
        assert_eq!(
            cache.term_list_at(3, built + Duration::from_millis(500)),
            Some(Bytes::from_static(b"{}"))
        );
        assert!(cache.term_list_at(4, built).is_none(), "mutation");
        assert!(
            cache.term_list_at(3, built + TERM_LIST_TTL).is_none(),
            "TTL turnover"
        );
    }

    #[test]
    fn the_receipt_list_is_shared_within_its_ttl_and_generation() {
        let cache = PollCache::default();
        let built = Instant::now();
        let records = Arc::new(Vec::new());
        cache.store_receipts_at(5, records.clone(), built);
        assert!(Arc::ptr_eq(
            &cache
                .receipts_at(5, built + Duration::from_secs(1))
                .unwrap(),
            &records
        ));
        assert!(cache.receipts_at(6, built).is_none(), "mutation");
        assert!(
            cache.receipts_at(5, built + RECEIPTS_TTL).is_none(),
            "TTL turnover"
        );
    }

    #[test]
    fn topology_keeps_the_rows_in_order() {
        let document = json!({"sessions": [
            {"uid": "claude:a", "source": "claude", "sid": "a"},
            {"uid": "codex:b", "source": "codex", "sid": "b"},
            {"source": "grok"},
        ]});
        let rows = topology(&document);
        assert_eq!(
            rows.iter().map(|row| row.uid.as_str()).collect::<Vec<_>>(),
            ["claude:a", "codex:b"]
        );
    }
}
