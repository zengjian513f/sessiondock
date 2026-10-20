//! Dedicated writer thread: daily JSONL files with rolling 24-hour retention.

use std::{
    collections::HashMap,
    fs::{self, File, FileTimes, OpenOptions},
    io::{self, BufRead as _, BufReader, BufWriter, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{Receiver, RecvTimeoutError, TryRecvError},
    },
    time::{Duration, Instant, SystemTime},
};

use tokio_util::sync::CancellationToken;

use super::{Batch, Shared, intake::utc_date};

const TICK: Duration = Duration::from_millis(250);
const IDLE_SYNC: Duration = Duration::from_secs(1);
/// Writer-side drain budget once stop is observed; the async `shutdown()`
/// waits slightly longer so a normal drain always completes first.
const DRAIN_DEADLINE: Duration = Duration::from_millis(1500);
const ERROR_LOG_INTERVAL: Duration = Duration::from_secs(60);
const RETENTION: Duration = Duration::from_secs(24 * 3600);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
const PREFIX: &str = "browser-";
const SUFFIX: &str = ".jsonl";

struct Segment {
    file: File,
    path: PathBuf,
    date: String,
    bytes: u64,
}

pub struct Writer {
    directory: PathBuf,
    prefix: &'static str,
    shared: Arc<Shared>,
    shutdown: CancellationToken,
    segment: Option<Segment>,
    dirty: bool,
    last_write: Instant,
    last_error_log: Option<Instant>,
    last_cleanup: Instant,
    retention_cache: HashMap<PathBuf, (u64, SystemTime, SystemTime)>,
}

impl Writer {
    pub fn new(
        directory: PathBuf,
        shared: Arc<Shared>,
        shutdown: CancellationToken,
        prefix: &'static str,
    ) -> Self {
        Self {
            directory,
            prefix,
            shared,
            shutdown,
            segment: None,
            dirty: false,
            last_write: Instant::now(),
            last_error_log: None,
            last_cleanup: Instant::now(),
            retention_cache: HashMap::new(),
        }
    }

