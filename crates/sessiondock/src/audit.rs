//! Best-effort browser diagnostics intake.
//!
//! The legacy page posts small batches of browser-side receipts to
//! `POST /api/audit/browser`. When `SESSIONDOCK_AUDIT_DIR` is configured,
//! diagnostics are queued without waiting for disk:
//!
//! * The HTTP route applies Python's 4 MiB body and 100-event limits.
//! * Only structured metadata survives. The client `content` field is parsed
//!   with the request but is not retained. Secret keys are redacted and nested
//!   diagnostic data stops after Python's depth 12.
//! * The writer is a dedicated OS thread fed by a `std::sync::mpsc` channel
//!   using Python's 20,000-item queue bound. Producers use `try_send`.
//! * Records are appended to one JSONL file per UTC day. Matching daily files
//!   older than Python's 14-day retention window are removed.
//! * Durability policy: every batch is written with one `write_all`; the file is
//!   `fdatasync`ed when the writer has been idle for one second after writes,
//!   when a segment is rotated out, and during graceful shutdown. Losing the
//!   last second of diagnostics on a power failure is acceptable; blocking the
//!   HTTP path is not.
//! * Graceful shutdown drains what is already queued within a bounded deadline
//!   and counts anything beyond it as dropped.

mod intake;
pub mod query;
mod writer;

#[cfg(test)]
mod tests;

use std::{
    io,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime},
};

use serde::Serialize;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub use intake::{Batch, Rejection};

/// Python's protocol limits plus shutdown and fault-injection controls.
#[derive(Clone)]
pub struct Limits {
    /// Route body limit. It matches the Python service's 4 MiB because the
    /// legacy page re-queues and retries any non-2xx batch indefinitely and
    /// its `dom.snapshot` receipts carry up to 40 x 4000 characters each; the
    /// body copy is transient and `content` is never retained.
    pub body_bytes: usize,
    /// Events per request; more is `413`.
    pub max_events: usize,
    /// Python `audit.QUEUE_LIMIT`; the Rust channel carries prepared batches.
    pub queue_batches: usize,
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
            queue_batches: 20_000,
            shutdown_deadline: Duration::from_secs(2),
            gate: None,
        }
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
    fn reserve(&self, bytes: usize) {
        self.queued_bytes.fetch_add(bytes, Ordering::AcqRel);
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

/// Admission ticket proving the service was open when body reading began.
pub struct Admission {
    _private: (),
}

impl std::fmt::Debug for Admission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Admission").finish_non_exhaustive()
    }
}

pub struct AuditService {
    shared: Arc<Shared>,
    tx: mpsc::SyncSender<Batch>,
    done: watch::Receiver<bool>,
    shutdown: CancellationToken,
}

impl AuditService {
    /// Creates the directory when needed and starts the writer thread.
    /// No file is created until the first batch arrives.
    pub fn open(
        directory: PathBuf,
        limits: Limits,
        shutdown: CancellationToken,
    ) -> io::Result<Self> {
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700));
        }
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
            .name("sessiondock-audit".into())
            .spawn(move || {
                worker.run(rx);
                let _ = done_tx.send(true);
            })?;
        Ok(Self {
            shared,
            tx,
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

    /// Check shutdown before the request body is copied.
    pub fn admit(&self) -> Result<Admission, Refusal> {
        if self.shutdown.is_cancelled() || self.shared.stop.load(Ordering::Acquire) {
            return Err(Refusal::Closed);
        }
        Ok(Admission { _private: () })
    }

    /// Validates the body, builds the immutable JSONL snapshot and hands it to
    /// the writer without blocking. Structural errors are reported to the
    /// client; invalid individual events are skipped and counted.
    pub fn submit(
        &self,
        _admission: Admission,
        client: IpAddr,
        body: &[u8],
    ) -> Result<Outcome, Rejection> {
        let limits = &self.shared.limits;
        let prepared = intake::prepare(
            body,
            client,
            limits.max_events,
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
        self.shared.reserve(actual);
        match self.tx.try_send(batch) {
            Ok(()) => {
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
                self.shared.release(actual);
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

/// Prepare the configured directory as Python's audit store does.
pub fn validate_directory(path: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    path.canonicalize()
}
