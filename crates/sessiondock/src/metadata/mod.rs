//! SessionDock preferences with reads and atomic publication.

mod disk;
mod model;

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub use model::{
    Attachment, GroupCatalog, GroupCatalogUpdate, MetadataSnapshot, NestParent, SCHEMA_VERSION,
    TimelinePin, fork_parent_uids,
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

    // Only Unix syncs the directory after the rename.
    #[cfg_attr(not(unix), allow(dead_code))]
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
        let (snapshot, fingerprint) = disk.load();
        Ok(Self {
            disk,
            state: Mutex::new(State {
                snapshot: Arc::new(snapshot),
                fingerprint,
            }),
        })
    }

    /// The canonical state directory (`SESSIONDOCK_STATE_DIR`).
    pub fn directory(&self) -> &Path {
        self.disk.directory()
    }

    pub fn snapshot(&self) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.reload(&mut state);
        Ok(state.snapshot.clone())
    }

    /// Re-read the file and keep the current `Arc` when the content hash matches.
    /// Decoding runs only after that hash differs. Missing and unreadable files
    /// still replace a fingerprinted snapshot with an empty one and clear the
    /// fingerprint; a repeated miss keeps the empty `Arc`.
    fn reload(&self, state: &mut State) {
        match self.disk.read() {
            None => {
                if state.fingerprint.is_some() {
                    state.snapshot = Arc::new(MetadataSnapshot::empty());
                    state.fingerprint = None;
                }
            }
            Some(file) => {
                if state.fingerprint.as_deref() != Some(file.fingerprint.as_str()) {
                    state.snapshot = Arc::new(disk::Disk::decode(&file.bytes));
                    state.fingerprint = Some(file.fingerprint);
                }
            }
        }
    }

    fn update(
        &self,
        transform: impl FnOnce(&MetadataSnapshot) -> Result<MetadataSnapshot, MetadataError>,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.reload(&mut state);
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

    pub fn transfer_rows(
        &self,
        rows: &std::collections::BTreeMap<String, serde_json::Value>,
        remove: bool,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.with_transfer_rows(rows, remove))
    }

    pub fn merge_group_catalog(
        &self,
        update: &GroupCatalogUpdate,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.with_group_catalog_update(update))
    }

    pub fn set_group(
        &self,
        uid: &str,
        group: Option<Option<String>>,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.with_group(uid, group))
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

    pub fn discard_invalid_initial_nest_parents(
        &self,
        invalid: &[(String, NestParent)],
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.without_invalid_initial_nest_parents(invalid))
    }

    pub fn initialize_nest_parents(
        &self,
        found: &[(String, NestParent)],
        identities: &std::collections::BTreeMap<(String, String), String>,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.with_initial_nest_parents(found, identities))
    }

    pub fn set_nest_display(
        &self,
        uid: &str,
        parent: Option<NestParent>,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.with_nest_display(uid, parent))
    }

    /// Persist a pin that follows a rewind the CLI made on its own screen. The
    /// caller must have verified the target against the frozen native
    /// inventory; this writes no native file.
    pub fn set_timeline_pin(
        &self,
        uid: &str,
        pin: TimelinePin,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| snapshot.with_timeline_pin(uid, pin))
    }
}
