//! Conservative Codex TUI delivery receipts and explicit durability barriers.
//!
//! This is not a working TUI sender. Native evidence below is supplied by a
//! trusted adapter, never directly deserialized from an HTTP request. Current
//! Codex TUI integration does not prove operation-to-turn association: matching
//! later text alone must remain uncertain. See `docs/delivery.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

const SCHEMA: u32 = 1;
// Matches the terminal host's text input limit; receipt history is not quota.
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// Stable host process identity, not a reusable terminal display name.
    pub host_instance: String,
    pub session_id: String,
    /// A trusted ownership generation. Not a credential or bearer token.
    pub ownership_epoch: String,
}

/// Opaque outbox preview metadata. The actual uploaded path is embedded in
/// the submitted text before this request reaches delivery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MediaRef(pub serde_json::Value);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload {
    pub uid: String,
    pub target: Target,
    /// Exact submitted text; only acknowledgment comparison trims its ends.
    pub text: String,
    pub media: Vec<MediaRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub request_id: String,
    pub payload: Payload,
}

/// A server-validated native prefix, not a browser-provided cursor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeCursor {
    pub source_identity: String,
    pub position: u64,
    pub head: String,
    pub anchor: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub epoch: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    pub epoch: String,
    pub request_id: String,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    CheckingDraft,
    DraftConflict,
    PrepareInFlight,
    EnterInFlight,
    Uncertain,
    Acknowledged,
    StopRequested,
    Completed,
    Stopped,
    FailedBeforeWrite,
    Discarded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completion {
    Succeeded,
    Failed,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeAcceptance {
    pub source_identity: String,
    pub record_id: String,
    pub start: u64,
    pub end: u64,
    pub turn_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub request: Request,
    pub created_ms: u64,
    pub revision: u64,
    pub state: State,
    /// Durable prepare claims, not number of native acceptances.
    pub attempts: u32,
    pub draft_token: Option<String>,
    pub approved_draft: Option<String>,
    pub confirmation: Option<NativeCursor>,
    pub watch: Option<NativeCursor>,
    pub enter_operation: Option<Operation>,
    pub accepted: Option<NativeAcceptance>,
    pub association: Option<Correlation>,
    pub completion: Option<Completion>,
    pub completion_record_end: Option<u64>,
    /// UI dismissal of an already attempted receipt (batch 32, Python
    /// `9b1c2fd`): the row is hidden and leaves automatic tracking, but its
    /// delivery state stays uncertain and its deduplication identity remains.
    /// Removing it never resends and never cancels input the TUI owns.
    #[serde(default)]
    pub dismissed: bool,
    pub issue: Option<String>,
}

impl Receipt {
    pub fn retryable(&self) -> bool {
        self.state == State::FailedBeforeWrite && self.attempts == 0
    }

    pub fn visible_in_outbox(&self) -> bool {
        !self.dismissed
            && !matches!(
                self.state,
                State::Acknowledged
                    | State::StopRequested
                    | State::Completed
                    | State::Stopped
                    | State::Discarded
            )
    }
}

/// Retains accepted, completed and discarded tombstones: replay never sends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema: u32,
    pub version: Version,
    pub receipts: BTreeMap<String, Receipt>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftState {
    Empty,
    Editing,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftObservation {
    pub target: Target,
    pub state: DraftState,
    /// Irreversible fingerprint of the exact composer frame plus cursor.
    pub token: String,
    pub native_cursor: NativeCursor,
}

/// A fresh observed composer, not merely a successful paste API return.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedEvidence {
    pub target: Target,
    pub observed_text: String,
    pub observed_media: Vec<MediaRef>,
    pub frame_token: String,
}

/// Strong association must be derived independently by a trusted adapter.
/// A later `task_started` or identical prompt is NOT an `OperationTurn` proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Correlation {
    NativeRequestId(String),
    OperationTurn {
        enter_operation: Operation,
        turn_id: String,
    },
    /// Python-compatible causal text match after the fixed native boundary.
    PossibleTextMatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AckEvidence {
    pub uid: String,
    /// Reader has revalidated this exact fixed prefix against native storage.
    pub validated_confirmation: NativeCursor,
    pub record: NativeAcceptance,
    pub text: String,
    pub observed_media: Vec<MediaRef>,
    /// False for telemetry, rebuilt prompts, developer context and commands.
    pub real_user_input: bool,
    pub correlation: Correlation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionEvidence {
    pub uid: String,
    pub validated_confirmation: NativeCursor,
    pub source_identity: String,
    pub turn_id: String,
    pub record_end: u64,
    pub outcome: Completion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnterResult {
    TransportReturned,
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Submit {
        request: Request,
        now_ms: u64,
    },
    Persisted(Version),
    DraftObserved {
        operation: Operation,
        observation: DraftObservation,
    },
    ConfirmDraft {
        request_id: String,
        uid: String,
        token: String,
    },
    InspectionFailed {
        operation: Operation,
        reason: String,
    },
    Prepared {
        operation: Operation,
        evidence: PreparedEvidence,
    },
    PrepareFailed {
        operation: Operation,
        reason: String,
    },
    EnterFinished {
        operation: Operation,
        result: EnterResult,
    },
    NativeAck {
        request_id: String,
        evidence: AckEvidence,
    },
    NativeCompletion {
        request_id: String,
        evidence: CompletionEvidence,
    },
    RequestStop {
        request_id: String,
        uid: String,
    },
    Retry {
        request_id: String,
        uid: String,
    },
    Discard {
        request_id: String,
        uid: String,
    },
    /// Python `_discard_message` for Codex after `9b1c2fd`: a pre-write row is
    /// discarded like `Discard`; an already attempted `uncertain` row is only
    /// hidden (`dismissed`), keeping its state and tombstone. Never a cancel.
    Dismiss {
        request_id: String,
        uid: String,
    },
    InspectNative {
        request_id: String,
        uid: String,
        replay: bool,
    },
    AdvanceWatch {
        operation: Operation,
        previous: NativeCursor,
        next: NativeCursor,
        validated_confirmation: NativeCursor,
    },
    NativeReset {
        operation: Operation,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Persist {
        version: Version,
        snapshot: Snapshot,
    },
    Replay {
        receipt: Box<Receipt>,
        pending: bool,
    },
    InspectComposer {
        operation: Operation,
        target: Target,
    },
    /// Recheck the exact frame under the bound writer lease. If overwrite is
    /// true, clear only that approved draft, verify empty, then paste safely.
    InjectPrepare {
        operation: Operation,
        payload: Payload,
        expected_frame: String,
        overwrite: bool,
    },
    /// Recheck prepared frame and ownership immediately before Enter.
    InjectEnter {
        operation: Operation,
        target: Target,
        expected_frame: String,
    },
    /// Must conditionally target the accepted current turn, never generic Esc.
    InterruptTurn {
        operation: Operation,
        target: Target,
        turn_id: String,
    },
    InspectNative {
        operation: Operation,
        from: NativeCursor,
        confirmation: NativeCursor,
        replay: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid(&'static str),
    Conflict,
    Missing,
    WrongState,
    StaleOperation,
    PersistencePending,
    UnprovenAcknowledgment,
    Capacity,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone)]
enum After {
    None,
    Inspect(String),
    Prepare {
        id: String,
        frame: String,
        overwrite: bool,
    },
    Enter {
        id: String,
        frame: String,
    },
    Stop(String),
}

struct Pending {
    snapshot: Snapshot,
    after: After,
}

pub struct Machine {
    epoch: String,
    committed: Snapshot,
    pending: Option<Pending>,
}

impl Machine {
    pub fn new(epoch: String) -> Result<Self, Error> {
        present(&epoch)?;
        Ok(Self {
            committed: Snapshot {
                schema: SCHEMA,
                version: Version {
                    epoch: epoch.clone(),
                    revision: 0,
                },
                receipts: BTreeMap::new(),
            },
            epoch,
            pending: None,
        })
    }

    /// Restoration never emits terminal mutations. All uncertain in-flight
    /// boundaries remain uncertain even if the old process never dispatched.
    pub fn restore(snapshot: Snapshot, new_epoch: String) -> Result<(Self, Vec<Effect>), Error> {
        validate_snapshot(&snapshot)?;
        present(&new_epoch)?;
        if snapshot.version.epoch == new_epoch {
            return Err(Error::Invalid("recovery requires a fresh epoch"));
        }
        let mut machine = Self {
            epoch: new_epoch,
            committed: snapshot.clone(),
            pending: None,
        };
        let mut restored = snapshot;
        for receipt in restored.receipts.values_mut() {
            receipt.approved_draft = None;
            match receipt.state {
                State::CheckingDraft => {
                    receipt.state = State::FailedBeforeWrite;
                    receipt.issue = Some("重启前未进入注入边界；显式重试需重新检查草稿".into());
                }
                State::PrepareInFlight | State::EnterInFlight => {
                    receipt.state = State::Uncertain;
                    receipt.issue =
                        Some("重启时存在注入中回执，是否写入未知；禁止自动重注入".into());
                }
                _ => {}
            }
        }
        let effects = machine.propose(restored, After::None)?;
        Ok((machine, effects))
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.committed
    }

    pub fn apply(&mut self, command: Command) -> Result<Vec<Effect>, Error> {
        if let Command::Persisted(version) = command {
            return self.commit(version);
        }
        if let Command::Submit { request, .. } = &command {
            validate_request(request)?;
            let existing = self
                .pending
                .as_ref()
                .and_then(|pending| pending.snapshot.receipts.get(&request.request_id))
                .or_else(|| self.committed.receipts.get(&request.request_id));
            if let Some(existing) = existing {
                if existing.request != *request {
                    return Err(Error::Conflict);
                }
                let pending = self
                    .committed
                    .receipts
                    .get(&request.request_id)
                    .is_none_or(|committed| committed.revision != existing.revision);
                return Ok(vec![Effect::Replay {
                    receipt: Box::new(existing.clone()),
                    pending,
                }]);
            }
        }
        if self.pending.is_some() {
            return Err(Error::PersistencePending);
        }
        match command {
            Command::Submit { request, now_ms } => {
                if self.critical_conflict(&request) {
                    return Err(Error::WrongState);
                }
                let id = request.request_id.clone();
                let receipt = Receipt {
                    request,
                    created_ms: now_ms,
                    revision: 0,
                    state: State::CheckingDraft,
                    attempts: 0,
                    draft_token: None,
                    approved_draft: None,
                    confirmation: None,
                    watch: None,
                    enter_operation: None,
                    accepted: None,
                    association: None,
                    completion: None,
                    completion_record_end: None,
                    dismissed: false,
                    issue: None,
                };
                self.change(receipt, After::Inspect(id))
            }
            Command::DraftObserved {
                operation,
                observation,
            } => {
                let mut row = self.operation(&operation, &[State::CheckingDraft])?;
                if observation.target != row.request.payload.target {
                    return Err(Error::Conflict);
                }
                if observation.state == DraftState::Unknown {
                    row.state = State::FailedBeforeWrite;
                    row.issue = Some("无法确认终端草稿；没有授权注入".into());
                    return self.change(row, After::None);
                }
                nonempty(&observation.token, 256)?;
                validate_cursor(&observation.native_cursor)?;
                if observation.state == DraftState::Editing
                    && row.approved_draft.as_ref() != Some(&observation.token)
                {
                    row.state = State::DraftConflict;
                    row.draft_token = Some(observation.token);
                    row.approved_draft = None;
                    row.issue = Some("终端有未提交草稿，必须确认当前版本".into());
                    return self.change(row, After::None);
                }
                let after = After::Prepare {
                    id: row.request.request_id.clone(),
                    frame: observation.token,
                    overwrite: observation.state == DraftState::Editing,
                };
                row.state = State::PrepareInFlight;
                row.attempts += 1;
                row.confirmation = Some(observation.native_cursor.clone());
                row.watch = Some(observation.native_cursor);
                row.issue = None;
                self.change(row, after)
            }
            Command::ConfirmDraft {
                request_id,
                uid,
                token,
            } => {
                let mut row = self.row(&request_id, &uid)?;
                if row.state != State::DraftConflict || row.draft_token.as_ref() != Some(&token) {
                    return Err(Error::Conflict);
                }
                if self.critical_conflict(&row.request) {
                    return Err(Error::WrongState);
                }
                row.approved_draft = Some(token);
                row.state = State::CheckingDraft;
                row.issue = None;
                self.change(row, After::Inspect(request_id))
            }
            Command::InspectionFailed { operation, reason } => {
                let mut row = self.operation(&operation, &[State::CheckingDraft])?;
                row.state = State::FailedBeforeWrite;
                row.issue = Some(reason);
                self.change(row, After::None)
            }
            Command::Prepared {
                operation,
                evidence,
            } => {
                let mut row = self.operation(&operation, &[State::PrepareInFlight])?;
                if evidence.target != row.request.payload.target
                    || evidence.observed_text != row.request.payload.text
                    || evidence.observed_media != row.request.payload.media
                    || nonempty(&evidence.frame_token, 256).is_err()
                {
                    row.state = State::Uncertain;
                    row.issue = Some("无法证明完整输入已准备好；禁止 Enter 和重注入".into());
                    return self.change(row, After::None);
                }
                row.state = State::EnterInFlight;
                row.enter_operation = Some(Operation {
                    epoch: self.epoch.clone(),
                    request_id: row.request.request_id.clone(),
                    revision: self.next_revision()?,
                });
                let after = After::Enter {
                    id: row.request.request_id.clone(),
                    frame: evidence.frame_token,
                };
                self.change(row, after)
            }
            Command::PrepareFailed { operation, reason } => {
                let mut row = self.operation(&operation, &[State::PrepareInFlight])?;
                row.state = State::Uncertain;
                row.issue = Some(reason);
                self.change(row, After::None)
            }
            Command::EnterFinished { operation, result } => {
                let mut row = self.operation(&operation, &[State::EnterInFlight])?;
                row.state = State::Uncertain;
                row.issue = Some(match result {
                    EnterResult::TransportReturned => "终端调用已返回，但没有可靠原生确认".into(),
                    EnterResult::Unknown(reason) => reason,
                });
                self.change(row, After::None)
            }
            Command::NativeAck {
                request_id,
                evidence,
            } => self.acknowledge(&request_id, evidence),
            Command::NativeCompletion {
                request_id,
                evidence,
            } => {
                let mut row = self.row(&request_id, &evidence.uid)?;
                let accepted = row.accepted.as_ref().ok_or(Error::UnprovenAcknowledgment)?;
                if row.confirmation.as_ref() != Some(&evidence.validated_confirmation)
                    || accepted.source_identity != evidence.source_identity
                    || accepted.turn_id != evidence.turn_id
                    || evidence.record_end <= accepted.end
                {
                    return Err(Error::UnprovenAcknowledgment);
                }
                if matches!(row.state, State::Completed | State::Stopped) {
                    return if row.completion == Some(evidence.outcome) {
                        Ok(vec![Effect::Replay {
                            receipt: Box::new(row),
                            pending: false,
                        }])
                    } else {
                        Err(Error::Conflict)
                    };
                }
                if !matches!(row.state, State::Acknowledged | State::StopRequested) {
                    return Err(Error::WrongState);
                }
                row.state = if evidence.outcome == Completion::Stopped {
                    State::Stopped
                } else {
                    State::Completed
                };
                row.completion = Some(evidence.outcome);
                row.completion_record_end = Some(evidence.record_end);
                row.issue = None;
                self.change(row, After::None)
            }
            Command::RequestStop { request_id, uid } => {
                let mut row = self.row(&request_id, &uid)?;
                if row.state == State::StopRequested {
                    return Ok(vec![Effect::Replay {
                        receipt: Box::new(row),
                        pending: false,
                    }]);
                }
                if row.state != State::Acknowledged {
                    return Err(Error::WrongState);
                }
                if self.critical_conflict(&row.request) {
                    return Err(Error::WrongState);
                }
                row.state = State::StopRequested;
                self.change(row, After::Stop(request_id))
            }
            Command::Retry { request_id, uid } => {
                let mut row = self.row(&request_id, &uid)?;
                if !row.retryable() {
                    return Err(Error::WrongState);
                }
                if self.critical_conflict(&row.request) {
                    return Err(Error::WrongState);
                }
                row.state = State::CheckingDraft;
                row.approved_draft = None;
                row.draft_token = None;
                row.issue = None;
                self.change(row, After::Inspect(request_id))
            }
            Command::Discard { request_id, uid } => {
                let mut row = self.row(&request_id, &uid)?;
                if row.state == State::Discarded {
                    return Ok(vec![Effect::Replay {
                        receipt: Box::new(row),
                        pending: false,
                    }]);
                }
                if row.attempts != 0
                    || !matches!(
                        row.state,
                        State::CheckingDraft | State::DraftConflict | State::FailedBeforeWrite
                    )
                {
                    return Err(Error::WrongState);
                }
                row.state = State::Discarded;
                row.approved_draft = None;
                row.issue = None;
                self.change(row, After::None)
            }
            Command::Dismiss { request_id, uid } => {
                let mut row = self.row(&request_id, &uid)?;
                if row.dismissed || !row.visible_in_outbox() {
                    return Ok(vec![Effect::Replay {
                        receipt: Box::new(row),
                        pending: false,
                    }]);
                }
                if row.attempts == 0 {
                    if !matches!(
                        row.state,
                        State::CheckingDraft | State::DraftConflict | State::FailedBeforeWrite
                    ) {
                        return Err(Error::WrongState);
                    }
                    row.state = State::Discarded;
                    row.approved_draft = None;
                    row.issue = None;
                    return self.change(row, After::None);
                }
                // Python permits retiring an injecting/confirming row. Hiding
                // it leaves the authorized operation and callback intact.
                // Display-only: keep the row's revision so an authorized
                // one-shot callback is not invalidated, and keep its state.
                row.dismissed = true;
                let mut next = self.committed.clone();
                next.receipts.insert(request_id, row);
                self.propose(next, After::None)
            }
            Command::InspectNative {
                request_id,
                uid,
                replay,
            } => {
                let row = self.row(&request_id, &uid)?;
                if !matches!(
                    row.state,
                    State::Uncertain | State::Acknowledged | State::StopRequested
                ) {
                    return Err(Error::WrongState);
                }
                let confirmation = row
                    .confirmation
                    .clone()
                    .ok_or(Error::Invalid("missing confirmation cursor"))?;
                let from = if replay {
                    confirmation.clone()
                } else {
                    row.watch
                        .clone()
                        .ok_or(Error::Invalid("missing watch cursor"))?
                };
                Ok(vec![Effect::InspectNative {
                    operation: self.token(&row),
                    from,
                    confirmation,
                    replay,
                }])
            }
            Command::AdvanceWatch {
                operation,
                previous,
                next,
                validated_confirmation,
            } => {
                let mut row = self.operation(
                    &operation,
                    &[State::Uncertain, State::Acknowledged, State::StopRequested],
                )?;
                validate_cursor(&next)?;
                if row.watch.as_ref() != Some(&previous)
                    || row.confirmation.as_ref() != Some(&validated_confirmation)
                    || next.source_identity != validated_confirmation.source_identity
                    || next.position < previous.position
                    || (next.position == previous.position && next != previous)
                {
                    return Err(Error::UnprovenAcknowledgment);
                }
                if next == previous {
                    return Ok(Vec::new());
                }
                row.watch = Some(next);
                self.change(row, After::None)
            }
            Command::NativeReset { operation, reason } => {
                let mut row =
                    self.operation(&operation, &[State::Uncertain, State::EnterInFlight])?;
                row.state = State::Uncertain;
                row.issue = Some(reason);
                // Never move the original confirmation fence after a reset.
                self.change(row, After::None)
            }
            Command::Persisted(_) => unreachable!(),
        }
    }

    fn acknowledge(&mut self, id: &str, evidence: AckEvidence) -> Result<Vec<Effect>, Error> {
        let mut row = self.row(id, &evidence.uid)?;
        if let Some(accepted) = &row.accepted {
            return if accepted == &evidence.record {
                Ok(vec![Effect::Replay {
                    receipt: Box::new(row),
                    pending: false,
                }])
            } else {
                Err(Error::Conflict)
            };
        }
        let boundary = row
            .confirmation
            .as_ref()
            .ok_or(Error::UnprovenAcknowledgment)?;
        if !matches!(row.state, State::Uncertain | State::EnterInFlight)
            || row.enter_operation.is_none()
            || boundary != &evidence.validated_confirmation
            || boundary.source_identity != evidence.record.source_identity
            || evidence.record.start < boundary.position
            || evidence.record.end <= evidence.record.start
            || evidence.record.record_id.is_empty()
            || !evidence.real_user_input
            || row.request.payload.text.trim() != evidence.text.trim()
            || row.request.payload.media != evidence.observed_media
        {
            return Err(Error::UnprovenAcknowledgment);
        }
        let correlated = match &evidence.correlation {
            Correlation::NativeRequestId(request) => request == &row.request.request_id,
            Correlation::OperationTurn {
                enter_operation,
                turn_id,
            } => {
                row.enter_operation.as_ref() == Some(enter_operation)
                    && turn_id == &evidence.record.turn_id
            }
            // Python retires the first causal native user record whose prompt
            // matches after trimming both ends. The fixed confirmation cursor
            // above supplies the same boundary here.
            Correlation::PossibleTextMatch => true,
        };
        if !correlated {
            return Err(Error::UnprovenAcknowledgment);
        }
        if self
            .committed
            .receipts
            .values()
            .filter_map(|other| other.accepted.as_ref())
            .any(|accepted| {
                accepted.source_identity == evidence.record.source_identity
                    && (accepted.record_id == evidence.record.record_id
                        || accepted.start == evidence.record.start
                        || (!evidence.record.turn_id.is_empty()
                            && accepted.turn_id == evidence.record.turn_id))
            })
        {
            return Err(Error::UnprovenAcknowledgment);
        }
        row.accepted = Some(evidence.record);
        row.association = Some(evidence.correlation);
        row.state = State::Acknowledged;
        row.issue = None;
        self.change(row, After::None)
    }

    fn row(&self, id: &str, uid: &str) -> Result<Receipt, Error> {
        let row = self.committed.receipts.get(id).ok_or(Error::Missing)?;
        if row.request.payload.uid != uid {
            return Err(Error::Missing);
        }
        Ok(row.clone())
    }

    fn critical_conflict(&self, request: &Request) -> bool {
        self.committed.receipts.values().any(|other| {
            let payload = &other.request.payload;
            other.request.request_id != request.request_id
                && matches!(
                    other.state,
                    State::CheckingDraft | State::PrepareInFlight | State::EnterInFlight
                )
                && (payload.uid == request.payload.uid
                    || (payload.target.host_instance == request.payload.target.host_instance
                        && payload.target.session_id == request.payload.target.session_id))
        })
    }

    fn token(&self, row: &Receipt) -> Operation {
        Operation {
            epoch: self.epoch.clone(),
            request_id: row.request.request_id.clone(),
            revision: row.revision,
        }
    }

    fn operation(&self, operation: &Operation, states: &[State]) -> Result<Receipt, Error> {
        let row = self
            .committed
            .receipts
            .get(&operation.request_id)
            .ok_or(Error::StaleOperation)?;
        if self.token(row) != *operation {
            return Err(Error::StaleOperation);
        }
        if !states.contains(&row.state) {
            return Err(Error::WrongState);
        }
        Ok(row.clone())
    }

    fn next_revision(&self) -> Result<u64, Error> {
        self.committed
            .version
            .revision
            .checked_add(1)
            .ok_or(Error::Capacity)
    }

    fn change(&mut self, mut row: Receipt, after: After) -> Result<Vec<Effect>, Error> {
        row.revision = self.next_revision()?;
        let mut next = self.committed.clone();
        next.receipts.insert(row.request.request_id.clone(), row);
        self.propose(next, after)
    }

    fn propose(&mut self, mut snapshot: Snapshot, after: After) -> Result<Vec<Effect>, Error> {
        snapshot.version = Version {
            epoch: self.epoch.clone(),
            revision: self.next_revision()?,
        };
        validate_snapshot(&snapshot)?;
        let effect = Effect::Persist {
            version: snapshot.version.clone(),
            snapshot: snapshot.clone(),
        };
        self.pending = Some(Pending { snapshot, after });
        Ok(vec![effect])
    }

    fn commit(&mut self, version: Version) -> Result<Vec<Effect>, Error> {
        if self
            .pending
            .as_ref()
            .map(|pending| &pending.snapshot.version)
            != Some(&version)
        {
            return Err(Error::StaleOperation);
        }
        let pending = self.pending.take().expect("version checked above");
        self.committed = pending.snapshot;
        let (id, kind) = match pending.after {
            After::None => return Ok(Vec::new()),
            After::Inspect(id) => (id, 0),
            After::Prepare {
                id,
                frame,
                overwrite,
            } => {
                let row = &self.committed.receipts[&id];
                return Ok(vec![Effect::InjectPrepare {
                    operation: self.token(row),
                    payload: row.request.payload.clone(),
                    expected_frame: frame,
                    overwrite,
                }]);
            }
            After::Enter { id, frame } => {
                let row = &self.committed.receipts[&id];
                return Ok(vec![Effect::InjectEnter {
                    operation: self.token(row),
                    target: row.request.payload.target.clone(),
                    expected_frame: frame,
                }]);
            }
            After::Stop(id) => (id, 1),
        };
        let row = &self.committed.receipts[&id];
        if kind == 0 {
            Ok(vec![Effect::InspectComposer {
                operation: self.token(row),
                target: row.request.payload.target.clone(),
            }])
        } else {
            Ok(vec![Effect::InterruptTurn {
                operation: self.token(row),
                target: row.request.payload.target.clone(),
                turn_id: row
                    .accepted
                    .as_ref()
                    .expect("stop requires native acceptance")
                    .turn_id
                    .clone(),
            }])
        }
    }
}

fn nonempty(value: &str, max: usize) -> Result<(), Error> {
    if value.trim().is_empty() || value.len() > max {
        Err(Error::Invalid("missing or oversized identity"))
    } else {
        Ok(())
    }
}
fn present(value: &str) -> Result<(), Error> {
    if value.trim().is_empty() {
        Err(Error::Invalid("missing identity"))
    } else {
        Ok(())
    }
}

fn validate_cursor(cursor: &NativeCursor) -> Result<(), Error> {
    nonempty(&cursor.source_identity, 1024)?;
    nonempty(&cursor.head, 256)?;
    nonempty(&cursor.anchor, 256)
}

fn validate_request(request: &Request) -> Result<(), Error> {
    if request.request_id.is_empty() || request.request_id.chars().count() > 128 {
        return Err(Error::Invalid(
            "request_id must be 1..128 Unicode characters",
        ));
    }
    nonempty(&request.payload.uid, 256)?;
    nonempty(&request.payload.target.host_instance, 256)?;
    nonempty(&request.payload.target.session_id, 256)?;
    nonempty(&request.payload.target.ownership_epoch, 256)?;
    nonempty(&request.payload.text, MAX_PAYLOAD_BYTES)?;
    Ok(())
}

pub(super) fn validate_snapshot(snapshot: &Snapshot) -> Result<(), Error> {
    if snapshot.schema != SCHEMA {
        return Err(Error::Invalid("unsupported delivery snapshot schema"));
    }
    present(&snapshot.version.epoch)?;
    let mut consumed = BTreeSet::new();
    for (id, row) in &snapshot.receipts {
        validate_request(&row.request)?;
        if id != &row.request.request_id
            || row.revision > snapshot.version.revision
            || row.revision == 0
        {
            return Err(Error::Invalid("receipt identity/revision is inconsistent"));
        }
        if row.attempts > 1 {
            return Err(Error::Invalid(
                "an armed submission cannot have been blindly retried",
            ));
        }
        if row.dismissed && row.attempts == 0 {
            return Err(Error::Invalid("only an attempted receipt can be dismissed"));
        }
        if row.attempts == 0
            && (row.enter_operation.is_some() || row.confirmation.is_some() || row.watch.is_some())
        {
            return Err(Error::Invalid(
                "pre-write receipt contains prior submission markers",
            ));
        }
        if row.state == State::DraftConflict && row.draft_token.is_none() {
            return Err(Error::Invalid("draft conflict has no frame identity"));
        }
        for token in [row.draft_token.as_ref(), row.approved_draft.as_ref()]
            .into_iter()
            .flatten()
        {
            nonempty(token, 256)?;
        }
        if let Some(operation) = &row.enter_operation {
            present(&operation.epoch)?;
            if operation.request_id != *id
                || operation.revision == 0
                || operation.revision > row.revision
            {
                return Err(Error::Invalid("invalid persisted Enter operation identity"));
            }
        }
        if row.state == State::EnterInFlight && row.enter_operation.is_none() {
            return Err(Error::Invalid(
                "Enter boundary lacks its operation identity",
            ));
        }
        if row.state == State::PrepareInFlight && row.enter_operation.is_some() {
            return Err(Error::Invalid(
                "prepare boundary already carries an Enter identity",
            ));
        }
        if row.attempts > 0 {
            let confirmation = row
                .confirmation
                .as_ref()
                .ok_or(Error::Invalid("missing durable confirmation fence"))?;
            let watch = row
                .watch
                .as_ref()
                .ok_or(Error::Invalid("missing watch cursor"))?;
            validate_cursor(confirmation)?;
            validate_cursor(watch)?;
            if watch.source_identity != confirmation.source_identity
                || watch.position < confirmation.position
            {
                return Err(Error::Invalid("watch escaped its confirmation fence"));
            }
        }
        if matches!(
            row.state,
            State::CheckingDraft
                | State::DraftConflict
                | State::FailedBeforeWrite
                | State::Discarded
        ) != (row.attempts == 0)
        {
            return Err(Error::Invalid(
                "pre-write and in-flight receipt states conflict",
            ));
        }
        if matches!(
            row.state,
            State::Acknowledged | State::StopRequested | State::Completed | State::Stopped
        ) != row.accepted.is_some()
        {
            return Err(Error::Invalid(
                "accepted state lacks native acceptance evidence",
            ));
        }
        if row.accepted.is_some() != row.association.is_some() {
            return Err(Error::Invalid(
                "native acceptance lacks its association evidence",
            ));
        }
        if matches!(row.state, State::Completed | State::Stopped) != row.completion.is_some() {
            return Err(Error::Invalid("completion state lacks native outcome"));
        }
        if row.completion.is_some() != row.completion_record_end.is_some()
            || (row.state == State::Stopped && row.completion != Some(Completion::Stopped))
            || (row.state == State::Completed && row.completion == Some(Completion::Stopped))
        {
            return Err(Error::Invalid("inconsistent completion evidence"));
        }
        if let Some(accepted) = &row.accepted {
            nonempty(&accepted.record_id, 1024)?;
            let confirmation = row
                .confirmation
                .as_ref()
                .ok_or(Error::Invalid("accepted without confirmation"))?;
            if accepted.source_identity != confirmation.source_identity
                || accepted.start < confirmation.position
                || accepted.end <= accepted.start
                || row.enter_operation.is_none()
            {
                return Err(Error::Invalid("invalid native acknowledgment range"));
            }
            let correlated = match row.association.as_ref() {
                Some(Correlation::NativeRequestId(request)) => request == id,
                Some(Correlation::OperationTurn {
                    enter_operation,
                    turn_id,
                }) => {
                    row.enter_operation.as_ref() == Some(enter_operation)
                        && turn_id == &accepted.turn_id
                }
                Some(Correlation::PossibleTextMatch) => true,
                None => false,
            };
            if !correlated {
                return Err(Error::Invalid(
                    "persisted native association is not a proof",
                ));
            }
            if row
                .completion_record_end
                .is_some_and(|end| end <= accepted.end)
            {
                return Err(Error::Invalid("completion predates accepted input"));
            }
            let mut keys = vec![
                (
                    0,
                    accepted.source_identity.clone(),
                    accepted.record_id.clone(),
                ),
                (
                    2,
                    accepted.source_identity.clone(),
                    accepted.start.to_string(),
                ),
            ];
            if !accepted.turn_id.is_empty() {
                keys.push((
                    1,
                    accepted.source_identity.clone(),
                    accepted.turn_id.clone(),
                ));
            }
            for key in keys {
                if !consumed.insert(key) {
                    return Err(Error::Invalid("native input consumed more than once"));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
