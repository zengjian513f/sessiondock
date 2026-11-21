//! Persistent search-text cache: one file per main session holding the exact
//! searchable body (`search::body`) of one file version, plus the parse-slot
//! budget every producer shares (docs/read-model.md "搜索").
//!
//! An entry is keyed by the session uid and validated by its version — the
//! canonical JSON `SessionStore::search_version` derives from `stat` and the
//! index (data file `size/mtime/dev:ino:ctime`, Claude display pin, Codex
//! fixed-prefix parents with their versions) — plus the build fingerprint,
//! so a different binary never trusts text projected by an older parser.
//! Deterministic open failures (501/413) are cached under the same version so
//! an unreadable fork is not re-streamed by every search. A changed session is
//! parsed again whole and transiently: incremental decoding would need its
//! decoded records kept resident (several times the file), which the memory
//! rule of docs/read-model.md forbids. Files are
//! private (an existing 0700 directory of its own, entries 0600,
//! same-directory temporary + rename); the directory is bounded in bytes with
//! least-recently-used eviction. Nothing here reads native history: the
//! caller parses, this module remembers.
//!
//! Without a directory the cache lives in memory only, capped at 64 MiB.
//!
//! Next to the bodies, the cache keeps in memory a case-folded copy
//! (`search::fold`) of bodies it has served or produced, under its own
//! byte budget with the same least-recently-used eviction: a query's
//! prefilter (`search::prefilter`) decides on the folded copy whether a
//! body can match at all, so a body that cannot is never opened or read.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};

use serde_json::{Value, json};

use super::{SearchError, fold};
use crate::sessions::SearchVersion;

/// Bump when `search::body`'s text or the header format changes; the build
/// fingerprint already covers parser changes between binaries.
pub const SCHEMA: u32 = 1;
/// Memory-only cap when no directory is configured.
pub const MEMORY_ONLY_BYTES: u64 = 64 * 1024 * 1024;
/// Default byte budget of the resident folded copies
/// (`SESSIONDOCK_SEARCH_FOLD_BYTES`); bodies beyond it are read from the
/// cache file and matched without a prefilter.
pub const FOLD_BYTES: u64 = 128 * 1024 * 1024;
/// Per-entry accounting overhead of a folded copy (map entry, version key).
const FOLD_ENTRY_BYTES: u64 = 256;
/// One parse slot covers this many bytes of native input; larger files take
/// proportionally more slots so concurrent cold parses stay bounded in RSS
/// (a transient projection costs several times its file).
pub const SLOT_BYTES: u64 = 8 * 1024 * 1024;
const TEMP_PREFIX: &str = ".search-tmp-";
/// Read size of the chunked body stream; the buffer grows only for a line
/// longer than this and is reused across bodies by one worker.
pub const CHUNK_BYTES: usize = 1024 * 1024;

/// One cached outcome of projecting a main view for search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cached {
    Text(Arc<String>),
    /// A deterministic open failure (501/413) of exactly these file versions.
    Error {
        status: u16,
        message: String,
    },
}

impl Cached {
    fn kind(&self) -> &'static str {
        match self {
            Cached::Text(_) => "text",
            Cached::Error { .. } => "error",
        }
    }
    fn payload(&self) -> &str {
        match self {
            Cached::Text(text) => text,
            Cached::Error { message, .. } => message,
        }
    }
    /// Only failures that are a function of the bytes are worth keeping;
    /// 503 "changed while reading" and lock errors are retried next time.
    pub fn cacheable_error(status: u16) -> bool {
        matches!(status, 501 | 413)
    }
}

/// A cached body, streamed from its file (or held in memory-only mode).
pub struct TextReader {
    source: ReaderSource,
}

enum ReaderSource {
    File { file: fs::File, remaining: u64 },
    Memory(Arc<String>),
}

impl TextReader {
    pub fn memory(text: Arc<String>) -> Self {
        Self {
            source: ReaderSource::Memory(text),
        }
    }

