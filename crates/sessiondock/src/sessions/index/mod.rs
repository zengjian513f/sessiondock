//! Lazy session index (batch 34, WP-A): directory walk + `stat` + bounded
//! per-file head/tail summaries, cached by file stamp, read in parallel.
//! Design: docs/read-model.md. No startup parse, no session or byte caps,
//! one file's change never fails the list. Summaries are the only source for
//! `/api/sessions` rows, ownership/fork graphs, native ids and trash file sets.
//!
//! # Public API (the `SessionStore` facade in `sessions/mod.rs` is the caller)
//!
//! ```text
//! let index = Index::new(roots, codex_index);        // roots canonicalized once
//! let snapshot = index.refresh(force)?;              // Arc<IndexSnapshot>
//! snapshot.rows()          -> &Value   {"sessions": [...], "sig", "built_at"}
//! snapshot.sessions()      -> &[Value] the sorted rows (updated desc, uid asc)
//! snapshot.sig()           -> &str     sha1 of the serialized rows
//! snapshot.built_at()      -> f64      epoch seconds of publication
//! snapshot.candidate(uid)  -> Option<&CandidateRef>  path/source/stamp/native id/owner
//! snapshot.candidates()    -> iterator over every physical entry (agents included)
//! snapshot.agent(owner, id)-> Option<&CandidateRef>  a healthy agent of `owner`
//! snapshot.thread(src, id) -> Result<&CandidateRef>  Codex fixed-prefix parent (501/409)
//! snapshot.catalog()       -> crate::runtime::NativeCatalog (verified, from summaries)
//! signed_document(rows)    -> Value    re-sign rows after metadata enrichment
//! ```
//!
//! `refresh(false)` returns the previous snapshot within [`CHECK_TTL`] unless
//! the Codex name index changed; `refresh(true)` rescans. A rescan is a
//! directory walk (the same recursive shapes Python scans) plus one `stat`
//! per file. File symlinks and Claude project/session directory aliases are
//! followed like Python's `glob`; recursive Codex directory aliases are not
//! entered. Only files whose stamp
//! (`dev/ino/size/mtime_ns`) changed are re-read, on a bounded pool of
//! [`DEFAULT_WORKERS`] threads, each read bounded to the head/tail sizes in
//! [`summary`]. A file whose stamp changes across the read is re-read up to
//! [`READ_ATTEMPTS`] times and otherwise published with the stamp of the bytes
//! read; a file that vanishes is simply absent from the snapshot.
//!
//! Rows carry no `cursor`: the message cursor's `anchor` needs the projection
//! of the whole file and belongs to the per-session view. `CandidateRef`
//! exposes `committed` and `cursor_head` so the facade seeds the physical
//! `cursor: {end, head}` of every supported row (docs/history-pages.md).
//! Persisted metadata enrichment (stars, fork visibility, pins) is applied by
//! the facade on top of `sessions()`; it uses [`signed_document`] to re-sign.
//!
//! One more bounded read serves `agent_items[].active` (batch 36): for each
//! Claude main transcript that owns a sidecar whose last turn is open, the
//! stop notices are scanned from the owner file incrementally by committed
//! offset ([`agent_stops`]), cached by the owner's stamp, in the same
//! parallel pool. A hot refresh with unchanged stamps reads nothing; an
//! owner an active CLI appends to is read from its last consumed LF only.
//!
//! # What summaries cannot know (documented deltas from the old full parse)
//!
//! - `supported`/`migration_warnings` reflect only the head and tail:
//!   content-shape errors and bad `history_base` fail the row, corrupt lines,
//!   duplicate `session_meta` and unknown record kinds are counted, all only
//!   when they fall in those regions; Claude lineage notes (missing ancestor,
//!   cycle, missing leaf) and content-block notes need the projection and
//!   appear on the detail view only. Unknown-kind counts are exact for files the tail covers whole
//!   (≤ 512 KiB) and partial otherwise; they count records regardless of the
//!   active lineage. Complete native files are opened on demand.
//! - Python-parity corrections of the old rows: Codex main `updated` is the
//!   file mtime (Python `_iso(st_mtime)`, whole seconds), a Codex rollout
//!   without a user message is titled `(无标题) <stem[:16]>`, Claude titles
//!   follow `_title_from_text` (first plain line, 90 chars) and custom titles
//!   are kept verbatim, Claude `cwd` falls back to the tail majority and then
//!   the project directory name.
//! - Native ids and conflicts come from the head/tail records seen.
//! - A Codex head is 120 pieces (Python `_raw_meta`), a Claude head 40.

pub mod agent_stops;
pub mod graph;
pub mod names;
pub mod summary;

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cap_fs_ext::{DirExt, MetadataExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, Metadata};
use serde_json::{Value, json};

use super::{NativeCatalogEntry, SessionError, SessionRoots, hash, uid_for};
use agent_stops::{StopScan, Stops};
use graph::CutCheck;
use summary::claude::owner_path;
use summary::{DataFile, HEAD_BYTES, Input, RowSummary, SidecarBytes, TAIL_BYTES};

/// Rows younger than this are reused by `refresh(false)`.
pub const CHECK_TTL: Duration = Duration::from_millis(500);
/// Summary reads run on at most this many threads.
pub const DEFAULT_WORKERS: usize = 16;
/// Re-reads when a file's stamp changes across a summary read.
pub const READ_ATTEMPTS: usize = 3;
/// File version: `dev/ino/size/mtime_ns`. Equal stamps mean the cached
/// summary is still the summary of these bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Stamp {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime_ns: u128,
}

