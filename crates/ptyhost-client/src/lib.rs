//! Bounded asynchronous access to explicitly selected local ptyhost records.
//!
//! This crate never launches a process, selects a production/default directory,
//! deletes stale metadata, or speaks the browser's WebSocket protocol.

#![doc = include_str!("../README.md")]

mod association;
mod bound;
mod dto;
mod launch;
mod native_binding;
mod transport;
mod wire;

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::fs;
use tokio::io::{AsyncReadExt, ReadHalf, WriteHalf};
use tokio::time::timeout;

use dto::{HostRecord, validate_name};
use transport::LocalStream;
use wire::{FrameReader, FrameWriter};

pub use association::{
    Association, AssociationState, DeclaredRecord, HostObservation, LaunchIdentity, LaunchState,
    Source,
};
pub use bound::BoundTarget;
pub use dto::{
    CaptureKind, CaptureReply, ControlOp, ControlReply, CursorReply, ExitReason, HostEvent,
    SessionSummary, TerminalSize,
};
pub use launch::LaunchTarget;
pub use native_binding::{NativeBinding, NativeBindingState};

pub type Result<T> = std::result::Result<T, Error>;

/// Errors intentionally omit metadata, request bodies and arbitrary peer error
/// strings: a faulty peer can echo the token in its rejection or invalid JSON.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid host directory")]
    InvalidDirectory,
    #[error("invalid session name")]
    InvalidName,
    #[error("terminal dimensions must be nonzero")]
    InvalidSize,
    #[error("invalid client limits")]
    InvalidLimits,
    #[error("host session record not found")]
    NotFound,
    #[error("invalid host metadata")]
    InvalidMetadata,
    #[error("unsupported or unsafe local endpoint")]
    InvalidEndpoint,
    #[error("host rejected the request")]
    Rejected,
    #[error("invalid host request")]
    InvalidRequest,
    #[error("invalid host response")]
    InvalidReply,
    #[error("host identity changed during observation")]
    IdentityChanged,
    #[error("host does not advertise guarded instance control")]
    GuardUnsupported,
    #[error("host observation cannot identify the requested native session instance")]
    InvalidBinding,
    #[error(
        "host did not acknowledge the exact instance guard; an operation may have taken effect"
    )]
    GuardNotAcknowledged,
    #[error("host does not advertise guarded launch control")]
    LaunchGuardUnsupported,
    #[error("host observation cannot identify the requested launch instance")]
    InvalidLaunchBinding,
    #[error("host did not acknowledge the exact launch guard; an operation may have taken effect")]
    LaunchGuardNotAcknowledged,
    #[error("host does not support one-time native binding")]
    NativeBindingUnsupported,
    #[error("invalid native binding evidence")]
    InvalidNativeBinding,
    #[error("host already has a different native binding")]
    NativeBindingConflict,
    #[error("control line exceeds configured limit")]
    LineTooLarge,
    #[error("host frame exceeds configured limit")]
    FrameTooLarge,
    #[error("invalid host frame")]
    InvalidFrame,
    #[error("host stream closed during a message")]
    UnexpectedEof,
    #[error("host operation timed out; a write may already have taken effect")]
    Timeout,
    #[error("host stream cannot be reused after a cancelled or failed write")]
    Closed,
    #[error("local host I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_line_bytes: usize,
    pub max_frame_bytes: usize,
    pub operation_timeout: Duration,
    /// Optional deadline for partial reads and writes after attach. Python
    /// waits indefinitely after attach, so production uses `None`.
    pub partial_frame_timeout: Option<Duration>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_line_bytes: 4 * 1024 * 1024,
            // Replay may include the host's 32 MiB screen backlog plus history.
            max_frame_bytes: 64 * 1024 * 1024,
            operation_timeout: Duration::from_secs(10),
            partial_frame_timeout: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HostClient {
    directory: PathBuf,
    limits: Limits,
}