    pub fn len(&self) -> u64 {
        match &self.source {
            ReaderSource::File { remaining, .. } => *remaining,
            ReaderSource::Memory(text) => text.len() as u64,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The whole body in memory (non-chunkable queries only).
    pub fn read_all(self) -> io::Result<Arc<String>> {
        match self.source {
            ReaderSource::Memory(text) => Ok(text),
            ReaderSource::File { file, remaining } => {
                let mut data = Vec::with_capacity(remaining as usize);
                file.take(remaining).read_to_end(&mut data)?;
                if data.len() as u64 != remaining {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "short cache entry",
                    ));
                }
                String::from_utf8(data).map(Arc::new).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "cache entry is not UTF-8")
                })
            }
        }
    }

    /// Feed the body to `f` as line-aligned chunks: every chunk but the last
    /// ends with `\n`, the last is flagged. `buffer` is the caller's
    /// reusable read buffer. `f` returns whether to continue.
    pub fn for_each_chunk(
        mut self,
        buffer: &mut Vec<u8>,
        f: impl FnMut(&str, bool) -> Result<bool, SearchError>,
    ) -> Result<(), SearchError> {
        self.chunks(buffer, true, f)
    }

    /// Fold on UTF-8 boundaries, including inside long lines. Restore the
    /// reader so matching can still use the original body.
    fn folded(
        &mut self,
        buffer: &mut Vec<u8>,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, SearchError> {
        // Folding maps to a representative of equal or smaller UTF-8 width.
        // If the raw size exceeds the budget, count first to avoid building
        // a large partial copy that cannot be retained.
        let capacity = if self.len() <= limit as u64 {
            self.len() as usize
        } else {
            let mut bytes = 0;
            let mut fits = true;
            self.fold_chunks(buffer, |chunk| {
                if let Some(len) = fold::len_within(chunk, limit - bytes) {
                    bytes += len;
                } else {
                    fits = false;
                }
                fits
            })?;
            if !fits {
                return Ok(None);
            }
            bytes
        };
        let mut folded = Vec::with_capacity(capacity);
        let mut fits = true;
        self.fold_chunks(buffer, |chunk| {
            let remaining = capacity - folded.len();
            fits = chunk.len() <= remaining || fold::len_within(chunk, remaining).is_some();
            if fits {
                fold::fold_into(chunk, &mut folded);
            }
            fits
        })?;
        Ok(fits.then_some(folded))
    }

    fn fold_chunks(
        &mut self,
        buffer: &mut Vec<u8>,
        mut f: impl FnMut(&str) -> bool,
    ) -> Result<(), SearchError> {
        let position = match &mut self.source {
            ReaderSource::File { file, .. } => {
                Some(file.stream_position().map_err(cache_read_error)?)
            }
            ReaderSource::Memory(_) => None,
        };
        let result = self.chunks(buffer, false, |chunk, _| Ok(f(chunk)));
        if let (Some(position), ReaderSource::File { file, .. }) = (position, &mut self.source) {
            file.seek(SeekFrom::Start(position))
                .map_err(cache_read_error)?;
        }
        result
    }

    fn chunks(
        &mut self,
        buffer: &mut Vec<u8>,
        line_aligned: bool,
        mut f: impl FnMut(&str, bool) -> Result<bool, SearchError>,
    ) -> Result<(), SearchError> {
        let (file, mut remaining) = match &mut self.source {
            ReaderSource::Memory(text) => {
                f(text, true)?;
                return Ok(());
            }
            ReaderSource::File { file, remaining } => (file, *remaining),
        };
        let invalid = |_| SearchError::new(503, "session_error", "搜索文本缓存条目不是 UTF-8");
        buffer.clear();
        let mut filled = 0usize;
        loop {
            if buffer.len() < filled + CHUNK_BYTES {
                buffer.resize(filled + CHUNK_BYTES, 0);
            }
            let want = (buffer.len() - filled).min(remaining as usize);
            let read = if want == 0 {
                0
            } else {
                file.read(&mut buffer[filled..filled + want])
                    .map_err(cache_read_error)?
            };
            if read == 0 {
                if remaining != 0 {
                    return Err(SearchError::new(
                        503,
                        "session_error",
                        "搜索文本缓存条目被截断",
                    ));
                }
                let chunk = std::str::from_utf8(&buffer[..filled]).map_err(invalid)?;
                f(chunk, true)?;
                return Ok(());
            }
            let read_start = filled;
            filled += read;
            remaining -= read as u64;
            if remaining == 0 {
                let chunk = std::str::from_utf8(&buffer[..filled]).map_err(invalid)?;
                f(chunk, true)?;
                return Ok(());
            }
            let chunk = if line_aligned {
                let Some(newline) = buffer[read_start..filled]
                    .iter()
                    .rposition(|byte| *byte == b'\n')
                else {
                    // A line longer than the buffer: keep reading it.
                    continue;
                };
                std::str::from_utf8(&buffer[..read_start + newline + 1]).map_err(invalid)?
            } else {
                match std::str::from_utf8(&buffer[..filled]) {
                    Ok(chunk) => chunk,
                    Err(error) if error.error_len().is_none() => {
                        std::str::from_utf8(&buffer[..error.valid_up_to()]).map_err(invalid)?
                    }
                    Err(error) => return Err(invalid(error)),
                }
            };
            if !f(chunk, false)? {
                return Ok(());
            }
            let consumed = chunk.len();
            buffer.copy_within(consumed..filled, 0);
            filled -= consumed;
        }
    }
}