impl Stamp {
    fn of(meta: &Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.len(),
            mtime_ns: meta
                .modified()
                .ok()
                .map(cap_std::time::SystemTime::into_std)
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_nanos()),
        }
    }
}

/// One physical entry of the candidate table.
#[derive(Clone, Debug)]
pub struct CandidateRef {
    pub uid: String,
    pub source: &'static str,
    pub root: PathBuf,
    /// Display/UID path: the JSONL file, or the Grok session directory.
    pub path: PathBuf,
    /// The JSONL data file (Grok: `chat_history.jsonl`, may not exist).
    pub data: PathBuf,
    /// `agent-*.meta.json` (Claude sidecar) or `summary.json` (Grok).
    pub summary_path: Option<PathBuf>,
    /// Data file stamp; `None` only for a Grok session without a chat file.
    pub stamp: Option<Stamp>,
    pub summary_stamp: Option<Stamp>,
    /// Agent id for Claude sidecar files and Codex subagent rollouts.
    pub agent_id: Option<String>,
    /// Root owner uid of a healthy agent (the main session it is listed under).
    pub owner: Option<String>,
    /// Why an agent has no owner: 409 ambiguous Codex id, or 501 when its owner
    /// is not indexed (no row at all, like Python) or the relation is broken.
    /// Opening the agent's uid directly answers with this error.
    pub owner_error: Option<SessionError>,
    pub summary: Arc<RowSummary>,
}

impl CandidateRef {
    /// The declared native session id when the records seen agree on one.
    #[cfg(test)]
    pub fn native_id(&self) -> Option<&str> {
        self.summary.native_id.as_ref().ok().map(String::as_str)
    }
    /// A Claude sidecar or Codex subagent rollout (owned or not).
    pub fn is_agent(&self) -> bool {
        self.agent_id.is_some()
    }
    /// Offset after the last complete record, when the tail showed it.
    pub fn committed(&self) -> Option<u64> {
        self.summary.committed
    }
    /// `rs-m2-1` physical head hash of the committed prefix.
    pub fn cursor_head(&self) -> Option<&str> {
        self.summary.cursor_head.as_deref()
    }
}

/// Immutable published list state.
#[derive(Debug)]
pub struct IndexSnapshot {
    document: Value,
    candidates: BTreeMap<String, CandidateRef>,
    /// (source, native sid) → uids of every entry declaring it.
    threads: BTreeMap<(String, String), Vec<String>>,
    /// Owner uid → uids of its healthy agents.
    agents: BTreeMap<String, Vec<String>>,
    catalog: Vec<graph::CatalogSeed>,
}