impl HostClient {
    /// The directory is mandatory. Relative paths are bound to the current cwd
    /// now, not reinterpreted later. The caller must select an isolated/trusted dir.
    pub fn new(directory: impl Into<PathBuf>, limits: Limits) -> Result<Self> {
        let directory = directory.into();
        if directory.as_os_str().is_empty() {
            return Err(Error::InvalidDirectory);
        }
        if limits.max_line_bytes == 0
            || limits.max_line_bytes > u32::MAX as usize
            || limits.max_frame_bytes == 0
            || limits.max_frame_bytes > u32::MAX as usize
            || limits.max_frame_bytes > usize::MAX - 5
            || limits.operation_timeout.is_zero()
            || limits
                .partial_frame_timeout
                .is_some_and(|timeout| timeout.is_zero())
        {
            return Err(Error::InvalidLimits);
        }
        let directory = if directory.is_absolute() {
            directory
        } else {
            std::env::current_dir()?.join(directory)
        };
        Ok(Self { directory, limits })
    }

    /// Read metadata only. Missing directory is empty; malformed/symlink records
    /// are skipped. No PID probe, network connection, cleanup, or liveness claim.
    pub async fn discover(&self) -> Result<Vec<SessionSummary>> {
        timeout(self.limits.operation_timeout, async {
            let mut entries = match fs::read_dir(&self.directory).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
                Err(error) => return Err(error.into()),
            };
            let mut rows = Vec::new();
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.extension().is_none_or(|ext| ext != "json") {
                    continue;
                }
                let Some(name) = path.file_stem().and_then(|name| name.to_str()) else {
                    continue;
                };
                if validate_name(name).is_err() {
                    continue;
                }
                if let Ok(Some(record)) = self.read_record(name).await {
                    rows.push(record.summary()?);
                }
            }
            rows.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(rows)
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    pub async fn session(&self, name: &str) -> Result<Option<SessionSummary>> {
        validate_name(name)?;
        timeout(self.limits.operation_timeout, async {
            self.read_record(name)
                .await?
                .map(|record| record.summary())
                .transpose()
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    /// Read the reviewed metadata a record declares, without connecting or
    /// probing liveness. It reports what an unreachable host claimed; the
    /// caller must not treat it as a verified association.
    pub async fn declared(&self, name: &str) -> Result<Option<DeclaredRecord>> {
        validate_name(name)?;
        timeout(self.limits.operation_timeout, async {
            self.read_record(name)
                .await?
                .map(|record| {
                    Ok(DeclaredRecord {
                        summary: record.summary()?,
                        association: record.meta.association,
                        instance_id: record.meta.instance_id,
                    })
                })
                .transpose()
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    /// Verify a bounded Info exchange against both the starting and finishing
    /// record. Missing/malformed associations remain explicit unknown metadata;
    /// they do not prevent independent raw transport access.
    pub async fn probe(&self, name: &str) -> Result<HostObservation> {
        validate_name(name)?;
        timeout(self.limits.operation_timeout, async {
            let before = self.read_record(name).await?.ok_or(Error::NotFound)?;
            let mut stream = self.connect(&before).await?;
            let body = json!({"op": "info", "token": before.token.as_deref().unwrap_or_default()});
            wire::send_json(&mut stream, &body, self.limits.max_line_bytes).await?;
            let value =
                wire::read_json(&mut stream, &mut Vec::new(), self.limits.max_line_bytes).await?;
            check_ok(&value)?;
            self.observation_reply(&before, value).await
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    async fn observation_reply(
        &self,
        before: &HostRecord,
        value: Value,
    ) -> Result<HostObservation> {
        #[derive(Deserialize)]
        struct Reply {
            info: HostRecord,
            exited: bool,
        }
        let instance_guard_v1 = value
            .get("capabilities")
            .and_then(|caps| caps.get("instance_guard"))
            .and_then(Value::as_u64)
            == Some(1);
        let launch_guard_v1 = value
            .get("capabilities")
            .and_then(|caps| caps.get("launch_guard"))
            .and_then(Value::as_u64)
            == Some(1);
        let reply: Reply =
            serde_json::from_value(value.clone()).map_err(|_| Error::InvalidReply)?;
        let native_binding = native_binding::observe(&value, &reply.info.meta);
        let summary = reply.info.summary().map_err(|_| Error::InvalidReply)?;
        let after = self
            .read_record(&before.name)
            .await?
            .ok_or(Error::IdentityChanged)?;
        if !before.same_identity(&reply.info) || !before.same_identity(&after) {
            return Err(Error::IdentityChanged);
        }
        Ok(HostObservation {
            summary,
            association: reply.info.meta.association,
            instance_id: reply.info.meta.instance_id,
            exited: reply.exited,
            instance_guard_v1,
            launch: reply.info.meta.launch,
            launch_guard_v1,
            native_binding,
        })
    }

    /// A transport error or timeout after submission is ambiguous. Never blindly
    /// retry writes; native delivery ledgers belong in the application layer.
    pub async fn request(&self, name: &str, operation: ControlOp) -> Result<ControlReply> {
        validate_name(name)?;
        if let ControlOp::Rename { to } = &operation {
            validate_name(to)?;
        }
        timeout(self.limits.operation_timeout, async {
            let record = self.read_record(name).await?.ok_or(Error::NotFound)?;
            let mut stream = self.connect(&record).await?;
            let mut body = serde_json::to_value(&operation).map_err(|_| Error::InvalidRequest)?;
            body["token"] = json!(record.token.as_deref().unwrap_or_default());
            wire::send_json(&mut stream, &body, self.limits.max_line_bytes).await?;
            let value =
                wire::read_json(&mut stream, &mut Vec::new(), self.limits.max_line_bytes).await?;
            check_ok(&value)?;
            parse_reply(operation, value)
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    pub async fn attach(&self, name: &str, size: TerminalSize, replay: bool) -> Result<Attachment> {
        validate_name(name)?;
        timeout(self.limits.operation_timeout, async {
            let record = self.read_record(name).await?.ok_or(Error::NotFound)?;
            let mut stream = self.connect(&record).await?;
            let body = json!({"op": "attach", "token": record.token.as_deref().unwrap_or_default(),
                              "cols": size.cols(), "rows": size.rows(), "replay": replay});
            wire::send_json(&mut stream, &body, self.limits.max_line_bytes).await?;
            let mut buffer = Vec::new();
            let reply =
                wire::read_json(&mut stream, &mut buffer, self.limits.max_line_bytes).await?;
            check_ok(&reply)?;
            self.attachment_from_reply(stream, buffer, reply)
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    fn attachment_from_reply(
        &self,
        stream: LocalStream,
        buffer: Vec<u8>,
        reply: Value,
    ) -> Result<Attachment> {
        #[derive(Deserialize)]
        struct Shape {
            cols: u16,
            rows: u16,
        }
        let shape: Shape = serde_json::from_value(reply).map_err(|_| Error::InvalidReply)?;
        let size = TerminalSize::new(shape.cols, shape.rows).map_err(|_| Error::InvalidReply)?;
        let (reader, writer) = tokio::io::split(stream);
        Ok(Attachment {
            size,
            reader: AttachReader {
                inner: FrameReader::new(
                    reader,
                    buffer,
                    self.limits.max_frame_bytes,
                    self.limits.partial_frame_timeout,
                ),
            },
            writer: AttachWriter {
                inner: FrameWriter::new(
                    writer,
                    self.limits.max_frame_bytes,
                    self.limits.partial_frame_timeout,
                ),
            },
        })
    }

    async fn read_record(&self, name: &str) -> Result<Option<HostRecord>> {
        let path = self.directory.join(format!("{name}.json"));
        let metadata = match fs::symlink_metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_file() || metadata.len() > self.limits.max_line_bytes as u64 {
            return Err(Error::InvalidMetadata);
        }
        // take() also bounds a file that grows between metadata() and read().
        let file = fs::File::open(path).await?;
        let mut bytes = Vec::new();
        file.take(self.limits.max_line_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > self.limits.max_line_bytes {
            return Err(Error::InvalidMetadata);
        }
        let record: HostRecord =
            serde_json::from_slice(&bytes).map_err(|_| Error::InvalidMetadata)?;
        if record.name != name {
            return Err(Error::InvalidMetadata);
        }
        record.summary()?;
        Ok(Some(record))
    }

    async fn connect(&self, record: &HostRecord) -> Result<LocalStream> {
        if let Some(port) = record.port {
            if port == 0 || record.token.as_ref().is_none_or(|token| token.is_empty()) {
                return Err(Error::InvalidEndpoint);
            }
            // No supplied host/IP is ever used, even if present in metadata.
            return Ok(LocalStream::Tcp(
                tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await?,
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;
            let supplied = record.sock.as_deref().ok_or(Error::InvalidEndpoint)?;
            let expected_name = format!("{}.sock", record.name);
            if Path::new(supplied)
                .file_name()
                .is_none_or(|name| name != expected_name.as_str())
            {
                return Err(Error::InvalidEndpoint);
            }
            // Select the known host-layout socket in our explicit directory;
            // metadata cannot redirect us to an arbitrary filesystem endpoint.
            let path = self.directory.join(expected_name);
            if !fs::symlink_metadata(&path).await?.file_type().is_socket() {
                return Err(Error::InvalidEndpoint);
            }
            Ok(LocalStream::Unix(
                tokio::net::UnixStream::connect(path).await?,
            ))
        }
        #[cfg(not(unix))]
        Err(Error::InvalidEndpoint)
    }
}

fn check_ok(value: &Value) -> Result<()> {
    match value.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(()),
        Some(false) => Err(Error::Rejected),
        None => Err(Error::InvalidReply),
    }
}

fn parse_reply(operation: ControlOp, value: Value) -> Result<ControlReply> {
    match operation {
        ControlOp::Info => {
            #[derive(Deserialize)]
            struct Reply {
                info: HostRecord,
                exited: bool,
            }
            let reply: Reply = serde_json::from_value(value).map_err(|_| Error::InvalidReply)?;
            Ok(ControlReply::Info {
                info: reply.info.summary().map_err(|_| Error::InvalidReply)?,
                exited: reply.exited,
            })
        }
        ControlOp::Capture { .. } => serde_json::from_value(value)
            .map(ControlReply::Capture)
            .map_err(|_| Error::InvalidReply),
        ControlOp::Cursor => serde_json::from_value(value)
            .map(ControlReply::Cursor)
            .map_err(|_| Error::InvalidReply),
        ControlOp::Paste { .. } => {
            let bracketed = value
                .get("bracketed")
                .and_then(Value::as_bool)
                .ok_or(Error::InvalidReply)?;
            Ok(ControlReply::Paste { bracketed })
        }
        ControlOp::Rename { .. } => {
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .ok_or(Error::InvalidReply)?;
            validate_name(name).map_err(|_| Error::InvalidReply)?;
            Ok(ControlReply::Renamed {
                name: name.to_owned(),
            })
        }
        _ => Ok(ControlReply::Ack),
    }
}

pub struct Attachment {
    pub size: TerminalSize,
    pub reader: AttachReader,
    pub writer: AttachWriter,
}

impl Attachment {
    pub fn into_split(self) -> (AttachReader, AttachWriter) {
        (self.reader, self.writer)
    }
}

pub struct AttachReader {
    inner: FrameReader<ReadHalf<LocalStream>>,
}

impl AttachReader {
    /// Cancellation-safe. Idle and partial frames may remain silent indefinitely
    /// unless the caller explicitly configures a partial-frame deadline.
    pub async fn next(&mut self) -> Result<Option<HostEvent>> {
        self.inner.next().await
    }
}

pub struct AttachWriter {
    inner: FrameWriter<WriteHalf<LocalStream>>,
}

impl AttachWriter {
    /// If this future is cancelled or errors, drop/reconnect the attachment.
    /// Reusing its writer is prohibited; do not automatically resubmit the input.
    pub async fn send_data(&mut self, bytes: &[u8]) -> Result<()> {
        self.inner.data(bytes).await
    }

    pub async fn resize(&mut self, size: TerminalSize) -> Result<()> {
        self.inner.resize(size).await
    }

    /// Ends this client's input stream only. It does not issue host `kill`.
    pub async fn shutdown(&mut self) -> Result<()> {
        self.inner.shutdown().await
    }
}
