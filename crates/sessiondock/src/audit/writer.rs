//! Dedicated writer thread: daily JSONL files with fourteen-day retention.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
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
const RETENTION_DAYS: u64 = 14;
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
    shared: Arc<Shared>,
    shutdown: CancellationToken,
    segment: Option<Segment>,
    dirty: bool,
    last_write: Instant,
    last_error_log: Option<Instant>,
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
        }
    }

    pub fn run(mut self, rx: Receiver<Batch>) {
        loop {
            if self.stopping() {
                break;
            }
            match rx.recv_timeout(TICK) {
                Ok(batch) => self.handle(batch, None),
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
                Ok(batch) if Instant::now() < deadline => self.handle(batch, Some(deadline)),
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

    fn wait_gate(&self, mut deadline: Option<Instant>) -> bool {
        let Some(gate) = &self.shared.limits.gate else {
            return true;
        };
        loop {
            if gate.wait(Duration::from_millis(50)) {
                return true;
            }
            if deadline.is_none() && self.stopping() {
                deadline = Some(Instant::now() + DRAIN_DEADLINE);
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return false;
            }
        }
    }

    fn handle(&mut self, batch: Batch, deadline: Option<Instant>) {
        if !self.wait_gate(deadline) {
            self.discard(&batch);
            return;
        }
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
            eprintln!("audit writer: diagnostics dropped: {error}");
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

    /// Closes the current daily segment, opens today's file and applies
    /// Python's fourteen-day retention window.
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
            path,
            date: date.to_owned(),
            bytes: metadata.len(),
        });
        self.enforce_retention();
        Ok(())
    }

    /// Delete matching segments older than Python's retention window.
    fn enforce_retention(&mut self) {
        let mut segments = list_segments(&self.directory);
        segments.sort();
        let mut total: u64 = segments.iter().map(|segment| segment.size).sum();
        let active = self.segment.as_ref().map(|segment| segment.path.clone());
        let cutoff = utc_date(
            SystemTime::now()
                .checked_sub(Duration::from_secs(RETENTION_DAYS * 24 * 3600))
                .unwrap_or(SystemTime::UNIX_EPOCH),
        );
        for segment in &segments {
            if segment.date >= cutoff || active.as_ref() == Some(&segment.path) {
                continue;
            }
            match fs::remove_file(&segment.path) {
                Ok(()) => total -= segment.size,
                Err(error) => {
                    self.shared
                        .counters
                        .write_errors
                        .fetch_add(1, Ordering::Relaxed);
                    self.log_error(&error);
                }
            }
        }
        self.shared
            .counters
            .retained_bytes
            .store(total, Ordering::Relaxed);
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