impl IndexSnapshot {
    /// The exact `/api/sessions` document: `{"sessions", "sig", "built_at"}`.
    #[cfg(test)]
    pub fn rows(&self) -> &Value {
        &self.document
    }
    pub fn sessions(&self) -> &[Value] {
        self.document["sessions"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
    pub fn sig(&self) -> &str {
        self.document["sig"].as_str().unwrap_or("")
    }
    #[cfg(test)]
    pub fn built_at(&self) -> f64 {
        self.document["built_at"].as_f64().unwrap_or_default()
    }
    pub fn candidate(&self, uid: &str) -> Option<&CandidateRef> {
        self.candidates.get(uid)
    }
    pub fn candidates(&self) -> impl Iterator<Item = &CandidateRef> {
        self.candidates.values()
    }
    /// The healthy agent `id` listed under the main session `owner`
    /// (`row["agent_items"]`), by exact id; never by joining paths.
    pub fn agent(&self, owner: &str, id: &str) -> Option<&CandidateRef> {
        self.agents
            .get(owner)?
            .iter()
            .filter_map(|uid| self.candidates.get(uid))
            .find(|entry| entry.agent_id.as_deref() == Some(id))
    }
    /// The unique main (non-subagent) `source` transcript declaring the
    /// native thread id `sid`: a Codex fixed-prefix parent
    /// (`views::Dependencies::thread`). 501 when unindexed or a subagent
    /// file, 409 when the id is ambiguous — the graph's own rules.
    pub fn thread(&self, source: &str, sid: &str) -> Result<&CandidateRef, SessionError> {
        if source != "codex" {
            return Err(SessionError::new(501, "此数据源没有分叉父历史"));
        }
        if sid.is_empty() {
            return Err(SessionError::new(501, "历史依赖或子代理缺少父线程 ID"));
        }
        match self
            .threads
            .get(&(source.to_owned(), sid.to_owned()))
            .map(Vec::as_slice)
        {
            Some([uid]) => {
                let entry = &self.candidates[uid];
                if entry.summary.agent.is_some() {
                    return Err(SessionError::new(
                        501,
                        "分叉历史不能把子代理文件当作主线程父历史",
                    ));
                }
                Ok(entry)
            }
            Some(_) => Err(SessionError::new(409, "父线程 ID 在已配置索引中存在歧义")),
            None => Err(SessionError::new(
                501,
                "父线程不在已配置索引中，请确认显式数据源包含其原生文件",
            )),
        }
    }
    /// The declared physical fixed-prefix chain of a main Codex transcript,
    /// nearest parent first, from summaries alone (`Graph::chain` without the
    /// cut check): `(parent, cut)` pairs an open will read. Errors are the
    /// graph's own 501/409/413 codes, so a search-text version can carry the
    /// same failure the row shows instead of parsing the leaf to find it.
    pub fn physical_chain(&self, uid: &str) -> Result<Vec<(&CandidateRef, u64)>, SessionError> {
        let mut current = self
            .candidates
            .get(uid)
            .ok_or_else(|| SessionError::new(404, "会话不存在"))?;
        let mut chain = Vec::new();
        let mut seen = std::collections::BTreeSet::from([uid.to_owned()]);
        while let Some((sid, cut)) = graph::history_link(current)? {
            let parent = self.thread("codex", sid)?;
            if !seen.insert(parent.uid.clone()) {
                return Err(SessionError::new(501, "分叉历史依赖存在循环"));
            }
            chain.push((parent, cut));
            current = parent;
        }
        Ok(chain)
    }
    /// Verified runtime identity catalog from this snapshot's summaries.
    pub fn catalog(&self) -> crate::runtime::NativeCatalog {
        crate::runtime::NativeCatalog::from_native_entries(
            self.catalog
                .iter()
                .map(|seed| NativeCatalogEntry {
                    uid: seed.uid.clone(),
                    source: seed.source.to_owned(),
                    declared_ids: seed.declared_ids.clone(),
                    scope: seed.scope.clone(),
                    subagent: seed.subagent,
                })
                .collect(),
            self.sessions(),
        )
    }
}

/// Sort like today's list and wrap with a fresh signature and build time.
pub fn signed_document(mut rows: Vec<Value>) -> Value {
    rows.sort_by(|a, b| {
        b["updated"]
            .as_str()
            .cmp(&a["updated"].as_str())
            .then_with(|| a["uid"].as_str().cmp(&b["uid"].as_str()))
    });
    let sig = hash(&serde_json::to_vec(&rows).expect("rows serialize"));
    json!({"sessions": rows, "sig": sig, "built_at": epoch_now()})
}

fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// A candidate as discovered by the walk, before any file content is read.
#[derive(Clone, Debug)]
struct Discovered {
    source: &'static str,
    root: PathBuf,
    path: PathBuf,
    data: PathBuf,
    summary_path: Option<PathBuf>,
    stamp: Option<Stamp>,
    summary_stamp: Option<Stamp>,
    agent_id: Option<String>,
}

impl Discovered {
    fn key(&self) -> CacheKey {
        CacheKey {
            stamp: self.stamp,
            summary_stamp: self.summary_stamp,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CacheKey {
    stamp: Option<Stamp>,
    summary_stamp: Option<Stamp>,
}

struct Cached {
    key: CacheKey,
    summary: Arc<RowSummary>,
}

#[derive(Default)]
struct State {
    cache: HashMap<PathBuf, Cached>,
    /// (parent data path, parent stamp, cut) → boundary check outcome.
    cuts: HashMap<(PathBuf, Stamp, u64), CutCheck>,
    /// Claude owner data path → incremental stop-notice scan (kept while
    /// the owner is indexed, so a resumed agent continues from the last
    /// consumed offset).
    stops: HashMap<PathBuf, StopScan>,
    names: Option<Arc<names::NameIndex>>,
    snapshot: Option<Arc<IndexSnapshot>>,
    checked: Option<Instant>,
    /// Test/benchmark observability: files whose summary was (re)read.
    reads: usize,
    /// Test/benchmark observability: owner files whose stop scan advanced.
    stop_scans: usize,
}

pub struct Index {
    roots: SessionRoots,
    root_error: Option<String>,
    codex_index: Option<PathBuf>,
    workers: usize,
    state: Mutex<State>,
}

impl Index {
    /// Roots are canonicalized once, like today's `SessionStore`; a missing
    /// native root is reported by every refresh rather than becoming an empty
    /// library. A name index without a Codex root is simply unused.
    pub fn new(roots: SessionRoots, codex_index: Option<PathBuf>) -> Self {
        Self::with_workers(roots, codex_index, DEFAULT_WORKERS)
    }

    pub fn with_workers(roots: SessionRoots, codex_index: Option<PathBuf>, workers: usize) -> Self {
        let mut root_error = None;
        let mut freeze = |configured: Option<PathBuf>| {
            configured.map(|path| match path.canonicalize() {
                Ok(canonical) if canonical.is_dir() => canonical,
                _ => {
                    root_error = Some("已配置的数据源目录不存在或不可访问".to_owned());
                    path
                }
            })
        };
        let roots = SessionRoots {
            claude: freeze(roots.claude),
            codex: freeze(roots.codex),
            grok: freeze(roots.grok),
        };
        Self {
            roots,
            root_error,
            codex_index,
            workers: workers.max(1),
            state: Mutex::new(State::default()),
        }
    }

    /// Number of files whose summary has been read since construction.
    #[cfg(test)]
    pub fn reads(&self) -> usize {
        self.state.lock().map(|state| state.reads).unwrap_or(0)
    }

    /// Number of owner files whose stop-notice scan was advanced (opened
    /// and read past the last consumed LF) since construction.
    #[cfg(test)]
    pub fn stop_scans(&self) -> usize {
        self.state.lock().map(|state| state.stop_scans).unwrap_or(0)
    }

    /// The canonicalized roots this index walks.
    #[cfg(test)]
    pub fn roots(&self) -> &SessionRoots {
        &self.roots
    }

    /// Test hook: make the next `refresh(false)` rescan without waiting for
    /// the TTL (no wall-clock sleep in tests).
    #[cfg(test)]
    pub fn expire(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.checked = None;
        }
    }

    /// Publish the current list. Runs directory walks and bounded file reads:
    /// call it on a blocking executor, never on a reactor thread.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn refresh(&self, force: bool) -> Result<Arc<IndexSnapshot>, SessionError> {
        self.refresh_within(force, CHECK_TTL)
    }

    /// `refresh` with a caller-chosen reuse window: the previous snapshot is
    /// returned while it is younger than `ttl` (batch 44 WP-A: opening a
    /// view tolerates a few seconds of list staleness — the view `stat`s its
    /// own files — so the SSE publisher's 500 ms probes stop walking the
    /// roots every time).
    pub fn refresh_within(
        &self,
        force: bool,
        ttl: Duration,
    ) -> Result<Arc<IndexSnapshot>, SessionError> {
        if let Some(reason) = &self.root_error {
            return Err(SessionError::new(400, reason));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| SessionError::new(500, "会话索引锁不可用"))?;
        if !force
            && let Some(snapshot) = &state.snapshot
            && state.checked.is_some_and(|at| at.elapsed() < ttl)
            && !self.names_changed(&state)?
        {
            return Ok(snapshot.clone());
        }
        let discovered = self.discover()?;
        // Unchanged walk (batch 44 WP-A): the same files with the same stamps
        // and the same names file describe the snapshot already published —
        // every row, cut and stop scan is a function of those stamps. Skip the
        // rebuild (graph, rows, serialization, signature) and only refresh
        // the TTL, so the SSE publisher's 500 ms probes and every `open`
        // past the TTL cost a directory walk plus `stat`, not ~80 ms of CPU.
        if state.snapshot.is_some()
            && discovered.len() == state.cache.len()
            && discovered.iter().all(|candidate| {
                state
                    .cache
                    .get(&candidate.path)
                    .is_some_and(|cached| cached.key == candidate.key())
            })
            && !self.names_changed(&state)?
        {
            state.checked = Some(Instant::now());
            return Ok(state.snapshot.clone().expect("checked above"));
        }
        let names = self
            .codex_index
            .as_deref()
            .map(|path| names::load(path, state.names.as_ref()))
            .transpose()?;

        // Warm path: every unchanged stamp reuses its summary; only changed or
        // new files are read, in parallel.
        let mut pending = Vec::new();
        let mut reused: HashMap<PathBuf, Cached> = HashMap::new();
        for candidate in &discovered {
            match state.cache.get(&candidate.path) {
                Some(cached) if cached.key == candidate.key() => {
                    reused.insert(
                        candidate.path.clone(),
                        Cached {
                            key: cached.key,
                            summary: cached.summary.clone(),
                        },
                    );
                }
                _ => pending.push(candidate.clone()),
            }
        }
        let results = self.read_all(&pending);
        state.reads += pending.len();
        let mut cache = reused;
        let mut transient = Vec::new();
        for (candidate, outcome) in pending.iter().zip(results) {
            let Some(read) = outcome else { continue };
            if read.transient {
                transient.push(candidate.path.clone());
            }
            cache.insert(
                candidate.path.clone(),
                Cached {
                    key: read.key,
                    summary: read.summary,
                },
            );
        }

        let mut entries = BTreeMap::new();
        for candidate in discovered {
            let Some(cached) = cache.get(&candidate.path) else {
                continue;
            };
            let uid = uid_for(candidate.source, &candidate.path);
            entries.insert(
                uid.clone(),
                CandidateRef {
                    uid,
                    source: candidate.source,
                    root: candidate.root,
                    path: candidate.path,
                    data: candidate.data,
                    summary_path: candidate.summary_path,
                    stamp: cached.key.stamp,
                    summary_stamp: cached.key.summary_stamp,
                    agent_id: cached
                        .summary
                        .agent
                        .as_ref()
                        .map(|agent| agent.id.clone())
                        .or(candidate.agent_id),
                    owner: None,
                    owner_error: None,
                    summary: cached.summary.clone(),
                },
            );
        }
        let stops = self.refresh_stops(&mut state, &entries);
        let mut cuts = std::mem::take(&mut state.cuts);
        let mut next_cuts = HashMap::new();
        let mut check = |parent: &CandidateRef, cut: u64| -> CutCheck {
            let Some(stamp) = parent.stamp else {
                return CutCheck::Unreadable;
            };
            let key = (parent.data.clone(), stamp, cut);
            let outcome = cuts
                .remove(&key)
                .unwrap_or_else(|| check_cut(&parent.root, &parent.data, stamp, cut));
            next_cuts.insert(key, outcome);
            outcome
        };
        let built = graph::build(&entries, &mut check, &stops);
        state.cuts = next_cuts;
        let mut agents: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (agent, owner) in &built.owners {
            if let Some(entry) = entries.get_mut(agent) {
                entry.owner = Some(owner.clone());
            }
            agents.entry(owner.clone()).or_default().push(agent.clone());
        }
        for (agent, error) in built.agent_errors {
            if let Some(entry) = entries.get_mut(&agent) {
                entry.owner_error = Some(error);
            }
        }
        let mut threads: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for (uid, entry) in &entries {
            if !entry.summary.sid.is_empty() {
                threads
                    .entry((entry.source.to_owned(), entry.summary.sid.clone()))
                    .or_default()
                    .push(uid.clone());
            }
        }
        let mut rows = built.rows;
        if let Some(names) = &names {
            names.enrich(&mut rows, &entries);
        }
        let document = signed_document(rows);
        let snapshot = Arc::new(IndexSnapshot {
            document,
            candidates: entries,
            threads,
            agents,
            catalog: built.catalog,
        });
        for path in transient {
            cache.remove(&path);
        }
        state.cache = cache;
        state.names = names;
        state.snapshot = Some(snapshot.clone());
        state.checked = Some(Instant::now());
        Ok(snapshot)
    }

    /// Claude subagent stop notices per owner uid, for the owners that have
    /// a sidecar whose last turn is open (the only ones whose `active` can
    /// be true). Cached by the owner's stamp: an unchanged owner costs
    /// nothing, a grown one is read from its last consumed LF, all in the
    /// bounded pool. Scans of owners that stay indexed are retained.
    fn refresh_stops(
        &self,
        state: &mut State,
        entries: &BTreeMap<String, CandidateRef>,
    ) -> BTreeMap<String, Stops> {
        let mains: HashMap<&Path, &CandidateRef> = entries
            .values()
            .filter(|entry| entry.source == "claude" && entry.summary.agent.is_none())
            .map(|entry| (entry.path.as_path(), entry))
            .collect();
        let mut needed: BTreeMap<&str, &CandidateRef> = BTreeMap::new();
        for entry in entries.values() {
            if entry.source != "claude"
                || !entry
                    .summary
                    .agent
                    .as_ref()
                    .is_some_and(|agent| agent.open_turn)
            {
                continue;
            }
            if let Some(owner) = owner_path(&entry.path)
                && let Some(owner) = mains.get(owner.as_path())
            {
                needed.insert(owner.uid.as_str(), owner);
            }
        }
        let mut previous = std::mem::take(&mut state.stops);
        let mut result = BTreeMap::new();
        let mut pending = Vec::new();
        for (uid, owner) in needed {
            let Some(stamp) = owner.stamp else {
                continue;
            };
            match previous.remove(&owner.data) {
                Some(scan) if scan.stamp() == Some(stamp) => {
                    result.insert(uid.to_owned(), scan.stops().clone());
                    state.stops.insert(owner.data.clone(), scan);
                }
                scan => pending.push((owner, stamp, scan.unwrap_or_default())),
            }
        }
        state.stop_scans += pending.len();
        let scanned = parallel_map(self.workers, pending, |(owner, stamp, mut scan)| {
            agent_stops::update(&mut scan, &owner.root, &owner.data, stamp);
            (owner, scan)
        });
        for (owner, scan) in scanned {
            result.insert(owner.uid.clone(), scan.stops().clone());
            state.stops.insert(owner.data.clone(), scan);
        }
        for (path, scan) in previous {
            if mains.contains_key(path.as_path()) {
                state.stops.insert(path, scan);
            }
        }
        result
    }

    fn names_changed(&self, state: &State) -> Result<bool, SessionError> {
        self.codex_index
            .as_deref()
            .map(|path| names::changed(path, state.names.as_deref()))
            .transpose()
            .map(|changed| changed.unwrap_or(false))
    }

    fn discover(&self) -> Result<Vec<Discovered>, SessionError> {
        let mut found = Vec::new();
        for (source, configured) in [
            ("claude", &self.roots.claude),
            ("codex", &self.roots.codex),
            ("grok", &self.roots.grok),
        ] {
            let Some(root) = configured else { continue };
            let dir = Dir::open_ambient_dir(root, ambient_authority())
                .map_err(|_| SessionError::new(503, format!("{source} 数据源目录暂时不可枚举")))?;
            dir.entries()
                .map_err(|_| SessionError::new(503, format!("{source} 数据源目录暂时不可枚举")))?;
            let mut walk = Walk {
                root,
                found: &mut found,
            };
            match source {
                "claude" => walk.claude(&dir)?,
                "codex" => walk.codex(&dir, PathBuf::new())?,
                _ => walk.grok(&dir)?,
            }
        }
        Ok(found)
    }

    fn read_all(&self, pending: &[Discovered]) -> Vec<Option<ReadOutcome>> {
        parallel_map(self.workers, pending.iter().collect(), read_candidate)
    }
}

/// `items.map(work)` in order, on at most `workers` scoped threads (the
/// bounded pool every list-time file read shares).
fn parallel_map<T: Send, R: Send>(
    workers: usize,
    items: Vec<T>,
    work: impl Fn(T) -> R + Sync,
) -> Vec<R> {
    let workers = workers.min(items.len());
    if workers <= 1 {
        return items.into_iter().map(work).collect();
    }
    let items: Vec<Mutex<Option<T>>> = items.into_iter().map(Some).map(Mutex::new).collect();
    let slots: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else {
                        break;
                    };
                    let Some(item) = item.lock().ok().and_then(|mut item| item.take()) else {
                        continue;
                    };
                    let outcome = work(item);
                    if let Ok(mut slot) = slots[index].lock() {
                        *slot = Some(outcome);
                    }
                }
            });
        }
    });
    slots
        .into_iter()
        .map(|slot| slot.into_inner().ok().flatten().expect("every item mapped"))
        .collect()
}

