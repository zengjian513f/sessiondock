//! Bounded, best-effort browser diagnostics intake (M7).
//!
//! The legacy page posts small batches of browser-side receipts to
//! `POST /api/audit/browser`. This module accepts them only when an explicit
//! private `SESSIONDOCK_AUDIT_DIR` is configured, and it is designed so that
//! diagnostics can never change request-handling semantics:
//!
//! * Every request is admitted before its body is copied: shutdown, per-client
//!   token bucket, an in-flight parse permit and the queue byte budget are all
//!   checked first, using `Content-Length` as the upper bound. A saturated queue
//!   answers `202` with `dropped:true` without reading the body at all.
//! * Only structured metadata survives: the client `content` field (message
//!   text, composer drafts, outbox items) is skipped by the deserializer and
//!   never allocated; request headers are never read; secret-looking keys are
//!   redacted; absolute filesystem paths are replaced by `<path>`; strings,
//!   arrays, objects, depth and the serialized `data` size are bounded.
//! * The writer is a dedicated OS thread fed by a `std::sync::mpsc` channel
//!   bounded by both batch count and total bytes. Producers use `try_send`;
//!   they never block and never wait for disk.
//! * Records are appended as JSONL to `<dir>/browser-YYYY-MM-DD.jsonl`,
//!   rotated by size into `browser-YYYY-MM-DD.NNNN.jsonl`, with a total-bytes
//!   retention cap that deletes the oldest segments first (never the active one
//!   and never files that do not match the segment pattern).
//! * Durability policy: every batch is written with one `write_all`; the file is
//!   `fdatasync`ed when the writer has been idle for one second after writes,
//!   when a segment is rotated out, and during graceful shutdown. Losing the
//!   last second of diagnostics on a power failure is acceptable; blocking the
//!   HTTP path is not.
//! * Graceful shutdown drains what is already queued within a bounded deadline
//!   and counts anything beyond it as dropped.

mod intake;
mod limiter;
pub mod query;
mod writer;

#[cfg(test)]
mod tests;

use std::{
    io,
    net::IpAddr,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};

use serde::Serialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio_util::sync::CancellationToken;

pub use intake::{Batch, Rejection};

/// Operating budgets. The defaults are the production values; tests and fault
/// injection lower them explicitly through `Config::audit_limits`.
#[derive(Clone)]
pub struct Limits {
    /// Route body limit. It matches the Python service's 4 MiB because the
    /// legacy page re-queues and retries any non-2xx batch indefinitely and
    /// its `dom.snapshot` receipts carry up to 40 x 4000 characters each; the
    /// body copy is transient and `content` is never retained.
    pub body_bytes: usize,
    /// Events per request; more is `413` like the Python service.
    pub max_events: usize,
    /// Serialized `data` budget per event after sanitization.
    pub data_bytes: usize,
    /// Queue capacity in batches (channel bound).
    pub queue_batches: usize,
    /// Queue capacity in bytes of prepared JSONL.
    pub queue_bytes: usize,
    /// Concurrent body reads/parses.
    pub in_flight: usize,
    /// Token bucket refill per client IP.
    pub rate_per_second: f64,
    /// Token bucket capacity per client IP.
    pub burst: u32,
    /// Size-based segment rotation threshold.
    pub file_bytes: u64,
    /// Total bytes retained across all segments before the oldest are deleted.
    pub retained_bytes: u64,
    /// How long graceful shutdown waits for the writer to drain.
    pub shutdown_deadline: Duration,
    /// Fault injection: while held, the writer does not write. Tests use it to
    /// saturate the queue deterministically.
    pub gate: Option<Arc<Gate>>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            body_bytes: 4 * 1024 * 1024,
            max_events: 100,
            data_bytes: 8 * 1024,
            queue_batches: 256,
            queue_bytes: 4 * 1024 * 1024,
            in_flight: 4,
            rate_per_second: 10.0,
            burst: 40,
            file_bytes: 8 * 1024 * 1024,
            retained_bytes: 64 * 1024 * 1024,
            shutdown_deadline: Duration::from_secs(2),
            gate: None,
        }
    }
}

impl Limits {
    /// Upper bound of one prepared batch: every event is limited to its data
    /// budget plus the bounded envelope fields.
    pub fn batch_bytes(&self) -> usize {
        self.max_events
            .saturating_mul(self.data_bytes.saturating_add(intake::EVENT_OVERHEAD))
    }
}

