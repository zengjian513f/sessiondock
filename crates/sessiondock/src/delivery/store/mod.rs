//! Explicit-directory persistence for the independent provider state machines.
//! Successful commits return acknowledgments, never execute delivery effects.

mod disk;
mod json;

use std::{path::Path, sync::Mutex};

use serde::{Deserialize, Serialize};

use super::{claude, codex};

pub const LEDGER_FILENAME: &str = "delivery-ledger.json";
pub const LOCK_FILENAME: &str = ".delivery.lock";
pub const MAX_BYTES: usize = 32 * 1024 * 1024;

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
    UnsupportedSchema,
    Limit,
    NotPersist,
    TokenMismatch,
    Conflict,
    PayloadConflict,
    InvalidRecovery,
    /// The rename may have committed. Both reads and writes are frozen.
    Uncertain,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Do not include submitted text, paths or native evidence in errors.
        write!(f, "delivery store: {self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format: String,
    schema: u32,
    codex: codex::Snapshot,
    claude: claude::Snapshot,
    // Retaining the exact prior token permits an identical Persist replay,
    // including the fresh-epoch restore commit, without weakening epoch CAS.
    codex_previous: Option<codex::Version>,
    claude_previous: Option<claude::Version>,
}

impl Document {
    fn validate(&self) -> Result<(), Error> {
        if self.format != "agenthub-delivery" || self.schema != 1 {
            return Err(Error::UnsupportedSchema);
        }
        codex::validate_snapshot(&self.codex).map_err(|_| Error::Invalid)?;
        claude::validate(&self.claude).map_err(|_| Error::Invalid)?;
        for (old, revision) in [
            (
                self.codex_previous.as_ref().map(|v| (&v.epoch, v.revision)),
                self.codex.version.revision,
            ),
            (
                self.claude_previous
                    .as_ref()
                    .map(|v| (&v.epoch, v.revision)),
                self.claude.version.revision,
            ),
        ] {
            match old {
                Some((epoch, old_revision))
                    if !epoch.trim().is_empty()
                        && epoch.len() <= 128
                        && old_revision.checked_add(1) == Some(revision) => {}
                None if revision == 0 => {}
                _ => return Err(Error::Invalid),
            }
        }
        Ok(())
    }
}

struct State {
    document: Document,
    fingerprint: String,
    frozen: bool,
    codex_ready: bool,
    claude_ready: bool,
}

/// A single-process handle additionally guarded by a cross-process OS lease.
/// There must be exactly one live Machine per provider under this handle.
pub struct DeliveryStore {
    disk: disk::Disk,
    state: Mutex<State>,
}

impl DeliveryStore {
    /// Explicit first creation only. The existing directory must be empty;
    /// even a preexisting lock or abandoned temp file requires manual recovery.
    /// This never overwrites an initialized or partially initialized ledger.
    pub fn initialize(
        directory: &Path,
        codex_epoch: String,
        claude_epoch: String,
    ) -> Result<Self, Error> {
        let document = Document {
            format: "agenthub-delivery".into(),
            schema: 1,
            codex: codex::Machine::new(codex_epoch)
                .map_err(|_| Error::Invalid)?
                .snapshot()
                .clone(),
            claude: claude::Machine::new(claude_epoch)
                .map_err(|_| Error::Invalid)?
                .snapshot()
                .clone(),
            codex_previous: None,
            claude_previous: None,
        };
        let bytes = json::encode(&document)?;
        let disk = disk::Disk::open(directory, true)?;
        let fingerprint = disk.persist(&bytes, None)?;
        Ok(Self {
            disk,
            state: Mutex::new(State {
                document,
                fingerprint,
                frozen: false,
                codex_ready: true,
                claude_ready: true,
            }),
        })
    }

    /// Existing ledgers only; missing/corrupt/unknown data is never made empty.
    /// The caller must restore both Machines with fresh, unique process epochs
    /// and commit their recovery Persist proposals before normal commands.
    pub fn open(directory: &Path) -> Result<Self, Error> {
        let disk = disk::Disk::open(directory, false)?;
        let bytes = disk.read()?.ok_or(Error::MissingLedger)?;
        let document = json::decode(&bytes)?;
        Ok(Self {
            disk,
            state: Mutex::new(State {
                document,
                fingerprint: disk::hash(&bytes),
                frozen: false,
                codex_ready: false,
                claude_ready: false,
            }),
        })
    }

    pub fn snapshots(&self) -> Result<(codex::Snapshot, claude::Snapshot), Error> {
        let state = self.lock()?;
        self.disk.verify(Some(&state.fingerprint))?;
        Ok((state.document.codex.clone(), state.document.claude.clone()))
    }

