//! Scalar Codex turn state, independent of the metadata head/tail window.
//! Cold reads walk backwards only to the latest native boundary; appends
//! inspect only new complete lines. No message projection is retained.

use std::io;

use serde::Deserialize;
use serde_json::Value;

use super::{Stamp, StampedSource, TAIL_BYTES};

#[derive(Clone)]
pub(super) struct Scan {
    stamp: Stamp,
    committed: u64,
    pub turn: Option<&'static str>,
}

#[derive(Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: String,
    payload: Payload,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    error: Value,
}

pub(super) fn read(
    file: &mut dyn StampedSource,
    stamp: Stamp,
    previous: Option<&Scan>,
) -> io::Result<Scan> {
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
    'chunks: while position > floor {
        let start = position.saturating_sub(TAIL_BYTES).max(floor);
        let bytes = file.read_range(start, position - start)?;
        if bytes.len() as u64 != position - start {
            return Err(io::Error::other("Codex turn scan changed during read"));
        }
        let mut end = bytes.len();
        loop {
            let lf = memchr::memrchr(b'\n', &bytes[..end]);
            let begin = lf.map_or(0, |offset| offset + 1);
            if terminated {
                pieces.push(bytes[begin..end].to_vec());
                if lf.is_some() || start == floor {
                    // Join a cross-chunk record once, avoiding quadratic copies
                    // for long native records. Only scalar fields are decoded.
                    let raw: Vec<u8> = pieces.iter().rev().flatten().copied().collect();
                    pieces.clear();
                    if (memchr::memmem::find(&raw, b"event_msg").is_some()
                        || memchr::memmem::find(&raw, b"response_item").is_some())
                        && let Ok(record) = serde_json::from_slice::<Record>(&raw)
                    {
                        if record.kind == "response_item" {
                            asking.get_or_insert_with(|| {
                                matches!(
                                    record.payload.kind.as_str(),
                                    "function_call" | "custom_tool_call"
                                ) && crate::sessions::providers::question_tool(&record.payload.name)
                            });
                        } else if record.kind == "event_msg" {
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
                                break 'chunks;
                            }
                        }
                    }
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
    // Appends do not invalidate the prefix just scanned. Cache the original
    // stamp/committed LF so the next observation reads the new suffix. A
    // rewrite, truncation or replacement still invalidates this observation.
    if !file.stamp().is_some_and(|after| {
        (after.dev, after.ino) == (stamp.dev, stamp.ino)
            && (after == stamp || after.size > stamp.size)
    }) {
        return Err(io::Error::other("Codex turn scan changed during read"));
    }
    Ok(Scan {
        stamp,
        committed,
        turn,
    })
}