struct ReadOutcome {
    key: CacheKey,
    summary: Arc<RowSummary>,
    /// An I/O failure (permissions, a link or directory where a file should
    /// be): published for this snapshot but not cached, so a fix that leaves
    /// the stamp unchanged (chmod) is noticed by the next refresh.
    transient: bool,
}

struct Walk<'a> {
    root: &'a Path,
    found: &'a mut Vec<Discovered>,
}

fn utf8_name(entry: &cap_std::fs::DirEntry) -> Option<String> {
    entry.file_name().to_str().map(str::to_owned)
}

/// Directory children of `dir`, following ordinary directory aliases. An
/// unreadable subdirectory hides only its own files, never the list.
fn subdirectories(dir: &Dir, path: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(entries) = dir.entries() else {
        return names;
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let Some(name) = utf8_name(&entry) else {
            continue;
        };
        if kind.is_dir()
            || kind.is_symlink()
                && std::fs::metadata(path.join(&name)).is_ok_and(|meta| meta.is_dir())
        {
            names.push(name);
        }
    }
    names.sort();
    names
}

// Windows volume/file identities require metadata obtained from an open handle.
// Unix stat already carries the identity and avoids opening every discovery entry.
fn file_metadata(path: &Path) -> std::io::Result<Metadata> {
    #[cfg(windows)]
    {
        let metadata = std::fs::metadata(path)?;
        if !metadata.is_file() {
            return Err(std::io::Error::other("expected a regular history file"));
        }
        Metadata::from_file(&std::fs::File::open(path)?)
    }
    #[cfg(not(windows))]
    {
        std::fs::metadata(path).map(Metadata::from_just_metadata)
    }
}

