//! Bounded per-file row summaries (batch 34, WP-A).
//!
//! One native file is summarized from exactly two bounded reads — the first
//! [`HEAD_BYTES`] (at most [`CLAUDE_HEAD_LINES`] / [`CODEX_HEAD_LINES`]
//! newline-separated pieces) and the last [`TAIL_BYTES`] (complete records
//! only; a partial first line is dropped) — the same regions the Python
//! adapters' `_head_lines` / `_tail_lines` read. Every derived field follows
//! the corresponding `list_sessions` rule of the reference adapter; the
//! helpers in this module reproduce the Python string primitives those rules
//! depend on (`_norm_ts`, `_iso`, `_clip`, `_title_from_text`, `_is_injected`,
//! `_claude_bash_input`, `_claude_bash_output`, `_flatten_content`).
//!
//! Nothing here opens files: `summarize` is a pure function of the bytes the
//! index read, so each derivation is unit-testable without I/O.

pub(super) mod claude;
pub(super) mod codex;
pub(super) mod grok;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;

use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use regex::Regex;
use serde_json::Value;

use super::Stamp;
use crate::sessions::SessionError;
use crate::sessions::providers::Skipped;

/// Python `HEAD_BYTES`: the metadata head read never exceeds this.
pub const HEAD_BYTES: u64 = 96 * 1024;
/// Python `TAIL_BYTES`: rename/custom-title records are appended at the end.
pub const TAIL_BYTES: u64 = 512 * 1024;
/// Python `_head_lines(path)` default: Claude reads 40 pieces.
pub const CLAUDE_HEAD_LINES: usize = 40;
/// Python `CodexAdapter._raw_meta` reads 120 pieces.
pub const CODEX_HEAD_LINES: usize = 120;
/// Python `_first_jsonl_timestamp`: a Claude sidecar's created time comes
/// from its first 8 pieces.
pub const CLAUDE_AGENT_CREATED_LINES: usize = 8;
/// The bytes the index read from one data file, plus the stamp of the file
/// version those bytes belong to.
pub struct DataFile<'a> {
    /// First `min(size, HEAD_BYTES)` bytes.
    pub head: &'a [u8],
    /// Bytes `[tail_start, size)`; equals the whole file when it fits.
    pub tail: &'a [u8],
    pub tail_start: u64,
    pub stamp: Stamp,
}

/// A sidecar (`agent-*.meta.json`, `summary.json`) as read by the index.
pub enum SidecarBytes<'a> {
    Bytes {
        bytes: &'a [u8],
        stamp: Stamp,
    },
    /// Present but unreadable or over budget; the row says why.
    Failed {
        stamp: Option<Stamp>,
        reason: String,
    },
}

pub struct Input<'a> {
    pub source: &'static str,
    /// Display path: the JSONL file (Claude/Codex) or the session directory (Grok).
    pub path: &'a Path,
    /// Absent only for a Grok session whose `chat_history.jsonl` does not exist.
    pub data: Option<DataFile<'a>>,
    pub sidecar: Option<SidecarBytes<'a>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexMeta {
    /// A `session_meta` header was seen (today's rows carry
    /// `forked_from_id`/`history_base` only then).
    pub has_meta: bool,
    /// Python `str(meta.get("forked_from_id") or "")`.
    pub forked_from_id: String,
    /// `history_base` when it is an object, otherwise `null`.
    pub history_base: Value,
    /// Subagent rollouts only: `parent_thread_id` / spawn parent / fork parent.
    pub parent_thread_id: String,
}

/// Agent labels for Claude sidecar files and Codex subagent rollouts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentMeta {
    pub id: String,
    pub title: String,
    pub kind: String,
    /// The agent's last turn is not closed (Python `_claude_agent_tail` /
    /// `_codex_agent_tail`): Claude — the last user/assistant record is not
    /// an assistant `end_turn`; Codex — the last turn-boundary `event_msg` is
    /// `task_started`/`turn_started`. A Codex item is `active` exactly then;
    /// a Claude item also needs the owner's stop notices (`agent_stops`).
    pub open_turn: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrokMeta {
    pub chat_exists: bool,
}

