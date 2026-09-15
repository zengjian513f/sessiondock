//! Session read model: the lazy index (`index/`) is the only inventory, and
//! per-session views (`views/`) are built on demand. Design: docs/read-model.md.
//!
//! `SessionStore` is a thin facade. A list is the index's rows (directory
//! walk + `stat` + bounded head/tail summaries, no file is ever parsed
//! whole) decorated with the persisted metadata and re-signed; a session is
//! opened by streaming exactly its file(s) through `Views`, which keeps a
//! bounded LRU. Nothing here retains message objects for sessions nobody
//! opened, and one file's change never fails the list. Native files are
//! never modified.

mod debug_runs;
pub use debug_runs::{DEBUG_RUNS_FILENAME, RunIndex, debug_run_of};
mod history;
mod index;
mod native_input;
mod native_media;
mod native_tail;
pub use native_tail::{NativeCheckpoint, NativeTail, TailError, TailRecord};
mod pages;
mod views;
pub use pages::PageStore;
pub use views::ViewSnapshot;
pub(crate) use views::{
    Dependencies, Event, MessageBody, Parsed, Selected, ViewRequest, Views, open_transient,
    validate_message_query,
};
#[cfg(test)]
pub(crate) use views::{
    EncodedEvents, View, ViewParts, ViewStats, project_selected, read_bounded, semantic_anchor,
};
mod providers;
mod records;
pub(crate) use records::string_reader::JsonStringReader;
mod scope;
pub(crate) use scope::CatalogEntry as NativeCatalogEntry;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

use axum::body::Bytes;

use crate::metadata::{MetadataSnapshot, MetadataStore};
use serde::Deserialize;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

pub(crate) use index::{CandidateRef, IndexSnapshot};

/// Resident-memory hygiene. glibc keeps freed memory in
/// per-thread arenas and rarely returns it on its own; with 256 runtime
/// threads a dropped 300 MB projection stayed in RSS. `malloc_trim(0)` walks
/// every arena and gives free pages back, so an explicit trim after each
/// fresh parse, eviction, large response and search keeps RSS honest.
/// Capping the arena count (`M_ARENA_MAX=2`) was measured and rejected: with
/// the parallel index reads and blocking readers it turned malloc into a
/// futex storm (564k vs 2.6k futex calls over ten `/proc` scans) and cost
/// more CPU than it saved memory. Non-glibc targets compile to no-ops.
pub mod memory {
    /// Return free heap to the OS. Cheap when nothing is free; never blocks
    /// on other threads' allocations.
    pub fn release() {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        // SAFETY: malloc_trim(0) releases free memory at the top of arenas
        // and free pages inside them; it touches no live allocation.
        unsafe {
            libc::malloc_trim(0);
        }
    }

    /// How long a large response gets to leave the process before the
    /// coalesced trim runs (loopback sends 50 MB in well under this).
    const SOON: std::time::Duration = std::time::Duration::from_millis(500);

    /// `release`, off the request path: a large response body (tens of MB
    /// for a full read) is freed on whichever thread finished sending it
    /// and would otherwise stay in that arena; a hot read itself allocates
    /// nothing else worth a ~10 ms trim. One sleeping thread coalesces
    /// requests and trims once per burst, `SOON` after its last request
    /// (a burst that never pauses is trimmed every `4 × SOON` at most).
    pub fn release_soon() {
        use std::sync::{OnceLock, mpsc};
        static TRIMMER: OnceLock<mpsc::SyncSender<()>> = OnceLock::new();
        let sender = TRIMMER.get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel::<()>(1);
            std::thread::Builder::new()
                .name("memory-trim".into())
                .spawn(move || {
                    while receiver.recv().is_ok() {
                        for _ in 0..4 {
                            std::thread::sleep(SOON);
                            if receiver.try_recv().is_err() {
                                break;
                            }
                        }
                        release();
                    }
                })
                .expect("spawn the memory trim thread");
            sender
        });
        // A full slot means a trim is already pending.
        let _ = sender.try_send(());
    }
}

pub(crate) mod budgets {
    const KIB: usize = 1024;
    const MIB: usize = 1024 * KIB;
    /// 视图缓存：LRU 条数 + 序列化消息合计字节（默认值；运行时以
    /// [`caches()`] 为准，可用环境变量覆盖）。
    pub const VIEW_CACHE_ENTRIES: usize = 16;
    pub const VIEW_CACHE_BYTES: usize = 128 * MIB;
    pub const INLINE_STRING_BYTES: usize = 64 * 1024;
    /// Reusable decoded-AST cache behind incremental append: one entry per
    /// cached view, weight-bounded like the view cache. Its weight is a
    /// conservative estimate of the resident `serde_json::Value` tree, so
    /// this is the knob that decides how much of an open file stays in RAM
    /// between appends (defaults; runtime values in [`caches()`]).
    pub const AST_CACHE_ENTRIES: usize = 8;
    pub const AST_CACHE_BYTES: usize = 64 * MIB;

    /// Resident-memory budgets of the two in-process caches.
    /// Fixed once at startup from `Config::caches`; `caches()` before
    /// `configure` (library tests) yields the defaults above.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Caches {
        /// Parsed-file / view LRU entries (`SESSIONDOCK_CACHE_ENTRIES`).
        pub view_entries: usize,
        /// Serialized message bytes the view LRU keeps (`SESSIONDOCK_VIEW_CACHE_MB`).
        pub view_bytes: usize,
        /// Decoded-AST entries kept for append reuse (same entry count).
        pub ast_entries: usize,
        /// Estimated resident AST bytes kept (`SESSIONDOCK_AST_CACHE_MB`; 0
        /// keeps no AST: every append re-decodes its file).
        pub ast_bytes: usize,
    }
    impl Default for Caches {
        fn default() -> Self {
            Self {
                view_entries: VIEW_CACHE_ENTRIES,
                view_bytes: VIEW_CACHE_BYTES,
                ast_entries: AST_CACHE_ENTRIES,
                ast_bytes: AST_CACHE_BYTES,
            }
        }
    }
    static CACHES: std::sync::OnceLock<Caches> = std::sync::OnceLock::new();
    /// The process-wide cache budgets.
    pub fn caches() -> &'static Caches {
        CACHES.get_or_init(Caches::default)
    }
    /// Fix the budgets for this process. The first call wins; a later call
    /// with different values reports `false` (the caller logs it).
    pub fn configure(caches: Caches) -> bool {
        *CACHES.get_or_init(|| caches) == caches
    }
}

