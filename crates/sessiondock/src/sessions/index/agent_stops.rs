//! Claude subagent stop points from the owner's main transcript (batch 36,
//! WP-C; Python `_claude_agent_stops` / `_collect_agent_stops`).
//!
//! A subagent that stops (finished, failed, killed, gone with its process)
//! makes the parent write a `<task-notification>` or the foreground `Agent`
//! tool_result within ~150 ms; afterwards only SendMessage wakes it, and that
//! appends a user record to the subagent transcript. So "last subagent record
//! later than the parent's latest stop notice" is the running rule
//! ([`claude_active`]). Notices can be far from the tail, so the scan is
//! incremental by byte offset per owner file and reads only new complete
//! lines; a line is decoded only when it can name an agent. The index runs it
//! only for owners that have a sidecar with an open turn (docs/read-model.md).

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::LazyLock;

use regex::bytes::Regex;
use serde_json::Value;
use sha1::{Digest, Sha1};

use super::Stamp;
use super::summary::{norm_ts, py_str, truthy};

/// Agent id → latest stop time (`norm_ts` text).
pub type Stops = BTreeMap<String, String>;

/// Read granularity of the incremental scan.
const CHUNK: usize = 1024 * 1024;

static NOTICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s-u)<task-notification>.*?</task-notification>").expect("static regex")
});
static TASK_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<task-id>([A-Za-z0-9_-]{1,64})</task-id>").expect("static regex")
});
// Python's `b"..." in raw` pre-checks; literal regexes so the substring
// search is vectorized (the scan touches every byte of a cold owner file).
static HAS_TASK_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new("<task-id>").expect("literal"));
static HAS_AGENT_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""agentId""#).expect("literal"));
static HAS_TOOL_RESULT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""tool_result""#).expect("literal"));

/// Scan state of one owner file (Python's `_agent_stops[path]` entry).
#[derive(Clone, Debug, Default)]
pub struct StopScan {
    /// The file version the consumed bytes belong to; a different inode or a
    /// shorter file restarts the scan from offset 0.
    stamp: Option<Stamp>,
    /// Offset after the last LF read; a trailing partial line is read again.
    scanned: u64,
    stops: Stops,
    /// sha1(block)[..20] → (agent id, first-seen time): the same notice text
    /// is written again on dequeue/re-enqueue/absorption with later times.
    notices: HashMap<String, (String, String)>,
}

impl StopScan {
    pub fn stops(&self) -> &Stops {
        &self.stops
    }

    /// The version this scan has consumed.
    pub fn stamp(&self) -> Option<Stamp> {
        self.stamp
    }

    #[cfg(test)]
    pub fn scanned(&self) -> u64 {
        self.scanned
    }
}

/// Python `_collect_agent_stops`: merge one main-transcript line into the
/// scan. Copies of a notice text count only at their first-seen time; a
/// foreground Agent result is a stop unless it is only `async_launched`.
pub fn collect(raw: &[u8], scan: &mut StopScan) {
    let has_notice = HAS_TASK_ID.is_match(raw);
    let has_result = HAS_AGENT_ID.is_match(raw) && HAS_TOOL_RESULT.is_match(raw);
    if !(has_notice || has_result) {
        return;
    }
    let Ok(record) = serde_json::from_slice::<Value>(raw) else {
        return;
    };
    if !record.is_object() {
        return;
    }
    let Some(ts) = norm_ts(&record["timestamp"]) else {
        return;
    };
    if has_notice {
        for block in NOTICE.find_iter(raw) {
            let Some(captures) = TASK_ID.captures(block.as_bytes()) else {
                continue;
            };
            let agent_id = String::from_utf8_lossy(&captures[1]).into_owned();
            let digest = format!("{:x}", Sha1::digest(block.as_bytes()))[..20].to_owned();
            let seen = scan.notices.get(&digest).cloned();
            if seen.as_ref().is_some_and(|(_, first)| *first <= ts) {
                continue;
            }
            scan.notices.insert(digest, (agent_id.clone(), ts.clone()));
            match seen {
                None => {
                    if scan.stops.get(&agent_id).is_none_or(|stop| ts > *stop) {
                        scan.stops.insert(agent_id, ts.clone());
                    }
                }
                Some(_) => {
                    // An earlier copy written later (attachment after dequeue):
                    // recompute this agent's latest stop from its notices.
                    let latest = scan
                        .notices
                        .values()
                        .filter(|(agent, _)| *agent == agent_id)
                        .map(|(_, at)| at.clone())
                        .max()
                        .unwrap_or_default();
                    scan.stops.insert(agent_id, latest);
                }
            }
        }
    }
    if has_result {
        let result = &record["toolUseResult"];
        if result.is_object() && truthy(&result["agentId"]) && result["status"] != "async_launched"
        {
            let agent_id = py_str(&result["agentId"]);
            if scan.stops.get(&agent_id).is_none_or(|stop| ts > *stop) {
                scan.stops.insert(agent_id, ts);
            }
        }
    }
}