/// Everything a session-list row, the ownership graph and the native catalog
/// need from one file. A few hundred bytes; never a message object.
#[derive(Clone, Debug)]
pub struct RowSummary {
    pub sid: String,
    pub title: String,
    pub cwd: String,
    pub created: String,
    pub updated: String,
    pub size: u64,
    pub model: Value,
    pub branch: Value,
    pub codex: Option<CodexMeta>,
    pub agent: Option<AgentMeta>,
    pub grok: Option<GrokMeta>,
    /// Claude main transcripts: the last tail `continued-in` record's
    /// `continuedInSessionId` (Python `continued_in_sid`); the graph turns
    /// it into the row's `continued_in` uid when that sid is indexed.
    pub continued_in_sid: Option<String>,
    /// Native session identity from the records seen (scope rules of
    /// `sessions::scope::native_identity` applied to head + tail).
    pub native_id: Result<String, SessionError>,
    pub declared_ids: Vec<String>,
    /// A hard failure visible in the head/tail; the row is `supported:false`
    /// and `migration_warnings == [reason]`.
    pub unsupported: Option<String>,
    /// Non-fatal notes (unknown record kinds skipped, batch-33 wording).
    pub warnings: Vec<String>,
    /// Offset after the last LF of the data file when the tail showed it.
    pub committed: Option<u64>,
    /// `rs-m2-1` physical head hash of the committed prefix (the `head`
    /// field of a message cursor), when `committed` is known.
    pub cursor_head: Option<String>,
}

impl RowSummary {
    fn blank(sid: String, title: String, cwd: String, at: String) -> Self {
        Self {
            sid,
            title,
            cwd,
            created: at.clone(),
            updated: at,
            size: 0,
            model: Value::Null,
            branch: Value::Null,
            codex: None,
            agent: None,
            grok: None,
            continued_in_sid: None,
            native_id: Err(SessionError::new(
                501,
                "原生记录缺少明确会话 ID，不能用文件名或显示名称推断",
            )),
            declared_ids: Vec::new(),
            unsupported: None,
            warnings: Vec::new(),
            committed: None,
            cursor_head: None,
        }
    }

    pub fn supported(&self) -> bool {
        self.unsupported.is_none()
    }

    /// Public `migration_warnings`: the hard reason alone, or the notes.
    pub fn migration_warnings(&self) -> Vec<String> {
        match &self.unsupported {
            Some(reason) => vec![reason.clone()],
            None => self.warnings.clone(),
        }
    }
}

/// Summarize one candidate from the bytes the index read.
pub fn summarize(input: &Input<'_>) -> RowSummary {
    match input.source {
        "claude" => claude::summarize(input),
        "codex" => codex::summarize(input),
        _ => grok::summarize(input),
    }
}

// ---------------------------------------------------------------------------
// Head/tail record parsing (Python `_head_lines` / `_tail_lines` regions).
// ---------------------------------------------------------------------------

/// One complete JSON object record with its physical start offset.
pub struct Record {
    pub start: u64,
    /// Newline-separated piece index inside its region (Python line limits
    /// count pieces, blank ones included).
    pub line: usize,
    pub value: Value,
}

#[derive(Default)]
pub struct Region {
    pub records: Vec<Record>,
    /// Start offsets of complete lines that are not valid JSON objects.
    pub corrupt: Vec<u64>,
}

/// Python `_head_lines`: the first `limit` newline-separated pieces of the
/// head blob. Blank pieces count toward the limit but produce nothing; an
/// unterminated final piece (cut at `HEAD_BYTES` or still being appended) is
/// never decoded.
pub fn parse_head(blob: &[u8], limit: usize) -> Region {
    let mut region = Region::default();
    for (index, line) in split_lines(blob, 0).enumerate() {
        if index >= limit {
            break;
        }
        decode_line(&line, index, &mut region);
    }
    region
}

/// Python `_tail_lines`: every complete line of the tail blob; when the tail
/// starts inside the file its first piece is a partial record and dropped.
pub fn parse_tail(blob: &[u8], tail_start: u64) -> Region {
    let mut region = Region::default();
    for (index, line) in split_lines(blob, tail_start).enumerate() {
        if index == 0 && tail_start > 0 {
            continue;
        }
        decode_line(&line, index, &mut region);
    }
    region
}

