//! Process-owned metadata logger. No dependency on a parent pipe or async runtime.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, BufRead, Read, Write},
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const LIMIT: usize = 8 * 1024 * 1024;
const MAX_RECORD: usize = 16 * 1024;
const DAY: u64 = 86_400_000;
#[derive(Default)]
struct State {
    bytes: AtomicUsize,
    dropped: AtomicU64,
    errors: AtomicU64,
    cleanup_errors: AtomicU64,
    cleanup_scanned_bytes: AtomicU64,
    cleanup_ms: AtomicU64,
    written: AtomicU64,
    error_events: AtomicU64,
    warning_events: AtomicU64,
    coalesced: AtomicU64,
    stop: AtomicBool,
    done: AtomicBool,
}
struct Logger {
    tx: mpsc::SyncSender<Vec<u8>>,
    state: Arc<State>,
    layer: String,
    identity: Value,
}
static LOGGER: OnceLock<Logger> = OnceLock::new();
static TEMP_SERIAL: AtomicU64 = AtomicU64::new(0);
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn timestamp() -> String {
    let ms = now();
    let seconds = ms / 1000;
    let days = (seconds / 86400) as i64 + 719468;
    let era = days / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
        ms % 1000
    )
}
fn snapshot_state(s: &State) -> Value {
    json!({"enabled":true,"queued_bytes":s.bytes.load(Ordering::Relaxed),
    "dropped":s.dropped.load(Ordering::Relaxed),"write_errors":s.errors.load(Ordering::Relaxed),
    "cleanup_errors":s.cleanup_errors.load(Ordering::Relaxed),
    "cleanup_scanned_bytes":s.cleanup_scanned_bytes.load(Ordering::Relaxed),"cleanup_ms":s.cleanup_ms.load(Ordering::Relaxed),"written":s.written.load(Ordering::Relaxed),
    "error_events":s.error_events.load(Ordering::Relaxed),"warning_events":s.warning_events.load(Ordering::Relaxed),
    "coalesced":s.coalesced.load(Ordering::Relaxed)})
}
pub fn snapshot() -> Value {
    LOGGER
        .get()
        .map_or(json!({"enabled":false}), |l| snapshot_state(&l.state))
}
/// None explicitly means caller-owned stderr; its retention is outside this module.
pub fn init(directory: Option<PathBuf>, layer: &str, identity: Value) {
    LOGGER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(1024);
        let state = Arc::new(State::default());
        let worker = state.clone();
        let layer_owned = layer.to_owned();
        let identity_owned = identity.clone();
        let result = std::thread::Builder::new()
            .name("runtime-log".into())
            .spawn(move || {
                let mut sink = Sink {
                    directory,
                    scope: if layer_owned == "ptyhost" {
                        Scope::Own
                    } else {
                        Scope::All
                    },
                    cache: BTreeMap::new(),
                };
                let mut host_sink = if layer_owned == "server" {
                    std::env::var_os("SESSIONDOCK_PTYHOST_DIR").map(|p| Sink {
                        directory: Some(PathBuf::from(p).join("runtime-logs")),
                        scope: Scope::Exited,
                        cache: BTreeMap::new(),
                    })
                } else {
                    None
                };
                let mut buckets = BTreeMap::<String, Bucket>::new();
                let mut maintenance = Instant::now() - Duration::from_secs(60);
                let mut health = Instant::now();
                let mut reported = (0, 0, 0);
                loop {
                    match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(bytes) => {
                            worker.bytes.fetch_sub(bytes.len(), Ordering::Relaxed);
                            let row: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                            // Keep identities in the example, not the key: unique trace IDs cannot defeat rate limiting.
                            let key = ["level", "event", "code", "status", "path", "source"]
                                .map(|k| row[k].to_string())
                                .join("|");
                            if let Some(b) = buckets.get_mut(&key) {
                                b.count += 1;
                                b.last = row;
                                worker.coalesced.fetch_add(1, Ordering::Relaxed);
                            } else if buckets.len() < 128 {
                                sink.write(&bytes, &worker);
                                buckets.insert(
                                    key,
                                    Bucket {
                                        first: row.clone(),
                                        last: row,
                                        count: 0,
                                        since: Instant::now(),
                                    },
                                );
                            } else {
                                worker.dropped.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    let stopping = worker.stop.load(Ordering::Acquire);
                    buckets.retain(|_, b| {
                        if !stopping && b.since.elapsed() < Duration::from_secs(5) {
                            return true;
                        }
                        if b.count > 0 {
                            let mut row = b.last.clone();
                            row["repeat_count"] = json!(b.count);
                            row["first_timestamp_ms"] = b.first["timestamp_ms"].clone();
                            row["first_trace"] = b.first["trace"].clone();
                            row["span_ms"] = json!(b.since.elapsed().as_millis());
                            let mut bytes = row.to_string().into_bytes();
                            bytes.push(b'\n');
                            sink.write(&bytes, &worker);
                        }
                        false
                    });
                    if maintenance.elapsed() >= Duration::from_secs(60) {
                        let cleanup_started = Instant::now();
                        sink.clean(&worker);
                        if let Some(host) = &mut host_sink {
                            host.clean(&worker);
                        }
                        worker.cleanup_ms.store(
                            cleanup_started.elapsed().as_millis() as u64,
                            Ordering::Relaxed,
                        );
                        maintenance = Instant::now();
                    }
                    let totals = (
                        worker.dropped.load(Ordering::Relaxed),
                        worker.errors.load(Ordering::Relaxed),
                        worker.cleanup_errors.load(Ordering::Relaxed),
                    );
                    if (totals != reported && health.elapsed() >= Duration::from_secs(60))
                        || (stopping && worker.bytes.load(Ordering::Relaxed) == 0)
                    {
                        let mut row = snapshot_state(&worker);
                        row["event"] = json!("log.delivery");
                        row["level"] = json!("info");
                        row["timestamp_ms"] = json!(now());
                        row["ts"] = json!(timestamp());
                        row["layer"] = json!(layer_owned);
                        row["pid"] = json!(std::process::id());
                        row["identity"] = identity_owned.clone();
                        row["cumulative"] = json!(true);
                        let mut bytes = row.to_string().into_bytes();
                        bytes.push(b'\n');
                        if sink.write(&bytes, &worker) {
                            reported = totals;
                        }
                        health = Instant::now();
                    }
                    if stopping && worker.bytes.load(Ordering::Relaxed) == 0 {
                        break;
                    }
                }
                worker.done.store(true, Ordering::Release);
            });
        if result.is_err() {
            state.errors.fetch_add(1, Ordering::Relaxed);
            state.done.store(true, Ordering::Release);
        }
        Logger {
            tx,
            state,
            layer: layer.into(),
            identity,
        }
    });
}
struct Bucket {
    first: Value,
    last: Value,
    count: u64,
    since: Instant,
}
pub fn emit(level: &str, event: &str, mut row: Value) {
    let Some(l) = LOGGER.get() else {
        return;
    };
    if level == "error" {
        l.state.error_events.fetch_add(1, Ordering::Relaxed);
    }
    if level == "warn" {
        l.state.warning_events.fetch_add(1, Ordering::Relaxed);
    }
    if !row.is_object() {
        row = json!({"data":row});
    }
    row["ts"] = json!(timestamp());
    row["level"] = json!(level);
    row["event"] = json!(event);
    row["timestamp_ms"] = json!(now());
    row["layer"] = json!(l.layer);
    row["pid"] = json!(std::process::id());
    row["identity"] = l.identity.clone();
    let mut bytes = row.to_string().into_bytes();
    bytes.push(b'\n');
    let length = bytes.len();
    if length > MAX_RECORD
        || l.state.stop.load(Ordering::Acquire)
        || l.state
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |v| {
                v.checked_add(length).filter(|n| *n <= LIMIT)
            })
            .is_err()
    {
        l.state.dropped.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if l.tx.try_send(bytes).is_err() {
        l.state.bytes.fetch_sub(length, Ordering::Relaxed);
        l.state.dropped.fetch_add(1, Ordering::Relaxed);
    }
}
/// Blocking at most one second, never joins a stuck filesystem/stderr worker.
pub fn shutdown() -> bool {
    let Some(l) = LOGGER.get() else {
        return true;
    };
    l.state.stop.store(true, Ordering::Release);
    let start = Instant::now();
    while !l.state.done.load(Ordering::Acquire) && start.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(10));
    }
    l.state.done.load(Ordering::Acquire)
}
// Always retire our own temporary, including early read/flush/stat/rename errors.
// A failed sweep must not poison every subsequent sweep with create_new failures.
struct Temporary<'a> {
    path: PathBuf,
    state: &'a State,
}
impl Drop for Temporary<'_> {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            self.state.cleanup_errors.fetch_add(1, Ordering::Relaxed);
        }
    }
}
enum Scope {
    All,
    Own,
    Exited,
}
fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        pid <= i32::MAX as u32
            && (unsafe { libc::kill(pid as i32, 0) } == 0
                || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}