const CURSOR_SCHEMA: &str = "rs-m2-1";
/// How old the published list may be when a view is opened;
/// `/api/sessions` keeps the index's own 500 ms window.
const OPEN_TTL: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Clone, Debug, Default)]
pub struct SessionRoots {
    pub claude: Option<PathBuf>,
    pub codex: Option<PathBuf>,
    pub grok: Option<PathBuf>,
}

/// Identity proven by native records and inventory ownership, independent of
/// display metadata and of any delivery implementation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeScope {
    pub source: String,
    pub uid: String,
    pub session_id: String,
    pub agent_id: Option<String>,
}

/// Delivery executor evidence: one physical checkpoint of a Claude
/// main session, in the same terms the message cursor uses. `source_identity`
/// is the validated view identity, never a display SID or file name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeFence {
    pub source_identity: String,
    pub offset: u64,
    pub head: String,
    pub anchor: String,
}

/// One projected human `user` input committed after a fence, with the exact
/// physical byte span of its native record and the record's own UUID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeUserInput {
    pub uuid: String,
    pub start: u64,
    pub end: u64,
    pub text: String,
    pub ts: String,
}

pub struct NativeInputRead {
    pub scope: NativeScope,
    pub current: NativeFence,
    /// False when the supplied fence is no longer a checkpoint of this view
    /// (rewrite/truncation/identity change); `inputs` is then empty.
    pub fence_valid: bool,
    pub inputs: Vec<NativeUserInput>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessageQuery {
    pub agent: String,
    pub start: u64,
    pub head: String,
    pub anchor: String,
    /// Legacy query parameters are strings, not JSON booleans.
    pub append: String,
    pub window: String,
}

#[derive(Clone, Debug)]
pub struct SessionError {
    pub status: u16,
    pub message: String,
}

impl SessionError {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SessionError {}

/// `dev/ino/ctime` identity plus size and mtime of one native file: the unit
/// views invalidate on (the index keys its summaries by `dev/ino/size/mtime`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileStamp {
    pub size: u64,
    pub modified: u128,
    pub identity: String,
    pub file_identity: String,
}

impl FileStamp {
    /// The same file version as an index stamp: size, mtime and `dev/ino`
    /// (the index does not record ctime).
    fn matches(&self, stamp: &index::Stamp) -> bool {
        self.size == stamp.size
            && self.modified == stamp.mtime_ns
            && self.file_identity == format!("{}:{}", stamp.dev, stamp.ino)
    }
}

/// One native file to open: its provider, configured root, display path,
/// data file, optional summary sidecar and the stamps it was last seen with.
/// Paths come from the index's directory walk only (`CandidateRef`), never
/// from a request; `Views` restamps before every open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Candidate {
    pub source: &'static str,
    pub root: PathBuf,
    pub path: PathBuf,
    pub data: PathBuf,
    pub summary: Option<PathBuf>,
    pub stamps: Vec<FileStamp>,
}

impl Candidate {
    /// Grok's summary is mandatory; chat_history.jsonl may genuinely not exist.
    /// [summary] represents that absence, while [chat, summary] represents a
    /// present chat (including a zero-byte file). Other providers keep [data].
    pub(crate) fn data_stamp(&self) -> Option<&FileStamp> {
        if self.source == "grok" && self.summary.is_some() && self.stamps.len() == 1 {
            None
        } else {
            self.stamps.first()
        }
    }
    pub(crate) fn summary_stamp(&self) -> Option<&FileStamp> {
        self.summary.as_ref().and_then(|_| self.stamps.last())
    }

    /// The index entry as an openable candidate. Stamps are left empty on
    /// purpose: `Views` re-`stat`s and treats any difference from its cached
    /// parse as an append or rewrite of that one file.
    fn of(entry: &CandidateRef) -> Self {
        Self {
            source: entry.source,
            root: entry.root.clone(),
            path: entry.path.clone(),
            data: entry.data.clone(),
            summary: entry.summary_path.clone(),
            stamps: Vec::new(),
        }
    }

    /// Whether a parse made from this (restamped) candidate describes exactly
    /// the file version the index published for `entry`.
    fn is_version_of(&self, entry: &CandidateRef) -> bool {
        self.path == entry.path
            && match (self.data_stamp(), entry.stamp) {
                (Some(stamp), Some(index)) => stamp.matches(&index),
                (None, None) => true,
                _ => false,
            }
            && match (self.summary_stamp(), entry.summary_stamp) {
                (Some(stamp), Some(index)) => stamp.matches(&index),
                (None, None) => true,
                _ => false,
            }
    }
}

/// The list as published by the facade: the index snapshot it came from,
/// the metadata revision applied and the signed rows. `built_at` and `sig`
/// are kept while the rows are unchanged, so a forced rescan that finds
/// nothing new republishes the same document.
struct Published {
    index: Arc<IndexSnapshot>,
    metadata: Option<Arc<MetadataSnapshot>>,
    document: Arc<Value>,
    /// uid → position in `document["sessions"]`.
    positions: BTreeMap<String, usize>,
}