struct Line<'a> {
    start: u64,
    bytes: &'a [u8],
    terminated: bool,
}

fn split_lines(blob: &[u8], base: u64) -> impl Iterator<Item = Line<'_>> {
    let mut offset = 0usize;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        let rest = &blob[offset..];
        let start = base + offset as u64;
        match rest.iter().position(|byte| *byte == b'\n') {
            Some(position) => {
                offset += position + 1;
                Some(Line {
                    start,
                    bytes: &rest[..position],
                    terminated: true,
                })
            }
            None => {
                done = true;
                Some(Line {
                    start,
                    bytes: rest,
                    terminated: false,
                })
            }
        }
    })
}

fn decode_line(line: &Line<'_>, index: usize, region: &mut Region) {
    if !line.terminated {
        return;
    }
    let trimmed = line.bytes.trim_ascii();
    if trimmed.is_empty() {
        return;
    }
    match crate::sessions::records::decode_record(trimmed) {
        Ok(value) if value.is_object() => region.records.push(Record {
            start: line.start,
            line: index,
            value,
        }),
        _ => region.corrupt.push(line.start),
    }
}

/// Both regions of one file, iterated in file order without duplicates
/// (a small file's tail repeats its head).
pub struct Records {
    pub head: Region,
    pub tail: Region,
}

impl Records {
    pub fn parse(data: &DataFile<'_>, head_limit: usize) -> Self {
        Self {
            head: parse_head(data.head, head_limit),
            tail: parse_tail(data.tail, data.tail_start),
        }
    }

    pub fn empty() -> Self {
        Self {
            head: Region::default(),
            tail: Region::default(),
        }
    }

    /// Every record seen, in file order, each physical line once.
    pub fn all(&self) -> impl Iterator<Item = &Record> {
        let seen: BTreeSet<u64> = self.head.records.iter().map(|r| r.start).collect();
        self.head.records.iter().chain(
            self.tail
                .records
                .iter()
                .filter(move |record| !seen.contains(&record.start)),
        )
    }

    /// Complete lines seen that are not JSON objects. Skipped like Python
    /// `_head_lines`/`_tail_lines`; `skipped_warnings` reports the count.
    pub fn corrupt_lines(&self) -> usize {
        self.head
            .corrupt
            .iter()
            .chain(&self.tail.corrupt)
            .collect::<BTreeSet<_>>()
            .len()
    }
}

/// Offset after the last LF when the tail shows one (or the file is empty).
pub fn committed_end(data: &DataFile<'_>) -> Option<u64> {
    if data.stamp.size == 0 {
        return Some(0);
    }
    data.tail
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|position| data.tail_start + position as u64 + 1)
}

const CURSOR_SCHEMA: &str = "rs-m2-1";

/// Same value as the message cursor's `head` for a view committed at `end`.
pub fn cursor_head(data: &DataFile<'_>, end: u64) -> Option<String> {
    let take = usize::try_from(end.min(4096)).ok()?;
    let bytes = data.head.get(..take)?;
    Some(format!(
        "{CURSOR_SCHEMA}:{}",
        &crate::sessions::hash(bytes)[..16]
    ))
}

/// The scope rules of `sessions::scope::native_identity` over the records
/// seen. `extract` yields the declared id value of one record, if any.
pub fn native_identity<'a>(
    records: impl Iterator<Item = &'a Value>,
) -> (Result<String, SessionError>, Vec<String>) {
    let mut identity: Option<String> = None;
    let mut declared = BTreeSet::new();
    let mut error = None;
    for value in records {
        let Some(id) = value.as_str().filter(|id| {
            !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
        }) else {
            error.get_or_insert_with(|| {
                SessionError::new(501, "原生会话 ID 无效，不能确定操作范围")
            });
            continue;
        };
        declared.insert(id.to_owned());
        if identity.as_deref().is_some_and(|previous| previous != id) {
            error.get_or_insert_with(|| SessionError::new(409, "原生记录包含冲突的会话 ID"));
        }
        identity.get_or_insert_with(|| id.to_owned());
    }
    let identity = match error {
        Some(error) => Err(error),
        None => identity.ok_or_else(|| {
            SessionError::new(501, "原生记录缺少明确会话 ID，不能用文件名或显示名称推断")
        }),
    };
    (identity, declared.into_iter().collect())
}

