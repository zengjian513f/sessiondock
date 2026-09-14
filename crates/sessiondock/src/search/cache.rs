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

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};

use serde_json::{Value, json};

use super::SearchError;
use crate::sessions::SearchVersion;

/// Bump when `search::body`'s text or the header format changes; the build
/// fingerprint already covers parser changes between binaries.
pub const SCHEMA: u32 = 1;
/// Memory-only cap when no directory is configured.
pub const MEMORY_ONLY_BYTES: u64 = 64 * 1024 * 1024;
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
    Text(String),
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
    Memory(String),
}

impl TextReader {
    pub fn memory(text: String) -> Self {
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
    pub fn read_all(self) -> io::Result<String> {
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
                String::from_utf8(data).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "cache entry is not UTF-8")
                })
            }
        }
    }

    /// Feed the body to `f` as line-aligned chunks: every chunk but the last
    /// ends with `\n`, the last is flagged. `buffer` is the caller's
    /// reusable read buffer. `f` returns whether to continue.
    pub fn for_each_chunk(
        self,
        buffer: &mut Vec<u8>,
        mut f: impl FnMut(&str, bool) -> Result<bool, SearchError>,
    ) -> Result<(), SearchError> {
        let (mut file, mut remaining) = match self.source {
            ReaderSource::Memory(text) => {
                f(&text, true)?;
                return Ok(());
            }
            ReaderSource::File { file, remaining } => (file, remaining),
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
                    .map_err(|error| SearchError::new(503, "session_error", error.to_string()))?
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
            filled += read;
            remaining -= read as u64;
            if remaining == 0 {
                let chunk = std::str::from_utf8(&buffer[..filled]).map_err(invalid)?;
                f(chunk, true)?;
                return Ok(());
            }
            let Some(newline) = buffer[..filled].iter().rposition(|byte| *byte == b'\n') else {
                // A line longer than the buffer: keep reading it.
                continue;
            };
            let chunk = std::str::from_utf8(&buffer[..=newline]).map_err(invalid)?;
            if !f(chunk, false)? {
                return Ok(());
            }
            buffer.copy_within(newline + 1..filled, 0);
            filled -= newline + 1;
        }
    }
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

pub struct SearchCache {
    dir: Option<PathBuf>,
    limit: u64,
    fingerprint: String,
    state: Mutex<State>,
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
    if !matches!(source, "claude" | "codex" | "grok")
        || hex.len() != 16
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
    /// foreign files are ignored.
    pub fn open(dir: Option<PathBuf>, limit: u64) -> io::Result<Self> {
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
            fingerprint,
            state: Mutex::new(state),
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
        CacheStats {
            entries: state.entries.len(),
            bytes: state.total,
            limit: self.limit,
            persistent: self.dir.is_some(),
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
            if let Some(path) = self.path(&uid) {
                let _ = fs::remove_file(path);
            }
        }
    }

    /// Forget one session (its file is removed).
    pub fn remove(&self, uid: &str) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = state.entries.remove(uid) {
                state.total -= entry.bytes;
            }
        }
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
            fingerprint: build_fingerprint(),
            state: Mutex::new(State {
                entries: HashMap::new(),
                total: 0,
                clock: 0,
            }),
            inflight: Mutex::new(HashSet::new()),
            inflight_free: Condvar::new(),
        }
    }
}

/// Who is asking for a parse slot: a search request, or the warm-up that
/// yields whenever a search is waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Foreground,
    Background,
}

struct Slots {
    available: usize,
    foreground_waiting: usize,
}

