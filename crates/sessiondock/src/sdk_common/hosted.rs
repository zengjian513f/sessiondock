//! Ordered stdin delivery and independent output/stop handling for hosted stdio SDKs.
use super::{
    outbox::{Entry, Sender},
    redact,
};
use sdkhost_client::{Client, Status, Stream};
use serde_json::Value;
use std::{future::Future, io, pin::Pin, sync::atomic::Ordering, time::Duration};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(crate) type CatchUp = Option<oneshot::Sender<Result<(), String>>>;

pub(crate) trait Projection {
    const ATTACHMENT_BOUNDARY: bool = false;

    fn checkpoint(&self) -> io::Result<()>;
    fn written(&self, frame: &Value) -> io::Result<()>;
    fn write_failed(&self, error: &str);
    fn stdout(&mut self, batch: Stdout<'_>) -> Result<(), String>;
    fn stderr(&self, line: String);
    fn last_diagnostic(&self) -> String;
}

pub(crate) struct Stdout<'a> {
    client: &'a Client,
    bytes: &'a [u8],
    start: u64,
    pub offset: u64,
    pub live_start: u64,
    pub live_end: u64,
    pub eof: bool,
    pub catching_up: bool,
}

pub(crate) enum Observation {
    OffsetOnly,
    Delta,
    Published,
}

pub(crate) struct DeltaBatch {
    pending: bool,
    last_publish: std::time::Instant,
}

impl DeltaBatch {
    pub fn new() -> Self {
        Self {
            pending: false,
            last_publish: std::time::Instant::now(),
        }
    }

    pub fn record(&mut self, observation: Observation) {
        match observation {
            Observation::OffsetOnly => {}
            Observation::Delta => self.pending = true,
            Observation::Published => {
                // The immediate snapshot already includes every preceding delta.
                self.pending = false;
                self.last_publish = std::time::Instant::now();
            }
        }
    }

    pub fn due(&self, batch: &Stdout<'_>) -> bool {
        self.pending
            && (batch.eof
                || batch.offset >= batch.live_end
                || self.last_publish.elapsed() >= Duration::from_millis(100))
    }
}

impl Stdout<'_> {
    pub fn lines(&self) -> impl Iterator<Item = (u64, &[u8])> {
        let mut end = self.start;
        self.bytes
            .split_inclusive(|byte| *byte == b'\n')
            .map(move |line| {
                end += line.len() as u64;
                (end, line)
            })
    }

    pub fn received_at(&self, end: u64) -> Option<String> {
        crate::sdk_message_time::hosted(self.client, end, self.live_start)
    }
}

struct StreamBuffer {
    offset: u64,
    bytes: Vec<u8>,
}

impl StreamBuffer {
    fn new(offset: u64) -> Self {
        Self {
            offset,
            bytes: Vec::new(),
        }
    }

    async fn read(&mut self, client: &Client, stream: Stream, len: u64) -> io::Result<()> {
        // A live-edge read can long-poll for 250 ms. Each nonempty stream
        // gets a chunk per iteration without waiting on its quiet peer.
        if self.offset < len {
            let chunk = client.read(stream, self.offset).await?;
            self.offset = chunk.next_offset;
            self.bytes.extend_from_slice(&chunk.bytes);
        }
        Ok(())
    }

    fn complete(&self, eof: bool) -> &[u8] {
        let end = if eof {
            self.bytes.len()
        } else {
            self.bytes
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |position| position + 1)
        };
        &self.bytes[..end]
    }
}

pub(crate) struct Runtime<'a> {
    pub client: &'a Client,
    pub sender: &'a Sender,
    pub shutdown: &'a CancellationToken,
    pub stop: &'a CancellationToken,
    pub secrets: &'a [String],
}

#[derive(Default)]
pub(crate) struct Termination {
    pub detached: bool,
    pub deliberate: bool,
    pub retired: bool,
}

type PendingWrite<'a> = Pin<Box<dyn Future<Output = (Entry, Result<(), String>)> + Send + 'a>>;

fn host_error(source: &str, error: impl std::fmt::Display) -> String {
    format!("{source}_sdk_host: {error}")
}

pub(crate) fn uncertain_write(source: &str, state: &str) -> String {
    format!("{source}_sdk_write_uncertain: {state}")
}