fn cache_read_error(error: io::Error) -> SearchError {
    SearchError::new(503, "session_error", error.to_string())
}

/// A hit of the version-checked lookup.
pub enum Hit {
    Text(TextReader),
    Error { status: u16, message: String },
}

/// The answer of a version-checked lookup: the entry at exactly this
/// version under this build, or nothing (absent, another version, another
/// build, unreadable).
pub enum Lookup {
    Hit(Hit),
    Miss,
}

struct Entry {
    bytes: u64,
    used: u64,
    /// Memory-only mode keeps the payload here; on disk it is the file.
    memory: Option<(Value, Cached)>,
}

struct State {
    entries: HashMap<String, Entry>,
    total: u64,
    clock: u64,
}

/// The folded copy of one body at one version (`fold::fold`).
pub struct Folded {
    version: Value,
    pub text: Box<[u8]>,
}

struct FoldEntry {
    used: u64,
    folded: Arc<Folded>,
}

struct FoldTier {
    entries: HashMap<String, FoldEntry>,
    total: u64,
    clock: u64,
}

pub struct SearchCache {
    dir: Option<PathBuf>,
    limit: u64,
    fold_limit: u64,
    fingerprint: String,
    state: Mutex<State>,
    folded: Mutex<FoldTier>,
    inflight: Mutex<HashSet<String>>,
    inflight_free: Condvar,
}

/// Cache statistics for reports and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: u64,
    pub limit: u64,
    pub persistent: bool,
    /// Resident folded copies and their accounted bytes.
    pub folded_entries: usize,
    pub folded_bytes: u64,
    pub fold_limit: u64,
}

/// Search-text of one uid is produced by one thread at a time; the second
/// producer waits and re-reads the cache instead of parsing the same bytes.
pub struct InflightGuard<'a> {
    cache: &'a SearchCache,
    uid: String,
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.cache.inflight.lock() {
            set.remove(&self.uid);
        }
        self.cache.inflight_free.notify_all();
    }
}

fn uid_file_name(uid: &str) -> Option<String> {
    let (source, hex) = uid.split_once(':')?;
    if !matches!(
        source,
        "claude" | "codex" | "grok" | "opencode" | "agy" | "codebuddy" | "droid"
    ) || hex.len() != 16
        || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(format!("{source}-{hex}"))
}

fn file_name_uid(name: &str) -> Option<String> {
    let (source, hex) = name.split_once('-')?;
    let uid = format!("{source}:{hex}");
    uid_file_name(&uid)
        .filter(|round| round == name)
        .map(|_| uid)
}

