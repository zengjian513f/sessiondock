//! Byte-boundary receipts around the unchanged raw WebSocket tunnel.
//! Counters include WS framing; no payload, URI or credential is retained.

use super::client::UpstreamStream;
use serde_json::{Value, json};
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio_util::sync::CancellationToken;

pub type Observer = Arc<dyn Fn(Value) + Send + Sync>;

#[derive(Default)]
struct Boundary {
    bytes: u64,
    calls: u64,
    pending_since: Option<Instant>,
    eof: bool,
    error: Option<String>,
}

impl Boundary {
    fn note(&mut self, result: &Poll<io::Result<usize>>, read: bool) {
        match result {
            Poll::Pending => {
                self.pending_since.get_or_insert_with(Instant::now);
            }
            Poll::Ready(Ok(bytes)) => {
                self.pending_since = None;
                self.bytes += *bytes as u64;
                self.calls += 1;
                self.eof |= read && *bytes == 0;
            }
            Poll::Ready(Err(error)) => {
                self.pending_since = None;
                self.error = Some(format!("{:?}", error.kind()));
            }
        }
    }

    fn snapshot(&self) -> Value {
        json!({"bytes": self.bytes, "calls": self.calls, "eof": self.eof,
            "pending_ms": self.pending_since.map(|at| at.elapsed().as_millis() as u64),
            "error_kind": self.error})
    }
}

#[derive(Default)]
struct Counters {
    read: Boundary,
    write: Boundary,
}

struct Metered<T> {
    io: T,
    counters: Arc<Mutex<Counters>>,
}

impl<T: AsyncRead + Unpin> AsyncRead for Metered<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.io).poll_read(cx, buffer);
        let count = match &result {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(())) => Poll::Ready(Ok(buffer.filled().len() - before)),
            Poll::Ready(Err(error)) => Poll::Ready(Err(io::Error::from(error.kind()))),
        };
        self.counters.lock().unwrap().read.note(&count, true);
        result
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Metered<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.io).poll_write(cx, bytes);
        self.counters.lock().unwrap().write.note(&result, false);
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

fn report(
    observer: &Option<Observer>,
    browser: &Mutex<Counters>,
    node: &Mutex<Counters>,
    started: Instant,
    phase: &str,
    reason: &str,
) {
    if let Some(observer) = observer {
        let browser = browser.lock().unwrap();
        let node = node.lock().unwrap();
        observer(json!({"phase": phase, "reason": reason,
            "elapsed_ms": started.elapsed().as_millis() as u64,
            "browser_read": browser.read.snapshot(), "node_written": node.write.snapshot(),
            "node_read": node.read.snapshot(), "browser_written": browser.write.snapshot()}));
    }
}

pub async fn run(
    upgrade: hyper::upgrade::OnUpgrade,
    upstream: UpstreamStream,
    prefetched: Vec<u8>,
    shutdown: CancellationToken,
    observer: Option<Observer>,
) {
    let started = Instant::now();
    let browser_counts = Arc::new(Mutex::new(Counters::default()));
    let node_counts = Arc::new(Mutex::new(Counters::default()));
    node_counts.lock().unwrap().read.bytes = prefetched.len() as u64;
    report(
        &observer,
        &browser_counts,
        &node_counts,
        started,
        "upgrade",
        "opened",
    );
    let upgraded = match upgrade.await {
        Ok(upgraded) => upgraded,
        Err(_) => {
            report(
                &observer,
                &browser_counts,
                &node_counts,
                started,
                "upgrade",
                "failed",
            );
            return;
        }
    };
    let mut browser = Metered {
        io: hyper_util::rt::TokioIo::new(upgraded),
        counters: browser_counts.clone(),
    };
    let mut node = Metered {
        io: upstream,
        counters: node_counts.clone(),
    };
    // Preserve raw copy ordering and backpressure. The monitor never polls a
    // socket itself and cancellation never resets the accumulated evidence.
    let phase = Mutex::new("prefetch");
    let reason = {
        let transfer = async {
            if !prefetched.is_empty() && browser.write_all(&prefetched).await.is_err() {
                return "prefetch_failed";
            }
            *phase.lock().unwrap() = "copy";
            match tokio::io::copy_bidirectional(&mut browser, &mut node).await {
                Ok(_) => "eof",
                Err(_) => "io_error",
            }
        };
        tokio::pin!(transfer);
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break "shutdown",
                reason = &mut transfer => break reason,
                _ = tick.tick() => report(&observer, &browser_counts, &node_counts, started, *phase.lock().unwrap(), "periodic"),
            }
        }
    };
    report(
        &observer,
        &browser_counts,
        &node_counts,
        started,
        *phase.lock().unwrap(),
        reason,
    );
    let _ = browser.shutdown().await;
    let _ = node.shutdown().await;
}