impl Published {
    fn rows(&self) -> &[Value] {
        self.document["sessions"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
    fn row(&self, uid: &str) -> Option<&Value> {
        self.positions
            .get(uid)
            .and_then(|position| self.rows().get(*position))
    }
    fn metadata_revision(&self) -> Option<u64> {
        self.metadata.as_ref().map(|snapshot| snapshot.revision())
    }
}

/// The candidate list one search scans, frozen at admission: rows in the
/// published `updated` order plus the index they came from so each view can
/// be built on demand. Views are not retained by the pool; a cached one is
/// borrowed, a miss is streamed and dropped (docs/read-model.md "搜索").
pub struct SearchPool {
    /// Shared with the store's row cache: the same rows serve every search
    /// over the same published list and registry.
    pub rows: Arc<Vec<Value>>,
    published: Arc<Published>,
}

/// The candidate rows of the last search pool, reused while the published
/// list, the registry and the run id are the same (a search that parsed
/// nothing then allocates no rows).
struct SearchRows {
    published: Arc<Published>,
    runs: Arc<RunIndex>,
    run_id: String,
    rows: Arc<Vec<Value>>,
}

/// The version of one main view's searchable text (`SessionStore::search_version`):
/// a canonical JSON key that changes exactly when a parse could yield a
/// different body, plus the data file's `dev:ino` and size for the
/// search-text cache's parse budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchVersion {
    pub key: Value,
    pub data: Option<(String, u64)>,
}

fn stamp_json(stamp: &FileStamp) -> Value {
    json!([stamp.size, stamp.modified.to_string(), stamp.identity])
}

/// One published list plus the index behind it, for consumers that need
/// rows and the verified native catalog from the same snapshot (trash file
/// sets, lifecycle resume/takeover, `/api/live`). No file is parsed for it.
pub struct SessionSnapshot {
    pub list: Value,
    index: Arc<IndexSnapshot>,
}

impl SessionSnapshot {
    /// Control identity evidence from the index summaries of this snapshot.
    pub fn native_catalog(&self) -> crate::runtime::NativeCatalog {
        self.index.catalog()
    }
}

/// One debug-run view of a published list,
/// kept while the list and the registry are the same so the polling
/// default view costs no re-filtering (one per run id, so a monkey's view
/// and the ordinary view alternating keep both documents).
struct Filtered {
    published: Arc<Published>,
    runs: Arc<RunIndex>,
    document: Arc<Value>,
}

struct ListState {
    published: Option<Arc<Published>>,
    /// By run id (`""` = the ordinary view); at most [`SERIALIZED_VIEWS`].
    filtered: BTreeMap<String, Filtered>,
    /// Owner uids that vanished from the index since the views were last
    /// touched; applied before the next use of the view cache.
    evictions: Vec<String>,
    search_rows: Option<SearchRows>,
}

/// The serialized `/api/sessions` body of one view (docs/read-model.md
/// "列表响应字节缓存"): the exact bytes a hot request returns while the view
/// document (`sig`, `built_at`, rows) and the cached-view set it borrowed
/// decorations from are the ones it was rendered for.
struct SerializedList {
    /// The view document the bytes were rendered from (pointer identity:
    /// `publish` republishes the same `Arc` while `sig` is unchanged).
    document: Arc<Value>,
    /// `Views::revision` when the decorations were borrowed.
    revision: u64,
    bytes: Bytes,
}

/// At most this many debug-run views keep serialized bytes (the ordinary
/// view plus a few monkeys); more evict everything, like the predecessor.
const SERIALIZED_VIEWS: usize = 8;

/// Dependency resolution over one index snapshot: Codex `history_base`
/// parents by native thread id, with the graph's 501/409 codes. Paths are
/// never joined; they come from indexed candidates.
struct IndexDeps<'a> {
    index: &'a IndexSnapshot,
}

impl Dependencies for IndexDeps<'_> {
    fn thread(&self, source: &str, thread_id: &str) -> Result<Candidate, SessionError> {
        self.index.thread(source, thread_id).map(Candidate::of)
    }
}

/// A view request assembled from one published list.
struct Prepared {
    published: Arc<Published>,
    request: ViewRequest,
}

pub struct SessionStore {
    index: index::Index,
    metadata: Option<Arc<MetadataStore>>,
    /// `debug-runs.json` beside the metadata (docs/read-model.md "debug_run");
    /// `None` without a state directory: nothing is ever hidden.
    debug_runs: Option<debug_runs::DebugRuns>,
    list: Mutex<ListState>,
    views: Mutex<Views>,
    /// `Views::revision`, readable while an open holds the view lock.
    views_revision: Arc<AtomicU64>,
    /// Serialized list bodies by debug-run id; its lock also makes
    /// concurrent renders of one view single-flight (the second waits and
    /// then hits). Never held while waiting for the list or view lock.
    serialized: Mutex<BTreeMap<String, SerializedList>>,
}

impl SessionStore {
    pub fn new(roots: SessionRoots) -> Self {
        Self::with_metadata(roots, None)
    }

    pub fn with_metadata(roots: SessionRoots, metadata: Option<Arc<MetadataStore>>) -> Self {
        Self::with_metadata_and_names(roots, metadata, None)
    }

    /// The optional Codex name index is an explicit file, never inferred from
    /// a session directory or the user's home. Native files stay read-only.
    /// Roots are canonicalized once; a missing root is a configuration error
    /// reported by every list, never an empty library.
    pub fn with_metadata_and_names(
        roots: SessionRoots,
        metadata: Option<Arc<MetadataStore>>,
        index_path: Option<PathBuf>,
    ) -> Self {
        let debug_runs = metadata.as_ref().map(|metadata| {
            debug_runs::DebugRuns::new(metadata.directory().join(DEBUG_RUNS_FILENAME))
        });
        let views = Views::new();
        let views_revision = views.revision_handle();
        Self {
            index: index::Index::new(roots, index_path),
            metadata,
            debug_runs,
            list: Mutex::new(ListState {
                published: None,
                filtered: BTreeMap::new(),
                evictions: Vec::new(),
                search_rows: None,
            }),
            views: Mutex::new(views),
            views_revision,
            serialized: Mutex::new(BTreeMap::new()),
        }
    }

    /// The revision of the cached-view set (tests pin the byte cache on it).
    #[cfg(test)]
    pub(crate) fn views_revision(&self) -> u64 {
        self.views_revision.load(Ordering::Acquire)
    }

    /// The debug-run registry as it is now (reloaded when the file changed);
    /// an empty index without a state directory. Callers filter their own
    /// rows with it (`/api/live`, `/api/term/list`, search pools).
    pub fn debug_runs(&self) -> Arc<RunIndex> {
        match &self.debug_runs {
            Some(runs) => runs.current(),
            None => Arc::new(RunIndex::default()),
        }
    }

    /// The registry file this store reads (`--check-config` / diagnostics).
    pub fn debug_runs_path(&self) -> Option<&Path> {
        self.debug_runs.as_ref().map(debug_runs::DebugRuns::path)
    }

    #[cfg(test)]
    pub(crate) fn index(&self) -> &index::Index {
        &self.index
    }

