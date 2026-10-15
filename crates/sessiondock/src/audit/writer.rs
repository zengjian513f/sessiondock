//! Dedicated writer thread: daily JSONL files with rolling 24-hour retention.

use std::{
    fs::{self, File, OpenOptions},
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
    date: String,
    bytes: u64,
}

pub struct Writer {
    directory: PathBuf,
    shared: Arc<Shared>,
    shutdown: CancellationToken,
    segment: Option<Segment>,
    dirty: bool,
    last_write: Instant,
    last_error_log: Option<Instant>,
    last_cleanup: Instant,
}

impl Writer {
    pub fn new(directory: PathBuf, shared: Arc<Shared>, shutdown: CancellationToken) -> Self {
        Self {
            directory,
            shared,
            shutdown,
            segment: None,
            dirty: false,
            last_write: Instant::now(),
            last_error_log: None,
            last_cleanup: Instant::now(),
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
        let result = if rotate { self.rotate(&today) } else { Ok(()) }.and_then(|()| {
            let segment = self.segment.as_mut().expect("rotation opened a segment");
            segment.file.write_all(&batch.bytes)?;
            segment.bytes += length;
            Ok(())
        });
        let counters = &self.shared.counters;
        match result {
            Ok(()) => {
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
        let index = latest_index(&self.directory, date).map_or(0, |(index, _)| index);
        let path = self.directory.join(segment_name(date, index));
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
        for segment in list_segments(&self.directory) {
            if let Err(error) = prune_segment(&segment.path, cutoff) {
                self.shared
                    .counters
                    .write_errors
                    .fetch_add(1, Ordering::Relaxed);
                self.log_error(&error);
            }
        }
        self.shared.counters.retained_bytes.store(
            list_segments(&self.directory).iter().map(|s| s.size).sum(),
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

pub fn segment_name(date: &str, index: u32) -> String {
    if index == 0 {
        format!("{PREFIX}{date}{SUFFIX}")
    } else {
        format!("{PREFIX}{date}.{index:04}{SUFFIX}")
    }
}

/// Parses `browser-YYYY-MM-DD.jsonl` / `browser-YYYY-MM-DD.NNNN.jsonl`.
pub fn parse_segment_name(name: &str) -> Option<(String, u32)> {
    let rest = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
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
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let (date, index) = parse_segment_name(name.to_str()?)?;
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

fn latest_index(directory: &Path, date: &str) -> Option<(u32, u64)> {
    list_segments(directory)
        .into_iter()
        .filter(|segment| segment.date == date)
        .max_by_key(|segment| segment.index)
        .map(|segment| (segment.index, segment.size))
}

/// Filter by the server receipt time, never the untrusted browser clock or
/// filename date. Unknown/malformed records are preserved rather than guessing.
/// Only this writer owns matching files; evidence and business files are ignored.
fn prune_segment(path: &Path, cutoff: SystemTime) -> io::Result<()> {
    let cutoff: chrono::DateTime<chrono::Utc> = cutoff.into();
    // Most scans find nothing expired. Avoid rewriting retained data (and
    // allocating a temporary file) on every idle maintenance tick.
    let expired = |line: &[u8]| {
        serde_json::from_slice::<serde_json::Value>(line)
            .ok()
            .and_then(|row| row.get("received_at")?.as_str().map(str::to_owned))
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
            .is_some_and(|ts| ts < cutoff)
    };
    let mut found = false;
    for line in BufReader::new(File::open(path)?).split(b'\n') {
        if expired(&line?) {
            found = true;
            break;
        }
    }
    if !found {
        return Ok(());
    }
    let temporary = path.with_extension("jsonl.retention-tmp");
    let result = (|| {
        let input = BufReader::new(File::open(path)?);
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = BufWriter::new(options.open(&temporary)?);
        let mut removed = false;
        let mut retained = false;
        for line in input.split(b'\n') {
            let line = line?;
            if line.is_empty() {
                continue;
            }
            if expired(&line) {
                removed = true;
            } else {
                output.write_all(&line)?;
                output.write_all(b"\n")?;
                retained = true;
            }
        }
        output.flush()?;
        if removed && retained {
            output.get_ref().sync_all()?;
        }
        drop(output);
        if removed {
            if retained {
                fs::rename(&temporary, path)?;
            } else {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    })();
    // A failed cleanup leaves the original intact. Never touch unrelated files.
    let _ = fs::remove_file(temporary);
    result
}
