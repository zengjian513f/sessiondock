//! Scalar Codex activity/turn and Claude/Codex model state, independent of head/tail windows.
//! Cold reads walk backwards to the latest activity/model/turn boundary; appends
//! inspect only new complete lines. No message projection is retained.

use std::{borrow::Cow, io};

use serde::Deserialize;
use serde_json::Value;

use super::{DataFile, Stamp, StampedSource, TAIL_BYTES};

#[derive(Clone)]
pub(super) struct Scan {
    stamp: Stamp,
    committed: u64,
    pub turn: Option<&'static str>,
    pub turn_at: Option<String>,
    pub model: Value,
    pub activity_at: Option<String>,
}

#[derive(Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    timestamp: Value,
    #[serde(default)]
    payload: Payload,
    #[serde(default)]
    message: Payload,
    #[serde(rename = "isSidechain", default)]
    sidechain: Value,
}

#[derive(Default, Deserialize)]
struct Payload {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    error: Value,
    #[serde(default)]
    model: Value,
    #[serde(default)]
    role: String,
}

impl Record {
    fn codex_activity(&self) -> bool {
        match self.kind.as_str() {
            "response_item" => match self.payload.kind.as_str() {
                "message" => matches!(
                    self.payload.role.as_str(),
                    "" | "user" | "assistant" | "tool"
                ),
                "reasoning"
                | "function_call"
                | "function_call_output"
                | "custom_tool_call"
                | "custom_tool_call_output"
                | "local_shell_call"
                | "local_shell_call_output"
                | "web_search_call"
                | "tool_search_call" => true,
                _ => false,
            },
            "event_msg" => matches!(
                self.payload.kind.as_str(),
                "task_started"
                    | "turn_started"
                    | "task_complete"
                    | "turn_complete"
                    | "turn_aborted"
                    | "user_message"
                    | "agent_message"
                    | "agent_reasoning"
            ),
            "compacted" => true,
            _ => false,
        }
    }
}

