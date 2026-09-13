//! Session recycle bin. Native files move into recoverable entries; fork
//! parents and currently running sessions remain protected as in Python.

pub mod manifest;
mod plan;
#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::Serialize;
use serde_json::{Value, json};

use crate::sessions::SessionRoots;
use manifest::{
    EntryState, FILES_DIR, FileRecord, MANIFEST_NAME, MANIFEST_TEMP, Manifest, RunStateNote,
    now_unix, rfc3339,
};
pub use plan::{Plan, protection_set};

pub const LIST_DEFAULT: usize = usize::MAX;

#[derive(Clone, Debug)]
pub struct TrashError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl TrashError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TrashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TrashError {}

pub fn entry_id_is_valid(id: &str) -> bool {
    file_name_is_valid(id)
}

pub(crate) fn file_name_is_valid(name: &str) -> bool {
    !name.is_empty() && !matches!(name, "." | "..") && !name.contains(['/', '\\', '\0'])
}

/// One session's run state as observed for this request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunState {
    Running(String),
    Exited(String),
    Unknown(String),
}

/// Per-UID run states folded from one fresh runtime observation. Absent means
/// unknown; without a configured host directory every UID is unknown.
#[derive(Clone, Debug, Default)]
pub struct Liveness {
    pub configured: bool,
    pub states: BTreeMap<String, RunState>,
}

impl Liveness {
    pub fn from_runtime(snapshot: Option<&crate::runtime::RuntimeSnapshot>) -> Self {
        use crate::runtime::RunState as Observed;
        let Some(snapshot) = snapshot else {
            return Self::default();
        };
        fn detail<T: Serialize>(value: Option<&T>) -> String {
            value
                .and_then(|value| serde_json::to_value(value).ok())
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default()
        }
        let states = snapshot
            .sessions
            .iter()
            .map(|(uid, session)| {
                let state = match session.state {
                    Observed::Running => RunState::Running(detail(session.evidence.as_ref())),
                    Observed::Exited => RunState::Exited(detail(session.evidence.as_ref())),
                    Observed::Unknown => RunState::Unknown(detail(session.reason.as_ref())),
                };
                (uid.clone(), state)
            })
            .collect();
        Self {
            configured: true,
            states,
        }
    }