/// File entries with their target stamps, matching `Path.is_file()`/`stat()`.
fn files(dir: &Dir, path: &Path) -> Vec<(String, Stamp)> {
    let mut found = Vec::new();
    let Ok(entries) = dir.entries() else {
        return found;
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_file() && !kind.is_symlink() {
            continue;
        }
        let Some(name) = utf8_name(&entry) else {
            continue;
        };
        let Ok(meta) = file_metadata(&path.join(&name)) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        found.push((name, Stamp::of(&meta)));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

impl Walk<'_> {
    /// `<root>/<project>/<sid>.jsonl` and `<root>/<project>/<sid>/subagents/agent-*.jsonl`.
    fn claude(&mut self, root: &Dir) -> Result<(), SessionError> {
        for project in subdirectories(root, self.root) {
            let project_path = self.root.join(&project);
            let Ok(project_dir) = Dir::open_ambient_dir(&project_path, ambient_authority()) else {
                continue;
            };
            for (name, stamp) in files(&project_dir, &project_path) {
                if !name.ends_with(".jsonl") || stamp.size == 0 {
                    continue;
                }
                let path = project_path.join(&name);
                self.found.push(Discovered {
                    source: "claude",
                    root: self.root.to_path_buf(),
                    data: path.clone(),
                    path,
                    summary_path: None,
                    stamp: Some(stamp),
                    summary_stamp: None,
                    agent_id: None,
                });
            }
            for session in subdirectories(&project_dir, &project_path) {
                let session_path = project_path.join(&session);
                let Ok(agents_dir) =
                    Dir::open_ambient_dir(session_path.join("subagents"), ambient_authority())
                else {
                    continue;
                };
                let agents_path = project_path.join(&session).join("subagents");
                let listed = files(&agents_dir, &agents_path);
                let sidecars: HashMap<&str, Stamp> = listed
                    .iter()
                    .filter(|(name, _)| name.ends_with(".meta.json"))
                    .map(|(name, stamp)| (name.as_str(), *stamp))
                    .collect();
                for (name, stamp) in listed.iter().map(|(name, stamp)| (name.clone(), *stamp)) {
                    let Some(stem) = name.strip_suffix(".jsonl") else {
                        continue;
                    };
                    let Some(id) = stem.strip_prefix("agent-") else {
                        continue;
                    };
                    if stamp.size == 0 {
                        continue;
                    }
                    let sidecar = format!("{stem}.meta.json");
                    let summary_stamp = sidecars.get(sidecar.as_str()).copied();
                    let path = agents_path.join(&name);
                    self.found.push(Discovered {
                        source: "claude",
                        root: self.root.to_path_buf(),
                        data: path.clone(),
                        path,
                        summary_path: summary_stamp.map(|_| agents_path.join(&sidecar)),
                        stamp: Some(stamp),
                        summary_stamp,
                        agent_id: Some(id.to_owned()),
                    });
                }
            }
        }
        Ok(())
    }

    /// Every `*.jsonl` under the root, like Python `rglob`.
    fn codex(&mut self, dir: &Dir, relative: PathBuf) -> Result<(), SessionError> {
        let path = self.root.join(&relative);
        for (name, stamp) in files(dir, &path) {
            if !name.ends_with(".jsonl") || stamp.size == 0 {
                continue;
            }
            let path = self.root.join(&relative).join(&name);
            self.found.push(Discovered {
                source: "codex",
                root: self.root.to_path_buf(),
                data: path.clone(),
                path,
                summary_path: None,
                stamp: Some(stamp),
                summary_stamp: None,
                agent_id: None,
            });
        }
        for child in subdirectories(dir, &path) {
            let Ok(child_dir) = dir.open_dir_nofollow(&child) else {
                continue;
            };
            self.codex(&child_dir, relative.join(&child))?;
        }
        Ok(())
    }

    /// `<root>/<cwd>/<session>/summary.json` (+ optional `chat_history.jsonl`).
    fn grok(&mut self, root: &Dir) -> Result<(), SessionError> {
        for project in subdirectories(root, self.root) {
            let Ok(project_dir) = root.open_dir_nofollow(&project) else {
                continue;
            };
            let project_path = self.root.join(&project);
            for session in subdirectories(&project_dir, &project_path) {
                let Ok(_session_dir) = project_dir.open_dir_nofollow(&session) else {
                    continue;
                };
                let path = self.root.join(&project).join(&session);
                let Ok(summary) = file_metadata(&path.join("summary.json")) else {
                    continue;
                };
                if !summary.is_file() {
                    continue;
                }
                // Only a genuinely absent chat is an empty history.
                let chat_path = path.join("chat_history.jsonl");
                let chat = match std::fs::metadata(&chat_path) {
                    Ok(metadata) if metadata.is_file() => {
                        file_metadata(&chat_path).ok().map(|meta| Stamp::of(&meta))
                    }
                    // Python GrokAdapter.read uses Path.is_file(): a missing,
                    // inaccessible, or non-file chat is an empty history.
                    _ => None,
                };
                self.found.push(Discovered {
                    source: "grok",
                    root: self.root.to_path_buf(),
                    data: path.join("chat_history.jsonl"),
                    summary_path: Some(path.join("summary.json")),
                    path,
                    stamp: chat,
                    summary_stamp: Some(Stamp::of(&summary)),
                    agent_id: None,
                });
            }
        }
        Ok(())
    }
}

/// Python `GrokAdapter._dir_size` (`rglob("*")`): the bytes of every regular
/// file under the Grok session directory, recursively. Symlinked directories
/// are not entered (`rglob` recurses with `follow_symlinks=False`), a
/// symlinked file counts its target's size like `Path.is_file()`; the walk
/// is not otherwise capped. `None` when the directory cannot be opened.
pub(crate) fn directory_size(root: &Path, dir: &Path) -> Option<u64> {
    let relative = dir.strip_prefix(root).ok()?;
    let mut handle = Dir::open_ambient_dir(root, ambient_authority()).ok()?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return None;
        };
        handle = handle.open_dir_nofollow(name).ok()?;
    }
    let mut total = 0;
    directory_size_in(&handle, &mut total);
    Some(total)
}

