//! Per-connection terminal I/O receipts for the diagnostic audit.
//!
//! A browser that typed without seeing an echo can then be told apart from a
//! frame that never reached this server, a slow host write, and a CLI that
//! produced no output. Only counts and timings leave this module; terminal
//! bytes never do. The observer must not block the bridge.

use std::sync::Arc;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn a_window_reports_counts_once_and_empty_windows_stay_silent() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let observer: IoObserver = Arc::new(move |receipt| sink.lock().unwrap().push(receipt));
        let mut meter = IoMeter::new(IoDirection::Input);
        assert_eq!(meter.deadline(), None);
        meter.flush(Some(&observer));
        let start = Instant::now();
        meter.note(start, 3, Duration::from_millis(2));
        meter.note(
            start + Duration::from_millis(40),
            1,
            Duration::from_millis(7),
        );
        assert_eq!(meter.deadline(), Some(start + WINDOW));
        meter.flush(Some(&observer));
        meter.flush(Some(&observer));
        assert_eq!(meter.deadline(), None);
        let seen = seen.lock().unwrap();
        assert_eq!(
            *seen,
            vec![IoReceipt::Window {
                direction: IoDirection::Input,
                frames: 2,
                bytes: 4,
                span_ms: 40.0,
                max_write_ms: 7.0,
            }]
        );
    }
}
