//! Codex native acknowledgment observation: a pure classifier over records the
//! checked native reader committed after a fixed submission boundary.
//!
//! It never reads the screen, never opens a file itself, never manufactures a
//! native turn association. The correlation it can
//! establish from a raw TUI rollout is `Correlation::PossibleTextMatch`, which
//! `Machine::acknowledge` accepts after validating the fixed cursor, real-user
//! record, causal position and trimmed prompt. The output is
//! can acknowledge the causal matching receipt or advance its watch cursor.
//!
//! Records come from `ViewSnapshot::native_tail`: the opened view that
//! `SessionStore` parsed through `CheckedNative` + `RawIndex` + the strict
//! provider projection. See `docs/delivery-codex-ack.md`.

use super::codex::{
    AckEvidence, Completion, CompletionEvidence, Correlation, MediaRef, NativeAcceptance,
    NativeCursor,
};
use crate::sessions::{NativeCheckpoint, TailError, TailRecord, ViewSnapshot};

/// A terminal write older than this is
/// overdue and may be re-checked from the fixed boundary. Not a failure timeout.
pub const CONFIRM_TIMEOUT_MS: u64 = 8_000;
/// Python `_poll_outbox` spaces fixed-boundary replays by `CONFIRM_TIMEOUT`.
pub const REPLAY_INTERVAL_MS: u64 = 8_000;
/// Automatic tracking stops one hour after
/// delivery. The receipt keeps its state; it does not become retryable.
pub const TRACK_WINDOW_MS: u64 = 3_600_000;
/// Python `_outbox_loop` wakes every 500 ms; the Rust inventory refresh shares
/// the same 500 ms TTL, so polling faster observes nothing new.
pub const POLL_INTERVAL_MS: u64 = 500;

/// The fixed submission boundary persisted before Enter. It never moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Boundary {
    /// The receipt's `confirmation` cursor: a server-validated native cursor
    /// captured from the same frozen view before the terminal write.
    pub confirmation: NativeCursor,
    /// Monotonic sequence of the Enter operation (for example its receipt
    /// revision). Echoed unchanged so late observations can be fenced.
    pub sequence: u64,
    /// Server clock, milliseconds since the Unix epoch, when the durable Enter
    /// marker was committed. Records timestamped earlier are not this delivery.
    pub delivered_ms: u64,
}

impl Boundary {
    /// Build the fixed boundary from the frozen view that will be injected
    /// into. The caller must have verified that this view is the Codex main
    /// session owned by the target terminal; this helper proves nothing.
    pub fn capture(checkpoint: &NativeCheckpoint, sequence: u64, delivered_ms: u64) -> Self {
        Self {
            confirmation: NativeCursor {
                source_identity: checkpoint.source_identity.clone(),
                position: checkpoint.position,
                head: checkpoint.head.clone(),
                anchor: checkpoint.anchor.clone(),
            },
            sequence,
            delivered_ms,
        }
    }
}

/// What was submitted: the exact receipt payload, not the composer echo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivered {
    pub uid: String,
    pub text: String,
    pub media: Vec<MediaRef>,
}

/// How the matched record's timestamp compared with `delivered_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimestampCheck {
    /// A parsable timestamp not earlier than the delivery time.
    Verified,
    /// No parsable timestamp on the record. Like Python's `_causal`, this
    /// passes the time check; the physical boundary still applies.
    Absent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchedRecord {
    pub start: u64,
    pub end: u64,
    /// Native turn identity from the record itself, when it declares one.
    pub turn_id: Option<String>,
    pub recorded_ms: Option<u64>,
    pub timestamp: TimestampCheck,
}