fn directory_size_in(dir: &Dir, total: &mut u64) {
    let Ok(entries) = dir.entries() else {
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if let Ok(child) = dir.open_dir_nofollow(entry.file_name()) {
                directory_size_in(&child, total);
            }
            continue;
        }
        // A link counts its target like `Path.is_file()`/`stat()` do, as
        // long as the target stays inside this directory's sandbox.
        let meta = if kind.is_symlink() {
            dir.metadata(entry.file_name())
        } else {
            entry.metadata()
        };
        if let Ok(meta) = meta
            && meta.is_file()
        {
            *total = total.saturating_add(meta.len());
        }
    }
}

/// Open an indexed path with the same link-following behavior as Python's
/// `open()`. The path itself must still have been discovered below `root`.
fn open_indexed(root: &Path, path: &Path) -> std::io::Result<std::fs::File> {
    path.strip_prefix(root)
        .map_err(|_| std::io::Error::other("path outside root"))?;
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(file)
}

enum FileRead {
    Vanished,
    Unreadable,
    Data {
        head: Vec<u8>,
        tail: Vec<u8>,
        tail_start: u64,
        stamp: Stamp,
    },
}

/// One opened file version: its stamp and bounded range reads. Real files
/// are cap-std handles; tests substitute a source whose stamp moves.
trait StampedSource {
    fn stamp(&mut self) -> Option<Stamp>;
    fn read_range(&mut self, start: u64, length: u64) -> std::io::Result<Vec<u8>>;
}