/// Python `_claude_agent_stops`: bring `scan` up to the owner file at
/// `stamp`, reading only the bytes after the last consumed LF. A file that
/// is a different inode or shorter than what was consumed is rescanned from
/// the start; a file that cannot be opened at that inode leaves the scan as
/// it was (the next refresh sees a new stamp and tries again).
pub fn update(scan: &mut StopScan, root: &Path, data: &Path, stamp: Stamp) {
    if scan
        .stamp
        .is_none_or(|known| (known.dev, known.ino) != (stamp.dev, stamp.ino))
        || stamp.size < scan.scanned
    {
        *scan = StopScan::default();
    }
    if stamp.size > scan.scanned {
        let Ok(mut file) = super::open_indexed(root, data) else {
            return;
        };
        let Ok(meta) = cap_std::fs::Metadata::from_file(&file) else {
            return;
        };
        let current = Stamp::of(&meta);
        if (current.dev, current.ino) != (stamp.dev, stamp.ino) {
            return;
        }
        if current.size < scan.scanned {
            *scan = StopScan::default();
        }
        if let Err(()) = read_lines(&mut file, current.size, scan) {
            return;
        }
    }
    scan.stamp = Some(stamp);
}

/// Feed every complete line in `[scan.scanned, size)` to [`collect`] and
/// advance `scan.scanned` past the last LF read, regardless of record size.
fn read_lines(file: &mut std::fs::File, size: u64, scan: &mut StopScan) -> Result<(), ()> {
    file.seek(SeekFrom::Start(scan.scanned)).map_err(|_| ())?;
    let mut buffer = vec![0u8; CHUNK];
    let mut carry: Vec<u8> = Vec::new();
    let mut position = scan.scanned;
    while position < size {
        let want = usize::try_from((size - position).min(CHUNK as u64)).map_err(|_| ())?;
        let read = file.read(&mut buffer[..want]).map_err(|_| ())?;
        if read == 0 {
            break;
        }
        position += read as u64;
        let mut chunk = &buffer[..read];
        while !chunk.is_empty() {
            let Some(lf) = chunk.iter().position(|byte| *byte == b'\n') else {
                carry.try_reserve(chunk.len()).map_err(|_| ())?;
                carry.extend_from_slice(chunk);
                break;
            };
            if carry.is_empty() {
                collect(&chunk[..lf], scan);
            } else {
                carry.extend_from_slice(&chunk[..lf]);
                collect(&carry, scan);
                carry.clear();
            }
            chunk = &chunk[lf + 1..];
            scan.scanned = position - chunk.len() as u64;
        }
    }
    Ok(())
}

/// Python `_ts_after`: both `norm_ts`/`iso_seconds` texts as instants;
/// unparsable values are never later.
pub fn ts_after(later: &str, earlier: &str) -> bool {
    match (
        chrono::DateTime::parse_from_rfc3339(later),
        chrono::DateTime::parse_from_rfc3339(earlier),
    ) {
        (Ok(later), Ok(earlier)) => later > earlier,
        _ => false,
    }
}

/// The Claude `agent_items[].active` rule: the sidecar's last turn is not
/// closed and the owner has no stop notice for it, or the sidecar's last
/// record is later than the latest stop (it was woken by SendMessage).
pub fn claude_active(open_turn: bool, updated: &str, stopped_at: Option<&str>) -> bool {
    open_turn && stopped_at.is_none_or(|stopped| ts_after(updated, stopped))
}

#[cfg(test)]
mod tests;