    fn list_state(&self) -> Result<MutexGuard<'_, ListState>, SessionError> {
        self.list
            .lock()
            .map_err(|_| SessionError::new(500, "会话索引锁不可用"))
    }

    /// The view cache, with the evictions of vanished sessions applied. The
    /// list lock is never held while waiting for this one.
    fn views(&self) -> Result<MutexGuard<'_, Views>, SessionError> {
        let pending = std::mem::take(&mut self.list_state()?.evictions);
        let mut views = self
            .views
            .lock()
            .map_err(|_| SessionError::new(500, "会话视图锁不可用"))?;
        for uid in pending {
            views.evict(&uid);
        }
        Ok(views)
    }

    /// Publish the list: the index rows (TTL-cached unless `force`) with the
    /// index cursors, the persisted metadata and the timeline pins applied,
    /// re-signed. Cheap when neither the index nor the metadata changed.
    /// Runs the directory walk: call it on a blocking executor.
    fn publish(&self, force: bool) -> Result<Arc<Published>, SessionError> {
        self.publish_within(force, index::CHECK_TTL)
    }

    /// `publish` reusing an index younger than `ttl` (see `Index::refresh_within`).
    fn publish_within(
        &self,
        force: bool,
        ttl: std::time::Duration,
    ) -> Result<Arc<Published>, SessionError> {
        let mut state = self.list_state()?;
        let index = self.index.refresh_within(force, ttl)?;
        let metadata = self.metadata_snapshot()?;
        let revision = metadata.as_ref().map(|snapshot| snapshot.revision());
        if let Some(previous) = &state.published {
            if Arc::ptr_eq(&previous.index, &index) && previous.metadata_revision() == revision {
                return Ok(previous.clone());
            }
            if previous.index.sig() == index.sig() && previous.metadata_revision() == revision {
                // Same rows from a fresh walk: keep the document (and its
                // built_at), adopt the newest stamps.
                let published = Arc::new(Published {
                    index,
                    metadata,
                    document: previous.document.clone(),
                    positions: previous.positions.clone(),
                });
                state.published = Some(published.clone());
                return Ok(published);
            }
        }
        let mut rows = index.sessions().to_vec();
        for row in &mut rows {
            index_cursors(row, &index);
        }
        if let Some(metadata) = &metadata {
            metadata.enrich(&mut rows);
            for row in &mut rows {
                timeline_pin_row(row, &index, metadata);
            }
        }
        let mut document = index::signed_document(rows);
        if let Some(previous) = &state.published {
            if previous.document["sig"] == document["sig"] {
                document["built_at"] = previous.document["built_at"].clone();
            }
            // Views of sessions that vanished from the roots are dropped on
            // the next view-cache use; changed files invalidate themselves.
            let gone = previous
                .index
                .candidates()
                .filter(|entry| entry.owner.is_none() && index.candidate(&entry.uid).is_none())
                .map(|entry| entry.uid.clone())
                .collect::<Vec<_>>();
            state.evictions.extend(gone);
        }
        let positions = document["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(position, row)| row["uid"].as_str().map(|uid| (uid.to_owned(), position)))
            .collect();
        let published = Arc::new(Published {
            index,
            metadata,
            document: Arc::new(document),
            positions,
        });
        state.published = Some(published.clone());
        Ok(published)
    }

    /// Calls must run on a bounded blocking executor, not a Tokio reactor.
    /// `force` rescans the roots; otherwise rows younger than the index TTL
    /// are reused. The published document is decorated with what the view
    /// cache currently knows (`cursor.anchor`, pin retirement) for sessions
    /// whose cached view is the same file version the index published;
    /// `sig` covers the index rows and metadata only, so opening a session
    /// never changes the signature.
    pub fn list(&self, force: bool) -> Result<Value, SessionError> {
        let published = self.publish(force)?;
        let mut document = (*published.document).clone();
        // Never wait for an open in progress: a list is independent of the
        // sessions being parsed; their anchors simply stay absent.
        if let Ok(views) = self.views.try_lock() {
            view_decorations(&mut document, &views, &published);
        }
        Ok(document)
    }

    /// The list for liveness pairing (`/api/live`, the spawner tick): rows
    /// up to `OPEN_TTL` old are good enough to pair processes with sessions,
    /// so the 3 s poll shares one walk with the SSE publisher instead of
    /// forcing its own every time the roots changed.
    pub fn list_recent(&self) -> Result<Value, SessionError> {
        let published = self.publish_within(false, OPEN_TTL)?;
        let mut document = (*published.document).clone();
        if let Ok(views) = self.views.try_lock() {
            view_decorations(&mut document, &views, &published);
        }
        Ok(document)
    }

    /// `list_recent` without the clone and the view decorations: the
    /// published document itself (rows, `sig`, `built_at`), for consumers
    /// that read topology fields only (`/api/live`) and key their own
    /// caches on its identity. The same `Arc` comes back while the rows
    /// and metadata are unchanged.
    pub fn recent_document(&self) -> Result<Arc<Value>, SessionError> {
        Ok(self.publish_within(false, OPEN_TTL)?.document.clone())
    }

    /// `/api/sessions` for one view: the published list with the debug-run
    /// registry applied (the ordinary view
    /// hides every registered run, `debug_run` shows only that run), fork
    /// parents re-derived among the visible rows and the document re-signed
    /// over the visible rows plus the run id (`_view_signature`). Rows carry
    /// no `migration_warnings` unless unsupported; the
    /// detail `meta` keeps them all.
    pub fn list_view(&self, force: bool, debug_run: &str) -> Result<Value, SessionError> {
        self.list_view_unless(force, debug_run, "")
    }

    /// `list_view`, except that when the view's signature equals `sig` the
    /// caller gets `{"unchanged": true, "sig": …}` without the document being
    /// cloned, decorated or serialized (the legacy page polls
    /// `/api/sessions?sig=` every 8 s from every tab; the signature covers
    /// rows and metadata only, never view decorations).
    pub fn list_view_unless(
        &self,
        force: bool,
        debug_run: &str,
        sig: &str,
    ) -> Result<Value, SessionError> {
        let (published, document) = self.view_document(force, debug_run)?;
        if !force && !sig.is_empty() && document["sig"].as_str() == Some(sig) {
            return Ok(json!({"unchanged": true, "sig": sig}));
        }
        Ok(self.render_view(&published, &document).0)
    }

    /// `list_view_unless` as the response bytes, served from the per-view
    /// byte cache: a hot request whose `sig` differs (or is absent) is a
    /// lookup, not clone-decorate-serialize. An entry is reused while the
    /// view document is the same `Arc` (so `sig` and `built_at` are
    /// unchanged) and no cached view was inserted, replaced or evicted
    /// (`Views::revision`, which is what the decorations depend on).
    /// `force=1` rescans and re-renders like the predecessor; a render
    /// while an open holds the view lock is undecorated and not kept.
    pub fn list_view_bytes(
        &self,
        force: bool,
        debug_run: &str,
        sig: &str,
    ) -> Result<Bytes, SessionError> {
        let (published, document) = self.view_document(force, debug_run)?;
        if !force && !sig.is_empty() && document["sig"].as_str() == Some(sig) {
            let unchanged = json!({"unchanged": true, "sig": sig});
            return Ok(Bytes::from(
                serde_json::to_vec(&unchanged).expect("serde_json::Value serializes"),
            ));
        }
        let mut cache = self
            .serialized
            .lock()
            .map_err(|_| SessionError::new(500, "会话列表缓存锁不可用"))?;
        if !force
            && let Some(entry) = cache.get(debug_run)
            && Arc::ptr_eq(&entry.document, &document)
            && entry.revision == self.views_revision.load(Ordering::Acquire)
        {
            return Ok(entry.bytes.clone());
        }
        let (value, revision) = self.render_view(&published, &document);
        let bytes = Bytes::from(serde_json::to_vec(&value).expect("serde_json::Value serializes"));
        if let Some(revision) = revision {
            if cache.len() >= SERIALIZED_VIEWS && !cache.contains_key(debug_run) {
                cache.clear();
            }
            cache.insert(
                debug_run.to_owned(),
                SerializedList {
                    document,
                    revision,
                    bytes: bytes.clone(),
                },
            );
        }
        Ok(bytes)
    }

    /// The published list rendered for the wire: cloned, decorated from the
    /// view cache when its lock is free (an open in progress is never
    /// waited for; anchors then stay absent and `None` says so) and with
    /// the non-fatal row warnings stripped.
    fn render_view(&self, published: &Published, document: &Value) -> (Value, Option<u64>) {
        let mut value = document.clone();
        let revision = match self.views.try_lock() {
            Ok(views) => {
                view_decorations(&mut value, &views, published);
                Some(views.revision())
            }
            Err(_) => None,
        };
        strip_row_warnings(&mut value);
        (value, revision)
    }

    /// The signed document of one view: the published list itself for the
    /// ordinary view without registered runs, else the debug-run filtered
    /// document cached per (published list, registry, run id).
    fn view_document(
        &self,
        force: bool,
        debug_run: &str,
    ) -> Result<(Arc<Published>, Arc<Value>), SessionError> {
        let published = self.publish(force)?;
        let runs = self.debug_runs();
        let document: Arc<Value> = if debug_run.is_empty() && runs.is_empty() {
            published.document.clone()
        } else {
            let mut state = self.list_state()?;
            let cached = state.filtered.get(debug_run).filter(|filtered| {
                Arc::ptr_eq(&filtered.published, &published) && Arc::ptr_eq(&filtered.runs, &runs)
            });
            match cached {
                Some(filtered) => filtered.document.clone(),
                None => {
                    let mut rows = runs.filter_rows(published.rows().to_vec(), debug_run);
                    if let Some(metadata) = &published.metadata {
                        // Fork parents are computed over the filtered
                        // topology: a parent whose only fork is hidden is
                        // an ordinary row in this view.
                        metadata.enrich(&mut rows);
                    }
                    let sig = hash(
                        &serde_json::to_vec(&json!({"debug_run": debug_run, "sessions": rows}))
                            .expect("rows serialize"),
                    );
                    let document = Arc::new(json!({
                        "sessions": rows, "sig": sig,
                        "built_at": published.document["built_at"],
                    }));
                    if state.filtered.len() >= SERIALIZED_VIEWS
                        && !state.filtered.contains_key(debug_run)
                    {
                        state.filtered.clear();
                    }
                    state.filtered.insert(
                        debug_run.to_owned(),
                        Filtered {
                            published: published.clone(),
                            runs: runs.clone(),
                            document: document.clone(),
                        },
                    );
                    document
                }
            }
        };
        Ok((published, document))
    }

    pub fn messages(&self, uid: &str, query: &MessageQuery) -> Result<Value, SessionError> {
        validate_message_query(query)?;
        self.snapshot(uid, &query.agent)?.messages(query)
    }

    /// Rows and the verified native catalog from one fresh index snapshot.
    /// Run on the blocking reader (directory walk).
    pub fn search_snapshot(&self) -> Result<SessionSnapshot, SessionError> {
        let published = self.publish(true)?;
        Ok(SessionSnapshot {
            list: (*published.document).clone(),
            index: published.index.clone(),
        })
    }

    /// Freeze the candidate list for one search. Run on the blocking reader.
    pub fn search_pool(&self) -> Result<SearchPool, SessionError> {
        self.search_pool_view("")
    }

    /// `search_pool` for one debug-run view: only the rows that view lists
    /// are candidates (the results and `total_pool` alike).
    pub fn search_pool_view(&self, debug_run: &str) -> Result<SearchPool, SessionError> {
        let published = self.publish(false)?;
        let runs = self.debug_runs();
        if let Some(cached) = &self.list_state()?.search_rows
            && Arc::ptr_eq(&cached.published, &published)
            && (Arc::ptr_eq(&cached.runs, &runs) || (cached.runs.is_empty() && runs.is_empty()))
            && cached.run_id == debug_run
        {
            return Ok(SearchPool {
                rows: cached.rows.clone(),
                published,
            });
        }
        let rows = runs.filter_rows(published.rows().to_vec(), debug_run);
        let mut document = json!({"sessions": rows});
        strip_row_warnings(&mut document);
        let Value::Array(rows) = document["sessions"].take() else {
            unreachable!("array above");
        };
        let rows = Arc::new(rows);
        self.list_state()?.search_rows = Some(SearchRows {
            published: published.clone(),
            runs,
            run_id: debug_run.to_owned(),
            rows: rows.clone(),
        });
        Ok(SearchPool { rows, published })
    }

    /// The main view of one pool entry for matching: a still-current cached
    /// view is borrowed under the view lock; otherwise the file is streamed
    /// into a transient projection outside any lock and dropped by the caller.
    pub fn search_view(
        &self,
        pool: &SearchPool,
        uid: &str,
    ) -> Result<Arc<ViewSnapshot>, SessionError> {
        if let Some(cached) = self.search_view_cached(pool, uid)? {
            return Ok(cached);
        }
        self.search_view_transient(pool, uid)
    }

    /// Only the cheap probe of `search_view`: the cached view when it still
    /// describes the current files, without streaming anything.
    pub fn search_view_cached(
        &self,
        pool: &SearchPool,
        uid: &str,
    ) -> Result<Option<Arc<ViewSnapshot>>, SessionError> {
        let prepared = prepare(&pool.published, uid, "")?;
        let deps = IndexDeps {
            index: &prepared.published.index,
        };
        self.views()?.cached_current(&prepared.request, &deps)
    }

    /// A one-off projection of the main view outside every lock; nothing is
    /// retained (search-text cache misses of idle sessions).
    pub fn search_view_transient(
        &self,
        pool: &SearchPool,
        uid: &str,
    ) -> Result<Arc<ViewSnapshot>, SessionError> {
        let prepared = prepare(&pool.published, uid, "")?;
        let deps = IndexDeps {
            index: &prepared.published.index,
        };
        let prefixes = self.views()?.prefixes();
        open_transient(&prepared.request, &deps, Some(&prefixes))
    }

    /// Everything the searchable text of the main view depends on, from
    /// `stat` and the index alone (no file content): the data file version,
    /// the Claude display pin and the declared Codex fixed-prefix chain with
    /// each parent's version — or the graph error that makes it unreadable.
    /// Metadata sidecars, names and rows never enter the text, so they are
    /// deliberately not part of it. 503 when a file cannot be stamped.
    pub fn search_version(
        &self,
        pool: &SearchPool,
        uid: &str,
    ) -> Result<SearchVersion, SessionError> {
        let index = &pool.published.index;
        let owner = index
            .candidate(uid)
            .ok_or_else(|| SessionError::new(404, "会话不存在"))?;
        if owner.is_agent() {
            return Err(owner
                .owner_error
                .clone()
                .unwrap_or_else(|| SessionError::new(404, "子代理必须通过所属主会话访问")));
        }
        let candidate = restamp(&Candidate::of(owner))?;
        let data = candidate
            .data_stamp()
            .map(|stamp| (stamp.file_identity.clone(), stamp.size));
        let pin = pin_for(pool.published.metadata.as_ref(), &candidate, uid)
            .map(|pin| json!([pin.tip, pin.stale_end]));
        let chain = match index.physical_chain(uid) {
            Ok(chain) => {
                let mut parents = Vec::with_capacity(chain.len());
                for (parent, cut) in chain {
                    let stamped = restamp(&Candidate::of(parent))?;
                    parents.push(json!([
                        parent.path.to_string_lossy(),
                        cut,
                        stamped.data_stamp().map(stamp_json)
                    ]));
                }
                Value::Array(parents)
            }
            Err(error) => json!({"error": [error.status, error.message]}),
        };
        Ok(SearchVersion {
            key: json!({
                "source": candidate.source,
                "path": candidate.path.to_string_lossy(),
                "data": candidate.data_stamp().map(stamp_json),
                "pin": pin,
                "chain": chain,
            }),
            data,
        })
    }

    /// Open (or refresh) the view of `(uid, agent)`: the index is refreshed
    /// within its TTL so SSE-only clients discover new files, duplicate SIDs
    /// and changed ownership without list polling; the view itself re-`stat`s
    /// its files and extends or rebuilds. Run on the bounded blocking reader.
    pub fn snapshot(&self, uid: &str, agent: &str) -> Result<Arc<ViewSnapshot>, SessionError> {
        self.open(uid, agent, false)
    }

    fn open(&self, uid: &str, agent: &str, force: bool) -> Result<Arc<ViewSnapshot>, SessionError> {
        self.open_pass(uid, agent, force, false)
    }

    /// One open against the published list (`OPEN_TTL` old at most), plus at
    /// most one forced second pass (`rescanned`) that re-aligns the row with
    /// bytes the view already read.
    fn open_pass(
        &self,
        uid: &str,
        agent: &str,
        force: bool,
        rescanned: bool,
    ) -> Result<Arc<ViewSnapshot>, SessionError> {
        let prepared = self.prepare_within(uid, agent, force, OPEN_TTL)?;
        let deps = IndexDeps {
            index: &prepared.published.index,
        };
        let opened = self.views()?.open(&prepared.request, &deps);
        match opened {
            // The view read a newer version of its file than the index
            // published (an append, rewrite or a Grok chat created between
            // the last walk and this open): rescan once so the row the view
            // reports as `meta` (size, updated, title, chat_exists) describes
            // the same bytes. The cached view is reused, only its row changes.
            // (Pacing this rescan was tried and rejected: the
            // read-model tests and docs promise that `meta` and the bytes
            // agree; the walk+rebuild per observed append stays, see
            // docs/performance.md.)
            Ok(snapshot) if !force && !rescanned && !leaf_is_published(&prepared, &snapshot) => {
                self.open_pass(uid, agent, true, true)
            }
            Ok(snapshot) => Ok(snapshot),
            // A parent or agent file created, replaced or removed since the
            // index last walked the roots: rescan once and retry.
            Err(error) if !force && !rescanned && matches!(error.status, 404 | 409 | 501 | 503) => {
                self.open_pass(uid, agent, true, true)
            }
            Err(error) => Err(error),
        }
    }

    /// The view request for `(uid, agent)` from the current published list.
    fn prepare(&self, uid: &str, agent: &str, force: bool) -> Result<Prepared, SessionError> {
        self.prepare_within(uid, agent, force, OPEN_TTL)
    }

    fn prepare_within(
        &self,
        uid: &str,
        agent: &str,
        force: bool,
        ttl: std::time::Duration,
    ) -> Result<Prepared, SessionError> {
        // A view opens against a list up to OPEN_TTL old: new files, changed
        // ownership and duplicate SIDs still show up within seconds, while the
        // view itself re-`stat`s the files it displays.
        let published = self.publish_within(force, ttl)?;
        match prepare(&published, uid, agent) {
            Ok(prepared) => Ok(prepared),
            Err(error) if !force && matches!(error.status, 404 | 409 | 501) => {
                let published = self.publish(true)?;
                prepare(&published, uid, agent)
            }
            Err(error) => Err(error),
        }
    }

    /// Resolve native identity from the same fresh, restamped immutable view
    /// used by history reads. Must run on a bounded blocking executor.
    pub fn native_scope(&self, uid: &str, agent: &str) -> Result<NativeScope, SessionError> {
        self.snapshot(uid, agent)?.native_scope()
    }

    /// Delivery executor read; see `ViewSnapshot::claude_native_inputs`.
    /// Run on the bounded blocking reader executor.
    pub fn claude_native_inputs(
        &self,
        uid: &str,
        from: Option<&NativeFence>,
    ) -> Result<NativeInputRead, SessionError> {
        self.snapshot(uid, "")?.claude_native_inputs(from)
    }

    /// Native ids and agent ownership from one fresh index snapshot (head/tail
    /// records of every file, no full parse). No public display SID/name is
    /// used as binding evidence. Run on the bounded blocking reader executor.
    pub fn native_catalog(&self) -> Result<crate::runtime::NativeCatalog, SessionError> {
        Ok(self.publish(true)?.index.catalog())
    }

    /// Validate an operator-chosen Claude node against the freshly opened
    /// main view and return the tip to pin plus the boundary it applies to.
    /// Reads native bytes only; persisting the pin is the caller's step, and
    /// nothing here signals the CLI. Run on the bounded blocking executor.
    pub fn claude_rewind_target(
        &self,
        uid: &str,
        target: &str,
    ) -> Result<RewindTarget, SessionError> {
        let prepared = self.prepare(uid, "", true)?;
        let deps = IndexDeps {
            index: &prepared.published.index,
        };
        self.views()?
            .claude_rewind_target(&prepared.request, &deps, target)
    }

    /// Bounded view-cache statistics (views, parsed files, retained bytes).
    #[cfg(test)]
    pub(crate) fn view_stats(&self) -> Result<ViewStats, SessionError> {
        Ok(self.views()?.stats())
    }

    fn metadata_snapshot(&self) -> Result<Option<Arc<MetadataSnapshot>>, SessionError> {
        self.metadata
            .as_ref()
            .map(|store| store.snapshot())
            .transpose()
            .map_err(|error| SessionError::new(error.status, error.message))
    }
}

