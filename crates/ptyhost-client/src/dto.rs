use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The safe, browser-facing subset of a host record. Discovery does not prove liveness.
/// The host's arbitrary `meta`, `argv`, token and endpoint are deliberately omitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionSummary {
    pub name: String,
    pub created: u64,
    pub attached: bool,
    pub pid: u32,
    pub host_pid: u32,
    pub cwd: String,
    pub cmd: String,
    pub cols: u16,
    pub rows: u16,
    pub owned: bool,
    pub server: &'static str,
    pub backend: &'static str,
    /// The host accepts `mode:"grid"` attachments (false for hosts started
    /// before the grid protocol existed).
    pub grid: bool,
}

/// What an attachment streams back: raw pty bytes (xterm.js) or grid JSON lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AttachMode {
    #[default]
    Bytes,
    Grid,
}

impl AttachMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bytes => "bytes",
            Self::Grid => "grid",
        }
    }
}

/// Nonzero PTY dimensions. Browser hidden-view/minimum-size policy belongs above this crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TerminalSize {
    cols: u16,
    rows: u16,
}

impl TerminalSize {
    pub fn new(cols: u16, rows: u16) -> Result<Self> {
        if cols == 0 || rows == 0 {
            return Err(Error::InvalidSize);
        }
        Ok(Self { cols, rows })
    }

    pub fn cols(self) -> u16 {
        self.cols
    }

    pub fn rows(self) -> u16 {
        self.rows
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    Screen,
    Scrollback,
}

/// One control connection performs exactly one operation.
/// No Debug implementation: operations may contain a user's terminal input.
#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ControlOp {
    Info,
    Send {
        text: String,
    },
    Paste {
        text: String,
        bracketed: bool,
    },
    Keys {
        keys: Vec<String>,
    },
    Resize {
        #[serde(flatten)]
        size: TerminalSize,
    },
    Capture {
        kind: CaptureKind,
        styled: bool,
        join: bool,
        lines: usize,
    },
    Cursor,
    /// Grid history rows `[from, to)` (absolute, 0 = oldest); the host caps a page at 2000.
    GridRows {
        from: usize,
        to: usize,
    },
    Rename {
        to: String,
    },
    Kill {
        force: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CaptureReply {
    pub text: String,
    pub cursor: [u16; 2],
    pub alt: bool,
    pub cols: u16,
    pub rows: u16,
    /// Missing health fields mean unknown, never an implicitly healthy screen.
    pub lag: Option<u64>,
    pub dropped: Option<u64>,
    pub resets: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct GridRowsReply {
    pub rows: Vec<serde_json::Value>,
    pub from: usize,
    pub to: usize,
    pub total: usize,
    pub lag: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CursorReply {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
    pub alt: bool,
    pub lag: Option<u64>,
    pub dropped: Option<u64>,
    pub resets: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ControlReply {
    Ack,
    Info { info: SessionSummary, exited: bool },
    Paste { bracketed: bool },
    Capture(CaptureReply),
    Cursor(CursorReply),
    GridRows(GridRowsReply),
    Renamed { name: String },
}

/// Raw PTY bytes remain bytes, including invalid/split UTF-8.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    PtyDrainTimeout,
    PtyReadError,
    #[serde(other)]
    Unknown,
}

/// Exit reasons are a whitelist, never arbitrary host-provided error text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEvent {
    Data(Vec<u8>),
    Exit {
        code: i32,
        /// None is an older host: completeness was not declared.
        output_complete: Option<bool>,
        reason: Option<ExitReason>,
    },
}

// Private and intentionally neither Debug nor Serialize: TCP records contain credentials.
#[derive(Deserialize)]
pub(crate) struct HostRecord {
    pub name: String,
    pub host_pid: u32,
    pub pid: u32,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub boot_id: Option<String>,
    #[serde(default)]
    pub attached: bool,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub cmd: String,
    pub cols: u16,
    pub rows: u16,
    pub sock: Option<String>,
    pub port: Option<u16>,
    pub token: Option<String>,
    #[serde(default)]
    pub grid: bool,
    #[serde(default, deserialize_with = "crate::association::deserialize_metadata")]
    pub meta: crate::association::Metadata,
}

impl HostRecord {
    // Mutable screen state (attached/size) is deliberately excluded. Credentials
    // are compared privately and are never returned as an instance identifier.
    pub fn same_identity(&self, other: &Self) -> bool {
        self.name == other.name
            && self.host_pid == other.host_pid
            && self.pid == other.pid
            && self.created == other.created
            && self.boot_id == other.boot_id
            && self.sock == other.sock
            && self.port == other.port
            && self.token == other.token
            && self.cwd == other.cwd
            && self.cmd == other.cmd
            && self.meta == other.meta
    }

    pub fn summary(&self) -> Result<SessionSummary> {
        validate_name(&self.name).map_err(|_| Error::InvalidMetadata)?;
        if self.host_pid == 0 || self.pid == 0 || self.cols == 0 || self.rows == 0 {
            return Err(Error::InvalidMetadata);
        }
        Ok(SessionSummary {
            name: self.name.clone(),
            created: self.created,
            attached: self.attached,
            pid: self.pid,
            host_pid: self.host_pid,
            cwd: self.cwd.clone(),
            cmd: self.cmd.clone(),
            cols: self.cols,
            rows: self.rows,
            owned: true,
            server: "ptyhost",
            backend: "ptyhost",
            grid: self.grid,
        })
    }
}

pub(crate) fn validate_name(name: &str) -> Result<()> {
    // A portable single filename component, not an arbitrary path or device name.
    if name.is_empty()
        || name.len() > 128
        || name.starts_with('.')
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "/\\:<>\"|?*".contains(c))
    {
        return Err(Error::InvalidName);
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(Error::InvalidName);
    }
    Ok(())
}