// ---------------------------------------------------------------------------
// Python value primitives.
// ---------------------------------------------------------------------------

/// Python truthiness of a JSON value.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

/// Python `str(value)` for the scalar shapes that reach a row field.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

/// `value` when truthy, as text.
pub fn text_if_truthy(value: &Value) -> Option<String> {
    truthy(value).then(|| py_str(value))
}

/// Python `str.isspace` (Unicode White_Space plus the C0 separators 1C–1F).
pub fn py_is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

pub fn py_strip(text: &str) -> &str {
    text.trim_matches(py_is_space)
}

/// Python `str.splitlines()` boundaries.
pub fn py_splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        let boundary = match c {
            '\n' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => Some(index + c.len_utf8()),
            '\r' => {
                let mut next = index + 1;
                if chars.peek().is_some_and(|(_, c)| *c == '\n') {
                    chars.next();
                    next += 1;
                }
                Some(next)
            }
            _ => None,
        };
        if let Some(next) = boundary {
            lines.push(&text[start..index]);
            start = next;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Python `_clip`: whitespace-normalized, at most `n` characters plus `…`.
pub fn clip(text: &str, n: usize) -> String {
    let joined = text
        .split(py_is_space)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if joined.chars().count() > n {
        joined.chars().take(n).collect::<String>() + "…"
    } else {
        joined
    }
}

/// Python `_norm_ts`: a native timestamp as an RFC 3339 UTC instant with
/// millisecond precision, or `None` when it is falsy or unparsable.
pub fn norm_ts(value: &Value) -> Option<String> {
    if !truthy(value) {
        return None;
    }
    let instant = match value {
        Value::Number(number) => {
            let number = number.as_f64()?;
            let seconds = if number > 1e11 {
                number / 1000.0
            } else {
                number
            };
            let micros = (seconds * 1e6).round();
            if !micros.is_finite() || micros.abs() > i64::MAX as f64 {
                return None;
            }
            DateTime::from_timestamp_micros(micros as i64)?
        }
        Value::String(text) => parse_iso(&text.replace('Z', "+00:00"))?,
        _ => return None,
    };
    Some(instant.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// The subset of `datetime.fromisoformat` shapes native records use.
fn parse_iso(text: &str) -> Option<DateTime<Utc>> {
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Y-%m-%d %H:%M:%S%.f%:z",
        "%Y-%m-%dT%H:%M:%S%.f%z",
        "%Y-%m-%d %H:%M:%S%.f%z",
        "%Y-%m-%dT%H:%M%:z",
        "%Y-%m-%d %H:%M%:z",
    ] {
        if let Ok(parsed) = DateTime::parse_from_str(text, format) {
            return Some(parsed.to_utc());
        }
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(text, format) {
            return Some(parsed.and_utc());
        }
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|date| date.and_utc())
}

/// Python `_iso(st.st_mtime)`: file time truncated to whole seconds.
pub fn iso_seconds(mtime_ns: u128) -> String {
    let seconds = i64::try_from(mtime_ns / 1_000_000_000).unwrap_or(i64::MAX);
    DateTime::from_timestamp(seconds, 0)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

// ---------------------------------------------------------------------------
// Python text primitives used by title derivation.
// ---------------------------------------------------------------------------

static INJECTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)AGENTS\.md instructions|<INSTRUCTIONS>|<user_info>|<environment_context>|<system-reminder>|<command-name>|Caveat: The messages below|<local-command-(?:caveat|stdout)>|<task-notification>|This session is being continued from a previous conversation|# Global User Guidance|<project_instructions>|<user_instructions>",
    )
    .expect("static regex")
});

static TIMELINE_PROTOCOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(?:#\s*AGENTS\.md instructions|#\s*Global User Guidance|<INSTRUCTIONS>|<user_info>|<environment_context>|<system-reminder>|<command-name>|<local-command-(?:caveat|stdout)>|<task-notification>|<project_instructions>|<user_instructions>|Caveat:\s*The messages below were generated by the user while running local commands|This session is being continued from a previous conversation)",
    )
    .expect("static regex")
});

