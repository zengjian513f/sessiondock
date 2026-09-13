//! Single-writer durable creation receipts. No process, native history or HTTP I/O.
mod disk;
mod json;

use super::model::{
    self, BindingMethod, BindingRecord, BindingSpec, BindingState, Failure, Launch, LaunchSpec,
    MAX_EVIDENCE_BYTES, MAX_RECORDS, Record, State,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub const LEDGER_FILENAME: &str = "lifecycle-ledger.json";
pub const LOCK_FILENAME: &str = ".lifecycle.lock";
pub const MAX_BYTES: usize = 1024 * 1024;
/// Current ledger envelope schema. Schema 5 (WP-E) adds `finished_at` /
/// `discarded` to every record and `method` / `evidence` / `bound_at` to a
/// binding; older envelopes migrate strictly on open.
pub const SCHEMA: u32 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    DurabilityUnavailable,
    Io(&'static str, std::io::ErrorKind),
    UnsafePath,
    UnsafePermissions,
    ForeignDirectory,
    AlreadyInitialized,
    MissingLedger,
    WriterLocked,
    Changed,
    Invalid,
    InvalidSpec,
    InvalidRequest,
    UnsupportedSchema,
    Limit,
    Conflict,
    Missing,
    WrongState,
    StaleAuthority,
    RandomUnavailable,
    Uncertain,
    Frozen,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "lifecycle store: {self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format: String,
    schema: u32,
    revision: u64,
    records: BTreeMap<String, Record>,
    #[serde(skip)]
    legacy: bool,
}
impl Document {
    fn validate(&self) -> Result<(), Error> {
        if self.format != "agenthub-lifecycle" || self.schema != SCHEMA {
            return Err(Error::UnsupportedSchema);
        }
        if self.records.len() > MAX_RECORDS {
            return Err(Error::Limit);
        }
        if (self.revision == 0) != self.records.is_empty() {
            return Err(Error::Invalid);
        }
        let mut requests = BTreeSet::new();
        let mut launches = BTreeSet::new();
        let mut instances = BTreeSet::new();
        let mut hosts = BTreeSet::new();
        for (id, record) in &self.records {
            record.validate()?;
            if id != record.record_id()
                || record.revision() > self.revision
                || !requests.insert(record.request_id())
                || !launches.insert(record.launch_id())
                || !instances.insert(record.instance_id())
                || !hosts.insert(record.host_name())
            {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
}

/// Preparation authority is returned only by a new, durably committed create.
/// Duplicates and reopen never mint another authority. Neither token is Clone.
pub struct PreparedAuthority {
    owner: String,
    record_id: String,
    revision: u64,
}
/// A future launcher must own this token before spawning, preserve its instance
/// identity, and return exactly one result. This is not exactly-once process I/O.
pub struct StartAuthority {
    owner: String,
    record: Record,
}
impl StartAuthority {
    pub fn record(&self) -> &Record {
        &self.record
    }
}
pub struct CreateOutcome {
    pub record: Record,
    pub prepared: Option<PreparedAuthority>,
}

pub struct CancelAuthority {
    owner: String,
    record: Record,
}
impl CancelAuthority {
    pub fn record(&self) -> &Record {
        &self.record
    }
}
pub struct CancelOutcome {
    pub record: Record,
    pub authority: Option<CancelAuthority>,
}
pub struct BindingAuthority {
    owner: String,
    record: Record,
}
impl BindingAuthority {
    pub fn record(&self) -> &Record {
        &self.record
    }
}
pub enum BindingObservation {
    Confirmed(BindingSpec),
    Unavailable,
    Conflict,
}
pub struct BindingEvidence {
    record: Record,
    observation: BindingObservation,
}
impl BindingEvidence {
    pub fn new(record: &Record, observation: BindingObservation) -> Self {
        Self {
            record: record.clone(),
            observation,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation {
    Running,
    Exited,
    Unavailable,
}
/// Trusted observer evidence, bound to an exact receipt revision and process
/// identity. The caller must establish the actual OS/guarded-host observation.
pub struct ObservationEvidence {
    record_id: String,
    launch_id: String,
    instance_id: String,
    revision: u64,
    observation: Observation,
}
impl ObservationEvidence {
    pub fn new(record: &Record, observation: Observation) -> Self {
        Self {
            record_id: record.record_id().into(),
            launch_id: record.launch_id().into(),
            instance_id: record.instance_id().into(),
            revision: record.revision(),
            observation,
        }
    }
}

pub struct LifecycleStore {
    disk: disk::Disk,
    document: Document,
    fingerprint: String,
    owner: String,
    frozen: bool,
}
impl LifecycleStore {
    pub fn initialize(directory: &Path) -> Result<Self, Error> {
        require_absolute(directory)?;
        let owner = random()?;
        let document = Document {
            format: "agenthub-lifecycle".into(),
            schema: SCHEMA,
            revision: 0,
            records: BTreeMap::new(),
            legacy: false,
        };
        let bytes = json::encode(&document)?;
        let disk = disk::Disk::open(directory, true)?;
        let fingerprint = disk.persist(&bytes, None)?;
        Ok(Self {
            disk,
            document,
            fingerprint,
            owner,
            frozen: false,
        })
    }
    /// Starting always recovers to durable Uncertain before the handle is ready.
    /// Prepared remains a receipt only; this method never reissues spawn authority.
    pub fn open(directory: &Path) -> Result<Self, Error> {
        require_absolute(directory)?;
        let disk = disk::Disk::open(directory, false)?;
        let bytes = disk.read()?.ok_or(Error::MissingLedger)?;
        let document = json::decode(&bytes)?;
        let mut store = Self {
            disk,
            document,
            fingerprint: disk::hash(&bytes),
            owner: random()?,
            frozen: false,
        };
        if store.document.legacy
            || store.document.records.values().any(|r| {
                matches!(r.state, State::Starting | State::CancelRequested)
                    || r.binding
                        .as_ref()
                        .is_some_and(|binding| binding.state != BindingState::Uncertain)
            })
        {
            let mut recovered = store.document.clone();
            recovered.revision = if recovered.records.is_empty() {
                0
            } else {
                store.next_revision()?
            };
            for record in recovered.records.values_mut() {
                let mut changed = false;
                if matches!(record.state, State::Starting | State::CancelRequested) {
                    record.state = State::Uncertain;
                    changed = true;
                }
                if let Some(binding) = &mut record.binding
                    && binding.state != BindingState::Uncertain
                {
                    binding.state = BindingState::Uncertain;
                    changed = true;
                }
                if changed {
                    record.revision = recovered.revision;
                }
            }
            store.commit(recovered)?;
        }
        Ok(store)
    }
    pub fn create(&mut self, request_id: &str, spec: &LaunchSpec) -> Result<CreateOutcome, Error> {
        self.check()?;
        if !model::identifier(request_id, 8, 128) {
            return Err(Error::InvalidRequest);
        }
        spec.validate()?;
        if let Some(record) = self
            .document
            .records
            .values()
            .find(|r| r.request_id() == request_id)
        {
            if record.spec() != spec {
                return Err(Error::Conflict);
            }
            return Ok(CreateOutcome {
                record: record.clone(),
                prepared: None,
            });
        }
        if self.document.records.len() == MAX_RECORDS {
            return Err(Error::Limit);
        }
        // Deserialization is necessary for private disk receipts but is not
        // permission to bypass fresh cwd validation on a new creation intent.
        spec.verify_directory()?;
        // A Claude `--session-id` is minted exactly once, durably with the
        // intent, so replay and restart repeat the same command line.
        let session_id = match spec.launch() {
            Launch::NewAssigned => Some(uuid_v4()?),
            _ => None,
        };
        let record = Record {
            record_id: random()?,
            request_id: request_id.into(),
            spec: spec.clone(),
            launch_id: random()?,
            instance_id: random()?,
            host_name: format!("agenthub-{}", random()?),
            revision: self.next_revision()?,
            state: State::Prepared,
            failure: None,
            cancel_requested: false,
            binding: None,
            session_id,
            created_at: Some(now_unix()),
            finished_at: None,
            discarded: false,
        };
        let mut next = self.document.clone();
        next.revision = record.revision;
        if next
            .records
            .insert(record.record_id.clone(), record.clone())
            .is_some()
        {
            return Err(Error::RandomUnavailable);
        }
        // Validation also rejects collisions among generated launch/instance/name IDs.
        self.commit(next)?;
        Ok(CreateOutcome {
            prepared: Some(PreparedAuthority {
                owner: self.owner.clone(),
                record_id: record.record_id.clone(),
                revision: record.revision,
            }),
            record,
        })
    }
    pub fn lookup_request(
        &mut self,
        request_id: &str,
        spec: &LaunchSpec,
    ) -> Result<Option<Record>, Error> {
        self.check()?;
        if !model::identifier(request_id, 8, 128) {
            return Err(Error::InvalidRequest);
        }
        spec.validate()?;
        let found = self
            .document
            .records
            .values()
            .find(|record| record.request_id() == request_id);
        if found.is_some_and(|record| record.spec() != spec) {
            return Err(Error::Conflict);
        }
        Ok(found.cloned())
    }
    pub fn begin_start(&mut self, authority: PreparedAuthority) -> Result<StartAuthority, Error> {
        self.check()?;
        if authority.owner != self.owner {
            return Err(Error::StaleAuthority);
        }
        let record = self
            .document
            .records
            .get(&authority.record_id)
            .ok_or(Error::StaleAuthority)?;
        if record.state != State::Prepared || record.revision != authority.revision {
            return Err(Error::StaleAuthority);
        }
        record.spec.verify_directory()?;
        let record = self.transition(&authority.record_id, State::Starting, None)?;
        Ok(StartAuthority {
            owner: self.owner.clone(),
            record,
        })
    }
    pub fn mark_running(&mut self, authority: StartAuthority) -> Result<Record, Error> {
        self.finish_start(authority, State::Running, None)
    }
    pub fn mark_failed(
        &mut self,
        authority: StartAuthority,
        reason: Failure,
    ) -> Result<Record, Error> {
        self.finish_start(authority, State::Failed, Some(reason))
    }
    pub fn mark_uncertain(&mut self, authority: StartAuthority) -> Result<Record, Error> {
        self.finish_start(authority, State::Uncertain, None)
    }
    fn finish_start(
        &mut self,
        authority: StartAuthority,
        state: State,
        failure: Option<Failure>,
    ) -> Result<Record, Error> {
        self.check()?;
        if authority.owner != self.owner
            || self.document.records.get(authority.record.record_id()) != Some(&authority.record)
            || authority.record.state != State::Starting
        {
            return Err(Error::StaleAuthority);
        }
        self.transition(authority.record.record_id(), state, failure)
    }
    /// Trusted administrative cancellation of a never-started preparation only.
    pub fn cancel_prepared(&mut self, record_id: &str) -> Result<Record, Error> {
        let record = self.get(record_id)?;
        if record.state == State::Failed && record.failure == Some(Failure::PreparationCancelled) {
            return Ok(record);
        }
        if record.state != State::Prepared {
            return Err(Error::WrongState);
        }
        self.transition(
            record_id,
            State::Failed,
            Some(Failure::PreparationCancelled),
        )
    }
    /// Caller must correlate the actual process instance before reporting exit.
    /// The store performs no process discovery and never guesses a native UID/SID.
    pub fn mark_exited(&mut self, record_id: &str) -> Result<Record, Error> {
        let record = self.get(record_id)?;
        if record.state == State::Exited {
            return Ok(record);
        }
        if record.state != State::Running {
            return Err(Error::WrongState);
        }
        self.transition(record_id, State::Exited, None)
    }
    pub fn request_cancel(
        &mut self,
        record_id: &str,
        expected_instance_id: &str,
    ) -> Result<CancelOutcome, Error> {
        let record = self.get(record_id)?;
        if record.instance_id() != expected_instance_id {
            return Err(Error::Conflict);
        }
        if record.state == State::Prepared {
            return Ok(CancelOutcome {
                record: self.cancel_prepared(record_id)?,
                authority: None,
            });
        }
        if record.cancel_requested || matches!(record.state, State::Failed | State::Exited) {
            return Ok(CancelOutcome {
                record,
                authority: None,
            });
        }
        if !matches!(record.state, State::Running | State::Uncertain) {
            return Err(Error::WrongState);
        }
        let mut next = self.document.clone();
        next.revision = self.next_revision()?;
        let row = next.records.get_mut(record_id).ok_or(Error::Missing)?;
        row.state = State::CancelRequested;
        row.cancel_requested = true;
        if let Some(binding) = &mut row.binding {
            binding.state = BindingState::Uncertain;
        }
        row.revision = next.revision;
        let record = row.clone();
        self.commit(next)?;
        Ok(CancelOutcome {
            record: record.clone(),
            authority: Some(CancelAuthority {
                owner: self.owner.clone(),
                record,
            }),
        })
    }
    pub fn finish_cancel(
        &mut self,
        authority: CancelAuthority,
        observation: Observation,
    ) -> Result<Record, Error> {
        self.check()?;
        if authority.owner != self.owner
            || self.document.records.get(authority.record.record_id()) != Some(&authority.record)
        {
            return Err(Error::StaleAuthority);
        }
        self.observe(ObservationEvidence::new(&authority.record, observation))
    }
    pub fn observe(&mut self, evidence: ObservationEvidence) -> Result<Record, Error> {
        let record = self.get(&evidence.record_id)?;
        if record.launch_id() != evidence.launch_id
            || record.instance_id() != evidence.instance_id
            || record.revision() != evidence.revision
        {
            return Err(Error::StaleAuthority);
        }
        if !matches!(
            record.state,
            State::Running | State::Uncertain | State::CancelRequested | State::Exited
        ) {
            return Err(Error::WrongState);
        }
        if record.state == State::Exited {
            return Ok(record);
        }
        let state = match evidence.observation {
            Observation::Exited => State::Exited,
            Observation::Unavailable => State::Uncertain,
            Observation::Running if record.cancel_requested => record.state,
            Observation::Running => State::Running,
        };
        if state == record.state {
            return Ok(record);
        }
        self.transition(record.record_id(), state, None)
    }
    /// Called only for a fresh, operator-confirmed request. Matching retries
    /// persist a new Intent revision before obtaining another host-call token.
    pub fn begin_binding(
        &mut self,
        expected: &Record,
        spec: &BindingSpec,
    ) -> Result<BindingAuthority, Error> {
        self.begin_binding_with(expected, spec, BindingMethod::Operator, None)
    }
    /// `begin_binding` with the asserting method and, for process evidence,
    /// the private note describing it. A retry of the same spec keeps the
    /// first method/evidence unless the new request carries a note.
    pub fn begin_binding_with(
        &mut self,
        expected: &Record,
        spec: &BindingSpec,
        method: BindingMethod,
        evidence: Option<String>,
    ) -> Result<BindingAuthority, Error> {
        if evidence.as_deref().is_some_and(|note| {
            note.is_empty() || note.len() > MAX_EVIDENCE_BYTES || note.chars().any(char::is_control)
        }) {
            return Err(Error::InvalidSpec);
        }
        let current = self.get(expected.record_id())?;
        if current != *expected {
            return Err(Error::StaleAuthority);
        }
        if current.state != State::Running || current.cancel_requested {
            return Err(Error::WrongState);
        }
        if spec.source() != current.spec.source() || current.declared_sid().is_some() {
            return Err(Error::InvalidSpec);
        }
        if current
            .binding
            .as_ref()
            .is_some_and(|binding| binding.spec != *spec)
        {
            return Err(Error::Conflict);
        }
        let mut next = self.document.clone();
        next.revision = self.next_revision()?;
        let record = next
            .records
            .get_mut(current.record_id())
            .ok_or(Error::Missing)?;
        let previous = record.binding.take();
        record.binding = Some(BindingRecord {
            spec: spec.clone(),
            state: BindingState::Intent,
            method: previous.as_ref().map_or(method, |binding| binding.method),
            evidence: evidence.or_else(|| {
                previous
                    .as_ref()
                    .and_then(|binding| binding.evidence.clone())
            }),
            bound_at: previous.and_then(|binding| binding.bound_at),
        });
        record.revision = next.revision;
        let record = record.clone();
        self.commit(next)?;
        Ok(BindingAuthority {
            owner: self.owner.clone(),
            record,
        })
    }
    pub fn finish_binding(
        &mut self,
        authority: BindingAuthority,
        observation: BindingObservation,
    ) -> Result<Record, Error> {
        self.check()?;
        if authority.owner != self.owner {
            return Err(Error::StaleAuthority);
        }
        self.observe_binding(BindingEvidence::new(&authority.record, observation))
    }
    pub fn observe_binding(&mut self, evidence: BindingEvidence) -> Result<Record, Error> {
        let current = self.get(evidence.record.record_id())?;
        if current != evidence.record {
            return Err(Error::StaleAuthority);
        }
        let Some(binding) = &current.binding else {
            return Ok(current);
        };
        let conflict = match &evidence.observation {
            BindingObservation::Confirmed(observed) => binding.spec != *observed,
            BindingObservation::Conflict => true,
            BindingObservation::Unavailable => false,
        };
        // A missing observation after the exact instance exited (its host
        // record is gone) is not evidence against the binding it confirmed:
        // the Exited + Confirmed pair stays as the durable exit receipt (WP-E).
        let retained_exit_receipt = current.state == State::Exited
            && binding.state == BindingState::Confirmed
            && matches!(evidence.observation, BindingObservation::Unavailable);
        let state = if retained_exit_receipt
            || (!conflict && matches!(evidence.observation, BindingObservation::Confirmed(_)))
        {
            BindingState::Confirmed
        } else {
            BindingState::Uncertain
        };
        let record = if binding.state == state {
            current
        } else {
            let mut next = self.document.clone();
            next.revision = self.next_revision()?;
            let record = next
                .records
                .get_mut(current.record_id())
                .ok_or(Error::Missing)?;
            let binding = record.binding.as_mut().ok_or(Error::Invalid)?;
            binding.state = state;
            if state == BindingState::Confirmed && binding.bound_at.is_none() {
                binding.bound_at = Some(now_unix());
            }
            record.revision = next.revision;
            let record = record.clone();
            self.commit(next)?;
            record
        };
        if conflict {
            Err(Error::Conflict)
        } else {
            Ok(record)
        }
    }
    pub fn get(&mut self, record_id: &str) -> Result<Record, Error> {
        self.check()?;
        if !model::nonce(record_id) {
            return Err(Error::InvalidRequest);
        }
        self.document
            .records
            .get(record_id)
            .cloned()
            .ok_or(Error::Missing)
    }
    /// Full private receipts, intentionally bounded and not a public HTTP DTO.
    pub fn list(&mut self, offset: usize, limit: usize) -> Result<Vec<Record>, Error> {
        self.check()?;
        if limit == 0 || limit > MAX_RECORDS {
            return Err(Error::Limit);
        }
        Ok(self
            .document
            .records
            .values()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect())
    }
    /// Drop a finished (or durably cancelled) receipt from the pending view.
    /// Idempotent; a receipt that could still be running is `WrongState`.
    pub fn discard(&mut self, record_id: &str) -> Result<Record, Error> {
        let record = self.get(record_id)?;
        if record.discarded {
            return Ok(record);
        }
        if !record.discardable() {
            return Err(Error::WrongState);
        }
        let mut next = self.document.clone();
        next.revision = self.next_revision()?;
        let row = next.records.get_mut(record_id).ok_or(Error::Missing)?;
        row.discarded = true;
        row.revision = next.revision;
        let result = row.clone();
        self.commit(next)?;
        Ok(result)
    }
    fn next_revision(&self) -> Result<u64, Error> {
        self.document.revision.checked_add(1).ok_or(Error::Limit)
    }
    fn transition(
        &mut self,
        id: &str,
        state: State,
        failure: Option<Failure>,
    ) -> Result<Record, Error> {
        let mut next = self.document.clone();
        next.revision = self.next_revision()?;
        let record = next.records.get_mut(id).ok_or(Error::Missing)?;
        record.state = state;
        record.failure = failure;
        if matches!(state, State::Exited | State::Failed) && record.finished_at.is_none() {
            record.finished_at = Some(now_unix());
        }
        // An observed exit of the exact instance keeps a confirmed binding:
        // that is the durable exit receipt the runtime folds into `exited`
        // (WP-E; before, every exit downgraded it and the receipt never
        // applied). Uncertainty and cancellation still downgrade.
        if matches!(state, State::Uncertain | State::CancelRequested)
            && let Some(binding) = &mut record.binding
        {
            binding.state = BindingState::Uncertain;
        }
        record.revision = next.revision;
        let result = record.clone();
        self.commit(next)?;
        Ok(result)
    }
    fn check(&mut self) -> Result<(), Error> {
        if self.frozen {
            return Err(Error::Frozen);
        }
        if let Err(error) = self.disk.verify(Some(&self.fingerprint)) {
            self.frozen = true;
            return Err(error);
        }
        Ok(())
    }
    fn commit(&mut self, mut next: Document) -> Result<(), Error> {
        next.legacy = false;
        // Validation/capacity failures happen before disk writes and authorize no work.
        let bytes = json::encode(&next)?;
        match self.disk.persist(&bytes, Some(&self.fingerprint)) {
            Ok(fingerprint) => {
                self.document = next;
                self.fingerprint = fingerprint;
                Ok(())
            }
            Err(error) => {
                self.frozen = true;
                Err(error)
            }
        }
    }
}
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}
fn require_absolute(path: &Path) -> Result<(), Error> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(Error::UnsafePath)
    }
}
fn random() -> Result<String, Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| Error::RandomUnavailable)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
/// Lowercase RFC 4122 version-4 text UUID from the OS random source.
fn uuid_v4() -> Result<String, Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| Error::RandomUnavailable)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

#[cfg(all(test, unix))]
mod tests;