    /// Pass the Machine's committed version before applying its command.
    /// Only an exactly matching Persist effect can receive a Persisted ack.
    pub fn commit_codex(
        &self,
        expected: &codex::Version,
        effect: &codex::Effect,
    ) -> Result<codex::Command, Error> {
        let codex::Effect::Persist { version, snapshot } = effect else {
            return Err(Error::NotPersist);
        };
        if &snapshot.version != version {
            return Err(Error::TokenMismatch);
        }
        let mut state = self.lock()?;
        let current = &state.document.codex;
        if !state.codex_ready && version.epoch == current.version.epoch {
            return Err(Error::InvalidRecovery);
        }
        if current == snapshot && state.document.codex_previous.as_ref() == Some(expected) {
            self.disk.verify(Some(&state.fingerprint))?;
            return Ok(codex::Command::Persisted(version.clone()));
        }
        if &current.version != expected
            || expected.revision.checked_add(1) != Some(version.revision)
        {
            return Err(Error::Conflict);
        }
        codex::validate_snapshot(snapshot).map_err(|_| Error::Invalid)?;
        for (id, old) in &current.receipts {
            let next = snapshot.receipts.get(id).ok_or(Error::PayloadConflict)?;
            if next.request != old.request || next.created_ms != old.created_ms {
                return Err(Error::PayloadConflict);
            }
        }
        if version.epoch != expected.epoch {
            let (_, recovery) = codex::Machine::restore(current.clone(), version.epoch.clone())
                .map_err(|_| Error::InvalidRecovery)?;
            if !matches!(recovery.as_slice(), [codex::Effect::Persist { snapshot: next, .. }] if next == snapshot)
            {
                return Err(Error::InvalidRecovery);
            }
        }
        let mut next = state.document.clone();
        next.codex = snapshot.clone();
        next.codex_previous = Some(expected.clone());
        self.persist(&mut state, next)?;
        state.codex_ready = true;
        Ok(codex::Command::Persisted(version.clone()))
    }

    pub fn commit_claude(
        &self,
        expected: &claude::Version,
        effect: &claude::Effect,
    ) -> Result<claude::Command, Error> {
        let claude::Effect::Persist { version, snapshot } = effect else {
            return Err(Error::NotPersist);
        };
        if &snapshot.version != version {
            return Err(Error::TokenMismatch);
        }
        let mut state = self.lock()?;
        let current = &state.document.claude;
        if !state.claude_ready && version.epoch == current.version.epoch {
            return Err(Error::InvalidRecovery);
        }
        if current == snapshot && state.document.claude_previous.as_ref() == Some(expected) {
            self.disk.verify(Some(&state.fingerprint))?;
            return Ok(claude::Command::Persisted(version.clone()));
        }
        if &current.version != expected
            || expected.revision.checked_add(1) != Some(version.revision)
        {
            return Err(Error::Conflict);
        }
        claude::validate(snapshot).map_err(|_| Error::Invalid)?;
        for (id, old) in &current.receipts {
            let next = snapshot.receipts.get(id).ok_or(Error::PayloadConflict)?;
            if next.request != old.request
                || next.created_ms != old.created_ms
                || next.sequence != old.sequence
            {
                return Err(Error::PayloadConflict);
            }
        }
        if snapshot.next_sequence < current.next_sequence {
            return Err(Error::PayloadConflict);
        }
        if version.epoch != expected.epoch {
            let (_, recovery) = claude::Machine::restore(current.clone(), version.epoch.clone())
                .map_err(|_| Error::InvalidRecovery)?;
            if !matches!(recovery.as_slice(), [claude::Effect::Persist { snapshot: next, .. }] if next == snapshot)
            {
                return Err(Error::InvalidRecovery);
            }
        }
        let mut next = state.document.clone();
        next.claude = snapshot.clone();
        next.claude_previous = Some(expected.clone());
        self.persist(&mut state, next)?;
        state.claude_ready = true;
        Ok(claude::Command::Persisted(version.clone()))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, Error> {
        let state = self.state.lock().map_err(|_| Error::Uncertain)?;
        if state.frozen {
            return Err(Error::Uncertain);
        }
        Ok(state)
    }

    fn persist(&self, state: &mut State, document: Document) -> Result<(), Error> {
        let bytes = json::encode(&document)?;
        match self.disk.persist(&bytes, Some(&state.fingerprint)) {
            Ok(fingerprint) => {
                state.document = document;
                state.fingerprint = fingerprint;
                Ok(())
            }
            Err(error) => {
                if error == Error::Uncertain {
                    state.frozen = true;
                }
                Err(error)
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