static BASH_INPUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)^\s*<bash-input>(.*?)</bash-input>\s*$").expect("static regex")
});

static PAIRED_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<(command-[a-z-]+|system-reminder|local-command[a-z-]*)>").expect("static regex")
});

static SHORT_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[^>]{1,40}>").expect("static regex"));

/// Python `_is_injected`: searches the first 2000 characters.
pub fn is_injected(text: &str) -> bool {
    let window: String = text.chars().take(2000).collect();
    INJECTED.is_match(&window)
}

/// Python `_is_timeline_protocol`: a CLI protocol block at the very start.
pub fn is_timeline_protocol(text: &str) -> bool {
    TIMELINE_PROTOCOL.is_match(text)
}

/// Python `_claude_bash_input`.
pub fn claude_bash_input(text: &str) -> Option<String> {
    let captures = BASH_INPUT.captures(text)?;
    let decoded = html_escape::decode_html_entities(captures.get(1).map_or("", |m| m.as_str()));
    let command = py_strip(&decoded);
    if command.is_empty() {
        return None;
    }
    Some(if command.starts_with('!') {
        command.to_owned()
    } else {
        format!("! {command}")
    })
}

/// Whether Python `_claude_bash_output` returns a value: only an exact
/// sequence of `<bash-stdout>`/`<bash-stderr>` envelopes (case-insensitive,
/// paired with their own closing tag) and nothing but whitespace around them.
pub fn claude_bash_output_matches(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut spans = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        let Some(relative) = lower[position..].find("<bash-std") else {
            break;
        };
        let start = position + relative;
        let stream = if lower[start..].starts_with("<bash-stdout>") {
            "stdout"
        } else if lower[start..].starts_with("<bash-stderr>") {
            "stderr"
        } else {
            position = start + 1;
            continue;
        };
        let body_start = start + "<bash-stdout>".len();
        let closing = format!("</bash-{stream}>");
        match lower[body_start..].find(&closing) {
            Some(offset) => {
                let end = body_start + offset + closing.len();
                spans.push((start, end));
                position = end;
            }
            None => position = start + 1,
        }
    }
    if spans.is_empty() {
        return false;
    }
    let mut remaining = String::new();
    let mut cursor = 0;
    for (start, end) in spans {
        remaining.push_str(&text[cursor..start]);
        cursor = end;
    }
    remaining.push_str(&text[cursor..]);
    py_strip(&remaining).is_empty()
}

fn strip_paired_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut position = 0;
    while let Some(captures) = PAIRED_TAG.captures_at(text, position) {
        let opening = captures.get(0).expect("whole match");
        let name = captures.get(1).map_or("", |m| m.as_str());
        let closing = format!("</{name}>");
        match text[opening.end()..].find(&closing) {
            Some(offset) => {
                out.push_str(&text[position..opening.start()]);
                out.push(' ');
                position = opening.end() + offset + closing.len();
            }
            None => {
                let step = text[opening.start()..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
                out.push_str(&text[position..opening.start() + step]);
                position = opening.start() + step;
            }
        }
    }
    out.push_str(&text[position..]);
    out
}

/// Python `_title_from_text`.
pub fn title_from_text(text: &str) -> String {
    let text = strip_paired_tags(text);
    let text = SHORT_TAG.replace_all(&text, " ");
    for line in py_splitlines(&text) {
        let line = py_strip(line);
        if line.chars().count() >= 4
            && !(line.starts_with('#')
                || line.starts_with('-')
                || line.starts_with('*')
                || line.starts_with("```"))
        {
            return clip(line, 90);
        }
    }
    let fallback = clip(&text, 90);
    if fallback.is_empty() {
        "(无标题)".to_owned()
    } else {
        fallback
    }
}

/// Python `_flatten_content` restricted to `kind == "text"` parts, joined
/// with newlines. Shapes the reference adapter cannot read either (a scalar
/// `content`, a non-string `text`) are the same hard failures as today.
pub fn flatten_text(content: &Value) -> Result<String, String> {
    let mut parts = Vec::new();
    match content {
        Value::Null => {}
        Value::String(text) => parts.push(text.clone()),
        Value::Object(_) => push_text_block(content, &mut parts)?,
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(text) => parts.push(text.clone()),
                    Value::Object(_) => push_text_block(item, &mut parts)?,
                    _ => {}
                }
            }
        }
        _ => return Err("原生 content 类型无效".to_owned()),
    }
    Ok(parts.join("\n"))
}