/// `SCHEMA / crate version / size:mtime_ns of the running binary`: any
/// other binary rebuilds its entries instead of trusting text an older
/// projection produced. The identity is the executable's stamp, not a digest:
/// hashing the whole binary before the listener binds cost 7 s on a debug
/// build (startup must stay sub-second), and a redeployed binary is always a
/// new file. Unreadable metadata yields "unknown" (entries rebuild every start).
fn build_fingerprint() -> String {
    static FINGERPRINT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FINGERPRINT
        .get_or_init(|| {
            let exe = std::env::current_exe()
                .and_then(fs::metadata)
                .map(|meta| {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos())
                        .unwrap_or(0);
                    format!("{}:{mtime}", meta.len())
                })
                .unwrap_or_else(|_| "unknown".to_owned());
            format!("{SCHEMA}/{}/{exe}", env!("CARGO_PKG_VERSION"))
        })
        .clone()
}

fn check_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

impl SearchCache {
    /// Open the existing private directory and take stock of its entries;
    /// `None` keeps everything in memory. Leftover temporaries are removed;
    /// foreign files are ignored. `fold_limit` bounds the resident folded
    /// copies.
    pub fn open(dir: Option<PathBuf>, limit: u64, fold_limit: u64) -> io::Result<Self> {
        let fingerprint = build_fingerprint();
        let mut state = State {
            entries: HashMap::new(),
            total: 0,
            clock: 0,
        };
        let limit = match &dir {
            Some(dir) => {
                check_private_dir(dir)?;
                let mut found = Vec::new();
                for entry in fs::read_dir(dir)? {
                    let entry = entry?;
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else { continue };
                    let Ok(meta) = entry.metadata() else { continue };
                    if name.starts_with(TEMP_PREFIX) {
                        let _ = fs::remove_file(entry.path());
                        continue;
                    }
                    if !meta.is_file() {
                        continue;
                    }
                    let Some(uid) = file_name_uid(name) else {
                        continue;
                    };
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |duration| duration.as_secs());
                    found.push((modified, uid, meta.len()));
                }
                found.sort();
                for (_, uid, bytes) in found {
                    state.clock += 1;
                    state.total += bytes;
                    state.entries.insert(
                        uid,
                        Entry {
                            bytes,
                            used: state.clock,
                            memory: None,
                        },
                    );
                }
                limit
            }
            None => limit.min(MEMORY_ONLY_BYTES),
        };
        let mut cache = Self {
            dir,
            limit,
            fold_limit,
            fingerprint,
            state: Mutex::new(state),
            folded: Mutex::new(FoldTier {
                entries: HashMap::new(),
                total: 0,
                clock: 0,
            }),
            inflight: Mutex::new(HashSet::new()),
            inflight_free: Condvar::new(),
        };
        cache.evict(None);
        Ok(std::mem::take(&mut cache))
    }

    pub fn persistent(&self) -> bool {
        self.dir.is_some()
    }

    pub fn stats(&self) -> CacheStats {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let folded = self.folded.lock().unwrap_or_else(|e| e.into_inner());
        CacheStats {
            entries: state.entries.len(),
            bytes: state.total,
            limit: self.limit,
            persistent: self.dir.is_some(),
            folded_entries: folded.entries.len(),
            folded_bytes: folded.total,
            fold_limit: self.fold_limit,
        }
    }

    /// The resident folded copy of `uid` when it is of exactly `version`.
    pub fn folded(&self, uid: &str, version: &SearchVersion) -> Option<Arc<Folded>> {
        let mut tier = self.folded.lock().unwrap_or_else(|e| e.into_inner());
        tier.clock += 1;
        let clock = tier.clock;
        let entry = tier.entries.get_mut(uid)?;
        if entry.folded.version != version.key {
            return None;
        }
        entry.used = clock;
        Some(entry.folded.clone())
    }

    /// Whether `folded(uid, version)` would answer, without touching it.
    pub fn has_folded(&self, uid: &str, version: &SearchVersion) -> bool {
        let tier = self.folded.lock().unwrap_or_else(|e| e.into_inner());
        tier.entries
            .get(uid)
            .is_some_and(|entry| entry.folded.version == version.key)
    }

    /// Maximum bytes needed for a potentially cacheable folded body. A
    /// Unicode scalar occupies at most four input bytes and at least one
    /// folded byte. Use the full limit for bodies that might shrink to fit.
    pub fn folded_capacity(&self, len: u64) -> Option<usize> {
        let limit = self.fold_limit.checked_sub(FOLD_ENTRY_BYTES)?;
        (len.div_ceil(4) <= limit)
            .then(|| usize::try_from(len.min(limit)).ok())
            .flatten()
    }

    pub fn backfill_folded(
        &self,
        uid: &str,
        version: &SearchVersion,
        reader: &mut TextReader,
        buffer: &mut Vec<u8>,
        capacity: usize,
    ) -> Result<(), SearchError> {
        if let Some(folded) = reader.folded(buffer, capacity)? {
            self.remember_folded(uid, version, folded);
        }
        Ok(())
    }

    /// Keep the folded copy, evicting the least recently used copies past
    /// the resident budget. Both producers bound their allocation first.
    fn remember_folded(&self, uid: &str, version: &SearchVersion, folded: Vec<u8>) {
        let folded = folded.into_boxed_slice();
        let bytes = folded.len() as u64 + FOLD_ENTRY_BYTES;
        let mut tier = self.folded.lock().unwrap_or_else(|e| e.into_inner());
        tier.clock += 1;
        let clock = tier.clock;
        if let Some(previous) = tier.entries.remove(uid) {
            tier.total -= previous.folded.text.len() as u64 + FOLD_ENTRY_BYTES;
        }
        tier.total += bytes;
        tier.entries.insert(
            uid.to_owned(),
            FoldEntry {
                used: clock,
                folded: Arc::new(Folded {
                    version: version.key.clone(),
                    text: folded,
                }),
            },
        );
        while tier.total > self.fold_limit {
            let Some(victim) = tier
                .entries
                .iter()
                .filter(|(other, _)| other.as_str() != uid)
                .min_by_key(|(_, entry)| entry.used)
                .map(|(other, _)| other.clone())
            else {
                break;
            };
            let entry = tier.entries.remove(&victim).expect("selected entry");
            tier.total -= entry.folded.text.len() as u64 + FOLD_ENTRY_BYTES;
        }
    }

    fn forget_folded(&self, uid: &str) {
        let mut tier = self.folded.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = tier.entries.remove(uid) {
            tier.total -= entry.folded.text.len() as u64 + FOLD_ENTRY_BYTES;
        }
    }

    fn path(&self, uid: &str) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(uid_file_name(uid)?))
    }

    /// Serialize producers of one uid; the guard releases on drop.
    pub fn inflight(&self, uid: &str) -> InflightGuard<'_> {
        let mut set = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        while set.contains(uid) {
            set = self
                .inflight_free
                .wait(set)
                .unwrap_or_else(|e| e.into_inner());
        }
        set.insert(uid.to_owned());
        InflightGuard {
            cache: self,
            uid: uid.to_owned(),
        }
    }

    fn header(&self, uid: &str, version: &SearchVersion, cached: &Cached) -> Value {
        json!({
            "schema": SCHEMA,
            "build": self.fingerprint,
            "uid": uid,
            "version": version.key,
            "kind": cached.kind(),
            "status": match cached { Cached::Error { status, .. } => json!(status), _ => Value::Null },
            "bytes": cached.payload().len(),
        })
    }

    fn current(&self, header: &Value, version: &SearchVersion) -> bool {
        header["schema"] == SCHEMA
            && header["build"] == self.fingerprint
            && header["version"] == version.key
    }

    /// The entry of `uid` when it carries exactly `version` under this build;
    /// a text hit is a stream positioned at the body, not the body itself.
    pub fn get(&self, uid: &str, version: &SearchVersion) -> Lookup {
        let Some(path) = self.path(uid) else {
            return self.get_memory(uid, version);
        };
        let Ok(mut file) = fs::File::open(&path) else {
            return Lookup::Miss;
        };
        let Ok(meta) = file.metadata() else {
            return Lookup::Miss;
        };
        if !meta.is_file() {
            return Lookup::Miss;
        }
        let mut head = Vec::new();
        let newline = loop {
            let start = head.len();
            head.resize(start + 4096, 0);
            let Ok(read) = file.read(&mut head[start..]) else {
                return Lookup::Miss;
            };
            head.truncate(start + read);
            if let Some(position) = head[start..].iter().position(|byte| *byte == b'\n') {
                break start + position;
            }
            if read == 0 {
                return Lookup::Miss;
            }
        };
        let Ok(header) = serde_json::from_slice::<Value>(&head[..newline]) else {
            return Lookup::Miss;
        };
        if header["uid"] != uid {
            return Lookup::Miss;
        }
        if !self.current(&header, version) {
            return Lookup::Miss;
        }
        let expected = header["bytes"].as_u64().unwrap_or(u64::MAX);
        let payload_start = newline as u64 + 1;
        if meta.len().checked_sub(payload_start) != Some(expected) {
            return Lookup::Miss;
        }
        self.touch(uid);
        match header["kind"].as_str() {
            Some("text") => {
                if file.seek(SeekFrom::Start(payload_start)).is_err() {
                    return Lookup::Miss;
                }
                Lookup::Hit(Hit::Text(TextReader {
                    source: ReaderSource::File {
                        file,
                        remaining: expected,
                    },
                }))
            }
            Some("error") => {
                let mut message = String::new();
                if file.seek(SeekFrom::Start(payload_start)).is_err()
                    || file.take(expected).read_to_string(&mut message).is_err()
                {
                    return Lookup::Miss;
                }
                Lookup::Hit(Hit::Error {
                    status: header["status"].as_u64().unwrap_or(501) as u16,
                    message,
                })
            }
            _ => Lookup::Miss,
        }
    }

    fn get_memory(&self, uid: &str, version: &SearchVersion) -> Lookup {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.clock += 1;
        let clock = state.clock;
        let Some(entry) = state.entries.get_mut(uid) else {
            return Lookup::Miss;
        };
        let Some((header, cached)) = &entry.memory else {
            return Lookup::Miss;
        };
        if !self.current(header, version) {
            return Lookup::Miss;
        }
        entry.used = clock;
        Lookup::Hit(match cached {
            Cached::Text(text) => Hit::Text(TextReader::memory(text.clone())),
            Cached::Error { status, message } => Hit::Error {
                status: *status,
                message: message.clone(),
            },
        })
    }

    fn touch(&self, uid: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.clock += 1;
        let clock = state.clock;
        if let Some(entry) = state.entries.get_mut(uid) {
            entry.used = clock;
        }
    }

    /// Remember `cached` as the text of `uid` at `version`. Failures to
    /// persist are swallowed: the next search parses once more.
    pub fn put(&self, uid: &str, version: &SearchVersion, cached: &Cached) {
        let header = self.header(uid, version, cached);
        let bytes = cached.payload().len() as u64 + 256;
        match self.path(uid) {
            Some(path) => {
                if bytes > self.limit || self.write(&path, &header, cached).is_err() {
                    return;
                }
                self.record(uid, bytes, None);
            }
            None => {
                if bytes > self.limit {
                    return;
                }
                self.record(uid, bytes, Some((header, cached.clone())));
            }
        }
        self.forget_folded(uid);
        if let Cached::Text(text) = cached
            && let Some(capacity) = self.folded_capacity(text.len() as u64)
            && let Ok(Some(folded)) =
                TextReader::memory(text.clone()).folded(&mut Vec::new(), capacity)
        {
            self.remember_folded(uid, version, folded);
        }
        self.evict(Some(uid));
    }

    fn record(&self, uid: &str, bytes: u64, memory: Option<(Value, Cached)>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.clock += 1;
        let clock = state.clock;
        if let Some(previous) = state.entries.remove(uid) {
            state.total -= previous.bytes;
        }
        state.total += bytes;
        state.entries.insert(
            uid.to_owned(),
            Entry {
                bytes,
                used: clock,
                memory,
            },
        );
    }

    fn write(&self, path: &Path, header: &Value, cached: &Cached) -> io::Result<()> {
        let dir = path.parent().expect("cache file has a parent");
        let temp = dir.join(format!(
            "{TEMP_PREFIX}{}-{}",
            std::process::id(),
            path.file_name().and_then(|n| n.to_str()).unwrap_or("entry")
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options.open(&temp)?;
            file.write_all(&serde_json::to_vec(header).expect("header serializes"))?;
            file.write_all(b"\n")?;
            file.write_all(cached.payload().as_bytes())?;
            file.sync_data()?;
            drop(file);
            fs::rename(&temp, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    /// Drop least recently used entries until the total fits the limit;
    /// `keep` (the entry just written) is never the victim.
    fn evict(&self, keep: Option<&str>) {
        let victims = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let mut victims = Vec::new();
            while state.total > self.limit {
                let Some(uid) = state
                    .entries
                    .iter()
                    .filter(|(uid, _)| keep != Some(uid.as_str()))
                    .min_by_key(|(_, entry)| entry.used)
                    .map(|(uid, _)| uid.clone())
                else {
                    break;
                };
                let entry = state.entries.remove(&uid).expect("selected entry");
                state.total -= entry.bytes;
                victims.push(uid);
            }
            victims
        };
        for uid in victims {
            self.forget_folded(&uid);
            if let Some(path) = self.path(&uid) {
                let _ = fs::remove_file(path);
            }
        }
    }

    /// Forget one session (its file and folded copy are removed).
    pub fn remove(&self, uid: &str) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = state.entries.remove(uid) {
                state.total -= entry.bytes;
            }
        }
        self.forget_folded(uid);
        if let Some(path) = self.path(uid) {
            let _ = fs::remove_file(path);
        }
    }
}