/// Assemble the view request for `(uid, agent)` from one published list:
/// the owner's candidate, the agent's own file when an agent is selected,
/// the owner's persisted pin and its published row (topology, names and
/// metadata already applied — the view's `meta`).
fn prepare(published: &Arc<Published>, uid: &str, agent: &str) -> Result<Prepared, SessionError> {
    let index = &published.index;
    let owner = index
        .candidate(uid)
        .ok_or_else(|| SessionError::new(404, "会话不存在"))?;
    if owner.is_agent() {
        return Err(owner
            .owner_error
            .clone()
            .unwrap_or_else(|| SessionError::new(404, "子代理必须通过所属主会话访问")));
    }
    let selected = if agent.is_empty() {
        None
    } else {
        Some(Candidate::of(index.agent(uid, agent).ok_or_else(|| {
            SessionError::new(404, "子代理不存在或不属于此主会话")
        })?))
    };
    let row = published
        .row(uid)
        .cloned()
        .ok_or_else(|| SessionError::new(503, "会话行尚未发布，请重试"))?;
    let owner_candidate = Candidate::of(owner);
    Ok(Prepared {
        request: ViewRequest {
            uid: uid.to_owned(),
            agent: agent.to_owned(),
            pin: pin_for(published.metadata.as_ref(), &owner_candidate, uid),
            owner: owner_candidate,
            selected,
            row,
        },
        published: published.clone(),
    })
}

