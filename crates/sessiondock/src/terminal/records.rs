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
use ptyhost_record::Position;
use ptyhost_record::reader::{self, Event, Stamped};
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

/// Delete every recording of host `name` whose host is gone (a discarded
/// SSH session takes its recordings with it). Returns how many were removed;
/// a live recording is left alone.
pub fn remove_for_host(root: &Path, name: &str) -> io::Result<usize> {
    let mut removed = 0;
    for entry in list(root)? {
        if entry.name != name || entry.live {
            continue;
        }
        let Some(dir) = record_dir(root, &entry.id) else {
            continue;
        };
        std::fs::remove_dir_all(&dir)?;
        removed += 1;
    }
    Ok(removed)
}

/// Delete every recording left by a non-shell host that is gone. Agent hosts
/// run with `--no-record` (`Launcher::command`); this removes what hosts
/// started before that rule wrote, once they have exited. A recording whose
/// metadata names no source (a host not started by the launcher) is kept.
/// Returns how many were removed.
pub fn remove_agent_leftovers(root: &Path) -> io::Result<usize> {
    let mut removed = 0;
    for entry in list(root)? {
        let source = entry.meta.get("source").and_then(Value::as_str);
        if entry.live || source.is_none_or(|source| source == "shell") {
            continue;
        }
        let Some(dir) = record_dir(root, &entry.id) else {
            continue;
        };
        std::fs::remove_dir_all(&dir)?;
        removed += 1;
    }
    Ok(removed)
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

/// Scrollback rows kept by the grid replay model; also its snapshot's history budget.
const GRID_HISTORY: usize = 10000;
/// A recorded gap longer than this is played as this long (per unit of speed).
const MAX_PLAY_GAP: Duration = Duration::from_millis(2000);
/// Events closer than this are emitted together while playing at speed.
const PLAY_COALESCE_MS: u64 = 20;

struct Sender {
    sink: Sink,
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

    async fn binary(&mut self, payload: Vec<u8>) {
        if self.failed || payload.is_empty() {
            return;
        }
        for chunk in payload.chunks(CHUNK) {
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
}

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

/// What the browser receives: sanitized bytes for xterm.js, or grid JSON lines.
enum Output {
    Bytes(Sanitizer),
    Grid(Box<GridReplay>),
}

/// Lines/bytes produced by applying events, in order.
enum Emit {
    Text(Value),
    Bin(Vec<u8>),
}

/// Apply one event to the output, collecting what to send. `silent` applies without
/// emitting (used to rebuild state for a seek); the caller emits a snapshot after.
fn apply_event(output: &mut Output, event: &Event, silent: bool, out: &mut Vec<Emit>) -> bool {
    let mut exited = false;
    match output {
        Output::Bytes(sanitizer) => match event {
            Event::Output(bytes) => {
                if !silent {
                    let mut clean = Vec::with_capacity(bytes.len());
                    sanitizer.push(bytes, &mut clean);
                    out.push(Emit::Bin(clean));
                }
            }
            Event::Resize { cols, rows } => {
                if !silent {
                    out.push(Emit::Text(
                        json!({"t": "resize", "cols": cols, "rows": rows}),
                    ));
                }
            }
            Event::Checkpoint { cols, rows, state } => {
                if !silent {
                    out.push(Emit::Text(json!({"t": "gap"})));
                    out.push(Emit::Text(
                        json!({"t": "resize", "cols": cols, "rows": rows}),
                    ));
                    let mut clean = Vec::with_capacity(state.len());
                    sanitizer.push(state, &mut clean);
                    out.push(Emit::Bin(clean));
                }
            }
            Event::Exit(payload) => {
                let exit: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
                out.push(Emit::Text(json!({"t": "exit", "exit": exit})));
                exited = true;
            }
            Event::Mark(_) => {}
        },
        Output::Grid(replay) => match event {
            Event::Output(bytes) => {
                replay.screen.feed(bytes);
                let _ = replay.screen.take_responses();
                if !silent {
                    replay.screen.expire_sync();
                    if let Some(line) = replay.diff() {
                        out.push(Emit::Bin(line_bytes(&line)));
                    }
                }
            }
            Event::Resize { cols, rows } => {
                replay.screen.resize(*cols, *rows);
                if !silent {
                    let line = replay.snapshot(false);
                    out.push(Emit::Bin(line_bytes(&line)));
                }
            }
            Event::Checkpoint { cols, rows, state } => {
                **replay = GridReplay::new(*cols, *rows, state);
                if !silent {
                    out.push(Emit::Text(json!({"t": "gap"})));
                    let line = replay.snapshot(true);
                    out.push(Emit::Bin(line_bytes(&line)));
                }
            }
            Event::Exit(payload) => {
                let exit: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
                out.push(Emit::Text(json!({"t": "exit", "exit": exit})));
                exited = true;
            }
            Event::Mark(_) => {}
        },
    }
    exited
}

fn line_bytes(line: &str) -> Vec<u8> {
    let mut payload = Vec::with_capacity(line.len() + 1);
    payload.extend_from_slice(line.as_bytes());
    payload.push(b'\n');
    payload
}

/// Playback state that lives in blocking tasks (the model is CPU work).
struct Playback {
    dir: PathBuf,
    entry: RecordEntry,
    grid: bool,
    output: Output,
    /// Events read from disk but not yet applied (`buffer[buffer_at..]`).
    buffer: Vec<Stamped>,
    buffer_at: usize,
    /// Where the next page read starts (after `buffer`).
    next: Position,
    /// The last read reached the end of what is on disk.
    at_end: bool,
    exited: bool,
    /// Playback position, including a requested instant between recorded events.
    clock: u64,
}

impl Playback {
    fn new(dir: PathBuf, entry: RecordEntry, grid: bool, clock: u64) -> Self {
        Self {
            dir,
            entry,
            grid,
            output: Output::Bytes(Sanitizer::new()),
            buffer: Vec::new(),
            buffer_at: 0,
            next: Position::default(),
            at_end: false,
            exited: false,
            clock,
        }
    }

    fn buffered(&self) -> &[Stamped] {
        &self.buffer[self.buffer_at.min(self.buffer.len())..]
    }

    /// Fill the buffer from disk when it is drained (and the end was not reached
    /// yet, or the recording may have grown since).
    fn fill(&mut self) -> io::Result<()> {
        if self.buffer_at < self.buffer.len() {
            return Ok(());
        }
        let read = reader::read_from(&self.dir, self.next, PAGE_BYTES)?;
        self.buffer = read.events;
        self.buffer_at = 0;
        self.next = read.next;
        self.at_end = read.at_end;
        Ok(())
    }

    fn record_frame(&self, cols: u16, rows: u16) -> Value {
        let mut value = json!({
            "t": "record", "cols": cols, "rows": rows, "unix_ms": self.clock,
            "live": self.entry.live, "id": self.entry.id,
        });
        if self.grid {
            value["mode"] = Value::from("grid");
        }
        value
    }

    /// Rebuild at the latest checkpoint (ordinary open) or at `until` (seek): the
    /// last checkpoint at or before `until`, then every event up to `until`
    /// applied silently. Returns the opening frames (`record` + full state).
    fn build(&mut self, until: Option<u64>) -> io::Result<Vec<Emit>> {
        let checkpoint = match until {
            Some(t) => {
                reader::checkpoint_before(&self.dir, t)?.or(reader::latest_checkpoint(&self.dir)?)
            }
            None => reader::latest_checkpoint(&self.dir)?,
        };
        let Some(checkpoint) = checkpoint else {
            return Err(io::Error::other("record has no checkpoint"));
        };
        self.clock = checkpoint.unix_ms;
        self.exited = false;
        self.buffer.clear();
        self.buffer_at = 0;
        self.next = reader::position_after_checkpoint(&self.dir, &checkpoint)?;
        self.at_end = false;
        let mut out = Vec::new();
        let Some(t) = until else {
            self.output = if self.grid {
                Output::Grid(Box::new(GridReplay::new(
                    checkpoint.cols,
                    checkpoint.rows,
                    &checkpoint.state,
                )))
            } else {
                Output::Bytes(Sanitizer::new())
            };
            out.push(Emit::Text(
                self.record_frame(checkpoint.cols, checkpoint.rows),
            ));
            match &mut self.output {
                Output::Bytes(sanitizer) => {
                    let mut clean = Vec::with_capacity(checkpoint.state.len());
                    sanitizer.push(&checkpoint.state, &mut clean);
                    out.push(Emit::Bin(clean));
                }
                Output::Grid(replay) => {
                    let line = replay.snapshot(true);
                    out.push(Emit::Bin(line_bytes(&line)));
                }
            }
            return Ok(out);
        };
        // Seek: run a model silently up to `t`; the buffer keeps the rest of the page.
        let mut model = GridReplay::new(checkpoint.cols, checkpoint.rows, &checkpoint.state);
        'pages: loop {
            self.fill()?;
            if self.buffer_at >= self.buffer.len() {
                if self.at_end {
                    break;
                }
                continue;
            }
            while self.buffer_at < self.buffer.len() {
                let Stamped { unix_ms, event } = &self.buffer[self.buffer_at];
                if *unix_ms > t {
                    break 'pages;
                }
                match event {
                    Event::Output(bytes) => {
                        model.screen.feed(bytes);
                        let _ = model.screen.take_responses();
                    }
                    Event::Resize { cols, rows } => model.screen.resize(*cols, *rows),
                    Event::Checkpoint { cols, rows, state } => {
                        model = GridReplay::new(*cols, *rows, state);
                    }
                    Event::Exit(_) => self.exited = true,
                    Event::Mark(_) => {}
                }
                self.clock = *unix_ms;
                self.buffer_at += 1;
            }
            if self.at_end {
                break;
            }
        }
        // The screen can stay unchanged throughout an idle interval, but seeking
        // there must retain the requested position (also the origin for play).
        self.clock = t;
        model.screen.expire_sync();
        let captured = grid::capture(&model.screen);
        let (cols, rows) = (captured.cols, captured.rows);
        if self.grid {
            let mut replay = model;
            out.push(Emit::Text(self.record_frame(cols, rows)));
            let line = replay.snapshot(true);
            out.push(Emit::Bin(line_bytes(&line)));
            self.output = Output::Grid(Box::new(replay));
        } else {
            let state = model.screen.replay_bytes(GRID_HISTORY);
            let mut sanitizer = Sanitizer::new();
            out.push(Emit::Text(self.record_frame(cols, rows)));
            let mut clean = Vec::with_capacity(state.len());
            sanitizer.push(&state, &mut clean);
            out.push(Emit::Bin(clean));
            self.output = Output::Bytes(sanitizer);
        }
        Ok(out)
    }

    /// Apply every buffered event (filling once first). Fast mode.
    fn page(&mut self) -> io::Result<Vec<Emit>> {
        self.fill()?;
        let mut out = Vec::new();
        let events = std::mem::take(&mut self.buffer);
        for Stamped { unix_ms, event } in &events[self.buffer_at.min(events.len())..] {
            if apply_event(&mut self.output, event, false, &mut out) {
                self.exited = true;
            }
            self.clock = *unix_ms;
        }
        self.buffer_at = 0;
        if let Output::Grid(replay) = &mut self.output
            && replay.screen.expire_sync()
            && let Some(line) = replay.diff()
        {
            out.push(Emit::Bin(line_bytes(&line)));
        }
        Ok(out)
    }

    /// Timestamp of the next unapplied event, filling if needed.
    fn peek(&mut self) -> io::Result<Option<u64>> {
        self.fill()?;
        Ok(self.buffered().first().map(|s| s.unix_ms))
    }

    /// Apply the next batch: buffered events within [`PLAY_COALESCE_MS`] of the
    /// first one. Paced playback.
    fn batch(&mut self) -> Vec<Emit> {
        let mut out = Vec::new();
        let Some(first) = self.buffered().first().map(|s| s.unix_ms) else {
            return out;
        };
        while self.buffer_at < self.buffer.len() {
            let Stamped { unix_ms, event } = &self.buffer[self.buffer_at];
            if unix_ms.saturating_sub(first) > PLAY_COALESCE_MS {
                break;
            }
            if apply_event(&mut self.output, event, false, &mut out) {
                self.exited = true;
            }
            self.clock = *unix_ms;
            self.buffer_at += 1;
        }
        if let Output::Grid(replay) = &mut self.output
            && replay.screen.expire_sync()
            && let Some(line) = replay.diff()
        {
            out.push(Emit::Bin(line_bytes(&line)));
        }
        out
    }

    /// At the end of playback (byte mode): flush what the sanitizer holds back and
    /// reset the viewer's modes (mouse tracking, bracketed paste, hidden cursor…)
    /// so a program that died mid-screen leaves xterm.js usable. The screen
    /// content stays, so scrubbing afterwards starts from what is shown.
    fn flush_tail(&mut self) -> Vec<Emit> {
        match &mut self.output {
            Output::Bytes(sanitizer) => {
                let mut tail = Vec::new();
                sanitizer.finish(&mut tail);
                tail.extend_from_slice(VIEWER_RESET);
                vec![Emit::Bin(tail)]
            }
            Output::Grid(_) => Vec::new(),
        }
    }

    fn finished(&self, live: bool) -> bool {
        self.exited || (self.at_end && self.buffer_at >= self.buffer.len() && !live)
    }
}

/// Browser → server control while replaying.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Control {
    Seek(u64),
    Play(f64),
    Pause,
    Live,
    Closed,
}

fn parse_control(message: Message) -> Option<Control> {
    match message {
        Message::Close(_) => Some(Control::Closed),
        Message::Text(text) => {
            let value: Value = serde_json::from_str(&text).ok()?;
            match value.get("t").and_then(Value::as_str)? {
                "seek" => value
                    .get("unix_ms")
                    .and_then(Value::as_u64)
                    .map(Control::Seek),
                "play" => Some(Control::Play(
                    value
                        .get("speed")
                        .and_then(Value::as_f64)
                        .filter(|s| *s > 0.0 && s.is_finite())
                        .unwrap_or(1.0),
                )),
                "pause" => Some(Control::Pause),
                "live" => Some(Control::Live),
                _ => None,
            }
        }
        _ => None,
    }
}

async fn send_all(sender: &mut Sender, emits: Vec<Emit>) {
    for emit in emits {
        match emit {
            Emit::Text(value) => sender.text(value).await,
            Emit::Bin(bytes) => sender.binary(bytes).await,
        }
        if sender.failed {
            return;
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Everything as fast as possible, then follow the tail while live.
    Fast,
    Paused,
    /// Real-time pacing at speed × 1000.
    Playing(u64),
}

/// The whole playback: everything the async loop owns.
struct Session {
    sender: Sender,
    playback: Option<Playback>,
    mode: Mode,
    start_ms: u64,
    end_ms: u64,
    live: bool,
    host_pid: u64,
    /// `end` was sent for the current position (reset by seek/play).
    ended: bool,
}

impl Session {
    /// Run a blocking step on the playback state; `None` means the socket was
    /// already closed on error.
    async fn blocking<R: Send + 'static>(
        &mut self,
        step: impl FnOnce(&mut Playback) -> io::Result<R> + Send + 'static,
    ) -> Option<R> {
        let mut playback = self.playback.take()?;
        let joined = tokio::task::spawn_blocking(move || {
            let result = step(&mut playback);
            (playback, result)
        })
        .await;
        match joined {
            Ok((playback, Ok(value))) => {
                self.playback = Some(playback);
                Some(value)
            }
            Ok((playback, Err(err))) => {
                self.playback = Some(playback);
                let _ = err; // the socket is closed with 1011 by the caller
                None
            }
            Err(_) => None,
        }
    }

    fn clock(&self) -> u64 {
        self.playback
            .as_ref()
            .map(|p| p.clock)
            .unwrap_or(self.start_ms)
    }

    async fn send_clock(&mut self) {
        let clock = self.clock();
        let end = self.end_ms;
        self.sender
            .text(json!({"t": "clock", "unix_ms": clock, "end_ms": end}))
            .await;
    }

    /// Rebuild at `until` (None = latest) and send the opening frames.
    async fn rebuild(&mut self, until: Option<u64>) -> bool {
        let Some(out) = self.blocking(move |p| p.build(until)).await else {
            return false;
        };
        send_all(&mut self.sender, out).await;
        self.ended = false;
        self.send_clock().await;
        true
    }

    /// Apply a browser control; false when the connection must end.
    async fn control(&mut self, control: Control) -> bool {
        match control {
            Control::Closed => false,
            Control::Pause => {
                self.mode = Mode::Paused;
                true
            }
            Control::Seek(t) => {
                let t = t.clamp(self.start_ms, self.end_ms.max(self.start_ms));
                self.mode = Mode::Paused;
                self.rebuild(Some(t)).await
            }
            Control::Play(speed) => {
                let finished = self
                    .playback
                    .as_ref()
                    .map(|p| p.finished(self.live))
                    .unwrap_or(true);
                if finished {
                    // Playing past the end restarts from the beginning.
                    let start = self.start_ms;
                    if !self.rebuild(Some(start)).await {
                        return false;
                    }
                }
                self.mode = Mode::Playing((speed * 1000.0).round().max(1.0) as u64);
                true
            }
            Control::Live => {
                self.mode = Mode::Fast;
                self.rebuild(None).await
            }
        }
    }

    async fn finish_position(&mut self) {
        if self.ended {
            return;
        }
        if let Some(out) = self.blocking(|p| Ok(p.flush_tail())).await {
            send_all(&mut self.sender, out).await;
        }
        self.send_clock().await;
        self.sender.text(json!({"t": "end"})).await;
        self.ended = true;
        self.mode = Mode::Paused;
    }
}

/// Serve one recording over a WebSocket: fast replay of everything, then follow the
/// tail while live. Text frames `seek`/`play`/`pause`/`live` switch to scrubbing and
/// paced playback; `timeline` and `clock` frames keep the browser's slider in step.
async fn serve(
    dir: PathBuf,
    entry: RecordEntry,
    grid: bool,
    socket: WebSocket,
    shutdown: CancellationToken,
) {
    use futures_util::FutureExt;
    let (sink, mut incoming) = socket.split();
    let mut sender = Sender {
        sink,
        failed: false,
    };
    let bounds = {
        let dir = dir.clone();
        tokio::task::spawn_blocking(move || reader::bounds(&dir)).await
    };
    let Ok(Ok(Some((start_ms, end_ms)))) = bounds else {
        sender.close(1011, "record unreadable").await;
        return;
    };
    sender
        .text(json!({"t": "timeline", "start_ms": start_ms, "end_ms": end_ms, "live": entry.live}))
        .await;
    let mut session = Session {
        sender,
        playback: Some(Playback::new(dir, entry.clone(), grid, start_ms)),
        mode: Mode::Fast,
        start_ms,
        end_ms,
        live: entry.live,
        host_pid: entry.host_pid,
        ended: false,
    };
    if !session.rebuild(None).await {
        session.sender.close(1011, "record unreadable").await;
        return;
    }
    loop {
        if session.sender.failed {
            return;
        }
        // Controls that arrived meanwhile (the last one wins).
        let mut control: Option<Control> = None;
        while let Some(Some(message)) = incoming.next().now_or_never() {
            match message {
                Ok(message) => {
                    if let Some(c) = parse_control(message) {
                        control = Some(c);
                    }
                }
                Err(_) => return,
            }
        }
        if let Some(control) = control {
            if !session.control(control).await {
                session.sender.close(1011, "record unreadable").await;
                return;
            }
            continue;
        }
        match session.mode {
            Mode::Paused => {
                let control = tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => { session.sender.close(1001, "shutdown").await; return; }
                    message = incoming.next() => match message {
                        None | Some(Err(_)) => return,
                        Some(Ok(message)) => parse_control(message),
                    },
                };
                if let Some(control) = control
                    && !session.control(control).await
                {
                    session.sender.close(1011, "record unreadable").await;
                    return;
                }
            }
            Mode::Fast => {
                let Some(finished) = session.blocking(|p| Ok(p.finished(false))).await else {
                    session.sender.close(1011, "record unreadable").await;
                    return;
                };
                let live = session.live;
                if finished && !live || session.playback.as_ref().is_some_and(|p| p.exited) {
                    session.finish_position().await;
                    continue;
                }
                if finished {
                    // Live and caught up: wait for growth, a control or shutdown.
                    let control = tokio::select! {
                        biased;
                        _ = shutdown.cancelled() => { session.sender.close(1001, "shutdown").await; return; }
                        message = incoming.next() => match message {
                            None | Some(Err(_)) => return,
                            Some(Ok(message)) => parse_control(message),
                        },
                        _ = tokio::time::sleep(FOLLOW_INTERVAL) => None,
                    };
                    if let Some(control) = control {
                        if !session.control(control).await {
                            session.sender.close(1011, "record unreadable").await;
                            return;
                        }
                        continue;
                    }
                    session.live = host_alive(session.host_pid);
                    if !session.live {
                        // One more pass picks up what the host wrote before exiting.
                        let _ = session
                            .blocking(|p| {
                                p.at_end = false;
                                Ok(())
                            })
                            .await;
                    }
                }
                let Some(out) = session.blocking(|p| p.page()).await else {
                    session.sender.close(1011, "record unreadable").await;
                    return;
                };
                if !out.is_empty() {
                    send_all(&mut session.sender, out).await;
                    session.end_ms = session.end_ms.max(session.clock());
                    session.send_clock().await;
                }
            }
            Mode::Playing(speed_milli) => {
                let Some(next) = session.blocking(|p| p.peek()).await else {
                    session.sender.close(1011, "record unreadable").await;
                    return;
                };
                let Some(next) = next else {
                    let live = session.live;
                    let finished = session.playback.as_ref().is_some_and(|p| p.finished(live));
                    if finished {
                        session.finish_position().await;
                    } else {
                        // Caught up with a live recording: keep following in real time.
                        session.mode = Mode::Fast;
                    }
                    continue;
                };
                let gap_ms = next
                    .saturating_sub(session.clock())
                    .min(MAX_PLAY_GAP.as_millis() as u64);
                let wait = Duration::from_millis(gap_ms.saturating_mul(1000) / speed_milli.max(1));
                let control = tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => { session.sender.close(1001, "shutdown").await; return; }
                    message = incoming.next() => match message {
                        None | Some(Err(_)) => return,
                        Some(Ok(message)) => parse_control(message),
                    },
                    _ = tokio::time::sleep(wait) => None,
                };
                if let Some(control) = control {
                    if !session.control(control).await {
                        session.sender.close(1011, "record unreadable").await;
                        return;
                    }
                    continue;
                }
                let Some(out) = session.blocking(|p| Ok(p.batch())).await else {
                    session.sender.close(1011, "record unreadable").await;
                    return;
                };
                send_all(&mut session.sender, out).await;
                session.end_ms = session.end_ms.max(session.clock());
                session.send_clock().await;
            }
        }
    }
}