pub(super) fn read(
    data: &DataFile<'_>,
    mut open: impl FnMut() -> io::Result<std::fs::File>,
    previous: Option<&Scan>,
    source: &str,
    sidecar: bool,
) -> io::Result<Scan> {
    let stamp = data.stamp;
    // Same-size rewrites, truncations and inode changes must not inherit state.
    let previous = previous.filter(|scan| {
        (scan.stamp.dev, scan.stamp.ino) == (stamp.dev, stamp.ino)
            && (scan.stamp == stamp || scan.stamp.size < stamp.size)
    });
    let floor = previous.map_or(0, |scan| scan.committed);
    let mut position = stamp.size;
    let mut committed = floor;
    let mut terminated = false;
    let mut pieces: Vec<Vec<u8>> = Vec::new();
    let mut asking = None;
    let mut boundary = None;
    let mut turn_at = None;
    let mut model = None;
    let mut activity_at = None;
    let mut opened: Option<std::fs::File> = None;
    'chunks: while position > floor {
        let start = position.saturating_sub(TAIL_BYTES).max(floor);
        let bytes: Cow<'_, [u8]> = if start >= data.tail_start {
            let offset = (start - data.tail_start) as usize;
            let len = (position - start) as usize;
            let end_offset = offset.saturating_add(len);
            if end_offset > data.tail.len() {
                return Err(io::Error::other("Native state scan changed during read"));
            }
            Cow::Borrowed(&data.tail[offset..end_offset])
        } else {
            let file = match &mut opened {
                Some(file) => file,
                None => {
                    let mut file = open()?;
                    if file.stamp() != Some(data.stamp) {
                        return Err(io::Error::other("Native state scan changed during read"));
                    }
                    opened.insert(file)
                }
            };
            let bytes = file.read_range(start, position - start)?;
            if bytes.len() as u64 != position - start {
                return Err(io::Error::other("Native state scan changed during read"));
            }
            Cow::Owned(bytes)
        };
        let mut end = bytes.len();
        loop {
            let lf = memchr::memrchr(b'\n', &bytes[..end]);
            let begin = lf.map_or(0, |offset| offset + 1);
            if terminated {
                if lf.is_some() || start == floor {
                    // Most records fit in this chunk: inspect those bytes in
                    // place. Join only records that cross chunk boundaries.
                    let raw = if pieces.is_empty() {
                        Cow::Borrowed(&bytes[begin..end])
                    } else {
                        pieces.push(bytes[begin..end].to_vec());
                        let raw: Vec<u8> = pieces.iter().rev().flatten().copied().collect();
                        pieces.clear();
                        Cow::Owned(raw)
                    };
                    if (memchr::memmem::find(&raw, b"event_msg").is_some()
                        || memchr::memmem::find(&raw, b"response_item").is_some()
                        || memchr::memmem::find(&raw, b"assistant").is_some()
                        || memchr::memmem::find(&raw, b"compacted").is_some()
                        || memchr::memmem::find(&raw, b"turn_context").is_some())
                        && let Ok(record) = serde_json::from_slice::<Record>(&raw)
                    {
                        // Resuming an already running thread can append settings
                        // and telemetry without a new turn. Those writes must not
                        // make simply opening its history change sidebar order.
                        if source == "codex" && activity_at.is_none() && record.codex_activity() {
                            activity_at = super::summary::norm_ts(&record.timestamp);
                        }
                        let seen = match source {
                            "codex" if record.kind == "turn_context" => &record.payload.model,
                            "claude"
                                if record.kind == "assistant"
                                    && (sidecar || !super::summary::truthy(&record.sidechain)) =>
                            {
                                &record.message.model
                            }
                            _ => &Value::Null,
                        };
                        if model.is_none() && crate::sessions::providers::valid_native_model(seen) {
                            model = Some(seen.clone());
                        }
                        if source == "codex" && boundary.is_none() && record.kind == "response_item"
                        {
                            asking.get_or_insert_with(|| {
                                matches!(
                                    record.payload.kind.as_str(),
                                    "function_call" | "custom_tool_call"
                                ) && crate::sessions::providers::question_tool(&record.payload.name)
                            });
                        } else if source == "codex"
                            && boundary.is_none()
                            && record.kind == "event_msg"
                        {
                            boundary = match record.payload.kind.as_str() {
                                "task_started" | "turn_started" => Some("working"),
                                "task_complete" | "turn_complete" => {
                                    Some(if super::summary::truthy(&record.payload.error) {
                                        "failed"
                                    } else {
                                        "idle"
                                    })
                                }
                                "turn_aborted" => Some("aborted"),
                                _ => None,
                            };
                            if boundary.is_some() {
                                turn_at = super::summary::norm_ts(&record.timestamp);
                            }
                        }
                        if model.is_some()
                            && (source != "codex" || (boundary.is_some() && activity_at.is_some()))
                        {
                            break 'chunks;
                        }
                    }
                } else {
                    pieces.push(bytes[begin..end].to_vec());
                }
            } else if let Some(lf) = lf {
                // Ignore the trailing partial line until its LF is appended.
                committed = start + lf as u64 + 1;
                terminated = true;
            }
            let Some(lf) = lf else { break };
            end = lf;
        }
        position = start;
    }
    let mut turn = boundary.or_else(|| previous.and_then(|scan| scan.turn));
    if matches!(turn, Some("working" | "waiting")) {
        turn = match asking {
            Some(true) => Some("waiting"),
            Some(false) => Some("working"),
            None => turn,
        };
    }
    // As with head/tail summaries, publish complete bytes already read with
    // their starting stamp. A concurrent mtime update is not an I/O failure.
    // The next refresh sees the changed stamp; the filter above rebuilds on
    // same-size rewrites, truncation and inode changes, or scans a new suffix.
    Ok(Scan {
        stamp,
        committed,
        turn,
        turn_at: if boundary.is_some() {
            turn_at
        } else {
            previous.and_then(|scan| scan.turn_at.clone())
        },
        model: model
            .or_else(|| previous.map(|scan| scan.model.clone()))
            .unwrap_or(Value::Null),
        activity_at: activity_at.or_else(|| previous.and_then(|scan| scan.activity_at.clone())),
    })
}