impl Runtime<'_> {
    async fn write(&self, entry: Entry) -> (Entry, Result<(), String>) {
        let source = self.sender.source;
        let result = async {
            let receipt = match self
                .client
                .receipt(&entry.id)
                .await
                .map_err(|e| host_error(source, e))?
            {
                Some(receipt) => receipt,
                None => {
                    let mut bytes =
                        serde_json::to_vec(&entry.frame).map_err(|e| host_error(source, e))?;
                    bytes.push(b'\n');
                    self.client
                        .write(&entry.id, &bytes)
                        .await
                        .map_err(|e| host_error(source, e))?
                }
            };
            if receipt.state != "written" {
                return Err(uncertain_write(source, &receipt.state));
            }
            Ok(())
        }
        .await;
        (entry, result)
    }

    fn close_input(&self) {
        self.sender.closed.store(true, Ordering::Release);
    }

    async fn status(&self) -> Result<(Status, bool), String> {
        self.client
            .status_with_connection()
            .await
            .map_err(|e| host_error(self.sender.source, e))
    }

    pub async fn run<P: Projection>(
        &self,
        projection: &mut P,
        stdout_offset: u64,
        caught_up: &mut CatchUp,
        end: &mut Termination,
    ) -> Result<(), String> {
        let source = self.sender.source;
        let (mut status, mut online) = self.status().await?;
        let live_start = status.stdout_len;
        let mut stdout = StreamBuffer::new(stdout_offset);
        let mut stderr = StreamBuffer::new(0);
        let initial_entries = self.sender.len();
        let mut sent = 0;
        let mut pending: Option<PendingWrite<'_>> = None;
        let mut write_error = None;

        loop {
            if self.shutdown.is_cancelled() {
                end.detached = true;
                return Ok(());
            }
            if !end.deliberate && self.stop.is_cancelled() {
                end.deliberate = true;
                self.close_input();
                // Dropping the RPC future never retries a partially written
                // frame. The host can still signal the child while its pipe
                // write waits, and retains the original durable receipt.
                pending = None;
                if online
                    && !(status.exit.is_some() && status.stdout_eof && status.stderr_eof)
                    && let Err(error) = self.client.signal(9).await
                {
                    (status, online) = self.status().await?;
                    if !status.stdout_eof || !status.stderr_eof || status.exit.is_none() {
                        return Err(host_error(source, error));
                    }
                }
            }
            if !online || status.exit.is_some() {
                self.close_input();
                pending = None;
            } else if !end.deliberate
                && write_error.is_none()
                && pending.is_none()
                && let Some(entry) = self.sender.entry(sent)
            {
                // Persist the protocol association before the host can write.
                projection.checkpoint().map_err(|e| host_error(source, e))?;
                pending = Some(Box::pin(self.write(entry)));
            }

            // Offline Status requires a recorded native exit. A missing EOF
            // cannot change once its owner is gone. Drain stored complete
            // frames, and only flush an unterminated line at a recorded EOF.
            end.retired = !online;
            stdout
                .read(self.client, Stream::Stdout, status.stdout_len)
                .await
                .map_err(|e| host_error(source, e))?;
            stderr
                .read(self.client, Stream::Stderr, status.stderr_len)
                .await
                .map_err(|e| host_error(source, e))?;
            (status, online) = self.status().await?;
            end.retired |= !online;

            let eof = status.stdout_eof && stdout.offset >= status.stdout_len;
            let lines = stdout.complete(eof);
            let consumed = lines.len();
            projection.stdout(Stdout {
                client: self.client,
                bytes: lines,
                start: stdout.offset - stdout.bytes.len() as u64,
                offset: stdout.offset,
                live_start,
                live_end: status.stdout_len,
                eof,
                catching_up: caught_up.is_some(),
            })?;
            stdout.bytes.drain(..consumed);

            let lines = stderr.complete(status.stderr_eof && stderr.offset >= status.stderr_len);
            let consumed = lines.len();
            for line in lines.split_inclusive(|byte| *byte == b'\n') {
                projection.stderr(redact(
                    String::from_utf8_lossy(line).trim_end().to_owned(),
                    self.secrets,
                ));
            }
            stderr.bytes.drain(..consumed);

            let more = stdout.offset < status.stdout_len || stderr.offset < status.stderr_len;
            if !more && let Some(error) = &status.output_error {
                return Err(host_error(source, error));
            }
            if !more
                && let Some(exit) = &status.exit
                && (!online || (status.stdout_eof && status.stderr_eof))
            {
                end.retired = true;
                if !online && !(status.stdout_eof && status.stderr_eof) {
                    return Err(host_error(
                        source,
                        "sdkhost disconnected before both output streams reached EOF",
                    ));
                }
                if let Some(error) = write_error {
                    return Err(error);
                }
                if !end.deliberate && !exit.success() {
                    return Err(format!(
                        "{source}_sdk_disconnected: native CLI exit code {:?}, signal {:?}; {}",
                        exit.code,
                        exit.signal,
                        projection.last_diagnostic()
                    ));
                }
                return Ok(());
            }
            let caught_stdout = if P::ATTACHMENT_BOUNDARY {
                stdout.offset >= live_start && status.exit.is_none()
            } else {
                stdout.offset >= status.stdout_len && !(status.stdout_eof && status.exit.is_some())
            };
            if online
                && !end.deliberate
                && caught_stdout
                && (sent >= initial_entries || write_error.is_some())
                && let Some(reply) = caught_up.take()
            {
                projection.checkpoint().map_err(|e| host_error(source, e))?;
                let _ = reply.send(write_error.clone().map_or(Ok(()), Err));
            }

            tokio::select! {
                biased;
                _ = self.shutdown.cancelled() => { end.detached = true; return Ok(()); }
                _ = self.stop.cancelled(), if !end.deliberate => {}
                (entry, result) = async { pending.as_mut().unwrap().await }, if pending.is_some() => {
                    pending = None;
                    match result {
                        Ok(()) => {
                            projection.written(&entry.frame).map_err(|e| host_error(source, e))?;
                            sent += 1;
                        }
                        Err(error) => {
                            self.close_input();
                            projection.write_failed(&error);
                            write_error = Some(error);
                        }
                    }
                }
                _ = tokio::task::yield_now(), if more => {}
                _ = self.sender.wake.notified(), if pending.is_none() => {}
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
    }
}
