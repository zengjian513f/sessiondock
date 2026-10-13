//! Per-connection terminal I/O receipts for the diagnostic audit.
//!
//! A browser that typed without seeing an echo can then be told apart from a
//! frame that never reached this server, a slow host write, and a CLI that
//! produced no output. Only counts and timings leave this module; terminal
//! bytes never do. The observer must not block the bridge.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One aggregation window starts at its first frame and closes this long after.
pub const WINDOW: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoDirection {
    /// Browser frames written to the host (keys, pastes, resizes).
    Input,
    /// Host output frames written to the browser socket.
    Output,
}

#[derive(Clone, Debug, PartialEq)]
pub enum IoReceipt {
    /// Cumulative receive/write boundaries, retained across bridge cancellation.
    Progress(serde_json::Value),
    Window {
        direction: IoDirection,
        frames: u64,
        bytes: u64,
        /// First to last frame of the window.
        span_ms: f64,
        /// Slowest single host write (input, including the per-name gate) or
        /// browser socket write (output).
        max_write_ms: f64,
    },
    Closed {
        code: u16,
        reason: String,
    },
}

/// Lives outside the selected I/O futures, so revocation cannot discard the
/// final counters or the operation that was still pending. Never stores bytes.
pub(super) struct Progress {
    started: Instant,
    state: Mutex<ProgressState>,
}

struct ProgressState {
    received_frames: u64,
    received_bytes: u64,
    written_frames: u64,
    written_bytes: u64,
    pings: u64,
    pongs: u64,
    input_stage: &'static str,
    input_since: Instant,
    output_stage: &'static str,
    output_since: Instant,
}

impl Progress {
    pub(super) fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            state: Mutex::new(ProgressState {
                received_frames: 0,
                received_bytes: 0,
                written_frames: 0,
                written_bytes: 0,
                pings: 0,
                pongs: 0,
                input_stage: "attaching_host",
                input_since: now,
                output_stage: "attaching_host",
                output_since: now,
            }),
        }
    }

    pub(super) fn input(&self, stage: &'static str) {
        let mut state = self.state.lock().unwrap();
        state.input_stage = stage;
        state.input_since = Instant::now();
    }

    pub(super) fn output(&self, stage: &'static str) {
        let mut state = self.state.lock().unwrap();
        state.output_stage = stage;
        state.output_since = Instant::now();
    }

    pub(super) fn received(&self, bytes: usize) {
        let mut state = self.state.lock().unwrap();
        state.received_frames += 1;
        state.received_bytes += bytes as u64;
    }

    pub(super) fn written(&self, bytes: usize) {
        let mut state = self.state.lock().unwrap();
        state.written_frames += 1;
        state.written_bytes += bytes as u64;
    }

    pub(super) fn ping(&self) {
        self.state.lock().unwrap().pings += 1;
    }
    pub(super) fn pong(&self) {
        self.state.lock().unwrap().pongs += 1;
    }

    pub(super) fn report(&self, observer: Option<&IoObserver>, reason: &str) {
        let Some(observer) = observer else {
            return;
        };
        let state = self.state.lock().unwrap();
        let data = serde_json::json!({
            "reason": reason, "elapsed_ms": millis(self.started.elapsed()),
            "received_frames": state.received_frames, "received_bytes": state.received_bytes,
            "written_frames": state.written_frames, "written_bytes": state.written_bytes,
            "pings_received": state.pings, "pongs_sent": state.pongs,
            "input_stage": state.input_stage, "input_stage_ms": millis(state.input_since.elapsed()),
            "output_stage": state.output_stage, "output_stage_ms": millis(state.output_since.elapsed()),
        });
        drop(state);
        observer(IoReceipt::Progress(data));
    }

    pub(super) async fn watch(&self, observer: Option<&IoObserver>) {
        // A quiet socket still needs evidence that its receive loop is alive.
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            tick.tick().await;
            self.report(observer, "periodic");
        }
    }
}

pub type IoObserver = Arc<dyn Fn(IoReceipt) + Send + Sync>;

pub(super) struct IoMeter {
    direction: IoDirection,
    first: Option<Instant>,
    last: Option<Instant>,
    frames: u64,
    bytes: u64,
    max_write: Duration,
}

impl IoMeter {
    pub(super) fn new(direction: IoDirection) -> Self {
        Self {
            direction,
            first: None,
            last: None,
            frames: 0,
            bytes: 0,
            max_write: Duration::ZERO,
        }
    }

    /// Count one frame that arrived at `at` and took `write` to hand on.
    pub(super) fn note(&mut self, at: Instant, bytes: usize, write: Duration) {
        self.first.get_or_insert(at);
        self.last = Some(at);
        self.frames += 1;
        self.bytes += bytes as u64;
        self.max_write = self.max_write.max(write);
    }

    /// When the open window must be reported; `None` while nothing is pending.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.first.map(|first| first + WINDOW)
    }

    pub(super) fn flush(&mut self, observer: Option<&IoObserver>) {
        let (Some(first), Some(last)) = (self.first.take(), self.last.take()) else {
            return;
        };
        let receipt = IoReceipt::Window {
            direction: self.direction,
            frames: std::mem::take(&mut self.frames),
            bytes: std::mem::take(&mut self.bytes),
            span_ms: millis(last.duration_since(first)),
            max_write_ms: millis(std::mem::take(&mut self.max_write)),
        };
        if let Some(observer) = observer {
            observer(receipt);
        }
    }
}

fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 10_000.0).round() / 10.0
}