/// Whether the file the view was projected from is the version the index
/// published for it (the main file, or the agent's own file).
fn leaf_is_published(prepared: &Prepared, snapshot: &ViewSnapshot) -> bool {
    published_entry(prepared)
        .is_some_and(|entry| snapshot.view.parsed.candidate.is_version_of(entry))
}

fn published_entry(prepared: &Prepared) -> Option<&CandidateRef> {
    let index = &prepared.published.index;
    if prepared.request.agent.is_empty() {
        index.candidate(&prepared.request.uid)
    } else {
        index.agent(&prepared.request.uid, &prepared.request.agent)
    }
}

/// The physical part of the message cursor from the index: `{end, head}`
/// of the committed prefix, for supported rows and their agent items. The
/// semantic `anchor` needs the projection and is added from a cached view.
fn index_cursors(row: &mut Value, index: &IndexSnapshot) {
    let Some(uid) = row["uid"].as_str() else {
        return;
    };
    let uid = uid.to_owned();
    if row["supported"] != false
        && let Some(entry) = index.candidate(&uid)
        && let Some(cursor) = index_cursor(entry)
    {
        row["cursor"] = cursor;
    }
    // `get_mut`, never `row["agent_items"]`: IndexMut would insert a null.
    if let Some(items) = row.get_mut("agent_items").and_then(Value::as_array_mut) {
        for item in items {
            if item["supported"] != false
                && let Some(id) = item["id"].as_str()
                && let Some(entry) = index.agent(&uid, id)
                && let Some(cursor) = index_cursor(entry)
            {
                item["cursor"] = cursor;
            }
        }
    }
}