impl StampedSource for std::fs::File {
    fn stamp(&mut self) -> Option<Stamp> {
        Metadata::from_file(self).ok().map(|meta| Stamp::of(&meta))
    }
    fn read_range(&mut self, start: u64, length: u64) -> std::io::Result<Vec<u8>> {
        read_exact_at(self, start, length)
    }
}

enum OpenError {
    Vanished,
    Unreadable,
}

fn open_source(root: &Path, path: &Path) -> Result<std::fs::File, OpenError> {
    match open_indexed(root, path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(OpenError::Vanished),
        Err(_) => Err(OpenError::Unreadable),
    }
}

/// Head/tail bytes of a data file, re-read while its stamp moves under us.
fn read_data(root: &Path, path: &Path) -> FileRead {
    read_data_from(&mut || open_source(root, path).map(|file| Box::new(file) as Box<_>))
}

/// The bounded read with its retry policy: `stat`, read head/tail sized by
/// that stamp, `stat` again; a moved stamp re-opens up to [`READ_ATTEMPTS`]
/// times and the last attempt is published with the stamp its bytes belong to.
fn read_data_from(open: &mut dyn FnMut() -> Result<Box<dyn StampedSource>, OpenError>) -> FileRead {
    let mut last = None;
    for _ in 0..READ_ATTEMPTS {
        let mut file = match open() {
            Ok(file) => file,
            Err(OpenError::Vanished) => return FileRead::Vanished,
            Err(OpenError::Unreadable) => return FileRead::Unreadable,
        };
        let Some(before) = file.stamp() else {
            return FileRead::Unreadable;
        };
        let size = before.size;
        let (head, tail, tail_start) = if size <= TAIL_BYTES {
            let Ok(whole) = file.read_range(0, size) else {
                return FileRead::Unreadable;
            };
            let head_len = whole.len().min(HEAD_BYTES as usize);
            (whole[..head_len].to_vec(), whole, 0)
        } else {
            let Ok(head) = file.read_range(0, HEAD_BYTES) else {
                return FileRead::Unreadable;
            };
            let start = size - TAIL_BYTES;
            let Ok(tail) = file.read_range(start, TAIL_BYTES) else {
                return FileRead::Unreadable;
            };
            (head, tail, start)
        };
        let Some(after) = file.stamp() else {
            return FileRead::Unreadable;
        };
        let read = FileRead::Data {
            head,
            tail,
            tail_start,
            stamp: before,
        };
        if after == before {
            return read;
        }
        last = Some(read);
    }
    last.unwrap_or(FileRead::Unreadable)
}

fn read_exact_at(
    file: &mut (impl Read + Seek),
    start: u64,
    length: u64,
) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(start))?;
    let mut buffer = Vec::new();
    file.take(length).read_to_end(&mut buffer)?;
    Ok(buffer)
}

enum SidecarRead {
    Vanished,
    Bytes(Vec<u8>, Stamp),
    Failed(Option<Stamp>, String),
}