    pub fn state(&self, uid: &str) -> RunState {
        if !self.configured {
            return RunState::Unknown("no_runtime".into());
        }
        self.states
            .get(uid)
            .cloned()
            .unwrap_or_else(|| RunState::Unknown("no_instance".into()))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Deleted {
    pub uid: String,
    pub title: String,
    pub entry_id: String,
    /// Entry directory, the legacy `trash` field.
    pub trash: String,
    pub files: usize,
    pub bytes: u64,
    pub run_state: RunStateNote,
    pub forced: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Refused {
    pub uid: String,
    pub title: String,
    pub code: &'static str,
    pub error: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub needs_force: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_state: Option<RunStateNote>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DeleteOutcome {
    pub deleted: Vec<Deleted>,
    /// Python policy refusals: protected parents and running sessions.
    pub skipped: Vec<Refused>,
    /// Missing sessions and filesystem failures.
    pub failed: Vec<Refused>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Restored {
    pub uid: String,
    pub source: String,
    pub title: String,
    pub path: String,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct PurgeOutcome {
    pub removed: usize,
    pub freed: u64,
    pub errors: Vec<String>,
    pub failed: Vec<Value>,
    /// Entries still present after this bounded call.
    pub remaining: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Listing {
    pub items: Vec<Value>,
    pub count: usize,
    pub size: u64,
    pub dir: String,
    pub limit: usize,
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

/// One trash directory entry: its id and the manifest (or why it is unreadable).
type ScannedEntry = (String, Result<Manifest, TrashError>);

pub struct TrashService {
    directory: PathBuf,
    roots: SessionRoots,
    lock: Mutex<()>,
}

impl TrashService {
    /// `directory` must already have passed [`validate_directory`]; roots are
    /// frozen (canonicalized) so restore targets can be checked against the
    /// same boundaries the inventory used.
    pub fn open(directory: PathBuf, roots: SessionRoots) -> io::Result<Self> {
        let directory = validate_directory(&directory)?;
        let freeze = |root: Option<PathBuf>| root.and_then(|path| path.canonicalize().ok());
        Ok(Self {
            directory,
            roots: SessionRoots {
                claude: freeze(roots.claude),
                codex: freeze(roots.codex),
                grok: freeze(roots.grok),
            },
            lock: Mutex::new(()),
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn entry_dir(&self, id: &str) -> Result<PathBuf, TrashError> {
        if !entry_id_is_valid(id) {
            return Err(TrashError::new(
                400,
                "invalid_entry_id",
                "回收站条目 ID 无效",
            ));
        }
        Ok(self.directory.join(id))
    }

    /// Batch soft delete against one frozen snapshot. Never all-or-nothing:
    /// each UID gets its own result; the protection set is fixed up front.
    pub fn delete(
        &self,
        rows: &[Value],
        liveness: &Liveness,
        uids: &[String],
        force: bool,
    ) -> DeleteOutcome {
        let _guard = self.guard();
        let protected = protection_set(rows);
        let mut outcome = DeleteOutcome::default();
        let mut seen = BTreeSet::new();
        for uid in uids {
            if !seen.insert(uid.clone()) {
                continue;
            }
            let row = rows
                .iter()
                .find(|row| row["uid"].as_str() == Some(uid.as_str()));
            let title = row
                .and_then(|row| row["title"].as_str())
                .unwrap_or("")
                .to_owned();
            let refused = |code, error: String, needs_force, run_state| Refused {
                uid: uid.clone(),
                title: title.clone(),
                code,
                error,
                needs_force,
                run_state,
            };
            let Some(row) = row else {
                outcome
                    .failed
                    .push(refused("not_found", "会话不存在".into(), false, None));
                continue;
            };
            if protected.contains(uid) {
                outcome.skipped.push(refused(
                    "fork_parent_protected",
                    "父会话只能隐藏，不能删除".into(),
                    false,
                    None,
                ));
                continue;
            }
            let (note, forced) = match liveness.state(uid) {
                RunState::Running(evidence) => {
                    outcome.skipped.push(refused(
                        "session_running",
                        "会话仍在运行，请先停止".into(),
                        false,
                        Some(RunStateNote {
                            state: "running".into(),
                            detail: evidence,
                        }),
                    ));
                    continue;
                }
                RunState::Exited(evidence) => (
                    RunStateNote {
                        state: "exited".into(),
                        detail: evidence,
                    },
                    false,
                ),
                RunState::Unknown(reason) => {
                    let note = RunStateNote {
                        state: "unknown".into(),
                        detail: reason.clone(),
                    };
                    (note, force)
                }
            };
            let plan = match Plan::derive(row, &self.roots) {
                Ok(plan) => plan,
                Err(error) => {
                    outcome
                        .failed
                        .push(refused(error.code, error.message, false, Some(note)));
                    continue;
                }
            };
            match self.move_into_trash(&plan, note.clone(), forced) {
                Ok(deleted) => outcome.deleted.push(deleted),
                Err(error) => {
                    outcome
                        .failed
                        .push(refused(error.code, error.message, false, Some(note)))
                }
            }
        }
        outcome
    }

    /// Plan one published row without moving anything; stamps are captured now.
    pub fn plan_for(&self, row: &Value) -> Result<Plan, TrashError> {
        Plan::derive(row, &self.roots)
    }

    /// Execute a plan against the current contents of its inventory paths.
    pub fn move_planned(
        &self,
        plan: &Plan,
        run_state: RunStateNote,
        forced: bool,
    ) -> Result<Deleted, TrashError> {
        let _guard = self.guard();
        self.move_into_trash(plan, run_state, forced)
    }

    fn move_into_trash(
        &self,
        plan: &Plan,
        run_state: RunStateNote,
        forced: bool,
    ) -> Result<Deleted, TrashError> {
        let entry_id = self.new_entry_id(&plan.source, &plan.uid)?;
        let entry_dir = self.directory.join(&entry_id);
        let files_dir = entry_dir.join(FILES_DIR);
        create_private_dir(&entry_dir)?;
        if let Err(error) = create_private_dir(&files_dir) {
            let _ = fs::remove_dir(&entry_dir);
            return Err(error);
        }
        let deleted_at_unix = now_unix();
        let mut manifest = Manifest {
            version: manifest::MANIFEST_VERSION,
            entry_id: entry_id.clone(),
            uid: plan.uid.clone(),
            source: plan.source.clone(),
            sid: plan.sid.clone(),
            title: plan.title.clone(),
            cwd: plan.cwd.clone(),
            created: plan.created.clone(),
            updated: plan.updated.clone(),
            origin: plan.origin.clone(),
            root: plan.root.clone(),
            deleted_at: rfc3339(deleted_at_unix),
            deleted_at_unix,
            state: EntryState::Moving,
            forced,
            run_state,
            bytes: plan.bytes(),
            files: plan
                .files
                .iter()
                .enumerate()
                .map(|(index, file)| FileRecord {
                    name: entry_file_name(index, &file.origin),
                    origin: file.origin.clone(),
                    role: file.role,
                    stamp: file.stamp.clone(),
                    in_trash: false,
                })
                .collect(),
        };
        if let Err(error) = manifest.write(&entry_dir) {
            let _ = fs::remove_dir(&files_dir);
            let _ = fs::remove_dir(&entry_dir);
            return Err(error);
        }
        let mut failure = None;
        for index in 0..manifest.files.len() {
            let record = &manifest.files[index];
            let destination = files_dir.join(&record.name);
            let result = move_entry(&record.origin, &destination);
            match result {
                Ok(()) => manifest.files[index].in_trash = true,
                Err(error) => {
                    // A cross-device source removal can fail after publishing
                    // its complete copy. Keep that copy in the recovery record.
                    if fs::symlink_metadata(&destination).is_ok() {
                        manifest.files[index].in_trash = true;
                    }
                    failure = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = failure {
            // Return what already moved; only then discard the entry.
            for record in manifest.files.iter_mut().filter(|record| record.in_trash) {
                if move_entry(&files_dir.join(&record.name), &record.origin).is_ok() {
                    record.in_trash = false;
                }
            }
            if manifest.files.iter().any(|record| record.in_trash) {
                manifest.state = EntryState::Partial;
                let _ = manifest.write(&entry_dir);
                return Err(TrashError::new(
                    500,
                    "move_failed",
                    format!(
                        "{}；部分文件已在回收站条目 {entry_id} 中，需要手动检查",
                        error.message
                    ),
                ));
            }
            let _ = fs::remove_file(entry_dir.join(MANIFEST_NAME));
            let _ = fs::remove_file(entry_dir.join(MANIFEST_TEMP));
            let _ = fs::remove_dir(&files_dir);
            let _ = fs::remove_dir(&entry_dir);
            return Err(error);
        }
        manifest.state = EntryState::Trashed;
        manifest.write(&entry_dir)?;
        Ok(Deleted {
            uid: plan.uid.clone(),
            title: plan.title.clone(),
            entry_id,
            trash: crate::sessions::path_text(&entry_dir).into_owned(),
            files: manifest.files.len(),
            bytes: manifest.bytes,
            run_state: manifest.run_state,
            forced,
        })
    }

    fn new_entry_id(&self, source: &str, uid: &str) -> Result<String, TrashError> {
        let stamp = chrono::DateTime::from_timestamp(i64::try_from(now_unix()).unwrap_or(0), 0)
            .unwrap_or_default()
            .format("%Y%m%dT%H%M%SZ");
        let hash: String = uid
            .rsplit(':')
            .next()
            .unwrap_or("")
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(16)
            .collect();
        for _ in 0..8 {
            let mut random = [0u8; 4];
            getrandom::fill(&mut random).map_err(|_| {
                TrashError::new(500, "entropy_unavailable", "无法生成回收站条目 ID")
            })?;
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let id = format!("{stamp}-{source}-{hash}-{suffix}");
            if entry_id_is_valid(&id) && fs::symlink_metadata(self.directory.join(&id)).is_err() {
                return Ok(id);
            }
        }
        Err(TrashError::new(
            500,
            "entry_collision",
            "回收站条目 ID 反复冲突",
        ))
    }

    /// Read every manifest, newest first.
    fn scan(&self) -> Result<(Vec<ScannedEntry>, bool), TrashError> {
        let mut entries = Vec::new();
        let truncated = false;
        let directory = fs::read_dir(&self.directory)
            .map_err(|_| TrashError::new(503, "trash_unreadable", "回收站目录暂时不可枚举"))?;
        for entry in directory {
            let entry = entry.map_err(|_| {
                TrashError::new(503, "trash_unreadable", "回收站目录条目暂时不可读取")
            })?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !entry_id_is_valid(name) {
                continue;
            }
            let kind = entry
                .file_type()
                .map_err(|_| TrashError::new(503, "trash_unreadable", "无法检查回收站条目类型"))?;
            if !kind.is_dir() || kind.is_symlink() {
                continue;
            }
            entries.push((name.to_owned(), Manifest::read(&entry.path())));
        }
        entries.sort_by(|(left_id, left), (right_id, right)| {
            let key = |manifest: &Result<Manifest, TrashError>| {
                manifest.as_ref().map(|m| m.deleted_at_unix).unwrap_or(0)
            };
            key(right)
                .cmp(&key(left))
                .then_with(|| right_id.cmp(left_id))
        });
        Ok((entries, truncated))
    }

    fn item(&self, id: &str, manifest: &Result<Manifest, TrashError>) -> Value {
        match manifest {
            Ok(manifest) => {
                let restorable = self.restorable(manifest);
                let mut item = json!({
                    "id": id, "entry_id": id, "uid": manifest.uid, "source": manifest.source,
                    "sid": manifest.sid, "name": manifest.origin.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    "title": manifest.title, "cwd": manifest.cwd, "updated": manifest.updated,
                    "deleted_at": manifest.deleted_at, "deleted_ts": manifest.deleted_at_unix,
                    "size": manifest.bytes, "bytes": manifest.bytes, "files": manifest.files.len(),
                    "kind": manifest.kind(), "origin": crate::sessions::path_text(&manifest.origin),
                    "state": manifest.state, "forced": manifest.forced, "run_state": manifest.run_state,
                    "recorded": true, "restorable": restorable.is_ok(),
                });
                if let Err(error) = restorable {
                    item["reason"] = json!(error.message);
                    item["reason_code"] = json!(error.code);
                }
                item
            }
            Err(error) => json!({
                "id": id, "entry_id": id, "uid": "", "source": "", "sid": "", "name": id,
                "title": id, "cwd": "", "updated": "", "deleted_at": "", "deleted_ts": 0,
                "size": 0, "bytes": 0, "files": 0, "kind": "file", "origin": "",
                "state": "corrupt", "recorded": false, "restorable": false,
                "reason": format!("清单不可用：{}", error.message),
            }),
        }
    }

    /// Restore to the recorded paths when those paths are available.
    fn restorable(&self, manifest: &Manifest) -> Result<(), TrashError> {
        for file in &manifest.files {
            match fs::symlink_metadata(&file.origin) {
                Ok(_) => {
                    return Err(TrashError::new(
                        409,
                        "restore_conflict",
                        format!("原路径已存在同名文件: {}", file.origin.display()),
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err(TrashError::new(503, "stat_failed", "原路径暂时无法检查"));
                }
            }
        }
        Ok(())
    }

    pub fn list(&self, limit: usize, cursor: Option<&str>) -> Result<Listing, TrashError> {
        let limit = limit.max(1);
        let _guard = self.guard();
        let (entries, truncated) = self.scan()?;
        let size = entries
            .iter()
            .filter_map(|(_, manifest)| manifest.as_ref().ok())
            .map(|manifest| manifest.bytes)
            .sum();
        let start = match cursor {
            None | Some("") => 0,
            Some(cursor) => {
                if !entry_id_is_valid(cursor) {
                    return Err(TrashError::new(400, "invalid_cursor", "回收站分页游标无效"));
                }
                entries
                    .iter()
                    .position(|(id, _)| id == cursor)
                    .map(|position| position + 1)
                    .ok_or_else(|| {
                        TrashError::new(400, "invalid_cursor", "回收站分页游标已失效，请重新读取")
                    })?
            }
        };
        let page: Vec<_> = entries.iter().skip(start).take(limit).collect();
        let next_cursor = if start + page.len() < entries.len() {
            page.last().map(|(id, _)| id.clone())
        } else {
            None
        };
        Ok(Listing {
            items: page
                .iter()
                .map(|(id, manifest)| self.item(id, manifest))
                .collect(),
            count: entries.len(),
            size,
            dir: crate::sessions::path_text(&self.directory).into_owned(),
            limit,
            next_cursor,
            truncated,
        })
    }

    pub fn restore(&self, id: &str) -> Result<Restored, TrashError> {
        let _guard = self.guard();
        let entry_dir = self.entry_dir(id)?;
        match fs::symlink_metadata(&entry_dir) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(TrashError::new(404, "entry_not_found", "回收站条目不存在")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(TrashError::new(404, "entry_not_found", "回收站条目不存在"));
            }
            Err(_) => {
                return Err(TrashError::new(
                    503,
                    "trash_unreadable",
                    "回收站条目暂时不可检查",
                ));
            }
        }
        let mut manifest = Manifest::read(&entry_dir)?;
        if manifest.entry_id != id {
            return Err(TrashError::new(
                409,
                "manifest_invalid",
                "回收站清单与条目目录不一致",
            ));
        }
        self.restorable(&manifest)?;
        let files_dir = entry_dir.join(FILES_DIR);
        let mut restored = Vec::new();
        let mut failure = None;
        for (index, record) in manifest.files.iter().enumerate() {
            let result = ensure_parent(&record.origin)
                .and_then(|()| match fs::symlink_metadata(&record.origin) {
                    Ok(_) => Err(TrashError::new(
                        409,
                        "restore_conflict",
                        format!("原路径已存在同名文件: {}", record.origin.display()),
                    )),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(_) => Err(TrashError::new(503, "stat_failed", "原路径暂时无法检查")),
                })
                .and_then(|()| move_entry(&files_dir.join(&record.name), &record.origin));
            match result {
                Ok(()) => restored.push(index),
                Err(error) if error.code == "file_move_source_cleanup" => {
                    // The cross-device helper publishes the complete origin
                    // before attempting to remove the trash-side source. Treat
                    // this item like every earlier restored item so rollback
                    // accounts for both names and can mark a real partial state.
                    restored.push(index);
                    failure = Some(error);
                    break;
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = failure {
            let mut stuck = false;
            for index in restored.into_iter().rev() {
                let record = &manifest.files[index];
                let held = files_dir.join(&record.name);
                if move_entry(&record.origin, &held).is_err() {
                    stuck = true;
                    // A failed cross-device cleanup may leave either or both
                    // complete names. Record the trash-side reality; Partial
                    // tells callers to inspect an origin-side duplicate too.
                    manifest.files[index].in_trash = fs::symlink_metadata(&held).is_ok();
                } else {
                    manifest.files[index].in_trash = true;
                }
            }
            if stuck {
                manifest.state = EntryState::Partial;
                let _ = manifest.write(&entry_dir);
                return Err(TrashError::new(
                    500,
                    "restore_failed",
                    format!(
                        "{}；部分文件已放回原处，条目 {id} 需要手动检查",
                        error.message
                    ),
                ));
            }
            return Err(error);
        }
        let _ = fs::remove_file(entry_dir.join(MANIFEST_TEMP));
        fs::remove_file(entry_dir.join(MANIFEST_NAME))
            .and_then(|()| fs::remove_dir(&files_dir))
            .and_then(|()| fs::remove_dir(&entry_dir))
            .map_err(|_| {
                TrashError::new(
                    500,
                    "entry_cleanup_failed",
                    "文件已恢复，但回收站条目目录无法清理",
                )
            })?;
        Ok(Restored {
            uid: manifest.uid,
            source: manifest.source,
            title: manifest.title,
            path: crate::sessions::path_text(&manifest.origin).into_owned(),
            files: manifest.files.len(),
            bytes: manifest.bytes,
        })
    }

    /// Remove explicit entries. Each id gets its own result.
    pub fn purge_ids(&self, ids: &[String]) -> Result<PurgeOutcome, TrashError> {
        if ids.is_empty() {
            return Err(TrashError::new(400, "invalid_purge", "需要回收站条目 ID"));
        }
        let _guard = self.guard();
        let mut outcome = PurgeOutcome::default();
        let mut seen = BTreeSet::new();
        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            match self.purge_one(id) {
                Ok(freed) => {
                    outcome.removed += 1;
                    outcome.freed += freed;
                }
                Err(error) => {
                    outcome.errors.push(format!("{id}: {}", error.message));
                    outcome
                        .failed
                        .push(json!({"id": id, "code": error.code, "error": error.message}));
                }
            }
        }
        outcome.remaining = self.scan()?.0.len();
        Ok(outcome)
    }

    /// Remove all entries deleted at least `days` days ago, oldest first.
    pub fn purge_older_than(&self, days: u64) -> Result<PurgeOutcome, TrashError> {
        let _guard = self.guard();
        let cutoff = now_unix().saturating_sub(days.saturating_mul(86_400));
        let (entries, _) = self.scan()?;
        let candidates: Vec<_> = entries
            .iter()
            .rev()
            .filter(|(_, manifest)| match manifest {
                Ok(manifest) => manifest.deleted_at_unix <= cutoff,
                // A corrupt entry has no reliable timestamp; only an explicit
                // full purge (days = 0) removes it.
                Err(_) => days == 0,
            })
            .map(|(id, _)| id.clone())
            .collect();
        let mut outcome = PurgeOutcome::default();
        for id in &candidates {
            match self.purge_one(id) {
                Ok(freed) => {
                    outcome.removed += 1;
                    outcome.freed += freed;
                }
                Err(error) => {
                    outcome.errors.push(format!("{id}: {}", error.message));
                    outcome
                        .failed
                        .push(json!({"id": id, "code": error.code, "error": error.message}));
                }
            }
        }
        outcome.remaining = self.scan()?.0.len();
        Ok(outcome)
    }

    /// Only the entry's own manifest and `files/*` regular files are removed.
    /// Anything unexpected inside the entry leaves it untouched.
    fn purge_one(&self, id: &str) -> Result<u64, TrashError> {
        let entry_dir = self.entry_dir(id)?;
        match fs::symlink_metadata(&entry_dir) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(TrashError::new(404, "entry_not_found", "回收站条目不存在")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(TrashError::new(404, "entry_not_found", "回收站条目不存在"));
            }
            Err(_) => {
                return Err(TrashError::new(
                    503,
                    "trash_unreadable",
                    "回收站条目暂时不可检查",
                ));
            }
        }
        let freed = manifest::directory_bytes(&entry_dir.join(FILES_DIR));
        fs::remove_dir_all(&entry_dir)
            .map_err(|_| TrashError::new(500, "purge_failed", "回收站条目无法删除"))?;
        Ok(freed)
    }
}

fn entry_file_name(index: usize, origin: &Path) -> String {
    let base: String = origin
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .take(200)
        .collect();
    let base = base.trim_start_matches('.').to_owned();
    if base.is_empty() {
        format!("{index}")
    } else {
        format!("{index}-{base}")
    }
}

fn create_private_dir(path: &Path) -> Result<(), TrashError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|_| TrashError::new(500, "entry_create_failed", "无法创建回收站条目目录"))
}

/// Move the named entry, including across filesystems.
fn move_entry(from: &Path, to: &Path) -> Result<(), TrashError> {
    #[cfg(test)]
    if FAIL_AFTER_PUBLISH.with(|fail| fail.replace(false)) {
        fs::copy(from, to)
            .map_err(|_| TrashError::new(500, "restore_failed", "无法模拟已发布的恢复目标"))?;
        return Err(TrashError::new(
            503,
            "file_move_source_cleanup",
            "目标已完整发布，但无法移除源名称",
        ));
    }
    crate::files::move_recycle_entry(from, to)
        .map_err(|error| TrashError::new(error.status, error.code, error.message))
}

#[cfg(test)]
thread_local! {
    static FAIL_AFTER_PUBLISH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn fail_next_move_after_publish() {
    FAIL_AFTER_PUBLISH.with(|fail| fail.set(true));
}

fn ensure_parent(path: &Path) -> Result<(), TrashError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| TrashError::new(500, "restore_failed", "无法重建原始目录"))?;
    }
    Ok(())
}

pub fn validate_directory(path: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(path)?;
    path.canonicalize()
}