/// Serve one recording as sanitized bytes for xterm.js.
pub async fn stream(
    dir: PathBuf,
    entry: RecordEntry,
    socket: WebSocket,
    shutdown: CancellationToken,
) {
    serve(dir, entry, false, socket, shutdown).await
}

/// Grid-mode counterpart of [`stream`]: the recording is replayed through the
/// terminal model and the browser receives grid JSON lines.
pub async fn stream_grid(
    dir: PathBuf,
    entry: RecordEntry,
    socket: WebSocket,
    shutdown: CancellationToken,
) {
    serve(dir, entry, true, socket, shutdown).await
}

#[cfg(test)]
mod tests {
    use super::{RECORDS_SUBDIR, list, remove_agent_leftovers, valid_id};

    #[test]
    fn agent_leftovers_go_and_shell_recordings_stay() {
        let root = tempfile::tempdir().unwrap();
        let records = root.path().join(RECORDS_SUBDIR);
        let write = |id: &str, meta: serde_json::Value| {
            let dir = records.join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("meta.json"), meta.to_string()).unwrap();
        };
        // Exited: `ended_ms` set, host pid 0, on every platform.
        let exited = |name: &str, source: Option<&str>| {
            let mut meta = serde_json::json!({"name": name, "host_pid": 0, "created_ms": 1,
                "ended_ms": 2, "cols": 80, "rows": 24, "argv": [], "meta": {}});
            if let Some(source) = source {
                meta["meta"]["source"] = serde_json::json!(source);
            }
            meta
        };
        write("1-0-shell", exited("shell", Some("shell")));
        write("2-0-claude", exited("claude", Some("claude")));
        write("3-0-codex", exited("codex", Some("codex")));
        write("4-0-manual", exited("manual", None));
        // Still running (no exit recorded): never touched, whatever its source.
        let mut live = exited("live", Some("claude"));
        live["host_pid"] = serde_json::json!(std::process::id());
        live.as_object_mut().unwrap().remove("ended_ms");
        write("5-1-live", live);
        assert_eq!(list(root.path()).unwrap().len(), 5);

        assert_eq!(remove_agent_leftovers(root.path()).unwrap(), 2);
        let left: Vec<String> = list(root.path())
            .unwrap()
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(left, ["5-1-live", "4-0-manual", "1-0-shell"]);
        assert_eq!(remove_agent_leftovers(root.path()).unwrap(), 0);
        assert_eq!(
            remove_agent_leftovers(&root.path().join("absent")).unwrap(),
            0
        );
    }

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
