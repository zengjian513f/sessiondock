//! Bounded per-file row summaries.
//!
//! One native file is summarized from exactly two bounded reads — the first
//! [`HEAD_BYTES`] (at most [`CLAUDE_HEAD_LINES`] / [`CODEX_HEAD_LINES`]
//! newline-separated pieces) and the last [`TAIL_BYTES`] (complete records
//! only; a partial first line is dropped) — the same
//! head and tail regions. Every derived field follows
//! the corresponding `list_sessions` rule of the reference adapter; the
//! helpers in this module reproduce the string primitives those rules
//! depend on (`_norm_ts`, `_iso`, `_clip`, `_title_from_text`, `_is_injected`,
//! `_claude_bash_input`, `_claude_bash_output`, `_flatten_content`).
//!
//! Nothing here opens files: `summarize` is a pure function of the bytes the
//! index read, so each derivation is unit-testable without I/O.

pub(crate) mod agy;
pub(super) mod claude;
pub(super) mod codex;
pub(super) mod grok;
pub mod kiro;
pub(crate) mod opencode;

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

use super::Stamp;
use crate::sessions::SessionError;
use crate::sessions::providers::Skipped;
pub(crate) use crate::sessions::scope::declared_identity as native_identity;
pub use crate::sessions::text::{
    claude_bash_input, claude_bash_output_matches, clip, flatten_text, is_codex_protocol_injection,
    is_injected, iso_seconds, norm_ts, py_str, py_strip, text_if_truthy, title_from_text, truthy,
    unquote,
};

/// The metadata head read never exceeds this.
pub const HEAD_BYTES: u64 = 96 * 1024;
/// Rename/custom-title records are appended at the end.
pub const TAIL_BYTES: u64 = 512 * 1024;
/// Claude reads 40 pieces.
pub const CLAUDE_HEAD_LINES: usize = 40;
/// Codex reads 120 pieces.
pub const CODEX_HEAD_LINES: usize = 120;
/// A Claude sidecar's created time comes
/// from its first 8 pieces.
pub const CLAUDE_AGENT_CREATED_LINES: usize = 8;
/// The bytes the index read from one data file, plus the stamp of the file
/// version those bytes belong to.
#[derive(Clone, Copy)]
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
    /// Ordinal of the first native session_meta (rollout generation boundary).
    pub start_ordinal: Option<u64>,
    /// A `session_meta` header was seen (today's rows carry
    /// `forked_from_id`/`history_base` only then).
    pub has_meta: bool,
    /// `forked_from_id` as text, empty when absent.
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
    /// Mirrored native parent SID (Agy); other providers use their own provenance.
    pub parent_sid: Option<String>,
    pub title: String,
    pub kind: String,
    /// The agent's last turn is not closed:
    /// Claude — the last user/assistant record is not
    /// an assistant `end_turn`; Codex — the last turn-boundary `event_msg` is
    /// `task_started`/`turn_started`. Both sources also reconcile the
    /// owner's explicit stop evidence (`agent_stops`).
    pub open_turn: bool,
    /// Codex's latest turn boundary, unaffected by settings/token records.
    pub turn_at: Option<String>,
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
    /// `continuedInSessionId`; the graph turns
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
    /// Claude/Codex main transcripts: the state of the latest turn as the
    /// tail shows it (`working`/`waiting`/`idle`/`aborted`/`failed`, the
    /// conversation's status vocabulary); `None` when the tail decides
    /// nothing or the source has no rule (docs/read-model.md).
    pub turn: Option<&'static str>,
    /// Claude main transcripts: background tasks (Monitor watches and
    /// backgrounded Bash commands) the tail shows started and not yet
    /// ended; the main turn waits on them (docs/read-model.md).
    pub background: usize,
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
            background: 0,
            turn: None,
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
        "opencode" => opencode::summarize(input),
        "agy" => agy::summarize(input),
        "kiro" => kiro::summarize(input),
        _ => grok::summarize(input),
    }
}

// ---------------------------------------------------------------------------
// Head/tail record parsing (head / tail regions).
// ---------------------------------------------------------------------------

/// One complete JSON object record with its physical start offset.
pub struct Record {
    pub start: u64,
    /// Newline-separated piece index inside its region (line limits
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

/// The first `limit` newline-separated pieces of the
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

/// Every complete line of the tail blob; when the tail
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
    match crate::sessions::records::decode_summary_record(trimmed) {
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

    /// Complete lines seen that are not JSON objects. Skipped by the
    /// head/tail line readers; `skipped_warnings` reports the count.
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

/// Last record (file order) whose timestamp normalizes, tail first, then the
/// head as the bounded stand-in for the backward whole-file scan.
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
            "agy" => match kind {
                "USER_INPUT" | "PLANNER_RESPONSE" | "ERROR_MESSAGE" | "SYSTEM_MESSAGE"
                | "CHECKPOINT" => {}
                _ if value["content"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty())
                    || value["error"]
                        .as_str()
                        .is_some_and(|text| !text.trim().is_empty())
                    || value["media"]
                        .as_array()
                        .is_some_and(|media| !media.is_empty()) => {}
                other => skipped.note("Agy 记录类型", other),
            },
            "opencode" => match kind {
                "user" | "assistant" | "synthetic" | "idle" | "model-switched"
                | "agent-switched" | "location-switched" | "compaction" => {}
                other => skipped.note("OpenCode 记录类型", other),
            },
            "kiro" => {
                let kiro_kind = value
                    .get("kind")
                    .or_else(|| value.get("message").and_then(|m| m.get("kind")))
                    .or_else(|| value.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                match kiro_kind {
                    "user" | "assistant" | "tool_call" | "tool_result" | "session_start"
                    | "error" | "tombstone" | "system" | "checkpoint" | "Prompt"
                    | "AssistantMessage" | "ToolResults" | "System" | "Metadata" | "" => {}
                    other => skipped.note("Kiro 记录类型", other),
                }
            }
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