/// Writer hold for fault injection. Open by default.
pub struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            open: Mutex::new(true),
            changed: Condvar::new(),
        }
    }
}

impl Gate {
    pub fn hold(&self) {
        *self
            .open
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
    }

    pub fn release(&self) {
        *self
            .open
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.changed.notify_all();
    }

    /// Returns whether the gate is open after waiting at most `timeout`.
    fn wait(&self, timeout: Duration) -> bool {
        let guard = self
            .open
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *guard {
            return true;
        }
        let (guard, _) = self
            .changed
            .wait_timeout(guard, timeout)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard
    }
}

#[derive(Default)]
struct Counters {
    accepted_events: AtomicU64,
    accepted_batches: AtomicU64,
    rejected_events: AtomicU64,
    rejected_requests: AtomicU64,
    rate_limited_requests: AtomicU64,
    dropped_events: AtomicU64,
    dropped_batches: AtomicU64,
    written_events: AtomicU64,
    written_bytes: AtomicU64,
    write_errors: AtomicU64,
    retained_bytes: AtomicU64,
}

/// Counter snapshot published through `/api/health`. Every field is a plain
/// number so tests can assert drops without opening the private directory.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Snapshot {
    pub enabled: bool,
    pub accepted_events: u64,
    pub accepted_batches: u64,
    pub rejected_events: u64,
    pub rejected_requests: u64,
    pub rate_limited_requests: u64,
    pub dropped_events: u64,
    pub dropped_batches: u64,
    pub written_events: u64,
    pub written_bytes: u64,
    pub write_errors: u64,
    pub retained_bytes: u64,
    pub queued_batches: u64,
    pub queued_bytes: u64,
}

struct Shared {
    limits: Limits,
    counters: Counters,
    queued_bytes: AtomicUsize,
    queued_batches: AtomicUsize,
    stop: AtomicBool,
    sequence: AtomicU64,
}

impl Shared {
    fn reserve(&self, bytes: usize) -> bool {
        let mut current = self.queued_bytes.load(Ordering::Acquire);
        loop {
            let next = match current.checked_add(bytes) {
                Some(next) if next <= self.limits.queue_bytes => next,
                _ => return false,
            };
            match self.queued_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    fn release(&self, bytes: usize) {
        self.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
    }

    fn snapshot(&self) -> Snapshot {
        let counters = &self.counters;
        let load = |value: &AtomicU64| value.load(Ordering::Relaxed);
        Snapshot {
            enabled: true,
            accepted_events: load(&counters.accepted_events),
            accepted_batches: load(&counters.accepted_batches),
            rejected_events: load(&counters.rejected_events),
            rejected_requests: load(&counters.rejected_requests),
            rate_limited_requests: load(&counters.rate_limited_requests),
            dropped_events: load(&counters.dropped_events),
            dropped_batches: load(&counters.dropped_batches),
            written_events: load(&counters.written_events),
            written_bytes: load(&counters.written_bytes),
            write_errors: load(&counters.write_errors),
            retained_bytes: load(&counters.retained_bytes),
            queued_batches: self.queued_batches.load(Ordering::Relaxed) as u64,
            queued_bytes: self.queued_bytes.load(Ordering::Relaxed) as u64,
        }
    }
}

/// Why a request was refused before its body was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Closed,
    RateLimited { retry_after: Duration },
    Busy,
}

/// Result of a submitted batch. `dropped` means the queue had no capacity;
/// the HTTP answer is still `202` because diagnostics loss is acceptable and
/// the legacy page must not retry the same batch forever.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub accepted: usize,
    pub skipped: usize,
    pub dropped: bool,
}

/// Admission ticket: holds the in-flight parse permit and the transient byte
/// reservation until the batch is either queued or abandoned.
pub struct Admission {
    shared: Arc<Shared>,
    reserved: Option<usize>,
    _permit: OwnedSemaphorePermit,
}

impl Admission {
    /// True when the queue refused the estimated batch: respond without
    /// reading the body.
    pub fn dropped(&self) -> bool {
        self.reserved.is_none()
    }
}