fn index_cursor(entry: &CandidateRef) -> Option<Value> {
    match (entry.committed(), entry.cursor_head()) {
        (Some(end), Some(head)) => Some(json!({"end": end, "head": head})),
        // A Grok session without a chat file projects to an empty history.
        _ if entry.source == "grok" && entry.stamp.is_none() => {
            Some(json!({"end": 0, "head": views::head(&[], 0)}))
        }
        _ => None,
    }
}

/// The persisted Claude display pin on its row, from the metadata alone:
/// `retired:false` is certain while the file has not grown past the pinned
/// boundary; otherwise retirement needs the projection and comes from the
/// cached view (`view_decorations`) or the opened session.
fn timeline_pin_row(row: &mut Value, index: &IndexSnapshot, metadata: &MetadataSnapshot) {
    let Some(uid) = row["uid"].as_str() else {
        return;
    };
    let Some(entry) = index.candidate(uid) else {
        return;
    };
    if entry.source != "claude" || entry.is_agent() {
        return;
    }
    let Some(pin) = metadata.timeline(uid) else {
        return;
    };
    let mut value = json!({
        "target": pin.target, "tip": pin.tip, "stale_end": pin.stale_end,
        "pinned_at": pin.pinned_at, "native_rewind": false,
    });
    if entry.committed() == Some(pin.stale_end) {
        value["retired"] = json!(false);
    }
    row["timeline_pin"] = value;
}

/// Public list rows carry `migration_warnings` only when unsupported (the
/// fatal reason, like the batch-35 contract); the non-fatal notes of a
/// supported row (and of its `agent_items`) stay in the detail `meta` only.
/// Nothing in the frontend reads it.
pub(crate) fn strip_row_warnings(document: &mut Value) {
    fn strip(row: &mut Value) {
        if row["supported"] != false
            && let Some(object) = row.as_object_mut()
        {
            object.remove("migration_warnings");
        }
    }
    // `get_mut`, never `row["agent_items"]`: IndexMut would insert a null.
    for row in document
        .get_mut("sessions")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        strip(row);
        for item in row
            .get_mut("agent_items")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            strip(item);
        }
    }
}