impl Default for SearchCache {
    fn default() -> Self {
        Self {
            dir: None,
            limit: MEMORY_ONLY_BYTES,
            fold_limit: FOLD_BYTES,
            fingerprint: build_fingerprint(),
            state: Mutex::new(State {
                entries: HashMap::new(),
                total: 0,
                clock: 0,
            }),
            folded: Mutex::new(FoldTier {
                entries: HashMap::new(),
                total: 0,
                clock: 0,
            }),
            inflight: Mutex::new(HashSet::new()),
            inflight_free: Condvar::new(),
        }
    }
}

struct Slots {
    available: usize,
}

/// The parse budget: `total` slots shared by every search.
/// A parse of `bytes` takes `ceil(bytes / SLOT_BYTES)` slots (capped at
/// `total`), so cold parses in flight stay bounded in bytes, not only in
/// count.
pub struct ParseSlots {
    total: usize,
    unit: u64,
    state: Mutex<Slots>,
    changed: Condvar,
}

pub struct SlotGuard<'a> {
    slots: &'a ParseSlots,
    weight: usize,
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.slots.state.lock().unwrap_or_else(|e| e.into_inner());
        state.available += self.weight;
        drop(state);
        self.slots.changed.notify_all();
    }
}

impl ParseSlots {
    pub fn new(total: usize) -> Self {
        Self::with_unit(total, SLOT_BYTES)
    }

    /// `total` slots of `unit` bytes each.
    pub fn with_unit(total: usize, unit: u64) -> Self {
        let total = total.max(1);
        Self {
            total,
            unit: unit.max(1),
            state: Mutex::new(Slots { available: total }),
            changed: Condvar::new(),
        }
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn weight(&self, bytes: u64) -> usize {
        (bytes.div_ceil(self.unit).max(1) as usize).min(self.total)
    }

    /// Wait for `weight` slots; `None` once `cancelled` is set while waiting.
    pub fn acquire(&self, weight: usize, cancelled: &AtomicBool) -> Option<SlotGuard<'_>> {
        let weight = weight.clamp(1, self.total);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let admitted = loop {
            let ready = state.available >= weight;
            if ready {
                state.available -= weight;
                break true;
            }
            if cancelled.load(Ordering::Relaxed) {
                break false;
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        };
        drop(state);
        self.changed.notify_all();
        // `then`, not `then_some`: a guard built eagerly would release on drop.
        admitted.then(|| SlotGuard {
            slots: self,
            weight,
        })
    }
}
