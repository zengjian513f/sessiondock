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
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::body::Bytes;
use indexmap::IndexMap;
use serde_json::Value;

use crate::{
    lifecycle::model::Record,
    runtime::{
        Generation, RuntimeSnapshot,
        procscan::{Scan, SessionRow, parse_created},
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
    pub rows: Arc<[SessionRow]>,
    pub sdk_uids: Vec<String>,
    pub retained_sdk_uids: Vec<String>,
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
            && (Arc::ptr_eq(&self.rows, &other.rows) || self.rows == other.rows)
            && self.sdk_uids == other.sdk_uids
            && self.retained_sdk_uids == other.retained_sdk_uids
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

pub struct CatalogDocCache {
    node_id: String,
    document: Arc<Value>,
    pub by_uid: HashMap<String, process_links::Session>,
    pub sessions: Vec<process_links::Session>,
    pub identities: BTreeMap<(String, String), String>,
}

struct CatalogWireEntry {
    node_id: String,
    boot_id: String,
    document: Arc<Value>,
    owners: Vec<process_links::agent::Owner>,
    full_bytes: Arc<[u8]>,
    bindings_bytes: Arc<[u8]>,
}

type TopologyCache = (Arc<Value>, Arc<[SessionRow]>);
type AttributionCache = (
    Arc<Scan>,
    Arc<[SessionRow]>,
    Arc<IndexMap<String, Vec<i64>>>,
);
pub type CatalogFrames = (Arc<[u8]>, Arc<[u8]>);

#[derive(Default)]
pub struct PollCache {
    live: Mutex<Option<LiveEntry>>,
    topology: Mutex<Option<TopologyCache>>,
    catalog_doc: Mutex<Option<Arc<CatalogDocCache>>>,
    attribution: Mutex<Option<AttributionCache>>,
    catalog_wire: Mutex<Option<CatalogWireEntry>>,
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
    /// Cached topology rows extracted from a published `recent_document`.
    pub fn topology(&self, document: &Arc<Value>) -> Arc<[SessionRow]> {
        {
            let cache = lock(&self.topology);
            if let Some((cached_doc, rows)) = cache.as_ref()
                && Arc::ptr_eq(cached_doc, document)
            {
                return rows.clone();
            }
        }
        let built: Arc<[SessionRow]> = topology(document).into();
        *lock(&self.topology) = Some((document.clone(), built.clone()));
        built
    }

    /// The `/api/live` body assembled from exactly these sources.
    pub fn live(&self, key: &LiveKey) -> Option<Arc<Value>> {
        let cache = lock(&self.live);
        let entry = cache.as_ref()?;
        entry.key.same(key).then(|| entry.response.clone())
    }

    pub fn store_live(&self, key: LiveKey, response: Arc<Value>) {
        *lock(&self.live) = Some(LiveEntry { key, response });
    }

    /// The most recently assembled `/api/live` body regardless of key. Used by
    /// the cheap per-session runtime read, which must not recompute a full-fleet
    /// scan: it reports fresh SDK status plus the last known external/tmux
    /// membership, and a background full refresh corrects any staleness.
    pub fn latest_live(&self) -> Option<Arc<Value>> {
        lock(&self.live)
            .as_ref()
            .map(|entry| entry.response.clone())
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

    /// Cached collector session catalog + `(source, sid) -> uid` map for a published `document`.
    pub fn catalog_doc(
        &self,
        node_id: &str,
        document: &Arc<Value>,
        rows: &[SessionRow],
    ) -> Arc<CatalogDocCache> {
        {
            let cache = lock(&self.catalog_doc);
            if let Some(entry) = cache.as_ref()
                && entry.node_id == node_id
                && Arc::ptr_eq(&entry.document, document)
            {
                return entry.clone();
            }
        }
        let titles: HashMap<_, _> = document["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| Some((v["uid"].as_str()?, v["title"].as_str()?)))
            .collect();
        let mut by_uid = HashMap::with_capacity(rows.len());
        let mut sessions = Vec::with_capacity(rows.len());
        let mut identities = BTreeMap::new();
        for row in rows {
            let session = process_links::Session {
                node_id: node_id.to_owned(),
                source: row.source.clone(),
                sid: row.sid.clone(),
                title: titles.get(row.uid.as_str()).map(|s| (*s).to_owned()),
                created: parse_created(&row.created),
            };
            identities.insert((row.source.clone(), row.sid.clone()), row.uid.clone());
            by_uid.insert(row.uid.clone(), session.clone());
            sessions.push(session);
        }
        sessions.sort_by(|a, b| (&a.source, &a.sid).cmp(&(&b.source, &b.sid)));
        let built = Arc::new(CatalogDocCache {
            node_id: node_id.to_owned(),
            document: document.clone(),
            by_uid,
            sessions,
            identities,
        });
        *lock(&self.catalog_doc) = Some(built.clone());
        built
    }

    /// Cached `Scan::attribution_processes` result for an unchanged `(scan, rows)` pair.
    pub fn attribution(
        &self,
        scan: &Arc<Scan>,
        rows: &Arc<[SessionRow]>,
    ) -> Arc<IndexMap<String, Vec<i64>>> {
        {
            let cache = lock(&self.attribution);
            if let Some((cached_scan, cached_rows, owned)) = cache.as_ref()
                && Arc::ptr_eq(cached_scan, scan)
                && Arc::ptr_eq(cached_rows, rows)
            {
                return owned.clone();
            }
        }
        let owned = Arc::new(scan.attribution_processes(rows));
        *lock(&self.attribution) = Some((scan.clone(), rows.clone(), owned.clone()));
        owned
    }

    /// Pre-serialized `Request::Catalog` frames (`bindings_only` false/true) for unchanged inputs.
    pub fn catalog_wire(
        &self,
        node_id: &str,
        boot_id: &str,
        document: &Arc<Value>,
        owners: &[process_links::agent::Owner],
        sessions: &[process_links::Session],
    ) -> std::io::Result<CatalogFrames> {
        {
            let cache = lock(&self.catalog_wire);
            if let Some(entry) = cache.as_ref()
                && entry.node_id == node_id
                && entry.boot_id == boot_id
                && Arc::ptr_eq(&entry.document, document)
                && entry.owners == owners
            {
                return Ok((entry.full_bytes.clone(), entry.bindings_bytes.clone()));
            }
        }
        let request = process_links::agent::Request::Catalog(process_links::agent::Catalog {
            node_id: node_id.to_owned(),
            boot_id: boot_id.to_owned(),
            owners: owners.to_vec(),
            sessions: sessions.to_vec(),
        });
        let full_bytes: Arc<[u8]> = process_links::agent::encode_request(&request, false)?.into();
        let bindings_bytes: Arc<[u8]> =
            process_links::agent::encode_request(&request, true)?.into();
        *lock(&self.catalog_wire) = Some(CatalogWireEntry {
            node_id: node_id.to_owned(),
            boot_id: boot_id.to_owned(),
            document: document.clone(),
            owners: owners.to_vec(),
            full_bytes: full_bytes.clone(),
            bindings_bytes: bindings_bytes.clone(),
        });
        Ok((full_bytes, bindings_bytes))
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
