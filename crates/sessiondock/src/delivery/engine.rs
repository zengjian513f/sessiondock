//! Synchronous, exclusive owner of the development delivery ledger and domains.
//! No native adapter or network endpoint is provided here.

use super::{
    claude, codex,
    store::{self, DeliveryStore},
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

const LOG_LIMIT: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Store(store::Error),
    Codex(codex::Error),
    Claude(claude::Error),
    RandomUnavailable,
    Frozen,
    CommitPending,
    DispatchPending,
    InternalCommand,
    Protocol,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "delivery engine: {self:?}")
    }
}
impl std::error::Error for Error {}

/// External actions deliberately have no Persist/Persisted variant, Clone or Debug.
pub enum Action {
    Codex(CodexAction),
    Claude(ClaudeAction),
}
pub enum CodexAction {
    InspectComposer {
        operation: codex::Operation,
        target: codex::Target,
    },
    Prepare {
        operation: codex::Operation,
        payload: codex::Payload,
        expected_frame: String,
        overwrite: bool,
    },
    Enter {
        operation: codex::Operation,
        target: codex::Target,
        expected_frame: String,
    },
    InterruptTurn {
        operation: codex::Operation,
        target: codex::Target,
        turn_id: String,
    },
    InspectNative {
        operation: codex::Operation,
        from: codex::NativeCursor,
        confirmation: codex::NativeCursor,
        replay: bool,
    },
}
pub enum ClaudeAction {
    InspectComposer {
        operation: claude::Operation,
        target: claude::Target,
    },
    Prepare {
        operation: claude::Operation,
        payload: claude::Payload,
        expected_frame: String,
        overwrite: bool,
    },
    Enter {
        operation: claude::Operation,
        target: claude::Target,
        expected_frame: String,
    },
    InterruptTurn {
        operation: claude::Operation,
        scope: claude::Scope,
        target: claude::Target,
        turn: claude::Turn,
    },
    InspectNative {
        operation: claude::Operation,
        scope: claude::Scope,
        from: claude::Cursor,
        confirmation: claude::Cursor,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Codex,
    Claude,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReceiptInfo {
    pub provider: Provider,
    pub id: String,
    pub revision: u64,
    /// False for a new ID whose first durable commit has not succeeded.
    pub known: bool,
    pub pending: bool,
    pub visible: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogEvent {
    Committed,
    CommitFailed,
    Frozen,
    DispatchIssued,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LogEntry {
    pub sequence: u64,
    pub event: LogEvent,
    pub provider: Option<Provider>,
}

/// Consume via claim before performing work. Dropping an unclaimed nonempty
/// batch freezes its originating engine at the next check; it is never replayed.
#[must_use = "claim external actions or explicitly drop the engine for recovery"]
pub struct DispatchBatch {
    actions: Vec<Action>,
    replays: Vec<ReceiptInfo>,
    lease: Option<Arc<AtomicU8>>,
}
impl DispatchBatch {
    pub fn replays(&self) -> &[ReceiptInfo] {
        &self.replays
    }
    pub fn action_count(&self) -> usize {
        self.actions.len()
    }
    /// Once claimed, the trusted driver owns execution and result reporting.
    /// This is a one-time handoff, not exactly-once execution across crashes.
    pub fn claim(mut self) -> Result<Vec<Action>, Error> {
        if let Some(lease) = self.lease.take() {
            lease
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .map_err(|_| Error::Frozen)?;
        }
        Ok(std::mem::take(&mut self.actions))
    }
}
impl Drop for DispatchBatch {
    fn drop(&mut self) {
        if let Some(lease) = &self.lease {
            lease.store(2, Ordering::Release);
        }
    }
}

enum Proposal {
    Codex {
        expected: codex::Version,
        effect: Box<codex::Effect>,
    },
    Claude {
        expected: claude::Version,
        effect: Box<claude::Effect>,
    },
}

pub struct DeliveryEngine {
    store: DeliveryStore,
    codex: codex::Machine,
    claude: claude::Machine,
    pending: Option<Proposal>,
    dispatch: Option<Arc<AtomicU8>>,
    frozen: bool,
    logs: VecDeque<LogEntry>,
    log_sequence: u64,
    #[cfg(test)]
    commit_fault: Option<(store::Error, bool)>,
}
impl Drop for DeliveryEngine {
    fn drop(&mut self) {
        // A detached batch from a dropped/reopened engine cannot authorize work.
        // Actions claimed before this drop are already the driver's responsibility.
        if let Some(lease) = &self.dispatch {
            let _ = lease.compare_exchange(0, 3, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}
impl DeliveryEngine {
    pub fn initialize(directory: &Path) -> Result<Self, Error> {
        let (ce, he) = (epoch()?, epoch()?);
        let store =
            DeliveryStore::initialize(directory, ce.clone(), he.clone()).map_err(Error::Store)?;
        Ok(Self::assemble(
            store,
            codex::Machine::new(ce).map_err(Error::Codex)?,
            claude::Machine::new(he).map_err(Error::Claude)?,
        ))
    }
    /// Both exact recovery proposals must commit and be acknowledged before
    /// this constructor returns a usable engine. Partial recovery returns Err.
    pub fn open(directory: &Path) -> Result<Self, Error> {
        let store = match DeliveryStore::open(directory) {
            Ok(store) => store,
            Err(store::Error::MissingLedger) => return Self::initialize(directory),
            Err(store::Error::Invalid | store::Error::UnsupportedSchema) => {
                let (ce, he) = (epoch()?, epoch()?);
                let store = DeliveryStore::reset(directory, ce.clone(), he.clone())
                    .map_err(Error::Store)?;
                return Ok(Self::assemble(
                    store,
                    codex::Machine::new(ce).map_err(Error::Codex)?,
                    claude::Machine::new(he).map_err(Error::Claude)?,
                ));
            }
            Err(error) => return Err(Error::Store(error)),
        };
        let snapshots = store.snapshots().map_err(Error::Store)?;
        let (codex, claude) = recover(&store, snapshots)?;
        Ok(Self::assemble(store, codex, claude))
    }
    fn assemble(store: DeliveryStore, codex: codex::Machine, claude: claude::Machine) -> Self {
        Self {
            store,
            codex,
            claude,
            pending: None,
            dispatch: None,
            frozen: false,
            logs: VecDeque::new(),
            log_sequence: 0,
            #[cfg(test)]
            commit_fault: None,
        }
    }
    fn log(&mut self, event: LogEvent, provider: Option<Provider>) {
        self.log_sequence = self.log_sequence.saturating_add(1);
        if self.logs.len() == LOG_LIMIT {
            self.logs.pop_front();
        }
        self.logs.push_back(LogEntry {
            sequence: self.log_sequence,
            event,
            provider,
        });
    }
    fn freeze(&mut self) {
        if !self.frozen {
            self.frozen = true;
            self.log(LogEvent::Frozen, None);
        }
    }
    fn check(&mut self) -> Result<(), Error> {
        if self.frozen {
            return Err(Error::Frozen);
        }
        if let Some(lease) = &self.dispatch {
            match lease.load(Ordering::Acquire) {
                0 => return Err(Error::DispatchPending),
                1 => self.dispatch = None,
                _ => {
                    self.freeze();
                    return Err(Error::Frozen);
                }
            }
        }
        let snapshots = match self.store.snapshots() {
            Ok(snapshots) => snapshots,
            Err(
                store::Error::MissingLedger
                | store::Error::Invalid
                | store::Error::UnsupportedSchema,
            ) => {
                let directory = self.store.directory().to_path_buf();
                let (ce, he) = (epoch()?, epoch()?);
                self.store = DeliveryStore::reset(&directory, ce.clone(), he.clone())
                    .map_err(Error::Store)?;
                self.codex = codex::Machine::new(ce).map_err(Error::Codex)?;
                self.claude = claude::Machine::new(he).map_err(Error::Claude)?;
                self.pending = None;
                return Ok(());
            }
            Err(error) => return Err(Error::Store(error)),
        };
        if self.codex.snapshot() != &snapshots.0 || self.claude.snapshot() != &snapshots.1 {
            let (codex, claude) = recover(&self.store, snapshots)?;
            self.codex = codex;
            self.claude = claude;
            self.pending = None;
        }
        Ok(())
    }
    /// `agent_id` is already resolved by the caller's inventory. The Codex
    /// ledger remains keyed by its native UID.
    pub fn apply_codex(
        &mut self,
        command: codex::Command,
        _agent_id: Option<&str>,
    ) -> Result<DispatchBatch, Error> {
        self.check()?;
        if matches!(command, codex::Command::Persisted(_)) {
            return Err(Error::InternalCommand);
        }
        if !matches!(command, codex::Command::Submit { .. }) && self.pending.is_some() {
            return Err(Error::CommitPending);
        }
        if self.pending.is_some() && !matches!(self.pending, Some(Proposal::Codex { .. })) {
            return Err(Error::CommitPending);
        }
        let expected = self.codex.snapshot().version.clone();
        let effects = self.codex.apply(command).map_err(Error::Codex)?;
        self.codex_effects(expected, effects)
    }
    pub fn apply_claude(&mut self, command: claude::Command) -> Result<DispatchBatch, Error> {
        self.check()?;
        if matches!(command, claude::Command::Persisted(_)) {
            return Err(Error::InternalCommand);
        }
        if !matches!(command, claude::Command::Enqueue { .. }) && self.pending.is_some() {
            return Err(Error::CommitPending);
        }
        if self.pending.is_some() && !matches!(self.pending, Some(Proposal::Claude { .. })) {
            return Err(Error::CommitPending);
        }
        let expected = self.claude.snapshot().version.clone();
        let effects = self.claude.apply(command).map_err(Error::Claude)?;
        self.claude_effects(expected, effects)
    }
    fn codex_effects(
        &mut self,
        expected: codex::Version,
        mut effects: Vec<codex::Effect>,
    ) -> Result<DispatchBatch, Error> {
        if effects
            .iter()
            .any(|e| matches!(e, codex::Effect::Persist { .. }))
        {
            if effects.len() != 1 || self.pending.is_some() {
                self.freeze();
                return Err(Error::Protocol);
            }
            self.pending = Some(Proposal::Codex {
                expected,
                effect: Box::new(effects.remove(0)),
            });
            return self.commit_pending();
        }
        let mut actions = Vec::new();
        let mut replays = Vec::new();
        for effect in effects {
            use codex::Effect::*;
            let action = match effect {
                Replay { receipt, pending } => {
                    replays.push(codex_info(
                        &receipt,
                        self.codex
                            .snapshot()
                            .receipts
                            .contains_key(&receipt.request.request_id),
                        pending,
                    ));
                    continue;
                }
                InspectComposer { operation, target } => {
                    CodexAction::InspectComposer { operation, target }
                }
                InjectPrepare {
                    operation,
                    payload,
                    expected_frame,
                    overwrite,
                } => CodexAction::Prepare {
                    operation,
                    payload,
                    expected_frame,
                    overwrite,
                },
                InjectEnter {
                    operation,
                    target,
                    expected_frame,
                } => CodexAction::Enter {
                    operation,
                    target,
                    expected_frame,
                },
                InterruptTurn {
                    operation,
                    target,
                    turn_id,
                } => CodexAction::InterruptTurn {
                    operation,
                    target,
                    turn_id,
                },
                InspectNative {
                    operation,
                    from,
                    confirmation,
                    replay,
                } => CodexAction::InspectNative {
                    operation,
                    from,
                    confirmation,
                    replay,
                },
                Persist { .. } => unreachable!(),
            };
            actions.push(Action::Codex(action));
        }
        Ok(self.batch(actions, replays))
    }
    fn claude_effects(
        &mut self,
        expected: claude::Version,
        mut effects: Vec<claude::Effect>,
    ) -> Result<DispatchBatch, Error> {
        if effects
            .iter()
            .any(|e| matches!(e, claude::Effect::Persist { .. }))
        {
            if effects.len() != 1 || self.pending.is_some() {
                self.freeze();
                return Err(Error::Protocol);
            }
            self.pending = Some(Proposal::Claude {
                expected,
                effect: Box::new(effects.remove(0)),
            });
            return self.commit_pending();
        }
        let mut actions = Vec::new();
        let mut replays = Vec::new();
        for effect in effects {
            use claude::Effect::*;
            let action = match effect {
                Replay { receipt, pending } => {
                    replays.push(claude_info(
                        &receipt,
                        self.claude
                            .snapshot()
                            .receipts
                            .contains_key(&receipt.request.id),
                        pending,
                    ));
                    continue;
                }
                InspectComposer { operation, target } => {
                    ClaudeAction::InspectComposer { operation, target }
                }
                PrepareInput {
                    operation,
                    payload,
                    expected_frame,
                    overwrite,
                } => ClaudeAction::Prepare {
                    operation,
                    payload,
                    expected_frame,
                    overwrite,
                },
                PressEnter {
                    operation,
                    target,
                    expected_frame,
                } => ClaudeAction::Enter {
                    operation,
                    target,
                    expected_frame,
                },
                InterruptTurn {
                    operation,
                    scope,
                    target,
                    turn,
                } => ClaudeAction::InterruptTurn {
                    operation,
                    scope,
                    target,
                    turn,
                },
                InspectNative {
                    operation,
                    scope,
                    from,
                    confirmation,
                } => ClaudeAction::InspectNative {
                    operation,
                    scope,
                    from,
                    confirmation,
                },
                Persist { .. } => unreachable!(),
            };
            actions.push(Action::Claude(action));
        }
        Ok(self.batch(actions, replays))
    }
    fn batch(&mut self, actions: Vec<Action>, replays: Vec<ReceiptInfo>) -> DispatchBatch {
        let lease = if actions.is_empty() {
            None
        } else {
            self.log(LogEvent::DispatchIssued, None);
            let lease = Arc::new(AtomicU8::new(0));
            self.dispatch = Some(lease.clone());
            Some(lease)
        };
        DispatchBatch {
            actions,
            replays,
            lease,
        }
    }
    /// Retry only the retained exact Persist, never a terminal action.
    pub fn retry_commit(&mut self) -> Result<DispatchBatch, Error> {
        let had_pending = self.pending.is_some();
        self.check()?;
        // A commit can reach disk even when its acknowledgment is lost. In
        // that case `check` reloads and restores the durable snapshot, which
        // consumes the stale in-memory proposal. Recovery deliberately emits
        // no terminal action because an in-flight write is now uncertain.
        if had_pending && self.pending.is_none() {
            return Ok(self.batch(Vec::new(), Vec::new()));
        }
        self.commit_pending()
    }
    fn commit_pending(&mut self) -> Result<DispatchBatch, Error> {
        #[cfg(test)]
        if matches!(self.commit_fault, Some((_, false))) {
            let (error, _) = self.commit_fault.take().expect("checked");
            return self.commit_error(error);
        }
        let Some(proposal) = self.pending.as_ref() else {
            return Err(Error::CommitPending);
        };
        match proposal {
            Proposal::Codex { expected, effect } => {
                let codex::Effect::Persist { version, .. } = effect.as_ref() else {
                    self.freeze();
                    return Err(Error::Protocol);
                };
                let version = version.clone();
                let ack = match self.store.commit_codex(expected, effect) {
                    Ok(ack) => ack,
                    Err(e) => return self.commit_error(e),
                };
                #[cfg(test)]
                if let Some((error, true)) = self.commit_fault.take() {
                    return self.commit_error(error);
                }
                if ack != codex::Command::Persisted(version) {
                    self.freeze();
                    return Err(Error::Protocol);
                }
                self.pending = None;
                let effects = match self.codex.apply(ack) {
                    Ok(e) => e,
                    Err(_) => {
                        self.freeze();
                        return Err(Error::Protocol);
                    }
                };
                self.log(LogEvent::Committed, Some(Provider::Codex));
                self.codex_effects(self.codex.snapshot().version.clone(), effects)
            }
            Proposal::Claude { expected, effect } => {
                let claude::Effect::Persist { version, .. } = effect.as_ref() else {
                    self.freeze();
                    return Err(Error::Protocol);
                };
                let version = version.clone();
                let ack = match self.store.commit_claude(expected, effect) {
                    Ok(ack) => ack,
                    Err(e) => return self.commit_error(e),
                };
                #[cfg(test)]
                if let Some((error, true)) = self.commit_fault.take() {
                    return self.commit_error(error);
                }
                if ack != claude::Command::Persisted(version) {
                    self.freeze();
                    return Err(Error::Protocol);
                }
                self.pending = None;
                let effects = match self.claude.apply(ack) {
                    Ok(e) => e,
                    Err(_) => {
                        self.freeze();
                        return Err(Error::Protocol);
                    }
                };
                self.log(LogEvent::Committed, Some(Provider::Claude));
                self.claude_effects(self.claude.snapshot().version.clone(), effects)
            }
        }
    }
    fn commit_error(&mut self, error: store::Error) -> Result<DispatchBatch, Error> {
        self.log(LogEvent::CommitFailed, None);
        Err(Error::Store(error))
    }
    pub fn receipts(
        &mut self,
        provider: Provider,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ReceiptInfo>, Error> {
        self.check()?;
        Ok(match provider {
            Provider::Codex => self
                .codex
                .snapshot()
                .receipts
                .values()
                .skip(offset)
                .take(limit)
                .map(|r| codex_info(r, true, false))
                .collect(),
            Provider::Claude => self
                .claude
                .snapshot()
                .receipts
                .values()
                .skip(offset)
                .take(limit)
                .map(|r| claude_info(r, true, false))
                .collect(),
        })
    }
    /// Safe diagnostics remain available when frozen. Entries contain no request,
    /// prompt, scope, file path, native proof or adapter-provided reason.
    pub fn logs(&self, after_sequence: u64, limit: usize) -> Result<Vec<LogEntry>, Error> {
        Ok(self
            .logs
            .iter()
            .filter(|e| e.sequence > after_sequence)
            .take(limit)
            .cloned()
            .collect())
    }
    pub fn codex_outbox(&mut self, uid: &str, _agent_id: Option<&str>) -> Result<Outbox, Error> {
        self.check()?;
        let snapshot = self.codex.snapshot();
        let mut rows = Vec::new();
        for row in snapshot
            .receipts
            .values()
            .filter(|r| r.request.payload.uid == uid && r.visible_in_outbox())
        {
            rows.push(OutboxRow::new(
                &row.request.request_id,
                uid,
                &row.request.payload.text,
                row.created_ms,
                codex_state(row.state),
                row.request
                    .payload
                    .media
                    .iter()
                    .map(|item| item.0.clone())
                    .collect(),
            ));
        }
        rows.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        Ok(Outbox {
            outbox: rows,
            outbox_version: OutboxVersion {
                epoch: snapshot.version.epoch.clone(),
                revision: snapshot.version.revision,
            },
        })
    }
    pub fn claude_outbox(&mut self, scope: &claude::Scope) -> Result<Outbox, Error> {
        self.check()?;
        let snapshot = self.claude.snapshot();
        let mut receipts: Vec<_> = snapshot
            .receipts
            .values()
            .filter(|r| &r.request.payload.scope == scope && r.visible_in_outbox())
            .collect();
        receipts.sort_by_key(|r| r.sequence);
        let mut rows = Vec::new();
        for row in receipts {
            let (state, error) = claude_state(row.state);
            rows.push(OutboxRow::new(
                &row.request.id,
                &scope.uid,
                &row.request.payload.text,
                row.created_ms,
                (state, u32::from(row.attempted), error),
                row.request
                    .payload
                    .attachments
                    .iter()
                    .map(|item| item.0.clone())
                    .collect(),
            ));
        }
        Ok(Outbox {
            outbox: rows,
            outbox_version: OutboxVersion {
                epoch: snapshot.version.epoch.clone(),
                revision: snapshot.version.revision,
            },
        })
    }
    /// Executor hooks: committed Claude receipts for the trusted
    /// in-process executor, which needs states, draft tokens, cursors and
    /// timestamps to schedule inspection and answer HTTP replays. These are
    /// full private rows (they carry prompt text) and must never be serialized
    /// to a transport; the outbox projection above remains the display API.
    pub fn claude_receipt(&mut self, id: &str) -> Result<Option<claude::Receipt>, Error> {
        self.check()?;
        Ok(self.claude.snapshot().receipts.get(id).cloned())
    }
    /// All committed Claude receipts, optionally limited to one exact scope,
    /// in persisted sequence order.
    pub fn claude_receipts(
        &mut self,
        scope: Option<&claude::Scope>,
    ) -> Result<Vec<claude::Receipt>, Error> {
        self.check()?;
        let mut rows: Vec<_> = self
            .claude
            .snapshot()
            .receipts
            .values()
            .filter(|row| scope.is_none_or(|scope| &row.request.payload.scope == scope))
            .cloned()
            .collect();
        rows.sort_by_key(|row| row.sequence);
        Ok(rows)
    }
    /// Executor hooks: committed Codex receipts, same trust rules
    /// as the Claude accessors above (private rows, never serialized).
    pub fn codex_receipt(&mut self, id: &str) -> Result<Option<codex::Receipt>, Error> {
        self.check()?;
        Ok(self.codex.snapshot().receipts.get(id).cloned())
    }
    /// All committed Codex receipts, optionally limited to one UID, in
    /// creation order (`created_ms`, then request ID) like the outbox.
    pub fn codex_receipts(&mut self, uid: Option<&str>) -> Result<Vec<codex::Receipt>, Error> {
        self.check()?;
        let mut rows: Vec<_> = self
            .codex
            .snapshot()
            .receipts
            .values()
            .filter(|row| uid.is_none_or(|uid| row.request.payload.uid == uid))
            .cloned()
            .collect();
        rows.sort_by(|a, b| {
            (a.created_ms, &a.request.request_id).cmp(&(b.created_ms, &b.request.request_id))
        });
        Ok(rows)
    }
}

fn recover(
    store: &DeliveryStore,
    (cs, hs): (codex::Snapshot, claude::Snapshot),
) -> Result<(codex::Machine, claude::Machine), Error> {
    let (mut codex, cp) = codex::Machine::restore(cs.clone(), fresh_epoch(&cs.version.epoch)?)
        .map_err(Error::Codex)?;
    let (mut claude, hp) = claude::Machine::restore(hs.clone(), fresh_epoch(&hs.version.epoch)?)
        .map_err(Error::Claude)?;
    let [cp @ codex::Effect::Persist { version: cv, .. }] = cp.as_slice() else {
        return Err(Error::Protocol);
    };
    let [hp @ claude::Effect::Persist { version: hv, .. }] = hp.as_slice() else {
        return Err(Error::Protocol);
    };
    let ca = store.commit_codex(&cs.version, cp).map_err(Error::Store)?;
    if ca != codex::Command::Persisted(cv.clone()) {
        return Err(Error::Protocol);
    }
    if !codex.apply(ca).map_err(Error::Codex)?.is_empty() {
        return Err(Error::Protocol);
    }
    let ha = store.commit_claude(&hs.version, hp).map_err(Error::Store)?;
    if ha != claude::Command::Persisted(hv.clone()) {
        return Err(Error::Protocol);
    }
    if !claude.apply(ha).map_err(Error::Claude)?.is_empty() {
        return Err(Error::Protocol);
    }
    Ok((codex, claude))
}

fn epoch() -> Result<String, Error> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| Error::RandomUnavailable)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn fresh_epoch(previous: &str) -> Result<String, Error> {
    let next = epoch()?;
    if next == previous {
        Err(Error::RandomUnavailable)
    } else {
        Ok(next)
    }
}
fn codex_info(r: &codex::Receipt, known: bool, pending: bool) -> ReceiptInfo {
    ReceiptInfo {
        provider: Provider::Codex,
        id: r.request.request_id.clone(),
        revision: r.revision,
        known,
        pending,
        visible: r.visible_in_outbox(),
    }
}
fn claude_info(r: &claude::Receipt, known: bool, pending: bool) -> ReceiptInfo {
    ReceiptInfo {
        provider: Provider::Claude,
        id: r.request.id.clone(),
        revision: r.revision,
        known,
        pending,
        visible: r.visible_in_outbox(),
    }
}

/// Full snapshot for one exact session scope. Prompt data belongs only in this
/// intentional display projection, never the diagnostics APIs above.
#[derive(Serialize)]
pub struct Outbox {
    pub outbox: Vec<OutboxRow>,
    pub outbox_version: OutboxVersion,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OutboxVersion {
    pub epoch: String,
    pub revision: u64,
}
#[derive(Serialize)]
pub struct OutboxRow {
    pub id: String,
    pub uid: String,
    pub text: String,
    pub media: Vec<serde_json::Value>,
    pub created: u64,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'static str>,
    pub attempts: u32,
    pub server: bool,
    #[serde(rename = "afterTs", skip_serializing_if = "Option::is_none")]
    pub after_ts: Option<String>,
}
impl OutboxRow {
    fn new(
        id: &str,
        uid: &str,
        text: &str,
        created: u64,
        projection: (&'static str, u32, Option<&'static str>),
        media: Vec<serde_json::Value>,
    ) -> Self {
        let (state, attempts, error) = projection;
        Self {
            id: id.into(),
            uid: uid.into(),
            text: text.into(),
            media,
            created,
            state,
            error,
            attempts,
            server: true,
            after_ts: None,
        }
    }
}
fn codex_state(state: codex::State) -> (&'static str, u32, Option<&'static str>) {
    use codex::State::*;
    match state {
        CheckingDraft => ("queued", 0, None),
        DraftConflict => ("failed", 0, Some("终端草稿冲突；需要明确确认")),
        FailedBeforeWrite => ("failed", 0, Some("写入前失败；可显式重试")),
        PrepareInFlight | EnterInFlight | Uncertain => {
            ("failed", 1, Some("发送结果待核对；禁止自动重试"))
        }
        Acknowledged | StopRequested | Completed | Stopped | Discarded => {
            unreachable!("hidden tombstone")
        }
    }
}
fn claude_state(state: claude::State) -> (&'static str, Option<&'static str>) {
    use claude::State::*;
    match state {
        Queued | Inspecting => ("persisted", None),
        DraftConflict | PrepareInFlight | EnterInFlight | Uncertain | NativeRemoved
        | NativeDequeued => ("ambiguous", Some("发送状态待核对；禁止自动重试")),
        NativeQueued => ("native_queued", None),
        Restored => ("restored", None),
        Interrupted => ("aborted", None),
        Accepted | StopRequested | Completed | Cancelled => unreachable!("hidden tombstone"),
    }
}

#[cfg(all(test, unix))]
mod tests;
