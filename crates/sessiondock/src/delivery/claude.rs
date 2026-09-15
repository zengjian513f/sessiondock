//! Independent, pure Claude prompt/queue delivery domain.
//!
//! The legacy AskUserQuestion hooks identify tools, not SessionDock submissions.
//! Strong associations below require a future trusted adapter. Text heuristics
//! never turn server persistence, native queuing or transport success into a
//! committed user prompt. No effect is executed by this module.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

// Matches the terminal host's text input limit; receipt history is not quota.
const MAX_PAYLOAD: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Scope {
    pub uid: String,
    pub session_id: String,
    /// None is the main conversation; a child never implicitly targets it.
    pub agent_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub host_instance: String,
    pub terminal_id: String,
    pub ownership_epoch: String,
}

/// Opaque outbox preview metadata. Persists every JSON value supplied
/// by the composer; the uploaded file path is already part of `text` and this
/// value is never a second terminal input channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Attachment(pub serde_json::Value);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload {
    pub scope: Scope,
    pub target: Target,
    pub text: String,
    pub attachments: Vec<Attachment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub payload: Payload,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub source_identity: String,
    pub offset: u64,
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
    Queued,
    Inspecting,
    DraftConflict,
    PrepareInFlight,
    EnterInFlight,
    Uncertain,
    NativeQueued,
    NativeDequeued,
    NativeRemoved,
    Accepted,
    StopRequested,
    Restored,
    Interrupted,
    Completed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub user_uuid: String,
    /// The contextual parent is not this request's own accepted turn.
    pub parent_turn_uuid: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Succeeded,
    Failed,
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Association {
    NativeRequestId(String),
    VerifiedEnter(Operation),
    /// A verified reference in this native record to its original queue entry.
    QueueEntry(String),
    PossibleTextMatch,
    /// Existing Claude bridge hooks provide only this kind of tool identity.
    QuestionToolHook(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRecord {
    pub source_identity: String,
    pub id: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeQueue {
    pub enqueue: NativeRecord,
    pub association: Association,
    pub last_change: Option<NativeRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Acceptance {
    pub record: NativeRecord,
    pub turn: Turn,
    pub association: Association,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnWitness {
    pub id: String,
    pub enter_operation: Operation,
    pub restored_frame: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub request: Request,
    pub sequence: u64,
    pub created_ms: u64,
    pub revision: u64,
    pub state: State,
    pub attempted: bool,
    pub draft_token: Option<String>,
    pub approved_draft: Option<String>,
    pub confirmation: Option<Cursor>,
    pub watch: Option<Cursor>,
    pub enter: Option<Operation>,
    pub native_queue: Option<NativeQueue>,
    pub accepted: Option<Acceptance>,
    pub returned: Option<ReturnWitness>,
    pub outcome: Option<Outcome>,
    pub completed_record: Option<NativeRecord>,
    /// UI dismissal is not acceptance, cancellation or forgetting the request.
    pub dismissed: bool,
    pub issue: Option<String>,
}

impl Receipt {
    pub fn visible_in_outbox(&self) -> bool {
        !self.dismissed
            && !matches!(
                self.state,
                State::Accepted | State::StopRequested | State::Completed | State::Cancelled
            )
    }

    fn local_waiter(&self) -> bool {
        matches!(
            self.state,
            State::Queued | State::Inspecting | State::DraftConflict
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema: u32,
    pub version: Version,
    pub next_sequence: u64,
    pub receipts: BTreeMap<String, Receipt>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerState {
    Empty,
    Editing,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composer {
    pub target: Target,
    pub state: ComposerState,
    pub frame_token: String,
    pub native_cursor: Cursor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub target: Target,
    pub observed_text: String,
    pub observed_attachments: Vec<Attachment>,
    pub frame_token: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeContext {
    pub scope: Scope,
    /// The trusted reader revalidated the ORIGINAL immutable input fence.
    pub confirmation: Cursor,
    pub record: NativeRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueChange {
    Enqueue {
        text: String,
        attachments: Vec<Attachment>,
        association: Association,
    },
    Dequeue {
        enqueue_record_id: String,
    },
    Remove {
        enqueue_record_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueEvidence {
    pub context: NativeContext,
    pub change: QueueChange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserEvidence {
    pub context: NativeContext,
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub turn: Turn,
    pub real_human_input: bool,
    pub association: Association,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionEvidence {
    pub context: NativeContext,
    pub turn: Turn,
    pub ancestor_user_uuid: String,
    pub selected_lineage: bool,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReturnEvidence {
    pub scope: Scope,
    pub target: Target,
    pub enter_operation: Operation,
    pub witness_id: String,
    /// Some means the exact payload was observed restored into the editor.
    pub restored: Option<Prepared>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Enqueue {
        request: Request,
        now_ms: u64,
    },
    Persisted(Version),
    DispatchNext {
        scope: Scope,
    },
    ComposerObserved {
        operation: Operation,
        composer: Composer,
    },
    ConfirmDraft {
        id: String,
        scope: Scope,
        token: String,
    },
    Prepared {
        operation: Operation,
        evidence: Prepared,
    },
    InjectionFinished {
        operation: Operation,
        transport_returned: bool,
    },
    InjectionFailed {
        operation: Operation,
        reason: String,
    },
    ObserveQueue {
        id: String,
        evidence: QueueEvidence,
    },
    ObserveUser {
        id: String,
        evidence: UserEvidence,
    },
    ObserveCompletion {
        id: String,
        evidence: CompletionEvidence,
    },
    ObserveReturn {
        id: String,
        evidence: ReturnEvidence,
    },
    RequestStop {
        id: String,
        scope: Scope,
    },
    Cancel {
        id: String,
        scope: Scope,
    },
    Dismiss {
        id: String,
        scope: Scope,
    },
    Timeout {
        id: String,
        scope: Scope,
    },
    InspectNative {
        id: String,
        scope: Scope,
        replay: bool,
    },
    AdvanceWatch {
        operation: Operation,
        previous: Cursor,
        next: Cursor,
        confirmation: Cursor,
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
    PrepareInput {
        operation: Operation,
        payload: Payload,
        expected_frame: String,
        overwrite: bool,
    },
    PressEnter {
        operation: Operation,
        target: Target,
        expected_frame: String,
    },
    InterruptTurn {
        operation: Operation,
        scope: Scope,
        target: Target,
        turn: Turn,
    },
    InspectNative {
        operation: Operation,
        scope: Scope,
        from: Cursor,
        confirmation: Cursor,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid(&'static str),
    Conflict,
    Missing,
    Busy,
    WrongState,
    Stale,
    Pending,
    Unproven,
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
    Prepare(String, String, bool),
    Enter(String, String),
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
                schema: 1,
                version: Version {
                    epoch: epoch.clone(),
                    revision: 0,
                },
                next_sequence: 1,
                receipts: BTreeMap::new(),
            },
            epoch,
            pending: None,
        })
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.committed
    }

    pub fn restore(snapshot: Snapshot, epoch: String) -> Result<(Self, Vec<Effect>), Error> {
        validate(&snapshot)?;
        present(&epoch)?;
        if epoch == snapshot.version.epoch {
            return Err(Error::Invalid("recovery requires a fresh epoch"));
        }
        let mut machine = Self {
            epoch,
            committed: snapshot.clone(),
            pending: None,
        };
        let mut next = snapshot;
        let revision = machine.next_revision()?;
        for row in next.receipts.values_mut() {
            row.approved_draft = None;
            match row.state {
                State::Inspecting => {
                    row.state = State::Queued;
                    row.revision = revision;
                }
                State::PrepareInFlight | State::EnterInFlight => {
                    row.state = State::Uncertain;
                    row.revision = revision;
                    row.issue = Some("重启时注入边界不明；不得重放清稿、粘贴或 Enter".into());
                }
                _ => {}
            }
        }
        let effects = machine.propose(next, After::None)?;
        Ok((machine, effects))
    }

    pub fn apply(&mut self, command: Command) -> Result<Vec<Effect>, Error> {
        if let Command::Persisted(version) = command {
            return self.commit(version);
        }
        if let Command::Enqueue { request, .. } = &command {
            validate_request(request)?;
            let existing = self
                .pending
                .as_ref()
                .and_then(|pending| pending.snapshot.receipts.get(&request.id))
                .or_else(|| self.committed.receipts.get(&request.id));
            if let Some(existing) = existing {
                if existing.request != *request {
                    return Err(Error::Conflict);
                }
                let pending = self
                    .committed
                    .receipts
                    .get(&request.id)
                    .is_none_or(|row| row.revision != existing.revision);
                return Ok(vec![Effect::Replay {
                    receipt: Box::new(existing.clone()),
                    pending,
                }]);
            }
        }
        if self.pending.is_some() {
            return Err(Error::Pending);
        }
        match command {
            Command::Enqueue { request, now_ms } => {
                let sequence = self.committed.next_sequence;
                let mut next = self.committed.clone();
                next.next_sequence = sequence.checked_add(1).ok_or(Error::Capacity)?;
                next.receipts.insert(
                    request.id.clone(),
                    Receipt {
                        request,
                        sequence,
                        created_ms: now_ms,
                        revision: self.next_revision()?,
                        state: State::Queued,
                        attempted: false,
                        draft_token: None,
                        approved_draft: None,
                        confirmation: None,
                        watch: None,
                        enter: None,
                        native_queue: None,
                        accepted: None,
                        returned: None,
                        outcome: None,
                        completed_record: None,
                        dismissed: false,
                        issue: None,
                    },
                );
                self.propose(next, After::None)
            }
            Command::DispatchNext { scope } => {
                let mut row = self
                    .committed
                    .receipts
                    .values()
                    .filter(|row| row.request.payload.scope == scope && row.local_waiter())
                    .min_by_key(|row| row.sequence)
                    .cloned()
                    .ok_or(Error::Missing)?;
                if row.state != State::Queued || self.critical_conflict(&row.request) {
                    return Err(Error::Busy);
                }
                row.state = State::Inspecting;
                let id = row.request.id.clone();
                self.change(row, After::Inspect(id))
            }
            Command::ComposerObserved {
                operation,
                composer,
            } => {
                let mut row = self.operation(&operation, &[State::Inspecting])?;
                if composer.target != row.request.payload.target {
                    return Err(Error::Conflict);
                }
                if composer.state == ComposerState::Unknown {
                    row.state = State::Queued;
                    row.approved_draft = None;
                    row.issue = Some("不能确认 Claude 编辑器，尚未授权写入".into());
                    return self.change(row, After::None);
                }
                identity(&composer.frame_token, 256)?;
                validate_cursor(&composer.native_cursor)?;
                if composer.state == ComposerState::Editing
                    && row.approved_draft.as_ref() != Some(&composer.frame_token)
                {
                    row.state = State::DraftConflict;
                    row.draft_token = Some(composer.frame_token);
                    row.approved_draft = None;
                    row.issue = Some("有未提交草稿；必须确认此版本并重新检查".into());
                    return self.change(row, After::None);
                }
                row.state = State::PrepareInFlight;
                row.attempted = true;
                row.issue = None;
                row.confirmation = Some(composer.native_cursor.clone());
                row.watch = Some(composer.native_cursor);
                let after = After::Prepare(
                    row.request.id.clone(),
                    composer.frame_token,
                    composer.state == ComposerState::Editing,
                );
                self.change(row, after)
            }
            Command::ConfirmDraft { id, scope, token } => {
                let mut row = self.row(&id, &scope)?;
                if row.state != State::DraftConflict || row.draft_token.as_ref() != Some(&token) {
                    return Err(Error::Conflict);
                }
                if self.critical_conflict(&row.request) {
                    return Err(Error::Busy);
                }
                row.approved_draft = Some(token);
                row.state = State::Inspecting;
                row.issue = None;
                self.change(row, After::Inspect(id))
            }
            Command::Prepared {
                operation,
                evidence,
            } => {
                let mut row = self.operation(&operation, &[State::PrepareInFlight])?;
                if !prepared_matches(&row, &evidence) {
                    row.state = State::Uncertain;
                    row.issue = Some("未证明完整正文准备完毕；不发送 Enter".into());
                    return self.change(row, After::None);
                }
                row.state = State::EnterInFlight;
                row.enter = Some(Operation {
                    epoch: self.epoch.clone(),
                    request_id: row.request.id.clone(),
                    revision: self.next_revision()?,
                });
                let after = After::Enter(row.request.id.clone(), evidence.frame_token);
                self.change(row, after)
            }
            Command::InjectionFinished {
                operation,
                transport_returned,
            } => {
                let mut row = self.operation(&operation, &[State::EnterInFlight])?;
                row.state = State::Uncertain;
                row.issue = Some(
                    if transport_returned {
                        "终端调用已返回，但尚无原生接收证明"
                    } else {
                        "终端调用结果不明；禁止重新注入"
                    }
                    .into(),
                );
                self.change(row, After::None)
            }
            Command::InjectionFailed { operation, reason } => {
                let mut row =
                    self.operation(&operation, &[State::PrepareInFlight, State::EnterInFlight])?;
                row.state = State::Uncertain;
                row.issue = Some(reason);
                self.change(row, After::None)
            }
            Command::ObserveQueue { id, evidence } => self.queue_event(&id, evidence),
            Command::ObserveUser { id, evidence } => self.accept(&id, evidence),
            Command::ObserveCompletion { id, evidence } => {
                let mut row = self.row(&id, &evidence.context.scope)?;
                context_matches(&row, &evidence.context)?;
                let accepted = row.accepted.as_ref().ok_or(Error::Unproven)?;
                if !evidence.selected_lineage
                    || evidence.turn != accepted.turn
                    || evidence.ancestor_user_uuid != accepted.turn.user_uuid
                    || evidence.context.record.start < accepted.record.end
                {
                    return Err(Error::Unproven);
                }
                if row.state == State::Completed {
                    return if row.outcome == Some(evidence.outcome) {
                        Ok(replay(row))
                    } else {
                        Err(Error::Conflict)
                    };
                }
                if !matches!(row.state, State::Accepted | State::StopRequested) {
                    return Err(Error::WrongState);
                }
                row.state = State::Completed;
                row.outcome = Some(evidence.outcome);
                row.completed_record = Some(evidence.context.record);
                row.issue = None;
                self.change(row, After::None)
            }
            Command::ObserveReturn { id, evidence } => {
                let mut row = self.row(&id, &evidence.scope)?;
                if !matches!(
                    row.state,
                    State::Uncertain | State::EnterInFlight | State::Restored | State::Interrupted
                ) || row.enter.as_ref() != Some(&evidence.enter_operation)
                    || evidence.target != row.request.payload.target
                    || identity(&evidence.witness_id, 256).is_err()
                {
                    return Err(Error::Unproven);
                }
                let restored_frame = evidence
                    .restored
                    .as_ref()
                    .map(|prepared| prepared.frame_token.clone());
                row.state = if let Some(prepared) = evidence.restored {
                    if !prepared_matches(&row, &prepared) {
                        return Err(Error::Unproven);
                    }
                    State::Restored
                } else {
                    State::Interrupted
                };
                row.returned = Some(ReturnWitness {
                    id: evidence.witness_id,
                    enter_operation: evidence.enter_operation,
                    restored_frame,
                });
                row.issue =
                    Some("已关联的输入中断/回填观察；仍不得盲重发，迟到原生记录可修正状态".into());
                self.change(row, After::None)
            }
            Command::RequestStop { id, scope } => {
                let mut row = self.row(&id, &scope)?;
                if row.state == State::StopRequested {
                    return Ok(replay(row));
                }
                if row.state != State::Accepted || self.critical_conflict(&row.request) {
                    return Err(Error::WrongState);
                }
                row.state = State::StopRequested;
                self.change(row, After::Stop(id))
            }
            Command::Cancel { id, scope } => {
                let mut row = self.row(&id, &scope)?;
                if row.state == State::Cancelled {
                    return Ok(replay(row));
                }
                if row.attempted || !row.local_waiter() {
                    return Err(Error::WrongState);
                }
                row.state = State::Cancelled;
                row.approved_draft = None;
                row.issue = None;
                self.change(row, After::None)
            }
            Command::Dismiss { id, scope } => {
                let mut row = self.row(&id, &scope)?;
                if row.dismissed {
                    return Ok(replay(row));
                }
                // Hiding an unsubmitted local waiter also prevents future dispatch.
                if !row.attempted && row.local_waiter() {
                    row.state = State::Cancelled;
                    row.approved_draft = None;
                }
                row.dismissed = true;
                if row.attempted {
                    // A display-only dismissal must not invalidate an already
                    // authorized one-shot write's callback or strand its state.
                    let mut next = self.committed.clone();
                    next.receipts.insert(id, row);
                    self.propose(next, After::None)
                } else {
                    self.change(row, After::None)
                }
            }
            Command::Timeout { id, scope } => {
                let mut row = self.row(&id, &scope)?;
                if !row.attempted
                    || matches!(
                        row.state,
                        State::Accepted | State::StopRequested | State::Completed
                    )
                {
                    return Err(Error::WrongState);
                }
                if matches!(row.state, State::PrepareInFlight | State::EnterInFlight) {
                    row.state = State::Uncertain
                }
                row.issue = Some("等待确认超时，只允许限频复核，不能重新注入".into());
                self.change(row, After::None)
            }
            Command::InspectNative { id, scope, replay } => {
                let row = self.row(&id, &scope)?;
                if !row.attempted
                    || matches!(
                        row.state,
                        State::PrepareInFlight | State::EnterInFlight | State::Completed
                    )
                {
                    return Err(Error::WrongState);
                }
                let confirmation = row.confirmation.clone().ok_or(Error::Unproven)?;
                let from = if replay {
                    confirmation.clone()
                } else {
                    row.watch.clone().ok_or(Error::Unproven)?
                };
                Ok(vec![Effect::InspectNative {
                    operation: self.token(&row),
                    scope,
                    from,
                    confirmation,
                }])
            }
            Command::AdvanceWatch {
                operation,
                previous,
                next,
                confirmation,
            } => {
                let mut row = self.operation(
                    &operation,
                    &[
                        State::Uncertain,
                        State::NativeQueued,
                        State::NativeDequeued,
                        State::NativeRemoved,
                        State::Restored,
                        State::Interrupted,
                        State::Accepted,
                        State::StopRequested,
                    ],
                )?;
                validate_cursor(&next)?;
                if row.confirmation.as_ref() != Some(&confirmation)
                    || row.watch.as_ref() != Some(&previous)
                    || next.source_identity != confirmation.source_identity
                    || next.offset < previous.offset
                    || (next.offset == previous.offset && next != previous)
                {
                    return Err(Error::Unproven);
                }
                if next == previous {
                    return Ok(Vec::new());
                }
                row.watch = Some(next);
                self.change(row, After::None)
            }
            Command::Persisted(_) => unreachable!(),
        }
    }

    fn queue_event(&mut self, id: &str, evidence: QueueEvidence) -> Result<Vec<Effect>, Error> {
        let mut row = self.row(id, &evidence.context.scope)?;
        if row.enter.is_none() {
            return Err(Error::Unproven);
        }
        context_matches(&row, &evidence.context)?;
        if row.accepted.is_some() {
            return Err(Error::WrongState);
        }
        match evidence.change {
            QueueChange::Enqueue {
                text,
                attachments,
                association,
            } => {
                if let Some(queue) = &row.native_queue {
                    return if queue.enqueue == evidence.context.record {
                        Ok(replay(row))
                    } else {
                        Err(Error::Conflict)
                    };
                }
                if !matches!(row.state, State::Uncertain | State::EnterInFlight)
                    || !payload_matches(&row, &text, &attachments)
                    || !associated(&row, &association, false)
                {
                    return Err(Error::Unproven);
                }
                if self
                    .committed
                    .receipts
                    .values()
                    .filter_map(|other| other.native_queue.as_ref())
                    .any(|queue| same_record(&queue.enqueue, &evidence.context.record))
                {
                    return Err(Error::Unproven);
                }
                row.native_queue = Some(NativeQueue {
                    enqueue: evidence.context.record,
                    association,
                    last_change: None,
                });
                row.state = State::NativeQueued;
            }
            change @ (QueueChange::Dequeue { .. } | QueueChange::Remove { .. }) => {
                let (enqueue_id, removed) = match change {
                    QueueChange::Dequeue { enqueue_record_id } => (enqueue_record_id, false),
                    QueueChange::Remove { enqueue_record_id } => (enqueue_record_id, true),
                    _ => unreachable!(),
                };
                let queue = row.native_queue.as_ref().ok_or(Error::Unproven)?;
                if enqueue_id != queue.enqueue.id
                    || evidence.context.record.start < queue.enqueue.end
                {
                    return Err(Error::Unproven);
                }
                if queue.last_change.as_ref() == Some(&evidence.context.record) {
                    return if (removed && row.state == State::NativeRemoved)
                        || (!removed && row.state == State::NativeDequeued)
                    {
                        Ok(replay(row))
                    } else {
                        Err(Error::Conflict)
                    };
                }
                if row.state != State::NativeQueued {
                    return Err(Error::WrongState);
                }
                if !removed
                    && self.committed.receipts.values().any(|other| {
                        other.request.payload.scope == row.request.payload.scope
                            && other.state == State::NativeQueued
                            && other
                                .native_queue
                                .as_ref()
                                .is_some_and(|native| native.enqueue.start < queue.enqueue.start)
                    })
                {
                    return Err(Error::Busy);
                }
                row.native_queue.as_mut().expect("checked").last_change =
                    Some(evidence.context.record);
                row.state = if removed {
                    State::NativeRemoved
                } else {
                    State::NativeDequeued
                };
            }
        }
        row.issue = None;
        self.change(row, After::None)
    }

    fn accept(&mut self, id: &str, evidence: UserEvidence) -> Result<Vec<Effect>, Error> {
        let mut row = self.row(id, &evidence.context.scope)?;
        context_matches(&row, &evidence.context)?;
        if let Some(accepted) = &row.accepted {
            return if accepted.record == evidence.context.record {
                Ok(replay(row))
            } else {
                Err(Error::Conflict)
            };
        }
        if !matches!(
            row.state,
            State::Uncertain
                | State::EnterInFlight
                | State::NativeQueued
                | State::NativeDequeued
                | State::NativeRemoved
                | State::Restored
                | State::Interrupted
        ) || !evidence.real_human_input
            || !payload_matches(&row, &evidence.text, &evidence.attachments)
            || !associated(&row, &evidence.association, true)
            || (row.enter.is_none() && evidence.association != Association::PossibleTextMatch)
        {
            return Err(Error::Unproven);
        }
        validate_turn(&evidence.turn)?;
        if self
            .committed
            .receipts
            .values()
            .filter_map(|other| other.accepted.as_ref())
            .any(|accepted| {
                same_record(&accepted.record, &evidence.context.record)
                    || (accepted.record.source_identity == evidence.context.record.source_identity
                        && accepted.turn.user_uuid == evidence.turn.user_uuid)
            })
        {
            return Err(Error::Unproven);
        }
        if let Some(queue) = &row.native_queue
            && evidence.context.record.start < queue.enqueue.end
        {
            return Err(Error::Unproven);
        }
        row.accepted = Some(Acceptance {
            record: evidence.context.record,
            turn: evidence.turn,
            association: evidence.association,
        });
        row.state = State::Accepted;
        row.issue = None;
        self.change(row, After::None)
    }

    fn row(&self, id: &str, scope: &Scope) -> Result<Receipt, Error> {
        let row = self.committed.receipts.get(id).ok_or(Error::Missing)?;
        if &row.request.payload.scope != scope {
            return Err(Error::Missing);
        }
        Ok(row.clone())
    }

    fn token(&self, row: &Receipt) -> Operation {
        Operation {
            epoch: self.epoch.clone(),
            request_id: row.request.id.clone(),
            revision: row.revision,
        }
    }

    fn operation(&self, operation: &Operation, states: &[State]) -> Result<Receipt, Error> {
        let row = self
            .committed
            .receipts
            .get(&operation.request_id)
            .ok_or(Error::Stale)?;
        if self.token(row) != *operation {
            return Err(Error::Stale);
        }
        if !states.contains(&row.state) {
            return Err(Error::WrongState);
        }
        Ok(row.clone())
    }

    fn critical_conflict(&self, request: &Request) -> bool {
        self.committed.receipts.values().any(|other| {
            other.request.id != request.id
                && matches!(
                    other.state,
                    State::Inspecting
                        | State::PrepareInFlight
                        | State::EnterInFlight
                        | State::StopRequested
                )
                && (other.request.payload.scope == request.payload.scope
                    || (other.request.payload.target.host_instance
                        == request.payload.target.host_instance
                        && other.request.payload.target.terminal_id
                            == request.payload.target.terminal_id))
        })
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
        next.receipts.insert(row.request.id.clone(), row);
        self.propose(next, after)
    }

    fn propose(&mut self, mut snapshot: Snapshot, after: After) -> Result<Vec<Effect>, Error> {
        snapshot.version = Version {
            epoch: self.epoch.clone(),
            revision: self.next_revision()?,
        };
        validate(&snapshot)?;
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
            return Err(Error::Stale);
        }
        let pending = self.pending.take().expect("checked pending version");
        self.committed = pending.snapshot;
        let effect = match pending.after {
            After::None => return Ok(Vec::new()),
            After::Inspect(id) => {
                let row = &self.committed.receipts[&id];
                Effect::InspectComposer {
                    operation: self.token(row),
                    target: row.request.payload.target.clone(),
                }
            }
            After::Prepare(id, frame, overwrite) => {
                let row = &self.committed.receipts[&id];
                Effect::PrepareInput {
                    operation: self.token(row),
                    payload: row.request.payload.clone(),
                    expected_frame: frame,
                    overwrite,
                }
            }
            After::Enter(id, frame) => {
                let row = &self.committed.receipts[&id];
                Effect::PressEnter {
                    operation: self.token(row),
                    target: row.request.payload.target.clone(),
                    expected_frame: frame,
                }
            }
            After::Stop(id) => {
                let row = &self.committed.receipts[&id];
                Effect::InterruptTurn {
                    operation: self.token(row),
                    scope: row.request.payload.scope.clone(),
                    target: row.request.payload.target.clone(),
                    turn: row
                        .accepted
                        .as_ref()
                        .expect("stop requires acceptance")
                        .turn
                        .clone(),
                }
            }
        };
        Ok(vec![effect])
    }
}

fn identity(value: &str, max: usize) -> Result<(), Error> {
    if value.trim().is_empty() || value.len() > max {
        Err(Error::Invalid("missing/oversized identity"))
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
fn replay(row: Receipt) -> Vec<Effect> {
    vec![Effect::Replay {
        receipt: Box::new(row),
        pending: false,
    }]
}

fn validate_request(request: &Request) -> Result<(), Error> {
    if request.id.is_empty() || request.id.chars().count() > 128 {
        return Err(Error::Invalid(
            "request ID must be 1..128 Unicode characters",
        ));
    }
    let payload = &request.payload;
    for value in [
        &payload.scope.uid,
        &payload.scope.session_id,
        &payload.target.host_instance,
        &payload.target.terminal_id,
        &payload.target.ownership_epoch,
    ] {
        identity(value, 256)?
    }
    if let Some(agent) = &payload.scope.agent_id {
        identity(agent, 256)?
    }
    identity(&payload.text, MAX_PAYLOAD)?;
    Ok(())
}

fn validate_cursor(cursor: &Cursor) -> Result<(), Error> {
    identity(&cursor.source_identity, 1024)?;
    identity(&cursor.head, 256)?;
    identity(&cursor.anchor, 256)
}
fn validate_turn(turn: &Turn) -> Result<(), Error> {
    identity(&turn.user_uuid, 256)?;
    if let Some(parent) = &turn.parent_turn_uuid {
        identity(parent, 256)?;
        if parent == &turn.user_uuid {
            return Err(Error::Invalid("turn cannot parent itself"));
        }
    }
    Ok(())
}

fn record_matches(row: &Receipt, record: &NativeRecord) -> Result<(), Error> {
    identity(&record.id, 1024)?;
    let fence = row.confirmation.as_ref().ok_or(Error::Unproven)?;
    if record.source_identity != fence.source_identity
        || record.start < fence.offset
        || record.end <= record.start
    {
        return Err(Error::Unproven);
    }
    Ok(())
}

fn context_matches(row: &Receipt, context: &NativeContext) -> Result<(), Error> {
    if row.request.payload.scope != context.scope
        || row.confirmation.as_ref() != Some(&context.confirmation)
    {
        return Err(Error::Unproven);
    }
    record_matches(row, &context.record)
}

fn associated(row: &Receipt, association: &Association, queue_allowed: bool) -> bool {
    match association {
        Association::NativeRequestId(id) => id == &row.request.id,
        Association::VerifiedEnter(operation) => row.enter.as_ref() == Some(operation),
        Association::QueueEntry(id) => {
            queue_allowed
                && row
                    .native_queue
                    .as_ref()
                    .is_some_and(|queue| queue.enqueue.id == *id)
        }
        Association::PossibleTextMatch => true,
        Association::QuestionToolHook(_) => false,
    }
}

fn same_record(a: &NativeRecord, b: &NativeRecord) -> bool {
    a.source_identity == b.source_identity && (a.id == b.id || a.start == b.start)
}
fn payload_matches(row: &Receipt, text: &str, attachments: &[Attachment]) -> bool {
    row.request.payload.text.trim() == text.trim() && row.request.payload.attachments == attachments
}
fn prepared_matches(row: &Receipt, proof: &Prepared) -> bool {
    row.request.payload.target == proof.target
        && row.request.payload.text == proof.observed_text
        && row.request.payload.attachments == proof.observed_attachments
        && identity(&proof.frame_token, 256).is_ok()
}

pub(super) fn validate(snapshot: &Snapshot) -> Result<(), Error> {
    if snapshot.schema != 1 || snapshot.next_sequence == 0 {
        return Err(Error::Invalid("unsupported Claude ledger schema/order"));
    }
    present(&snapshot.version.epoch)?;
    let mut sequences = BTreeSet::new();
    let mut accepted_records = BTreeSet::new();
    let mut queues = BTreeSet::new();
    for (id, row) in &snapshot.receipts {
        validate_request(&row.request)?;
        if &row.request.id != id
            || row.sequence == 0
            || row.sequence >= snapshot.next_sequence
            || !sequences.insert(row.sequence)
            || row.revision == 0
            || row.revision > snapshot.version.revision
        {
            return Err(Error::Invalid("invalid receipt identity/order/version"));
        }
        let prewrite = matches!(
            row.state,
            State::Queued | State::Inspecting | State::DraftConflict | State::Cancelled
        );
        if prewrite == row.attempted
            || (!row.attempted
                && (row.confirmation.is_some() || row.watch.is_some() || row.enter.is_some()))
        {
            return Err(Error::Invalid("conflicting prewrite/submission markers"));
        }
        if row.attempted {
            let fence = row
                .confirmation
                .as_ref()
                .ok_or(Error::Invalid("missing confirmation fence"))?;
            validate_cursor(fence)?;
            let watch = row
                .watch
                .as_ref()
                .ok_or(Error::Invalid("missing watch cursor"))?;
            validate_cursor(watch)?;
            if watch.source_identity != fence.source_identity || watch.offset < fence.offset {
                return Err(Error::Invalid("watch moved outside confirmation source"));
            }
        }
        for token in [row.draft_token.as_ref(), row.approved_draft.as_ref()]
            .into_iter()
            .flatten()
        {
            identity(token, 256)?
        }
        if row.state == State::DraftConflict && row.draft_token.is_none() {
            return Err(Error::Invalid("draft conflict has no token"));
        }
        if let Some(enter) = &row.enter {
            present(&enter.epoch)?;
            if enter.request_id != *id || enter.revision == 0 || enter.revision > row.revision {
                return Err(Error::Invalid("invalid persisted Enter identity"));
            }
        }
        if row.state == State::EnterInFlight && row.enter.is_none()
            || row.state == State::PrepareInFlight && row.enter.is_some()
        {
            return Err(Error::Invalid("inconsistent injection stage"));
        }
        if let Some(returned) = &row.returned {
            identity(&returned.id, 256)?;
            if row.enter.as_ref() != Some(&returned.enter_operation) {
                return Err(Error::Unproven);
            }
            if let Some(frame) = &returned.restored_frame {
                identity(frame, 256)?
            }
        }
        if matches!(row.state, State::Restored | State::Interrupted) {
            let returned = row.returned.as_ref().ok_or(Error::Invalid(
                "interrupted/restored state lacks associated witness",
            ))?;
            if (row.state == State::Restored) != returned.restored_frame.is_some() {
                return Err(Error::Unproven);
            }
        }
        if matches!(
            row.state,
            State::Accepted | State::StopRequested | State::Completed
        ) != row.accepted.is_some()
        {
            return Err(Error::Invalid("accepted state without proof"));
        }
        if (row.state == State::Completed) != row.outcome.is_some()
            || row.outcome.is_some() != row.completed_record.is_some()
        {
            return Err(Error::Invalid("completed state without native outcome"));
        }
        if matches!(
            row.state,
            State::NativeQueued | State::NativeDequeued | State::NativeRemoved
        ) && row.native_queue.is_none()
        {
            return Err(Error::Invalid("native queue state without proof"));
        }
        if let Some(queue) = &row.native_queue {
            record_matches(row, &queue.enqueue)?;
            if !associated(row, &queue.association, false) || row.enter.is_none() {
                return Err(Error::Unproven);
            }
            for key in [
                (
                    queue.enqueue.source_identity.clone(),
                    false,
                    queue.enqueue.id.clone(),
                ),
                (
                    queue.enqueue.source_identity.clone(),
                    true,
                    queue.enqueue.start.to_string(),
                ),
            ] {
                if !queues.insert(key) {
                    return Err(Error::Invalid("native queue entry consumed twice"));
                }
            }
            if let Some(change) = &queue.last_change {
                record_matches(row, change)?;
                if change.start < queue.enqueue.end {
                    return Err(Error::Unproven);
                }
            }
            if row.state == State::NativeQueued && queue.last_change.is_some()
                || matches!(row.state, State::NativeDequeued | State::NativeRemoved)
                    && queue.last_change.is_none()
            {
                return Err(Error::Invalid("inconsistent native queue transition"));
            }
        }
        if let Some(accepted) = &row.accepted {
            record_matches(row, &accepted.record)?;
            validate_turn(&accepted.turn)?;
            if !associated(row, &accepted.association, true)
                || (row.enter.is_none() && accepted.association != Association::PossibleTextMatch)
            {
                return Err(Error::Unproven);
            }
            for key in [
                (
                    accepted.record.source_identity.clone(),
                    0,
                    accepted.record.id.clone(),
                ),
                (
                    accepted.record.source_identity.clone(),
                    1,
                    accepted.record.start.to_string(),
                ),
                (
                    accepted.record.source_identity.clone(),
                    2,
                    accepted.turn.user_uuid.clone(),
                ),
            ] {
                if !accepted_records.insert(key) {
                    return Err(Error::Invalid("native user/turn consumed twice"));
                }
            }
            if let Some(completed) = &row.completed_record {
                record_matches(row, completed)?;
                if completed.start < accepted.record.end {
                    return Err(Error::Unproven);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