/// Exactly one qualifying user record after the boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PossibleMatch {
    /// `AckEvidence` with `Correlation::PossibleTextMatch`. The Machine applies
    /// the fixed-boundary and one-record-per-receipt checks before accepting it.
    pub evidence: AckEvidence,
    pub record: MatchedRecord,
    /// A completion status for the matched record's own turn, seen after it.
    /// Only meaningful once a stronger adapter has acknowledged the receipt.
    pub completion: Option<CompletionEvidence>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Absence {
    /// Identical user text after the boundary whose timestamp precedes the
    /// delivery time and was skipped.
    pub skipped_earlier: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Uncertainty {
    /// The fixed boundary no longer describes a committed prefix of the file:
    /// rewritten or truncated below it, or a changed logical view.
    CheckpointMismatch,
    /// The view is not the Codex main session named by the boundary/payload.
    ForeignScope,
    /// Whitespace-only text can never be matched.
    UnmatchableText,
    /// The frozen view could not be interrogated (sanitized status only).
    Unreadable { status: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Possible(Box<PossibleMatch>),
    Absent(Absence),
    Uncertain(Uncertainty),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub sequence: u64,
    /// The current committed cursor of the observed view, usable as the next
    /// `AdvanceWatch` target. `None` when the boundary could not be validated.
    pub watch: Option<NativeCursor>,
    pub outcome: Outcome,
}

/// Classify the records the checked reader committed after `boundary` in the
/// frozen `snapshot`. The caller refreshes the snapshot through
/// `SessionStore::snapshot(uid, "")` on every poll; nothing here touches disk.
pub fn observe(snapshot: &ViewSnapshot, boundary: &Boundary, delivered: &Delivered) -> Observation {
    let sequence = boundary.sequence;
    let checkpoint = snapshot.native_checkpoint();
    let uncertain = |reason: Uncertainty, watch: Option<NativeCursor>| Observation {
        sequence,
        watch,
        outcome: Outcome::Uncertain(reason),
    };
    if checkpoint.source != "codex"
        || checkpoint.agent_id.is_some()
        || checkpoint.uid != delivered.uid
        || checkpoint.source_identity != boundary.confirmation.source_identity
    {
        return uncertain(Uncertainty::ForeignScope, None);
    }
    let tail = match snapshot.native_tail(
        boundary.confirmation.position,
        &boundary.confirmation.head,
        &boundary.confirmation.anchor,
    ) {
        Ok(tail) => tail,
        Err(TailError::CheckpointMismatch) => {
            return uncertain(Uncertainty::CheckpointMismatch, None);
        }
        Err(TailError::Session(error)) => {
            return uncertain(
                Uncertainty::Unreadable {
                    status: error.status,
                },
                None,
            );
        }
    };
    let watch = Some(NativeCursor {
        source_identity: tail.checkpoint.source_identity.clone(),
        position: tail.checkpoint.position,
        head: tail.checkpoint.head.clone(),
        anchor: tail.checkpoint.anchor.clone(),
    });
    let expected = delivered.text.trim();
    if expected.is_empty() {
        return uncertain(Uncertainty::UnmatchableText, watch);
    }

    // Codex trims both ends before writing the rollout; internal whitespace
    // and newlines stay significant. Python walks native rows in order,
    // skips pre-delivery timestamps, and retires the first causal match.
    let mut skipped_earlier = 0_u32;
    let record = tail.records.iter().find(|record| {
        if !is_user_input(&record.message)
            || !record.message["text"]
                .as_str()
                .is_some_and(|text| text.trim() == expected)
        {
            return false;
        }
        if record.message["ts"]
            .as_str()
            .and_then(parse_rfc3339_ms)
            .is_some_and(|recorded| recorded < boundary.delivered_ms)
        {
            skipped_earlier = skipped_earlier.saturating_add(1);
            return false;
        }
        true
    });
    let Some(record) = record else {
        return Observation {
            sequence,
            watch,
            outcome: Outcome::Absent(Absence { skipped_earlier }),
        };
    };
    let recorded_ms = record.message["ts"].as_str().and_then(parse_rfc3339_ms);
    let timestamp = match recorded_ms {
        Some(_) => TimestampCheck::Verified,
        None => TimestampCheck::Absent,
    };
    let turn_id = record.message["turn_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    let acceptance = NativeAcceptance {
        source_identity: tail.checkpoint.source_identity.clone(),
        record_id: format!("codex-line:{}-{}", record.start, record.end),
        start: record.start,
        end: record.end,
        turn_id: turn_id.clone().unwrap_or_default(),
    };
    let completion = turn_id.as_deref().and_then(|turn| {
        tail.records
            .iter()
            .filter(|later| later.start >= record.end)
            .find_map(|later| completion_of(later, turn))
            .map(|(record_end, outcome)| CompletionEvidence {
                uid: delivered.uid.clone(),
                validated_confirmation: boundary.confirmation.clone(),
                source_identity: tail.checkpoint.source_identity.clone(),
                turn_id: turn.to_owned(),
                record_end,
                outcome,
            })
    });
    let evidence = AckEvidence {
        uid: delivered.uid.clone(),
        validated_confirmation: boundary.confirmation.clone(),
        record: acceptance,
        text: record.message["text"].as_str().unwrap_or("").to_owned(),
        observed_media: delivered.media.clone(),
        real_user_input: true,
        correlation: Correlation::PossibleTextMatch,
    };
    Observation {
        sequence,
        watch,
        outcome: Outcome::Possible(Box::new(PossibleMatch {
            evidence,
            record: MatchedRecord {
                start: record.start,
                end: record.end,
                turn_id,
                recorded_ms,
                timestamp,
            },
            completion,
        })),
    }
}

/// A real, counted human input in the projection. The provider projection has
/// already dropped developer prompts, rebuilt instruction blocks and
/// `goal.internal_context` records; inferred/uncounted events (for example a
/// `/rename` derived from the name index) are not native user records either.
fn is_user_input(message: &serde_json::Value) -> bool {
    message["role"] == "user" && message["counted"] != false && message["inferred"] != true
}

fn completion_of(record: &TailRecord, turn: &str) -> Option<(u64, Completion)> {
    let message = &record.message;
    if message["role"] != "status" || message["turn_id"] != turn {
        return None;
    }
    let outcome = match message["state"].as_str()? {
        "idle" => Completion::Succeeded,
        "failed" => Completion::Failed,
        "aborted" => Completion::Stopped,
        _ => return None,
    };
    Some((record.end, outcome))
}

/// The projection normalizes native timestamps to RFC 3339 UTC milliseconds
/// or `null`; anything else is treated as absent.
fn parse_rfc3339_ms(text: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .and_then(|date| u64::try_from(date.timestamp_millis()).ok())
}

/// Python `_poll_outbox` scheduling for one tracked Codex receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayPolicy {
    pub confirm_timeout_ms: u64,
    pub replay_interval_ms: u64,
    pub track_window_ms: u64,
}

impl Default for ReplayPolicy {
    fn default() -> Self {
        Self {
            confirm_timeout_ms: CONFIRM_TIMEOUT_MS,
            replay_interval_ms: REPLAY_INTERVAL_MS,
            track_window_ms: TRACK_WINDOW_MS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadPlan {
    /// Cheap tick: observe only if the committed end advanced past the watch
    /// cursor (`InspectNative { replay: false }`).
    Watch,
    /// Overdue and rate-limited: observe from the fixed boundary even if the
    /// tail did not visibly move (`InspectNative { replay: true }`).
    Replay,
    /// Past the tracking window: stop automatic polling. The receipt keeps its
    /// state and is not retryable; an explicit inspection may still run.
    Expired,
}

/// Per-receipt replay clock. It stores only the last replay instant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplayClock {
    last_replay_ms: Option<u64>,
}

impl ReplayClock {
    pub fn last_replay_ms(&self) -> Option<u64> {
        self.last_replay_ms
    }

    /// Decide this tick without side effects.
    pub fn peek(&self, policy: &ReplayPolicy, delivered_ms: u64, now_ms: u64) -> ReadPlan {
        let since_delivery = now_ms.saturating_sub(delivered_ms);
        if since_delivery >= policy.track_window_ms {
            return ReadPlan::Expired;
        }
        let overdue = since_delivery >= policy.confirm_timeout_ms;
        let spaced = self
            .last_replay_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= policy.replay_interval_ms);
        if overdue && spaced {
            ReadPlan::Replay
        } else {
            ReadPlan::Watch
        }
    }

    /// Decide this tick and record a replay instant when one is granted.
    pub fn plan(&mut self, policy: &ReplayPolicy, delivered_ms: u64, now_ms: u64) -> ReadPlan {
        let plan = self.peek(policy, delivered_ms, now_ms);
        if plan == ReadPlan::Replay {
            self.last_replay_ms = Some(now_ms);
        }
        plan
    }
}

/// Only the earliest still-tracked receipt per
/// UID is polled, and a receipt outside the window drops out of polling
/// without changing state. `receipts` are `(request_id, delivered_ms)` pairs
/// of one UID in creation order; the result is the one to observe this tick.
pub fn earliest_tracked<'a>(
    policy: &ReplayPolicy,
    receipts: impl IntoIterator<Item = (&'a str, u64)>,
    now_ms: u64,
) -> Option<&'a str> {
    receipts
        .into_iter()
        .find(|(_, delivered_ms)| now_ms.saturating_sub(*delivered_ms) < policy.track_window_ms)
        .map(|(id, _)| id)
}

#[cfg(test)]
mod tests;