    pub fn run(mut self, rx: Receiver<Batch>) {
        self.enforce_retention();
        loop {
            if self.last_cleanup.elapsed() >= CLEANUP_INTERVAL {
                self.enforce_retention();
            }
            if self.stopping() {
                break;
            }
            match rx.recv_timeout(TICK) {
                Ok(batch) => self.handle(batch),
                Err(RecvTimeoutError::Timeout) => self.idle_sync(),
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        self.drain(&rx);
        self.sync();
    }

    fn stopping(&self) -> bool {
        self.shared.stop.load(Ordering::Acquire) || self.shutdown.is_cancelled()
    }

    fn drain(&mut self, rx: &Receiver<Batch>) {
        let deadline = Instant::now() + DRAIN_DEADLINE;
        loop {
            match rx.try_recv() {
                Ok(batch) if Instant::now() < deadline => self.handle(batch),
                Ok(batch) => self.discard(&batch),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    /// Accounts a batch that left the queue without being written.
    fn discard(&mut self, batch: &Batch) {
        self.leave_queue(batch);
        let counters = &self.shared.counters;
        counters.dropped_batches.fetch_add(1, Ordering::Relaxed);
        counters
            .dropped_events
            .fetch_add(u64::from(batch.events), Ordering::Relaxed);
    }

    fn leave_queue(&self, batch: &Batch) {
        self.shared.queued_batches.fetch_sub(1, Ordering::AcqRel);
        self.shared.release(batch.bytes.len());
    }

    fn handle(&mut self, batch: Batch) {
        self.leave_queue(&batch);
        let length = batch.bytes.len() as u64;
        let today = utc_date(SystemTime::now());
        let rotate = self
            .segment
            .as_ref()
            .is_none_or(|segment| segment.date != today);
        let mut appended_from = None;
        let result = if rotate { self.rotate(&today) } else { Ok(()) }.and_then(|()| {
            let segment = self.segment.as_mut().expect("rotation opened a segment");
            let before = segment.file.metadata()?;
            appended_from = Some((before.len(), before.modified()?));
            segment.file.write_all(&batch.bytes)?;
            segment.bytes += length;
            Ok(())
        });
        let counters = &self.shared.counters;
        match result {
            Ok(()) => {
                // Our append cannot introduce old records. Keep the cached deadline
                // while accounting for the new suffix without reparsing the prefix.
                if let Some(segment) = &self.segment {
                    if let Some(cached) = self.retention_cache.get_mut(&segment.path)
                        && let Ok(metadata) = segment.file.metadata()
                        && appended_from == Some((cached.0, cached.1))
                        && cached.0 + length == metadata.len()
                    {
                        cached.0 = metadata.len();
                        cached.1 = metadata.modified().unwrap_or(SystemTime::now());
                        cached.2 = cached.2.min(SystemTime::now() + RETENTION);
                    }
                }
                counters.written_bytes.fetch_add(length, Ordering::Relaxed);
                counters
                    .written_events
                    .fetch_add(u64::from(batch.events), Ordering::Relaxed);
                counters.retained_bytes.fetch_add(length, Ordering::Relaxed);
                self.dirty = true;
                self.last_write = Instant::now();
            }
            Err(error) => {
                counters.write_errors.fetch_add(1, Ordering::Relaxed);
                counters.dropped_batches.fetch_add(1, Ordering::Relaxed);
                counters
                    .dropped_events
                    .fetch_add(u64::from(batch.events), Ordering::Relaxed);
                // Reopen lazily on the next batch instead of retrying in a loop.
                self.segment = None;
                self.log_error(&error);
            }
        }
    }

    fn log_error(&mut self, error: &io::Error) {
        if self
            .last_error_log
            .is_none_or(|last| last.elapsed() >= ERROR_LOG_INTERVAL)
        {
            self.last_error_log = Some(Instant::now());
            crate::log::warn(
                "audit.diagnostics_dropped",
                serde_json::json!({"error": error.to_string()}),
            );
        }
    }

    fn idle_sync(&mut self) {
        if self.dirty && self.last_write.elapsed() >= IDLE_SYNC {
            self.sync();
        }
    }

    fn sync(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if let Some(segment) = &self.segment
            && let Err(error) = segment.file.sync_data()
        {
            self.shared
                .counters
                .write_errors
                .fetch_add(1, Ordering::Relaxed);
            self.log_error(&error);
        }
    }

    /// Closes the current daily segment and opens today's file.
    fn rotate(&mut self, date: &str) -> io::Result<()> {
        self.sync();
        self.segment.take();
        let index = latest_index(&self.directory, date, self.prefix).map_or(0, |(index, _)| index);
        let path = self.directory.join(named_segment(date, index, self.prefix));
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "audit segment is not a regular file",
            ));
        }
        self.segment = Some(Segment {
            file,
            path,
            date: date.to_owned(),
            bytes: metadata.len(),
        });
        Ok(())
    }

    /// Runs on the dedicated writer, including idle periods and startup.
    /// Close our append handle before replacing a mixed-age file (also Windows).
    fn enforce_retention(&mut self) {
        self.last_cleanup = Instant::now();
        self.sync();
        self.segment.take();
        let cutoff = SystemTime::now()
            .checked_sub(RETENTION)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let now = SystemTime::now();
        let segments = list_named_segments(&self.directory, self.prefix);
        self.retention_cache
            .retain(|path, _| segments.iter().any(|s| &s.path == path));
        for segment in segments {
            let result = (|| {
                let metadata = fs::metadata(&segment.path)?;
                let modified = metadata.modified()?;
                if self
                    .retention_cache
                    .get(&segment.path)
                    .is_some_and(|cached| {
                        cached.0 == metadata.len() && cached.1 == modified && now < cached.2
                    })
                {
                    return Ok(());
                }
                self.shared
                    .counters
                    .retention_scanned_bytes
                    .fetch_add(metadata.len(), Ordering::Relaxed);
                let next = prune_segment(&segment.path, cutoff)?;
                if let Ok(metadata) = fs::metadata(&segment.path) {
                    self.retention_cache.insert(
                        segment.path.clone(),
                        (metadata.len(), metadata.modified()?, next),
                    );
                }
                Ok::<_, io::Error>(())
            })();
            if let Err(error) = result {
                self.shared
                    .counters
                    .write_errors
                    .fetch_add(1, Ordering::Relaxed);
                self.log_error(&error);
            }
        }
        // Only our exact crash-temporary names, never saved evidence or a
        // directory/symlink. Each failure is isolated from the other segments.
        if let Ok(entries) = fs::read_dir(&self.directory) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(base) = name.to_str().and_then(|s| s.strip_suffix(".retention-tmp"))
                else {
                    continue;
                };
                if parse_named_segment(base, self.prefix).is_none() {
                    continue;
                }
                if let Ok(meta) = fs::symlink_metadata(entry.path())
                    && meta.is_file()
                    && meta.modified().is_ok_and(|t| t < cutoff)
                    && let Err(error) = fs::remove_file(entry.path())
                {
                    self.shared
                        .counters
                        .write_errors
                        .fetch_add(1, Ordering::Relaxed);
                    self.log_error(&error);
                }
            }
        }
        self.shared.counters.retained_bytes.store(
            list_named_segments(&self.directory, self.prefix)
                .iter()
                .map(|s| s.size)
                .sum(),
            Ordering::Relaxed,
        );
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SegmentFile {
    pub date: String,
    pub index: u32,
    pub path: PathBuf,
    pub size: u64,
}

fn named_segment(date: &str, index: u32, prefix: &str) -> String {
    if index == 0 {
        format!("{prefix}{date}{SUFFIX}")
    } else {
        format!("{prefix}{date}.{index:04}{SUFFIX}")
    }
}

/// Parses `browser-YYYY-MM-DD.jsonl` / `browser-YYYY-MM-DD.NNNN.jsonl`.
fn parse_named_segment(name: &str, prefix: &str) -> Option<(String, u32)> {
    let rest = name.strip_prefix(prefix)?.strip_suffix(SUFFIX)?;
    let (date, index) = match rest.split_once('.') {
        None => (rest, 0),
        Some((date, index)) => {
            if index.len() != 4 || !index.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            (date, index.parse().ok()?)
        }
    };
    let bytes = date.as_bytes();
    let valid = bytes.len() == 10
        && bytes
            .iter()
            .enumerate()
            .all(|(position, byte)| match position {
                4 | 7 => *byte == b'-',
                _ => byte.is_ascii_digit(),
            });
    valid.then(|| (date.to_owned(), index))
}

/// Regular files matching the segment pattern; anything else in the directory
/// is ignored and never deleted.
pub fn list_segments(directory: &Path) -> Vec<SegmentFile> {
    list_named_segments(directory, PREFIX)
}

fn list_named_segments(directory: &Path, prefix: &str) -> Vec<SegmentFile> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let (date, index) = parse_named_segment(name.to_str()?, prefix)?;
            let metadata = fs::symlink_metadata(entry.path()).ok()?;
            metadata.is_file().then(|| SegmentFile {
                date,
                index,
                path: entry.path(),
                size: metadata.len(),
            })
        })
        .collect()
}

