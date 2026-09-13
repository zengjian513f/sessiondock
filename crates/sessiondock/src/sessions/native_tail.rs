//! Native records after a validated physical checkpoint, with their exact
//! line ranges, for native acknowledgment adapters (`delivery::codex_adapter`).
//!
//! This performs no new file I/O. The records come from the opened view that
//! `SessionStore` produced through `CheckedNative`, `RawIndex` and the strict
//! provider projection; the caller refreshes that view the same way history
//! reads do (`SessionStore::snapshot`). Physical byte offsets belong to the
//! selected leaf file only; inherited fork prefixes are never part of a tail.
use super::{MessageQuery, SessionError, ViewSnapshot};
use serde_json::Value;

/// One projected event after the boundary and the exact physical range of the
/// native JSONL line it came from: `[start, end)` in the leaf file, where
/// `end` is the offset just past that line's LF.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailRecord {
    pub start: u64,
    pub end: u64,
    /// The ordinary projected message (`role`, `text`, `ts`, `turn_id`, ...),
    /// identical to what `/api/messages` publishes for this event.
    pub message: Value,
}

/// The stable identity and current committed cursor of a selected view.
/// `source_identity` is the view identity bound into semantic anchors; it does
/// not change on ordinary appends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCheckpoint {
    pub source: String,
    pub uid: String,
    pub agent_id: Option<String>,
    pub source_identity: String,
    pub position: u64,
    pub head: String,
    pub anchor: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeTail {
    /// Identity and the *current* committed cursor (not the queried boundary).
    pub checkpoint: NativeCheckpoint,
    /// The validated boundary the records follow.
    pub position: u64,
    pub records: Vec<TailRecord>,
}

#[derive(Clone, Debug)]
pub enum TailError {
    /// The boundary is not a committed LF checkpoint of the current file, or
    /// its head/anchor no longer match: the file was rewritten or truncated
    /// below the boundary, or the logical view changed. Nothing after it can
    /// be attributed.
    CheckpointMismatch,
    Session(SessionError),
}

impl ViewSnapshot {
    /// Identity plus the committed cursor of this immutable view, suitable for
    /// capturing a fixed confirmation boundary before a terminal write.
    pub fn native_checkpoint(&self) -> NativeCheckpoint {
        let meta = &self.view.meta;
        NativeCheckpoint {
            source: meta["source"].as_str().unwrap_or("").to_owned(),
            uid: meta["uid"].as_str().unwrap_or("").to_owned(),
            agent_id: meta["agent_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_owned),
            source_identity: self.view.identity.clone(),
            position: self.view.parsed.committed as u64,
            head: self.head.clone(),
            anchor: self.anchor.clone(),
        }
    }

    /// Records strictly after `position`, after validating that `position`,
    /// `head` and `anchor` still describe a committed prefix of this view
    /// exactly like an `/api/messages` cursor. Inherited fork events (physical
    /// end zero) are excluded; status events are included so a caller can see
    /// turn completion after a matched record.
    pub fn native_tail(
        &self,
        position: u64,
        head: &str,
        anchor: &str,
    ) -> Result<NativeTail, TailError> {
        let query = MessageQuery {
            start: position,
            head: head.to_owned(),
            anchor: anchor.to_owned(),
            ..MessageQuery::default()
        };
        super::validate_message_query(&query).map_err(TailError::Session)?;
        if !self.valid_checkpoint(&query) {
            return Err(TailError::CheckpointMismatch);
        }
        let parsed = &self.view.parsed;
        let mut records = Vec::new();
        for event in self.view.events() {
            if event.end == 0 || event.end <= position {
                continue;
            }
            // Every event end is a committed LF end from the decoder, and
            // `position` is itself an LF checkpoint below it, so the line start
            // (the checkpoint immediately before `end`) is never below it.
            let start = parsed.raw_index.record_start(event.end);
            if start < position || !parsed.raw_index.is_checkpoint(event.end) {
                return Err(TailError::Session(SessionError::new(
                    500,
                    "原生事件的物理范围与已提交的行边界不一致",
                )));
            }
            records.push(TailRecord {
                start,
                end: event.end,
                message: event.message.clone(),
            });
        }
        Ok(NativeTail {
            checkpoint: self.native_checkpoint(),
            position,
            records,
        })
    }
}
