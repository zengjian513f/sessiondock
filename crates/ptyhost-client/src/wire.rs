use std::io;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{Duration, Instant, timeout, timeout_at};

use crate::{Error, HostEvent, Result, TerminalSize};

const HEADER_BYTES: usize = 5;
const FRAME_DATA: u8 = 1;
const FRAME_RESIZE: u8 = 2;
const FRAME_EXIT: u8 = 3;

/// Reads without losing bytes following the JSON newline (often the replay's first frame).
pub(crate) async fn read_json<R: AsyncRead + Unpin>(
    reader: &mut R,
    buffer: &mut Vec<u8>,
    max_line: usize,
) -> Result<Value> {
    loop {
        if let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            if end > max_line {
                return Err(Error::LineTooLarge);
            }
            let value =
                serde_json::from_slice::<Value>(&buffer[..end]).map_err(|_| Error::InvalidReply)?;
            if !value.is_object() {
                return Err(Error::InvalidReply);
            }
            buffer.drain(..=end);
            return Ok(value);
        }
        if buffer.len() > max_line {
            return Err(Error::LineTooLarge);
        }
        let mut chunk = [0_u8; 8192];
        let length = reader.read(&mut chunk).await?;
        if length == 0 {
            return Err(Error::UnexpectedEof);
        }
        buffer.extend_from_slice(&chunk[..length]);
    }
}

pub(crate) async fn send_json<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
    max_line: usize,
) -> Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| Error::InvalidRequest)?;
    if bytes.len() > max_line {
        return Err(Error::LineTooLarge);
    }
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

pub(crate) struct FrameReader<R> {
    reader: R,
    buffer: Vec<u8>,
    max_frame: usize,
    frame_timeout: Option<Duration>,
    deadline: Option<Instant>,
    ended: bool,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub fn new(
        reader: R,
        buffer: Vec<u8>,
        max_frame: usize,
        frame_timeout: Option<Duration>,
    ) -> Self {
        Self {
            reader,
            buffer,
            max_frame,
            frame_timeout,
            deadline: None,
            ended: false,
        }
    }

    /// Idle output has no timeout. An optional partial-frame deadline is preserved
    /// across cancellation. `read` is cancellation-safe; partial data stays in self.
    pub async fn next(&mut self) -> Result<Option<HostEvent>> {
        if self.ended {
            return Ok(None);
        }
        let result = self.next_inner().await;
        if result.is_err() {
            self.ended = true;
        }
        result
    }

    async fn next_inner(&mut self) -> Result<Option<HostEvent>> {
        loop {
            if !self.buffer.is_empty() && self.deadline.is_none() {
                self.deadline = self.frame_timeout.map(|timeout| Instant::now() + timeout);
            }
            if self.buffer.len() >= HEADER_BYTES {
                let kind = self.buffer[0];
                if !matches!(kind, FRAME_DATA | FRAME_EXIT) {
                    return Err(Error::InvalidFrame);
                }
                let length = u32::from_be_bytes(
                    self.buffer[1..HEADER_BYTES]
                        .try_into()
                        .map_err(|_| Error::InvalidFrame)?,
                ) as usize;
                if length > self.max_frame {
                    return Err(Error::FrameTooLarge);
                }
                let total = HEADER_BYTES
                    .checked_add(length)
                    .ok_or(Error::FrameTooLarge)?;
                if self.buffer.len() >= total {
                    let payload = self.buffer[HEADER_BYTES..total].to_vec();
                    self.buffer.drain(..total);
                    self.deadline = None;
                    if kind == FRAME_DATA {
                        return Ok(Some(HostEvent::Data(payload)));
                    }
                    #[derive(serde::Deserialize)]
                    struct Exit {
                        code: i32,
                        #[serde(default)]
                        output_complete: Option<bool>,
                        #[serde(default)]
                        reason: Option<crate::ExitReason>,
                    }
                    let exit: Exit =
                        serde_json::from_slice(&payload).map_err(|_| Error::InvalidFrame)?;
                    if exit.reason.is_some() && exit.output_complete != Some(false) {
                        return Err(Error::InvalidFrame);
                    }
                    self.ended = true;
                    return Ok(Some(HostEvent::Exit {
                        code: exit.code,
                        output_complete: exit.output_complete,
                        reason: exit.reason,
                    }));
                }
            }
            let mut chunk = [0_u8; 8192];
            let count = if let Some(deadline) = self.deadline {
                timeout_at(deadline, self.reader.read(&mut chunk))
                    .await
                    .map_err(|_| Error::Timeout)??
            } else {
                self.reader.read(&mut chunk).await?
            };
            if count == 0 {
                self.ended = true;
                return if self.buffer.is_empty() {
                    Ok(None)
                } else {
                    Err(Error::UnexpectedEof)
                };
            }
            self.buffer.extend_from_slice(&chunk[..count]);
        }
    }
}

pub(crate) struct FrameWriter<W> {
    writer: W,
    max_frame: usize,
    write_timeout: Option<Duration>,
    interrupted: bool,
}

impl<W: AsyncWrite + Unpin> FrameWriter<W> {
    pub fn new(writer: W, max_frame: usize, write_timeout: Option<Duration>) -> Self {
        Self {
            writer,
            max_frame,
            write_timeout,
            interrupted: false,
        }
    }

    pub async fn data(&mut self, data: &[u8]) -> Result<()> {
        self.frame(FRAME_DATA, data).await
    }

    pub async fn resize(&mut self, size: TerminalSize) -> Result<()> {
        let payload = serde_json::to_vec(&size).map_err(|_| Error::InvalidRequest)?;
        self.frame(FRAME_RESIZE, &payload).await
    }

    async fn frame(&mut self, kind: u8, payload: &[u8]) -> Result<()> {
        if self.interrupted {
            return Err(Error::Closed);
        }
        if payload.len() > self.max_frame || payload.len() > u32::MAX as usize {
            return Err(Error::FrameTooLarge);
        }
        let mut frame = Vec::with_capacity(HEADER_BYTES + payload.len());
        frame.push(kind);
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(payload);
        // Cancellation may interrupt write_all in the middle of a frame. Leave
        // this set until the entire write completes, prohibiting corrupt reuse.
        self.interrupted = true;
        let write = async {
            self.writer.write_all(&frame).await?;
            self.writer.flush().await?;
            Ok::<_, io::Error>(())
        };
        if let Some(deadline) = self.write_timeout {
            timeout(deadline, write)
                .await
                .map_err(|_| Error::Timeout)??;
        } else {
            write.await?;
        }
        self.interrupted = false;
        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        self.interrupted = true;
        if let Some(deadline) = self.write_timeout {
            timeout(deadline, self.writer.shutdown())
                .await
                .map_err(|_| Error::Timeout)??;
        } else {
            self.writer.shutdown().await?;
        }
        Ok(())
    }
}
