//! SessionDock preferences with reads and atomic publication.

mod disk;
mod model;

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub use model::{
    ActivityStop, Attachment, MetadataSnapshot, PendingRewind, SCHEMA_VERSION, SpawnedBy,
    StopState, TimelinePin, fork_parent_uids,
};

pub const METADATA_FILENAME: &str = "session-metadata.json";

#[derive(Clone, Debug)]
pub struct MetadataError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl MetadataError {
    pub(super) fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub(super) fn uncertain() -> Self {
        Self::new(
            503,
            "metadata_commit_uncertain",
            "元数据已替换，但持久化同步未完成；重新读取可确认当前状态",
        )
    }
}

impl std::fmt::Display for MetadataError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for MetadataError {}

struct State {
    snapshot: Arc<MetadataSnapshot>,
    fingerprint: Option<String>,
}

pub struct MetadataStore {
    disk: disk::Disk,
    state: Mutex<State>,
}

impl MetadataStore {
    /// Open the configured metadata directory, creating it when needed.
    pub fn open(directory: &Path) -> Result<Self, MetadataError> {
        let disk = disk::Disk::open(directory)?;
        let (snapshot, fingerprint) = disk.load()?;
        Ok(Self {
            disk,
            state: Mutex::new(State {
                snapshot: Arc::new(snapshot),
                fingerprint,
            }),
        })
    }

    /// The canonical state directory (`SESSIONDOCK_STATE_DIR`); the debug-run
    /// registry the session read model consults lives beside the metadata.
    pub fn directory(&self) -> &Path {
        self.disk.directory()
    }

    pub fn snapshot(&self) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (snapshot, fingerprint) = self.disk.load()?;
        if state.fingerprint != fingerprint {
            state.snapshot = Arc::new(snapshot);
            state.fingerprint = fingerprint;
        }
        Ok(state.snapshot.clone())
    }

    fn update(
        &self,
        transform: impl FnOnce(&MetadataSnapshot) -> Result<MetadataSnapshot, MetadataError>,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (snapshot, fingerprint) = self.disk.load()?;
        if state.fingerprint != fingerprint {
            state.snapshot = Arc::new(snapshot);
            state.fingerprint = fingerprint;
        }
        let next = transform(&state.snapshot)?;
        if next.revision() == state.snapshot.revision() {
            return Ok(state.snapshot.clone());
        }
        match self.disk.persist(&next) {
            Ok(fingerprint) => {
                // Publish only after every durability step has succeeded.
                state.snapshot = Arc::new(next);
                state.fingerprint = Some(fingerprint);
                Ok(state.snapshot.clone())
            }
            Err(error) => Err(error),
        }
    }

    pub fn set_starred(
        &self,
        uid: &str,
        starred: bool,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| {
                MetadataError::new(
                    503,
                    "metadata_clock_invalid",
                    "系统时钟无效，不能记录收藏时间",
                )
            })?
            .as_secs_f64();
        self.update(|snapshot| snapshot.with_starred(uid, starred, now))
    }

    /// Records a published upload path for a session; idempotent per path.
    pub fn record_attachment(
        &self,
        uid: &str,
        attachment: Attachment,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.with_attachment(uid, attachment))
    }

    pub fn set_fork_visibility(
        &self,
        uids: &[String],
        visible: bool,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.with_fork_visibility(uids, visible))
    }

    /// Persist newly observed spawners, first
    /// relation wins; returns how many sessions were recorded this time.
    pub fn record_spawn_parents(
        &self,
        found: &[(String, SpawnedBy)],
    ) -> Result<usize, MetadataError> {
        if found.is_empty() {
            return Ok(0);
        }
        let before = self.snapshot()?;
        let after = self.update(|snapshot| snapshot.with_spawn_parents(found))?;
        let recorded: std::collections::BTreeSet<&str> = found
            .iter()
            .map(|(uid, _)| uid.trim())
            .filter(|uid| before.spawned_by(uid).is_none() && after.spawned_by(uid).is_some())
            .collect();
        Ok(recorded.len())
    }

    // Domain-only hooks for a future authenticated, verified terminal workflow.
    // These methods do not perform the native action or establish confirmation.
    pub fn stop_activity(
        &self,
        uid: &str,
        stop: ActivityStop,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.with_activity_stop(uid, stop))
    }
    pub fn clear_inferred_activity_stop(
        &self,
        uid: &str,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.without_inferred_activity_stop(uid))
    }
    pub fn begin_timeline_rewind(
        &self,
        uid: &str,
        pending: PendingRewind,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.with_pending_rewind(uid, pending))
    }
    pub fn finish_timeline_rewind(
        &self,
        uid: &str,
        confirmed_tip: &str,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.with_confirmed_rewind(uid, confirmed_tip))
    }
    pub fn cancel_timeline_rewind(
        &self,
        uid: &str,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.without_pending_rewind(uid))
    }

    /// Persist a validated display pin. The caller must have verified the
    /// target against the frozen native inventory; this is not a native rewind.
    pub fn set_timeline_pin(
        &self,
        uid: &str,
        pin: TimelinePin,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.with_timeline_pin(uid, pin))
    }
    pub fn clear_timeline_pin(&self, uid: &str) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.without_timeline_pin(uid))
    }
}

#[cfg(test)]
mod tests;
