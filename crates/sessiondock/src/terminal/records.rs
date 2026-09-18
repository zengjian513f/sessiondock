//! Read-only access to ptyhost session recordings (`<ptyhost dir>/records/<id>/`).
//!
//! A recording is the raw pty output plus resize events, checkpointed by the
//! host (see `ptyhost-record`). This module lists recordings and streams one to
//! a browser WebSocket: the latest checkpoint first, then every later event,
//! following the tail while the host is still writing. Nothing here takes an
//! ownership lease, sends input, or talks to the host process: the files are
//! the only source, so an exited host's recording is served the same way.
//!
//! Browser wire (all text frames are JSON with a `t` field):
//!
//! | direction | frame | meaning |
//! | --- | --- | --- |
//! | → browser | `{"t":"record","cols","rows","unix_ms","live"}` | first frame; size of the checkpoint |
//! | → browser | binary | sanitized terminal bytes (checkpoint state, then output) |
//! | → browser | `{"t":"resize","cols","rows"}` | apply before the following bytes |
//! | → browser | `{"t":"gap"}` | data was lost; a fresh resize + checkpoint follows, reset the terminal |
//! | → browser | `{"t":"exit","exit":{…}}` | the recorded host exit payload |
//! | → browser | `{"t":"end"}` | nothing more will come; preceded by the viewer reset bytes |
//! | browser → | anything | ignored (read-only) except Close |
//!
//! With `mode=grid` the same recording is fed through the host's terminal model
//! (`ptyhost-screen`) inside the Web service, and the browser receives the grid
//! protocol (`snapshot` / `diff` JSON lines in binary frames, see
//! `docs/terminal-grid.md`) instead of sanitized bytes. Live and history then share
//! one emulator. `record`, `exit` and `end` text frames are the same as above.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use ptyhost_record::reader::{self, Event, Read, Stamped};
use ptyhost_record::sanitize::{Sanitizer, VIEWER_RESET};
use ptyhost_screen::Screen;
use ptyhost_screen::grid::{self, GridState};
use serde::Serialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Sub-directory of the ptyhost directory that holds one directory per recording.
pub const RECORDS_SUBDIR: &str = "records";
/// Payload bytes fetched per blocking read.
const PAGE_BYTES: usize = 2 * 1024 * 1024;
/// Poll interval while following a live recording's tail.
const FOLLOW_INTERVAL: Duration = Duration::from_millis(150);
/// Largest binary WebSocket message sent to the browser.
const CHUNK: usize = 32 * 1024;

/// One recording as listed to the browser.
#[derive(Clone, Debug, Serialize)]
pub struct RecordEntry {
    pub id: String,
    pub name: String,
    pub host_pid: u64,
    pub created_ms: u64,
    pub ended_ms: Option<u64>,
    pub exit: Option<Value>,
    pub cols: u16,
    pub rows: u16,
    pub cwd: String,
    pub argv: Vec<String>,
    pub meta: Value,
    pub bytes: u64,
    pub segments: usize,
    /// Still being written: no exit recorded and the host process is alive.
    pub live: bool,
}