impl std::fmt::Debug for Admission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Admission")
            .field("reserved", &self.reserved)
            .finish_non_exhaustive()
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        if let Some(bytes) = self.reserved.take() {
            self.shared.release(bytes);
        }
    }
}

pub struct AuditService {
    shared: Arc<Shared>,
    tx: mpsc::SyncSender<Batch>,
    limiter: Mutex<limiter::Limiter>,
    in_flight: Arc<Semaphore>,
    done: watch::Receiver<bool>,
    shutdown: CancellationToken,
}

impl AuditService {
    /// Starts the writer thread for an already validated private directory.
    /// No file is created until the first batch arrives.
    pub fn open(
        directory: PathBuf,
        limits: Limits,
        shutdown: CancellationToken,
    ) -> io::Result<Self> {
        if !directory.is_absolute() || !directory.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "audit requires an explicit existing absolute directory",
            ));
        }
        let limiter = limiter::Limiter::new(limits.burst, limits.rate_per_second);
        let in_flight = Arc::new(Semaphore::new(limits.in_flight.max(1)));
        let (tx, rx) = mpsc::sync_channel(limits.queue_batches.max(1));
        let shared = Arc::new(Shared {
            limits,
            counters: Counters::default(),
            queued_bytes: AtomicUsize::new(0),
            queued_batches: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            sequence: AtomicU64::new(1),
        });
        let (done_tx, done) = watch::channel(false);
        let worker = writer::Writer::new(directory, shared.clone(), shutdown.clone());
        std::thread::Builder::new()
            .name("agenthub-audit".into())
            .spawn(move || {
                worker.run(rx);
                let _ = done_tx.send(true);
            })?;
        Ok(Self {
            shared,
            tx,
            limiter: Mutex::new(limiter),
            in_flight,
            done,
            shutdown,
        })
    }

    pub fn limits(&self) -> &Limits {
        &self.shared.limits
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot()
    }

    /// Counts a request refused by the transport before parsing (for example
    /// a body over the route limit).
    pub fn note_rejected(&self) {
        self.shared
            .counters
            .rejected_requests
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Cheap checks that run before the request body is copied. The byte
    /// reservation uses the declared length (capped by the batch bound) as an
    /// upper estimate and is shrunk to the prepared size on submission.
    pub fn admit(
        &self,
        client: IpAddr,
        content_length: Option<usize>,
    ) -> Result<Admission, Refusal> {
        if self.shutdown.is_cancelled() || self.shared.stop.load(Ordering::Acquire) {
            return Err(Refusal::Closed);
        }
        let now = Instant::now();
        let wait = self
            .limiter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take(client, now);
        if let Some(retry_after) = wait {
            self.shared
                .counters
                .rate_limited_requests
                .fetch_add(1, Ordering::Relaxed);
            return Err(Refusal::RateLimited { retry_after });
        }
        let permit = self
            .in_flight
            .clone()
            .try_acquire_owned()
            .map_err(|_| Refusal::Busy)?;
        let bound = self.shared.limits.batch_bytes();
        let estimate = content_length.map_or(bound, |length| length.min(bound));
        let reserved = if self.shared.queued_batches.load(Ordering::Acquire)
            < self.shared.limits.queue_batches
            && self.shared.reserve(estimate)
        {
            Some(estimate)
        } else {
            self.shared
                .counters
                .dropped_batches
                .fetch_add(1, Ordering::Relaxed);
            None
        };
        Ok(Admission {
            shared: self.shared.clone(),
            reserved,
            _permit: permit,
        })
    }

    /// Validates the body, builds the immutable JSONL snapshot and hands it to
    /// the writer without blocking. Structural errors are reported to the
    /// client; invalid individual events are skipped and counted.
    pub fn submit(
        &self,
        mut admission: Admission,
        client: IpAddr,
        body: &[u8],
    ) -> Result<Outcome, Rejection> {
        let Some(reserved) = admission.reserved else {
            return Ok(Outcome {
                dropped: true,
                ..Outcome::default()
            });
        };
        let limits = &self.shared.limits;
        let prepared = intake::prepare(
            body,
            client,
            limits.max_events,
            limits.data_bytes,
            &self.shared.sequence,
            SystemTime::now(),
        )
        .inspect_err(|_| {
            self.shared
                .counters
                .rejected_requests
                .fetch_add(1, Ordering::Relaxed);
        })?;
        let counters = &self.shared.counters;
        counters
            .rejected_events
            .fetch_add(prepared.skipped as u64, Ordering::Relaxed);
        let Some(batch) = prepared.batch else {
            return Ok(Outcome {
                accepted: 0,
                skipped: prepared.skipped,
                dropped: false,
            });
        };
        let accepted = batch.events as usize;
        let actual = batch.bytes.len();
        // Shrink (or, if the estimate was too small, top up) the reservation
        // to the real snapshot size before the queue takes ownership of it.
        if actual <= reserved {
            self.shared.release(reserved - actual);
        } else if !self.shared.reserve(actual - reserved) {
            self.shared.release(reserved);
            admission.reserved = None;
            counters.dropped_batches.fetch_add(1, Ordering::Relaxed);
            counters
                .dropped_events
                .fetch_add(accepted as u64, Ordering::Relaxed);
            return Ok(Outcome {
                accepted,
                skipped: prepared.skipped,
                dropped: true,
            });
        }
        admission.reserved = Some(actual);
        if self.shutdown.is_cancelled() || self.shared.stop.load(Ordering::Acquire) {
            counters.dropped_batches.fetch_add(1, Ordering::Relaxed);
            counters
                .dropped_events
                .fetch_add(accepted as u64, Ordering::Relaxed);
            return Ok(Outcome {
                accepted,
                skipped: prepared.skipped,
                dropped: true,
            });
        }
        self.shared.queued_batches.fetch_add(1, Ordering::AcqRel);
        match self.tx.try_send(batch) {
            Ok(()) => {
                // The writer releases the bytes once the batch leaves the queue.
                admission.reserved = None;
                counters.accepted_batches.fetch_add(1, Ordering::Relaxed);
                counters
                    .accepted_events
                    .fetch_add(accepted as u64, Ordering::Relaxed);
                Ok(Outcome {
                    accepted,
                    skipped: prepared.skipped,
                    dropped: false,
                })
            }
            Err(_) => {
                self.shared.queued_batches.fetch_sub(1, Ordering::AcqRel);
                counters.dropped_batches.fetch_add(1, Ordering::Relaxed);
                counters
                    .dropped_events
                    .fetch_add(accepted as u64, Ordering::Relaxed);
                Ok(Outcome {
                    accepted,
                    skipped: prepared.skipped,
                    dropped: true,
                })
            }
        }
    }

    /// Stops admission and waits, at most `shutdown_deadline`, for the writer
    /// to drain and fsync. Safe to call repeatedly; returns whether the writer
    /// finished within the deadline.
    pub async fn shutdown(&self) -> bool {
        self.shared.stop.store(true, Ordering::Release);
        let mut done = self.done.clone();
        let deadline = self.shared.limits.shutdown_deadline;
        tokio::time::timeout(deadline, async move {
            loop {
                if *done.borrow_and_update() {
                    return true;
                }
                if done.changed().await.is_err() {
                    return *done.borrow();
                }
            }
        })
        .await
        .unwrap_or(false)
    }
}

impl Drop for AuditService {
    fn drop(&mut self) {
        // The writer notices the flag on its next tick (<= 250 ms), drains what
        // is queued within its own deadline and exits; nothing waits on it.
        self.shared.stop.store(true, Ordering::Release);
    }
}

/// Configuration check for `SESSIONDOCK_AUDIT_DIR`: an existing absolute
/// directory without relative jumps, symlinked ancestors or, on Unix, group and
/// world permissions. Returns the canonical path for overlap comparisons.
pub fn validate_directory(path: &Path) -> io::Result<PathBuf> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "SESSIONDOCK_AUDIT_DIR requires an explicit existing absolute directory without relative jumps",
        )
    };
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(invalid());
    }
    for ancestor in path.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor).map_err(|_| invalid())?;
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        if metadata.file_type().is_symlink() || reparse {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "audit directory and its ancestors must not be symlinks or reparse points",
            ));
        }
        if !metadata.is_dir() {
            return Err(invalid());
        }
        #[cfg(unix)]
        if ancestor == path {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o777 != 0o700 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "audit directory requires private owner-only permissions (0700)",
                ));
            }
        }
    }
    path.canonicalize().map_err(|_| invalid())
}