/// Whole-file read of a sidecar, bounded by its observed source length.
fn read_sidecar(root: &Path, path: &Path, label: &str) -> SidecarRead {
    let mut last = None;
    for _ in 0..READ_ATTEMPTS {
        let mut file = match open_indexed(root, path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return SidecarRead::Vanished;
            }
            Err(_) => return SidecarRead::Failed(None, format!("{label}暂时不可读取")),
        };
        let Ok(before) = Metadata::from_file(&file) else {
            return SidecarRead::Failed(None, format!("{label}暂时不可读取"));
        };
        let before = Stamp::of(&before);
        let Ok(bytes) = read_exact_at(&mut file, 0, before.size) else {
            return SidecarRead::Failed(Some(before), format!("{label}暂时不可读取"));
        };
        let Ok(after) = Metadata::from_file(&file) else {
            return SidecarRead::Failed(Some(before), format!("{label}暂时不可读取"));
        };
        let after = Stamp::of(&after);
        let read = SidecarRead::Bytes(bytes, before);
        if after == before {
            return read;
        }
        last = Some(read);
    }
    last.unwrap_or_else(|| SidecarRead::Failed(None, format!("{label}暂时不可读取")))
}

/// Summarize one discovered candidate; `None` when its files vanished.
fn read_candidate(candidate: &Discovered) -> Option<ReadOutcome> {
    let data = match candidate.stamp {
        Some(_) => match read_data(&candidate.root, &candidate.data) {
            FileRead::Vanished if candidate.source == "grok" => None,
            FileRead::Vanished => return None,
            FileRead::Unreadable => {
                let summary = unreadable(candidate);
                return Some(ReadOutcome {
                    key: candidate.key(),
                    summary: Arc::new(summary),
                    transient: true,
                });
            }
            FileRead::Data {
                head,
                tail,
                tail_start,
                stamp,
            } => Some((head, tail, tail_start, stamp)),
        },
        None => None,
    };
    let mut transient = false;
    let sidecar = match &candidate.summary_path {
        Some(path) => {
            let label = if candidate.source == "grok" {
                "Grok summary.json "
            } else {
                "子代理元数据 (meta.json) "
            };
            match read_sidecar(&candidate.root, path, label) {
                SidecarRead::Vanished if candidate.source == "grok" => return None,
                SidecarRead::Vanished => None,
                SidecarRead::Bytes(bytes, stamp) => Some((Some(bytes), Some(stamp), None)),
                SidecarRead::Failed(stamp, reason) => {
                    transient = reason.contains("不可读取");
                    Some((None, stamp, Some(reason)))
                }
            }
        }
        None => None,
    };
    let data_file = data
        .as_ref()
        .map(|(head, tail, tail_start, stamp)| DataFile {
            head,
            tail,
            tail_start: *tail_start,
            stamp: *stamp,
        });
    let sidecar_bytes = sidecar
        .as_ref()
        .map(|(bytes, stamp, reason)| match (bytes, reason) {
            (Some(bytes), _) => SidecarBytes::Bytes {
                bytes,
                stamp: stamp.expect("bytes carry their stamp"),
            },
            (None, reason) => SidecarBytes::Failed {
                stamp: *stamp,
                reason: reason.clone().unwrap_or_default(),
            },
        });
    let mut summary = summary::summarize(&Input {
        source: candidate.source,
        path: &candidate.path,
        data: data_file,
        sidecar: sidecar_bytes,
    });
    // Python's Grok `size` is the whole session directory (updates.jsonl,
    // events, tool definitions…), refreshed with the summary/chat stamps
    // exactly as here: the cached summary carries the size read with it.
    if candidate.source == "grok"
        && let Some(size) = directory_size(&candidate.root, &candidate.path)
    {
        summary.size = size;
    }
    Some(ReadOutcome {
        key: CacheKey {
            stamp: data.as_ref().map(|(_, _, _, stamp)| *stamp),
            summary_stamp: sidecar.as_ref().and_then(|(_, stamp, _)| *stamp),
        },
        summary: Arc::new(summary),
        transient,
    })
}

/// A row for a file that exists but cannot be read (permissions, I/O).
fn unreadable(candidate: &Discovered) -> RowSummary {
    let mut summary = summary::summarize(&Input {
        source: candidate.source,
        path: &candidate.path,
        data: None,
        sidecar: None,
    });
    summary.size = candidate.stamp.map_or(0, |stamp| stamp.size);
    if let Some(stamp) = candidate.stamp {
        summary.created = summary::iso_seconds(stamp.mtime_ns);
        summary.updated = summary.created.clone();
    }
    summary.unsupported = Some("会话文件暂时不可读取".to_owned());
    summary.warnings.clear();
    summary
}

/// One-byte probe: is `cut` a line boundary of the parent at this stamp?
fn check_cut(root: &Path, data: &Path, stamp: Stamp, cut: u64) -> CutCheck {
    if cut > stamp.size {
        return CutCheck::BeyondEnd;
    }
    if cut == 0 {
        return CutCheck::Boundary;
    }
    let Ok(mut file) = open_indexed(root, data) else {
        return CutCheck::Unreadable;
    };
    let Ok(meta) = Metadata::from_file(&file) else {
        return CutCheck::Unreadable;
    };
    if Stamp::of(&meta) != stamp {
        return CutCheck::Unreadable;
    }
    match read_exact_at(&mut file, cut - 1, 1) {
        Ok(byte) if byte == b"\n" => CutCheck::Boundary,
        Ok(byte) if byte.len() == 1 => CutCheck::NotBoundary,
        _ => CutCheck::Unreadable,
    }
}

#[cfg(test)]
mod tests;