/// `<created_ms>-<host_pid>-<name>`: digits, digits, then the host's safe name.
pub fn valid_id(id: &str) -> bool {
    let mut parts = id.splitn(3, '-');
    let (Some(created), Some(pid), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let digits = |s: &str, max: usize| {
        !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit())
    };
    digits(created, 20)
        && digits(pid, 10)
        && !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The recording directory for a validated id, if it exists.
pub fn record_dir(root: &Path, id: &str) -> Option<PathBuf> {
    if !valid_id(id) {
        return None;
    }
    let dir = root.join(RECORDS_SUBDIR).join(id);
    dir.is_dir().then_some(dir)
}

fn host_alive(pid: u64) -> bool {
    #[cfg(target_os = "linux")]
    {
        pid > 0 && Path::new("/proc").join(pid.to_string()).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
}

fn read_entry(dir: &Path, id: &str) -> Option<RecordEntry> {
    let meta: Value = serde_json::from_slice(&std::fs::read(dir.join("meta.json")).ok()?).ok()?;
    let text = |key: &str| {
        meta.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let num = |key: &str| meta.get(key).and_then(Value::as_u64).unwrap_or(0);
    let segments = reader::list_segments(dir).unwrap_or_default();
    let ended_ms = meta.get("ended_ms").and_then(Value::as_u64);
    let host_pid = num("host_pid");
    Some(RecordEntry {
        id: id.to_string(),
        name: text("name"),
        host_pid,
        created_ms: num("created_ms"),
        ended_ms,
        exit: meta.get("exit").cloned(),
        cols: num("cols") as u16,
        rows: num("rows") as u16,
        cwd: text("cwd"),
        argv: meta
            .get("argv")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        meta: meta.get("meta").cloned().unwrap_or(Value::Null),
        bytes: segments.iter().map(|s| s.len).sum(),
        segments: segments.len(),
        live: ended_ms.is_none() && host_alive(host_pid),
    })
}

/// Every recording under `root`, newest first. A missing `records/` is empty.
pub fn list(root: &Path) -> io::Result<Vec<RecordEntry>> {
    let records = root.join(RECORDS_SUBDIR);
    let entries = match std::fs::read_dir(&records) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(id) = name.to_str() else { continue };
        if !valid_id(id) || !entry.path().is_dir() {
            continue;
        }
        if let Some(record) = read_entry(&entry.path(), id) {
            out.push(record);
        }
    }
    out.sort_by(|a, b| {
        b.created_ms
            .cmp(&a.created_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(out)
}

/// One recording by id; `None` when the id is invalid or absent.
pub fn entry(root: &Path, id: &str) -> Option<RecordEntry> {
    let dir = record_dir(root, id)?;
    read_entry(&dir, id)
}

type Sink = SplitSink<WebSocket, Message>;

struct Sender {
    sink: Sink,
    sanitizer: Sanitizer,
    failed: bool,
}

impl Sender {
    async fn text(&mut self, value: Value) {
        if self.failed {
            return;
        }
        if self
            .sink
            .send(Message::Text(value.to_string().into()))
            .await
            .is_err()
        {
            self.failed = true;
        }
    }

    /// One grid JSON line as a binary frame (no sanitizing: the model consumed the bytes).
    async fn raw_line(&mut self, line: &str) {
        if self.failed {
            return;
        }
        let mut payload = Vec::with_capacity(line.len() + 1);
        payload.extend_from_slice(line.as_bytes());
        payload.push(b'\n');
        if self
            .sink
            .send(Message::Binary(Bytes::from(payload)))
            .await
            .is_err()
        {
            self.failed = true;
        }
    }

    async fn bytes(&mut self, raw: &[u8]) {
        if self.failed || raw.is_empty() {
            return;
        }
        let mut clean = Vec::with_capacity(raw.len());
        self.sanitizer.push(raw, &mut clean);
        for chunk in clean.chunks(CHUNK) {
            if self
                .sink
                .send(Message::Binary(Bytes::copy_from_slice(chunk)))
                .await
                .is_err()
            {
                self.failed = true;
                return;
            }
        }
    }

    /// Flush the sanitizer's held-back tail and the viewer reset, then `end`.
    async fn finish(&mut self) {
        if self.failed {
            return;
        }
        let mut tail = Vec::new();
        self.sanitizer.finish(&mut tail);
        tail.extend_from_slice(VIEWER_RESET);
        if self
            .sink
            .send(Message::Binary(Bytes::from(tail)))
            .await
            .is_err()
        {
            self.failed = true;
            return;
        }
        self.text(json!({"t": "end"})).await;
    }

    async fn close(mut self, code: u16, reason: &str) {
        let _ = tokio::time::timeout(Duration::from_millis(250), async {
            let _ = self
                .sink
                .send(Message::Close(Some(CloseFrame {
                    code,
                    reason: reason.to_string().into(),
                })))
                .await;
        })
        .await;
    }

    /// Returns true when an `Exit` event was sent.
    async fn events(&mut self, read: &Read) -> bool {
        let mut exited = false;
        for Stamped { event, .. } in &read.events {
            match event {
                Event::Output(bytes) => self.bytes(bytes).await,
                Event::Resize { cols, rows } => {
                    self.text(json!({"t": "resize", "cols": cols, "rows": rows}))
                        .await
                }
                Event::Checkpoint { cols, rows, state } => {
                    self.text(json!({"t": "gap"})).await;
                    self.text(json!({"t": "resize", "cols": cols, "rows": rows}))
                        .await;
                    self.bytes(state).await;
                }
                Event::Exit(payload) => {
                    let exit: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
                    self.text(json!({"t": "exit", "exit": exit})).await;
                    exited = true;
                }
                Event::Mark(_) => {}
            }
            if self.failed {
                break;
            }
        }
        exited
    }
}

/// Serve one recording over a WebSocket until its end, the browser's close, or shutdown.
pub async fn stream(
    dir: PathBuf,
    entry: RecordEntry,
    socket: WebSocket,
    shutdown: CancellationToken,
) {
    let (sink, mut incoming) = socket.split();
    let mut sender = Sender {
        sink,
        sanitizer: Sanitizer::new(),
        failed: false,
    };
    let first = {
        let dir = dir.clone();
        tokio::task::spawn_blocking(move || reader::replay(&dir, PAGE_BYTES)).await
    };
    let (mut next, mut exited, mut live) = match first {
        Ok(Ok(Some((checkpoint, read)))) => {
            sender
                .text(json!({
                    "t": "record", "cols": checkpoint.cols, "rows": checkpoint.rows,
                    "unix_ms": checkpoint.unix_ms, "live": entry.live, "id": entry.id,
                }))
                .await;
            sender.bytes(&checkpoint.state).await;
            let exited = sender.events(&read).await;
            (read, exited, entry.live)
        }
        Ok(Ok(None)) => {
            sender.close(1011, "record has no checkpoint").await;
            return;
        }
        _ => {
            sender.close(1011, "record unreadable").await;
            return;
        }
    };
    let host_pid = entry.host_pid;
    loop {
        if sender.failed {
            return;
        }
        if exited || (next.at_end && !live) {
            break;
        }
        if next.at_end {
            // Follow the tail: wait, unless the browser leaves or the service stops.
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => { sender.close(1001, "shutdown").await; return; }
                message = incoming.next() => {
                    if matches!(message, None | Some(Err(_)) | Some(Ok(Message::Close(_)))) {
                        return;
                    }
                    continue;
                }
                _ = tokio::time::sleep(FOLLOW_INTERVAL) => {}
            }
            live = host_alive(host_pid);
        }
        let from = next.next;
        let page = {
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || reader::read_from(&dir, from, PAGE_BYTES)).await
        };
        match page {
            Ok(Ok(read)) => {
                if read.gap {
                    // read_from emits the re-checkpoint itself; nothing extra here.
                }
                exited = sender.events(&read).await;
                next = read;
            }
            _ => {
                sender.close(1011, "record unreadable").await;
                return;
            }
        }
    }
    sender.finish().await;
    sender.close(1000, "record end").await;
}

/// Scrollback rows kept by the replay model; also the snapshot's history budget.
const GRID_HISTORY: usize = 10000;

/// One recording, replayed through the terminal model, streamed as grid JSON lines.
struct GridReplay {
    screen: Screen,
    last: Option<GridState>,
    seq: u64,
}

impl GridReplay {
    fn new(cols: u16, rows: u16, state: &[u8]) -> Self {
        let mut screen = Screen::new(cols.max(1), rows.max(1), GRID_HISTORY);
        screen.feed(state);
        let _ = screen.take_responses();
        Self {
            screen,
            last: None,
            seq: 0,
        }
    }

    /// Full snapshot (`reset` chooses whether the browser drops its scrollback).
    fn snapshot(&mut self, reset: bool) -> String {
        let next = grid::capture(&self.screen);
        // 录制回放没有分页接口：reset 快照直接带上模型里的全部历史（≤ GRID_HISTORY）。
        let history = if reset {
            grid::history_rows(&self.screen, 0, next.history)
        } else {
            Vec::new()
        };
        self.seq += 1;
        let line = grid::snapshot_json(&next, &history, next.history, self.seq, reset);
        self.last = Some(next);
        line
    }

    /// Apply one page of recorded events; returns the JSON lines to send, in order.
    fn apply(&mut self, read: &Read) -> (Vec<String>, bool) {
        let mut out = Vec::new();
        let mut exited = false;
        let mut dirty = false;
        for Stamped { event, .. } in &read.events {
            match event {
                Event::Output(bytes) => {
                    self.screen.feed(bytes);
                    let _ = self.screen.take_responses();
                    dirty = true;
                }
                Event::Resize { cols, rows } => {
                    if dirty {
                        out.extend(self.diff());
                        dirty = false;
                    }
                    self.screen.resize(*cols, *rows);
                    out.push(self.snapshot(false));
                }
                Event::Checkpoint { cols, rows, state } => {
                    // Data was lost: rebuild the model from the checkpoint.
                    *self = Self::new(*cols, *rows, state);
                    self.seq = 0;
                    out.push(json!({"t": "gap"}).to_string());
                    out.push(self.snapshot(true));
                    dirty = false;
                }
                Event::Exit(payload) => {
                    if dirty {
                        out.extend(self.diff());
                        dirty = false;
                    }
                    let exit: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
                    out.push(json!({"t": "exit", "exit": exit}).to_string());
                    exited = true;
                }
                Event::Mark(_) => {}
            }
        }
        if dirty {
            // The end of a page is a frame boundary; finish any held synchronized update.
            self.screen.expire_sync();
            out.extend(self.diff());
        }
        (out, exited)
    }

    fn diff(&mut self) -> Option<String> {
        let next = grid::capture(&self.screen);
        let prev = self.last.take()?;
        let scrolled = if !next.alt && next.history > prev.history {
            grid::history_rows(&self.screen, prev.history, next.history)
        } else {
            Vec::new()
        };
        let title = self.screen.title();
        let line = grid::diff_json(&prev, &next, &scrolled, Some(title.as_str()), self.seq + 1);
        self.last = Some(next);
        if line.is_some() {
            self.seq += 1;
        }
        line
    }
}

/// Grid-mode counterpart of [`stream`]: the recording is replayed through the
/// terminal model and the browser receives grid JSON lines.
pub async fn stream_grid(
    dir: PathBuf,
    entry: RecordEntry,
    socket: WebSocket,
    shutdown: CancellationToken,
) {
    let (sink, mut incoming) = socket.split();
    let mut sender = Sender {
        sink,
        sanitizer: Sanitizer::new(),
        failed: false,
    };
    let first = {
        let dir = dir.clone();
        tokio::task::spawn_blocking(move || reader::replay(&dir, PAGE_BYTES)).await
    };
    let (checkpoint, read) = match first {
        Ok(Ok(Some(pair))) => pair,
        Ok(Ok(None)) => {
            sender.close(1011, "record has no checkpoint").await;
            return;
        }
        _ => {
            sender.close(1011, "record unreadable").await;
            return;
        }
    };
    sender
        .text(json!({
            "t": "record", "cols": checkpoint.cols, "rows": checkpoint.rows,
            "unix_ms": checkpoint.unix_ms, "live": entry.live, "id": entry.id, "mode": "grid",
        }))
        .await;
    // The model is CPU work; it lives in blocking tasks and is handed back each page.
    let mut replay = Some(
        tokio::task::spawn_blocking(move || {
            let mut replay = GridReplay::new(checkpoint.cols, checkpoint.rows, &checkpoint.state);
            let first = replay.snapshot(true);
            let (mut lines, exited) = replay.apply(&read);
            lines.insert(0, first);
            (replay, lines, exited, read)
        })
        .await,
    );
    let mut next;
    let mut exited;
    let mut live = entry.live;
    let host_pid = entry.host_pid;
    let mut model = match replay.take() {
        Some(Ok((model, lines, was_exited, read))) => {
            for line in lines {
                sender.raw_line(&line).await;
            }
            exited = was_exited;
            next = read;
            model
        }
        _ => {
            sender.close(1011, "record unreadable").await;
            return;
        }
    };
    loop {
        if sender.failed {
            return;
        }
        if exited || (next.at_end && !live) {
            break;
        }
        if next.at_end {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => { sender.close(1001, "shutdown").await; return; }
                message = incoming.next() => {
                    if matches!(message, None | Some(Err(_)) | Some(Ok(Message::Close(_)))) {
                        return;
                    }
                    continue;
                }
                _ = tokio::time::sleep(FOLLOW_INTERVAL) => {}
            }
            live = host_alive(host_pid);
        }
        let from = next.next;
        let dir_clone = dir.clone();
        let page = tokio::task::spawn_blocking(move || {
            let read = reader::read_from(&dir_clone, from, PAGE_BYTES)?;
            let (lines, exited) = model.apply(&read);
            Ok::<_, io::Error>((model, lines, exited, read))
        })
        .await;
        match page {
            Ok(Ok((returned, lines, was_exited, read))) => {
                model = returned;
                for line in lines {
                    sender.raw_line(&line).await;
                }
                exited = was_exited;
                next = read;
            }
            _ => {
                sender.close(1011, "record unreadable").await;
                return;
            }
        }
    }
    sender.text(json!({"t": "end"})).await;
    sender.close(1000, "record end").await;
}

#[cfg(test)]
mod tests {
    use super::valid_id;

    #[test]
    fn ids_are_strict() {
        assert!(valid_id("1758150000000-12345-claude_main"));
        assert!(valid_id("1-2-a.b-c"));
        assert!(!valid_id("../x"));
        assert!(!valid_id("1758150000000-12345-"));
        assert!(!valid_id("1758150000000-12345-a/b"));
        assert!(!valid_id("abc-1-x"));
        assert!(!valid_id(&format!("1-2-{}", "x".repeat(65))));
    }
}