struct Sink {
    scope: Scope,
    directory: Option<PathBuf>,
    cache: BTreeMap<PathBuf, (u64, SystemTime, u64)>,
}
impl Sink {
    fn write(&mut self, bytes: &[u8], s: &State) -> bool {
        let result = (|| -> io::Result<()> {
            if let Some(dir) = &self.directory {
                fs::create_dir_all(dir)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
                }
                // Reopen each write: rename/unlink cannot strand an active writer on an old fd.
                let path = dir.join(format!(
                    "runtime-{}-{}.jsonl",
                    std::process::id(),
                    now() / 60_000
                ));
                let mut opts = OpenOptions::new();
                opts.create(true).append(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    opts.mode(0o600)
                        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                }
                let mut file = opts.open(path)?;
                if !file.metadata()?.is_file() {
                    return Err(io::Error::other("runtime sink is not a regular file"));
                }
                file.write_all(bytes)
            } else {
                std::io::stderr().lock().write_all(bytes)
            }
        })();
        if result.is_ok() {
            s.written.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            s.errors.fetch_add(1, Ordering::Relaxed);
            false
        }
    }
    fn clean(&mut self, s: &State) {
        let Some(dir) = &self.directory else {
            return;
        };
        let entries = match fs::read_dir(dir) {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(_) => {
                s.cleanup_errors.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        // A persistent coordination inode is never unlinked. Lock contention skips
        // this sweep, never blocks the writer or the independent host lifecycle.
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let lock = match options.open(dir.join(".runtime-maintenance.lock")) {
            Ok(file) => file,
            Err(_) => {
                s.cleanup_errors.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        if !lock.metadata().is_ok_and(|m| m.is_file()) {
            s.cleanup_errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return,
            Err(_) => {
                s.cleanup_errors.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        let cutoff = now().saturating_sub(DAY);
        for entry in entries {
            let result = (|| -> io::Result<()> {
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if let Some(pid) = name.strip_prefix(".runtime-clean-") {
                    if matches!(pid.split('-').count(), 1 | 3)
                        && pid
                            .split('-')
                            .all(|part| !part.is_empty() && part.parse::<u64>().is_ok())
                        && entry.file_type()?.is_file()
                        && entry
                            .metadata()?
                            .modified()?
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis()
                            < cutoff as u128
                    {
                        fs::remove_file(entry.path())?;
                    }
                    return Ok(());
                }
                let Some(middle) = name
                    .strip_prefix("runtime-")
                    .and_then(|s| s.strip_suffix(".jsonl"))
                else {
                    return Ok(());
                };
                let Some((pid, minute)) = middle.split_once('-') else {
                    return Ok(());
                };
                if pid.parse::<u32>().is_err()
                    || minute.parse::<u64>().is_err()
                    || !entry.file_type()?.is_file()
                {
                    return Ok(());
                }
                let owner = pid.parse::<u32>().unwrap();
                if matches!(self.scope, Scope::Own) && owner != std::process::id()
                    || matches!(self.scope, Scope::Exited) && alive(owner)
                {
                    return Ok(());
                }
                if minute.parse::<u64>().unwrap_or(u64::MAX) >= now() / 60_000 {
                    return Ok(());
                }
                let path = entry.path();
                let meta = entry.metadata()?;
                let modified = meta.modified()?;
                if self.cache.get(&path).is_some_and(|&(len, mtime, expiry)| {
                    len == meta.len() && mtime == modified && expiry > now()
                }) {
                    return Ok(());
                }
                let fallback = modified
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                // Streaming copy bounds memory even for corrupt/unterminated large records.
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let (temporary, file) = loop {
                    let temporary = dir.join(format!(
                        ".runtime-clean-{}-{}-{}",
                        std::process::id(),
                        now(),
                        TEMP_SERIAL.fetch_add(1, Ordering::Relaxed)
                    ));
                    match options.open(&temporary) {
                        Ok(file) => break (temporary, file),
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                        Err(error) => return Err(error),
                    }
                };
                let _temporary = Temporary {
                    path: temporary.clone(),
                    state: s,
                };
                let mut out = io::BufWriter::new(file);
                s.cleanup_scanned_bytes
                    .fetch_add(meta.len(), Ordering::Relaxed);
                let work = (|| -> io::Result<(bool, u64)> {
                    let mut reader = io::BufReader::new(fs::File::open(&path)?);
                    let mut line = Vec::new();
                    let mut removed = false;
                    let mut expiry = u64::MAX;
                    let mut continuation = false;
                    loop {
                        line.clear();
                        let read = reader
                            .by_ref()
                            .take(64 * 1024)
                            .read_until(b'\n', &mut line)?;
                        if read == 0 {
                            break;
                        }
                        let mut stamp = serde_json::from_slice::<Value>(&line)
                            .ok()
                            .and_then(|r| r["timestamp_ms"].as_u64())
                            .unwrap_or(fallback);
                        // Oversized/malformed bytes use file mtime; no raw content is diagnosed.
                        if continuation || (line.last() != Some(&b'\n') && read == 64 * 1024) {
                            stamp = fallback;
                        }
                        continuation = line.last() != Some(&b'\n');
                        if stamp < cutoff {
                            removed = true;
                        } else {
                            out.write_all(&line)?;
                            expiry = expiry.min(stamp.saturating_add(DAY));
                        }
                    }
                    Ok((removed, expiry))
                })();
                if work.is_ok() {
                    out.flush()?;
                    out.get_ref()
                        .set_times(fs::FileTimes::new().set_modified(modified))?;
                }
                drop(out);
                match work {
                    Ok((removed, expiry)) => {
                        let mut cacheable = !removed;
                        if removed {
                            // Another host may have appended: defer instead of overwriting concurrent bytes.
                            let current = match fs::metadata(&path) {
                                Ok(meta) => meta,
                                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                                    return Ok(());
                                }
                                Err(error) => return Err(error),
                            };
                            if current.len() == meta.len() && current.modified()? == modified {
                                cacheable = true;
                                if expiry == u64::MAX {
                                    fs::remove_file(&path)?;
                                } else {
                                    fs::rename(&temporary, &path)?;
                                }
                            }
                        }
                        if cacheable && self.cache.len() < 4096 {
                            if let Ok(meta) = fs::metadata(&path) {
                                self.cache
                                    .insert(path, (meta.len(), meta.modified()?, expiry));
                            }
                        }
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            })();
            if result.is_err() {
                s.cleanup_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.cache.retain(|path, _| path.exists());
    }
}

/// Drain external diagnostic bytes without retaining unbounded lines or private text.
#[allow(dead_code)]
pub fn drain_diagnostics(mut reader: impl Read, event: &str, lost: &AtomicU64) {
    let mut buffer = [0u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => {
                let lines = buffer[..length]
                    .iter()
                    .filter(|&&b| b == b'\n')
                    .count()
                    .max(1) as u64;
                let total = lost.fetch_add(lines, Ordering::Relaxed) + lines;
                emit(
                    "warn",
                    event,
                    json!({"bytes":length,"diagnostic_chunks":1,"lost":total}),
                );
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                lost.fetch_add(1, Ordering::Relaxed);
                emit(
                    "warn",
                    event,
                    json!({"read_error":e.kind().to_string(),"lost":lost.load(Ordering::Relaxed)}),
                );
                break;
            }
        }
    }
}