/// The parse budget: `total` slots shared by every search and the warm-up.
/// A parse of `bytes` takes `ceil(bytes / SLOT_BYTES)` slots (capped at
/// `total`), so cold parses in flight stay bounded in bytes, not only in
/// count. Background acquisition waits while any foreground request waits.
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
            state: Mutex::new(Slots {
                available: total,
                foreground_waiting: 0,
            }),
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
    pub fn acquire(
        &self,
        weight: usize,
        priority: Priority,
        cancelled: &AtomicBool,
    ) -> Option<SlotGuard<'_>> {
        let weight = weight.clamp(1, self.total);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if priority == Priority::Foreground {
            state.foreground_waiting += 1;
        }
        let admitted = loop {
            let ready = state.available >= weight
                && (priority == Priority::Foreground || state.foreground_waiting == 0);
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
        if priority == Priority::Foreground {
            state.foreground_waiting -= 1;
        }
        drop(state);
        self.changed.notify_all();
        // `then`, not `then_some`: a guard built eagerly would release on drop.
        admitted.then(|| SlotGuard {
            slots: self,
            weight,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tempdir() follows the umask; the cache wants an owner-only directory.
    fn private_dir(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        }
        #[cfg(not(unix))]
        fs::create_dir(&dir).unwrap();
        dir
    }

    fn text_of(lookup: Lookup) -> Option<String> {
        match lookup {
            Lookup::Hit(Hit::Text(reader)) => Some(reader.read_all().unwrap()),
            _ => None,
        }
    }

    fn error_of(lookup: Lookup) -> Option<(u16, String)> {
        match lookup {
            Lookup::Hit(Hit::Error { status, message }) => Some((status, message)),
            _ => None,
        }
    }

    fn version(size: u64, mtime: &str) -> SearchVersion {
        SearchVersion {
            key: json!({"source": "claude", "path": "/p/a.jsonl",
                "data": [size, mtime, "1:2:3:4"], "pin": null, "chain": []}),
            data: Some(("1:2".to_owned(), size)),
        }
    }

    #[test]
    fn cache_directories_are_created_and_follow_ordinary_aliases() {
        let dir = tempfile::tempdir().unwrap();
        assert!(SearchCache::open(Some(dir.path().join("absent")), 1 << 20).is_ok());
        let private = private_dir(dir.path(), "private");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let wide = dir.path().join("wide");
            fs::create_dir(&wide).unwrap();
            fs::set_permissions(&wide, fs::Permissions::from_mode(0o750)).unwrap();
            assert!(SearchCache::open(Some(wide), 1 << 20).is_ok());
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&private, &link).unwrap();
            assert!(SearchCache::open(Some(link), 1 << 20).is_ok());
        }
        assert!(SearchCache::open(Some(private), 1 << 20).is_ok());
    }

    #[test]
    fn disk_entries_round_trip_and_detect_versions() {
        let temp = tempfile::tempdir().unwrap();
        let dir = private_dir(temp.path(), "cache");
        let cache = SearchCache::open(Some(dir.clone()), 1024 * 1024).unwrap();
        let uid = "claude:0123456789abcdef";
        assert!(matches!(cache.get(uid, &version(10, "1")), Lookup::Miss));
        cache.put(uid, &version(10, "1"), &Cached::Text("hello 猫".into()));
        assert_eq!(
            text_of(cache.get(uid, &version(10, "1"))).as_deref(),
            Some("hello 猫")
        );
        // Any other version of the same session is a miss.
        assert!(matches!(cache.get(uid, &version(20, "2")), Lookup::Miss));
        assert!(matches!(cache.get(uid, &version(5, "2")), Lookup::Miss));
        cache.put(
            uid,
            &version(20, "2"),
            &Cached::Error {
                status: 501,
                message: "坏".into(),
            },
        );
        assert_eq!(
            error_of(cache.get(uid, &version(20, "2"))),
            Some((501, "坏".to_owned()))
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file = dir.join("claude-0123456789abcdef");
            assert_eq!(
                fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // A restart takes stock of what is on disk.
        let reopened = SearchCache::open(Some(dir), 1024 * 1024).unwrap();
        assert_eq!(reopened.stats().entries, 1);
        assert!(matches!(
            reopened.get(uid, &version(20, "2")),
            Lookup::Hit(_)
        ));
    }

    #[test]
    fn eviction_is_least_recently_used_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let cache = SearchCache::open(Some(private_dir(temp.path(), "cache")), 3 * 1024).unwrap();
        let text = Cached::Text("x".repeat(800));
        let uids = [
            "claude:0000000000000001",
            "claude:0000000000000002",
            "claude:0000000000000003",
        ];
        for uid in uids {
            cache.put(uid, &version(1, "1"), &text);
        }
        assert_eq!(cache.stats().entries, 2, "third entry evicts the oldest");
        assert!(matches!(cache.get(uids[0], &version(1, "1")), Lookup::Miss));
        // Touching the second makes the third the victim.
        assert!(matches!(
            cache.get(uids[1], &version(1, "1")),
            Lookup::Hit(_)
        ));
        cache.put("codex:00000000000000aa", &version(1, "1"), &text);
        assert!(matches!(
            cache.get(uids[1], &version(1, "1")),
            Lookup::Hit(_)
        ));
        assert!(matches!(cache.get(uids[2], &version(1, "1")), Lookup::Miss));
        assert!(cache.stats().bytes <= 3 * 1024);
        // Oversized payloads are never stored.
        cache.put(
            "grok:00000000000000bb",
            &version(1, "1"),
            &Cached::Text("y".repeat(4000)),
        );
        assert!(matches!(
            cache.get("grok:00000000000000bb", &version(1, "1")),
            Lookup::Miss
        ));
    }

    #[test]
    fn memory_only_mode_and_foreign_files() {
        let cache = SearchCache::open(None, 1 << 40).unwrap();
        assert_eq!(cache.stats().limit, MEMORY_ONLY_BYTES);
        assert!(!cache.persistent());
        cache.put(
            "grok:0000000000000001",
            &version(1, "1"),
            &Cached::Text("t".into()),
        );
        assert!(matches!(
            cache.get("grok:0000000000000001", &version(1, "1")),
            Lookup::Hit(_)
        ));
        assert!(matches!(
            cache.get("grok:0000000000000001", &version(2, "1")),
            Lookup::Miss
        ));
        let temp = tempfile::tempdir().unwrap();
        let dir = private_dir(temp.path(), "cache");
        fs::write(dir.join("README"), b"not an entry").unwrap();
        fs::write(dir.join(".search-tmp-1-x"), b"leftover").unwrap();
        fs::write(dir.join("claude-zz"), b"bad name").unwrap();
        let cache = SearchCache::open(Some(dir.clone()), 1 << 20).unwrap();
        assert_eq!(cache.stats().entries, 0);
        assert!(!dir.join(".search-tmp-1-x").exists());
        assert!(dir.join("README").exists());
        assert!(uid_file_name("claude:../x").is_none());
        assert!(uid_file_name("other:0123456789abcdef").is_none());
    }

    #[test]
    fn chunked_reads_are_line_aligned_and_complete() {
        let temp = tempfile::tempdir().unwrap();
        let dir = private_dir(temp.path(), "cache");
        let cache = SearchCache::open(Some(dir.clone()), 1 << 30).unwrap();
        let uid = "codex:0123456789abcdef";
        let long_line = "长".repeat(CHUNK_BYTES / 2);
        let text = format!("a\nbb\n{long_line}\n\nlast 猫");
        cache.put(uid, &version(1, "1"), &Cached::Text(text.clone()));
        let Lookup::Hit(Hit::Text(reader)) = cache.get(uid, &version(1, "1")) else {
            panic!("hit expected");
        };
        assert_eq!(reader.len(), text.len() as u64);
        let mut buffer = Vec::new();
        let mut chunks = Vec::new();
        reader
            .for_each_chunk(&mut buffer, |chunk, last| {
                chunks.push((chunk.to_owned(), last));
                Ok(true)
            })
            .unwrap();
        assert_eq!(
            chunks.iter().map(|(c, _)| c.as_str()).collect::<String>(),
            text
        );
        let (last, flags): (Vec<_>, Vec<_>) = chunks.iter().map(|(c, l)| (c, *l)).unzip();
        assert!(last[..last.len() - 1].iter().all(|c| c.ends_with('\n')));
        assert_eq!(flags.iter().filter(|l| **l).count(), 1);
        assert!(flags.last().copied().unwrap());
        // Stopping early is honoured; a truncated entry is an error, not a body.
        let Lookup::Hit(Hit::Text(reader)) = cache.get(uid, &version(1, "1")) else {
            panic!("hit expected");
        };
        let mut seen = 0;
        reader
            .for_each_chunk(&mut buffer, |_, _| {
                seen += 1;
                Ok(false)
            })
            .unwrap();
        assert_eq!(seen, 1);
        let path = dir.join("codex-0123456789abcdef");
        let mut bytes = fs::read(&path).unwrap();
        bytes.truncate(bytes.len() - 3);
        fs::write(&path, bytes).unwrap();
        assert!(matches!(cache.get(uid, &version(1, "1")), Lookup::Miss));
    }

    #[test]
    fn parse_slots_weight_and_background_priority() {
        let slots = ParseSlots::new(4);
        assert_eq!(slots.weight(0), 1);
        assert_eq!(slots.weight(SLOT_BYTES), 1);
        assert_eq!(slots.weight(SLOT_BYTES + 1), 2);
        assert_eq!(slots.weight(100 * SLOT_BYTES), 4);
        let never = AtomicBool::new(false);
        let now = AtomicBool::new(true);
        let big = slots.acquire(3, Priority::Background, &never).unwrap();
        assert!(
            slots.acquire(2, Priority::Foreground, &now).is_none(),
            "cancelled while waiting"
        );
        assert_eq!(
            slots.state.lock().unwrap().available,
            1,
            "a refused acquisition releases nothing"
        );
        let small = slots.acquire(1, Priority::Foreground, &never).unwrap();
        drop(small);
        drop(big);
        let all = slots.acquire(9, Priority::Foreground, &never).unwrap();
        assert_eq!(all.weight, 4);
        drop(all);
        // The background yields to a waiting foreground request.
        let held = slots.acquire(4, Priority::Background, &never).unwrap();
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        std::thread::scope(|scope| {
            let (flag, slots) = (&cancelled, &slots);
            let waiter =
                scope.spawn(move || slots.acquire(1, Priority::Foreground, flag).is_some());
            std::thread::sleep(Duration::from_millis(50));
            assert!(slots.acquire(1, Priority::Background, &now).is_none());
            drop(held);
            assert!(waiter.join().unwrap());
        });
    }

    #[test]
    fn inflight_guard_serializes_one_uid() {
        let cache = SearchCache::default();
        let first = cache.inflight("claude:0000000000000001");
        let other = cache.inflight("claude:0000000000000002");
        drop(other);
        let started = std::time::Instant::now();
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                let _second = cache.inflight("claude:0000000000000001");
                started.elapsed()
            });
            std::thread::sleep(Duration::from_millis(40));
            drop(first);
            assert!(waiter.join().unwrap() >= Duration::from_millis(30));
        });
    }
}