/// Borrow from the view cache what the list cannot derive itself, for every
/// row whose cached view is exactly the file version (and pin) the index
/// published: the semantic `cursor.anchor` and the pin's retirement state.
fn view_decorations(document: &mut Value, views: &Views, published: &Published) {
    let index = &published.index;
    let Some(rows) = document["sessions"].as_array_mut() else {
        return;
    };
    for row in rows {
        let Some(uid) = row["uid"].as_str().map(str::to_owned) else {
            continue;
        };
        if let Some(entry) = index.candidate(&uid)
            && let Some(view) = views.cached(&uid, "")
            && view.view.parsed.candidate.is_version_of(entry)
            && view.view.parsed.pin
                == pin_for(
                    published.metadata.as_ref(),
                    &view.view.parsed.candidate,
                    &uid,
                )
        {
            if row["cursor"].is_object() {
                row["cursor"]["anchor"] = json!(view.anchor);
            }
            if row["timeline_pin"].is_object() && view.view.meta["timeline_pin"].is_object() {
                row["timeline_pin"] = view.view.meta["timeline_pin"].clone();
            }
        }
        let Some(items) = row.get_mut("agent_items").and_then(Value::as_array_mut) else {
            continue;
        };
        for item in items {
            if let Some(id) = item["id"].as_str()
                && let Some(entry) = index.agent(&uid, id)
                && let Some(view) = views.cached(&uid, id)
                && view.view.parsed.candidate.is_version_of(entry)
                && item["cursor"].is_object()
            {
                item["cursor"]["anchor"] = json!(view.anchor);
            }
        }
    }
}

/// Resolved pin request: the tip to display and the committed boundary whose
/// later native records retire it. Not a native operation of any kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewindTarget {
    pub tip: String,
    pub stale_end: u64,
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}

pub(crate) fn path_text(path: &Path) -> std::borrow::Cow<'_, str> {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    {
        // `str(Path.resolve())` uses an ordinary drive/UNC spelling,
        // while Rust canonicalize returns the Win32 verbatim `\\?\` form.
        // UIDs are a hash of that spelling and must stay identical whether
        // the caller passes a configured path or its canonicalized equivalent.
        if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
            return format!("\\\\{rest}").replace('/', "\\").into();
        }
        if let Some(rest) = text.strip_prefix("\\\\?\\") {
            return rest.replace('/', "\\").into();
        }
        return text.replace('/', "\\").into();
    }
    #[cfg(not(windows))]
    text
}

pub(crate) fn uid_for(source: &str, path: &Path) -> String {
    format!("{source}:{}", &hash(path_text(path).as_bytes())[..16])
}

fn timestamp(nanos: u128) -> String {
    chrono::DateTime::from_timestamp(
        (nanos / 1_000_000_000) as i64,
        (nanos % 1_000_000_000) as u32,
    )
    .unwrap_or_default()
    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub(crate) fn stamp(path: &Path) -> Result<FileStamp, SessionError> {
    let file =
        fs::File::open(path).map_err(|_| SessionError::new(503, "已配置的会话文件暂时不可读取"))?;
    file_stamp(&file)
}

pub(super) fn file_stamp(file: &fs::File) -> Result<FileStamp, SessionError> {
    #[cfg(windows)]
    let metadata = cap_std::fs::Metadata::from_file(file)
        .map_err(|_| SessionError::new(503, "已配置的会话文件暂时不可读取"))?;
    #[cfg(not(windows))]
    let metadata = file
        .metadata()
        .map_err(|_| SessionError::new(503, "已配置的会话文件暂时不可读取"))?;
    if !metadata.is_file() {
        return Err(SessionError::new(403, "会话输入必须是普通文件"));
    }
    Ok(metadata_stamp(&metadata))
}

#[cfg(not(windows))]
fn metadata_stamp(metadata: &fs::Metadata) -> FileStamp {
    let modified = metadata
        .modified()
        .unwrap_or(UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let identity = {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec()
        )
    };
    let file_identity = {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    };
    FileStamp {
        size: metadata.len(),
        modified,
        identity,
        file_identity,
    }
}

#[cfg(windows)]
fn metadata_stamp(metadata: &cap_std::fs::Metadata) -> FileStamp {
    use cap_fs_ext::MetadataExt;
    let modified = metadata
        .modified()
        .ok()
        .map(cap_std::time::SystemTime::into_std)
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    let file_identity = format!("{}:{}", metadata.dev(), metadata.ino());
    FileStamp {
        size: metadata.len(),
        modified,
        identity: file_identity.clone(),
        file_identity,
    }
}

pub(crate) fn trusted_path(root: &Path, path: &Path) -> Result<(), SessionError> {
    if !path.starts_with(root) {
        return Err(SessionError::new(403, "会话输入路径越过已配置的数据源边界"));
    }
    Ok(())
}

pub(crate) fn restamp(candidate: &Candidate) -> Result<Candidate, SessionError> {
    let mut next = candidate.clone();
    next.stamps.clear();
    let has_data = if candidate.source == "grok" {
        trusted_path(&candidate.root, &candidate.path)?;
        fs::metadata(&candidate.data).is_ok_and(|metadata| metadata.is_file())
    } else {
        true
    };
    if has_data {
        trusted_path(&candidate.root, &candidate.data)?;
        next.stamps.push(stamp(&candidate.data)?);
    }
    if let Some(summary) = &candidate.summary {
        trusted_path(&candidate.root, summary)?;
        let summary_stamp = stamp(summary)?;
        next.stamps.push(summary_stamp);
    }
    Ok(next)
}

fn claude_agent_of(candidate: &Candidate) -> String {
    if candidate.source == "claude"
        && candidate
            .data
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "subagents")
    {
        candidate
            .data
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("agent-"))
            .unwrap_or("")
    } else {
        ""
    }
    .to_owned()
}

/// The persisted display pin that applies to this physical entry: Claude main
/// sessions only. Anything else parses exactly as before.
fn pin_for(
    metadata: Option<&Arc<MetadataSnapshot>>,
    candidate: &Candidate,
    uid: &str,
) -> Option<crate::metadata::TimelinePin> {
    if candidate.source != "claude" || !claude_agent_of(candidate).is_empty() {
        return None;
    }
    metadata?.timeline(uid).cloned()
}

#[cfg(test)]
mod grok_tests;
mod media_projection;
#[cfg(test)]
mod media_tests;
#[cfg(test)]
mod native_scope_tests;

#[cfg(test)]
mod native_catalog_tests;
#[cfg(test)]
mod tests;