fn latest_index(directory: &Path, date: &str, prefix: &str) -> Option<(u32, u64)> {
    list_named_segments(directory, prefix)
        .into_iter()
        .filter(|segment| segment.date == date)
        .max_by_key(|segment| segment.index)
        .map(|segment| (segment.index, segment.size))
}

/// Receipt time governs readable records. Unattributed bytes in an owned
/// segment expire 24h after its last modification; recent readable rows survive.
/// Cache the earliest expiry so unchanged recent files need only metadata checks.
fn prune_segment(path: &Path, cutoff: SystemTime) -> io::Result<SystemTime> {
    let modified = fs::metadata(path)?.modified()?;
    let timestamp = |line: &[u8]| -> SystemTime {
        serde_json::from_slice::<serde_json::Value>(line)
            .ok()
            .and_then(|row| {
                row.get("received_at")
                    .or_else(|| row.get("ts"))?
                    .as_str()
                    .map(str::to_owned)
            })
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
            .map(SystemTime::from)
            .unwrap_or(modified)
    };
    let temporary = path.with_extension("jsonl.retention-tmp");
    let result = (|| {
        let mut next = SystemTime::now() + RETENTION;
        let mut output: Option<BufWriter<File>> = None;
        let mut prefix_bytes = 0u64;
        let mut retained = false;
        let mut reader = BufReader::new(File::open(path)?);
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line)?;
            if read == 0 {
                break;
            }
            let trimmed = line.strip_suffix(b"\n").unwrap_or(&line);
            let at = timestamp(trimmed);
            if at < cutoff {
                if output.is_none() {
                    let mut options = OpenOptions::new();
                    options.write(true).create(true).truncate(true);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.mode(0o600);
                    }
                    let mut writer = BufWriter::new(options.open(&temporary)?);
                    if prefix_bytes > 0 {
                        std::io::copy(
                            &mut std::io::Read::take(File::open(path)?, prefix_bytes),
                            &mut writer,
                        )?;
                    }
                    output = Some(writer);
                }
            } else {
                next = next.min(at + RETENTION);
                retained = true;
                if let Some(writer) = &mut output {
                    writer.write_all(trimmed)?;
                    writer.write_all(b"\n")?;
                } else {
                    prefix_bytes += read as u64;
                }
            }
        }
        let Some(mut output) = output else {
            return Ok(next);
        };
        output.flush()?;
        if retained {
            // Rewriting must not renew the age of malformed retained bytes.
            output
                .get_ref()
                .set_times(FileTimes::new().set_modified(modified))?;
            output.get_ref().sync_all()?;
        }
        drop(output);
        if retained {
            fs::rename(&temporary, path)?;
        } else {
            fs::remove_file(path)?;
        }
        Ok(next)
    })();
    let _ = fs::remove_file(temporary);
    result
}