fn push_text_block(block: &Value, parts: &mut Vec<String>) -> Result<(), String> {
    if matches!(
        block["type"].as_str(),
        Some("text" | "input_text" | "output_text" | "summary_text")
    ) {
        match &block["text"] {
            Value::Null => parts.push(String::new()),
            Value::String(text) => parts.push(text.clone()),
            _ => return Err("原生文本块的 text 必须是字符串".to_owned()),
        }
    }
    Ok(())
}

/// Python `_is_codex_protocol_injection("user", text, native_meta)`.
pub fn is_codex_protocol_injection(text: &str, native_meta: &Value) -> bool {
    native_meta["content_item_kinds"]
        .as_array()
        .is_some_and(|kinds| kinds.iter().any(|kind| kind == "goal.internal_context"))
        || is_timeline_protocol(text)
}

/// `urllib.parse.unquote`: percent UTF-8 decoding with replacement for invalid
/// bytes, malformed escapes retained, `+` literal.
pub fn unquote(name: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    let bytes = name.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%'
            && offset + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[offset + 1]), hex(bytes[offset + 2]))
        {
            decoded.push((high << 4) | low);
            offset += 3;
        } else {
            decoded.push(bytes[offset]);
            offset += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Last record (file order) whose timestamp normalizes, tail first, then the
/// head as the bounded stand-in for Python's backward whole-file scan.
pub fn latest_timestamp(records: &Records) -> Option<String> {
    records
        .tail
        .records
        .iter()
        .rev()
        .chain(records.head.records.iter().rev())
        .find_map(|record| norm_ts(&record.value["timestamp"]))
}

/// The record-kind skip notes today's projection would produce for these
/// records (content-block notes need the projection and are not derived),
/// then the count of complete lines that were not JSON objects.
pub fn skipped_warnings(source: &str, records: &Records) -> Vec<String> {
    let mut skipped = Skipped::default();
    for record in records.all() {
        let value = &record.value;
        let kind = value["type"].as_str().unwrap_or("");
        match source {
            "claude" => match kind {
                "user"
                | "assistant"
                | "system"
                | "queue-operation"
                | "custom-title"
                | "last-prompt"
                | "ai-title"
                | "file-history-snapshot"
                | "progress"
                | "summary" => {}
                "attachment" => {
                    let attachment = value["attachment"]["type"].as_str().unwrap_or("");
                    if ![
                        "queued_command",
                        "compact_file_reference",
                        "total_tokens_reminder",
                    ]
                    .contains(&attachment)
                    {
                        skipped.note("Claude attachment 类型", attachment);
                    }
                }
                other => skipped.note("Claude 记录类型", other),
            },
            "codex" => match kind {
                "session_meta" | "turn_context" | "compacted" => {}
                "event_msg" => match value["payload"]["type"].as_str().unwrap_or("") {
                    "task_started" | "task_complete" | "turn_aborted" | "user_message"
                    | "agent_message" | "agent_reasoning" | "token_count" => {}
                    other => skipped.note("Codex event_msg", other),
                },
                "response_item" => match value["payload"]["type"].as_str().unwrap_or("") {
                    "message"
                    | "reasoning"
                    | "function_call"
                    | "custom_tool_call"
                    | "local_shell_call"
                    | "function_call_output"
                    | "custom_tool_call_output"
                    | "local_shell_call_output"
                    | "web_search_call"
                    | "tool_search_call" => {}
                    other => skipped.note("Codex response_item", other),
                },
                other => skipped.note("Codex 记录类型", other),
            },
            _ => match kind {
                "reasoning" | "tool_result" | "user" | "assistant" | "system" => {}
                other => skipped.note("Grok 记录类型", other),
            },
        }
    }
    let mut warnings = skipped.warnings();
    warnings.extend(crate::sessions::records::invalid_lines_warning(
        records.corrupt_lines(),
    ));
    warnings
}

#[cfg(test)]
mod tests;
