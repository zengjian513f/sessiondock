//! Session recycle bin: soft delete into an explicit private directory.
//!
//! Fail-closed capability. It exists only when `SESSIONDOCK_TRASH_DIR` names an
//! existing private directory (0700 on Unix) disjoint from every other
//! configured root; otherwise the routes stay `501` and `capabilities.trash`
//! is `false`.
//!
//! Semantics:
//! - A delete moves exactly the files the published index rows named for the
//!   session (Claude transcript plus its subagent transcripts and `.meta.json`
//!   sidecars, Codex rollout plus owned subagent rollouts, Grok `summary.json`
//!   and `chat_history.jsonl`) into `<trash>/<entry id>/files/` under a
//!   `manifest.json`. Directories, attachments and anything the inventory did
//!   not name stay where they are; nothing is ever deleted outside the trash.
//! - Every file's size / mtime / identity is captured when the plan is built
//!   from the frozen snapshot and verified again immediately before its
//!   rename (`409 changed_since_inventory`). Symlinks are never followed.
//! - Rename only, within one filesystem. A cross-device rename is rejected
//!   (`409 cross_filesystem`) rather than degraded into a copy that could leave
//!   two half-written copies; already moved files of the same session are
//!   renamed back.
//! - Refused (`409`): fork parents of any non-deleted session
//!   (`fork_parent_protected`, computed once per request), sessions whose
//!   managed instance is verified `running` (`session_running`, never
//!   overridable), and sessions whose run state is `unknown`
//!   (`run_state_unknown`) unless the request carries `force:true`. Without a
//!   configured host directory every session is unknown, so deletion then
//!   always requires `force`. An unknown state is not proof of exit: a CLI
//!   this service never observed may still be writing the file.
//! - Restore refuses (`409 restore_conflict`) when any original path exists
//!   again; it never overwrites or renames around a conflict. It renames each
//!   file back inside the same filesystem and rolls earlier files back if a
//!   later one fails.
//! - Purge removes whole entries (explicit ids or entries older than `days`),
//!   bounded to [`BATCH_LIMIT`] per call. Listing is paginated (`limit` ≤
//!   [`LIST_LIMIT`]).

pub mod manifest;
mod plan;
#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

use serde::Serialize;
use serde_json::{Value, json};

use crate::sessions::SessionRoots;
use manifest::{
    EntryState, FILES_DIR, FileRecord, FileRole, MANIFEST_NAME, MANIFEST_TEMP, Manifest,
    RunStateNote, Stamp, now_unix, rfc3339,
};
pub use plan::{Plan, protection_set};

/// Per-request bound for batch delete UIDs, purge ids and purge-by-age.
pub const BATCH_LIMIT: usize = 200;
/// Largest page the listing returns.
pub const LIST_LIMIT: usize = 200;
pub const LIST_DEFAULT: usize = 100;
/// Directory scan bound; more entries than this are reported as truncated.
pub const SCAN_LIMIT: usize = 10_000;
pub const FILES_PER_ENTRY_LIMIT: usize = 1024;
const ENTRY_ID_LIMIT: usize = 128;

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
    !id.is_empty()
        && id.len() <= ENTRY_ID_LIMIT
        && id
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub(crate) fn file_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
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
    /// Policy refusals: protected parent, running, unknown without force.
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
                    if !force {
                        outcome.skipped.push(refused(
                            "run_state_unknown",
                            format!(
                                "运行状态未知（{}），不能确认 CLI 已退出；确认后带 force 重试",
                                unknown_explanation(&reason)
                            ),
                            true,
                            Some(note),
                        ));
                        continue;
                    }
                    (note, true)
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

    /// Execute a plan built earlier. Every file stamp is verified again
    /// right before its rename; any change since planning refuses the move.
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
            let capture = if record.role == FileRole::Directory {
                Stamp::capture_directory(&record.origin)
            } else {
                Stamp::capture(&record.origin)
            };
            let result = capture.and_then(|current| {
                if !record.stamp.matches(&current, record.role) {
                    return Err(TrashError::new(
                        409,
                        "changed_since_inventory",
                        "会话文件在索引之后发生变化，已取消删除",
                    ));
                }
                rename_same_filesystem(&record.origin, &destination)
            });
            match result {
                Ok(()) => manifest.files[index].in_trash = true,
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = failure {
            // Return what already moved; only then discard the entry.
            for record in manifest.files.iter_mut().filter(|record| record.in_trash) {
                if rename_same_filesystem(&files_dir.join(&record.name), &record.origin).is_ok() {
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
            trash: entry_dir.to_string_lossy().into_owned(),
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

    /// Read every manifest (bounded), newest first.
    fn scan(&self) -> Result<(Vec<ScannedEntry>, bool), TrashError> {
        let mut entries = Vec::new();
        let mut truncated = false;
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
            if entries.len() >= SCAN_LIMIT {
                truncated = true;
                break;
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
                    "kind": manifest.kind(), "origin": manifest.origin.to_string_lossy(),
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

    fn root_for(&self, source: &str) -> Option<&Path> {
        match source {
            "claude" => self.roots.claude.as_deref(),
            "codex" => self.roots.codex.as_deref(),
            "grok" => self.roots.grok.as_deref(),
            _ => None,
        }
    }

    /// Restorable only while every original path is absent and still inside
    /// the currently configured root for its source.
    fn restorable(&self, manifest: &Manifest) -> Result<(), TrashError> {
        if manifest.state != EntryState::Trashed {
            return Err(TrashError::new(
                409,
                "entry_not_restorable",
                "条目不完整（删除时中断），只能彻底删除",
            ));
        }
        let Some(root) = self.root_for(&manifest.source) else {
            return Err(TrashError::new(
                409,
                "restore_outside_roots",
                "该来源当前没有配置的数据源目录",
            ));
        };
        if manifest.root != root {
            return Err(TrashError::new(
                409,
                "restore_outside_roots",
                "原始数据源目录与当前配置不同，不能自动恢复",
            ));
        }
        for file in &manifest.files {
            plan::trusted_within(root, &file.origin)
                .map_err(|error| TrashError::new(409, "restore_outside_roots", error.message))?;
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
        let limit = limit.clamp(1, LIST_LIMIT);
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
            dir: self.directory.to_string_lossy().into_owned(),
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
        let root = self
            .root_for(&manifest.source)
            .expect("restorable checked root")
            .to_path_buf();
        let files_dir = entry_dir.join(FILES_DIR);
        // Verify every trashed file before touching the native tree.
        for record in &manifest.files {
            let held = files_dir.join(&record.name);
            let current = if record.role == FileRole::Directory {
                Stamp::capture_directory(&held)
            } else {
                Stamp::capture(&held)
            }
            .map_err(|error| {
                TrashError::new(
                    409,
                    "changed_since_trashed",
                    format!("回收站中的文件已变化: {}", error.message),
                )
            })?;
            if !record.stamp.matches(&current, record.role) {
                return Err(TrashError::new(
                    409,
                    "changed_since_trashed",
                    "回收站中的文件与清单记录不一致，拒绝恢复",
                ));
            }
        }
        let mut restored = Vec::new();
        let mut failure = None;
        for (index, record) in manifest.files.iter().enumerate() {
            let result = ensure_parent(&root, &record.origin)
                .and_then(|()| match fs::symlink_metadata(&record.origin) {
                    Ok(_) => Err(TrashError::new(
                        409,
                        "restore_conflict",
                        format!("原路径已存在同名文件: {}", record.origin.display()),
                    )),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(_) => Err(TrashError::new(503, "stat_failed", "原路径暂时无法检查")),
                })
                .and_then(|()| {
                    rename_same_filesystem(&files_dir.join(&record.name), &record.origin)
                });
            match result {
                Ok(()) => restored.push(index),
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
                if rename_same_filesystem(&record.origin, &files_dir.join(&record.name)).is_err() {
                    stuck = true;
                    manifest.files[index].in_trash = false;
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
            path: manifest.origin.to_string_lossy().into_owned(),
            files: manifest.files.len(),
            bytes: manifest.bytes,
        })
    }

    /// Remove explicit entries. Each id gets its own result.
    pub fn purge_ids(&self, ids: &[String]) -> Result<PurgeOutcome, TrashError> {
        if ids.is_empty() || ids.len() > BATCH_LIMIT {
            return Err(TrashError::new(
                400,
                "invalid_purge",
                format!("需要 1 至 {BATCH_LIMIT} 个回收站条目 ID"),
            ));
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

    /// Remove entries deleted at least `days` days ago, oldest first, at most
    /// [`BATCH_LIMIT`] per call (`days = 0` empties the bin in bounded steps).
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
            .take(BATCH_LIMIT)
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
        let unexpected = || {
            TrashError::new(
                409,
                "unexpected_content",
                "回收站条目包含非回收站写入的内容，拒绝清除",
            )
        };
        let files_dir = entry_dir.join(FILES_DIR);
        let mut files = Vec::new();
        for entry in fs::read_dir(&entry_dir)
            .map_err(|_| TrashError::new(503, "trash_unreadable", "回收站条目暂时不可枚举"))?
        {
            let entry = entry
                .map_err(|_| TrashError::new(503, "trash_unreadable", "回收站条目暂时不可枚举"))?;
            let name = entry.file_name();
            let kind = entry.file_type().map_err(|_| unexpected())?;
            if kind.is_symlink() {
                return Err(unexpected());
            }
            if name == MANIFEST_NAME || name == MANIFEST_TEMP {
                if !kind.is_file() {
                    return Err(unexpected());
                }
            } else if name == FILES_DIR {
                if !kind.is_dir() {
                    return Err(unexpected());
                }
                for file in fs::read_dir(&files_dir).map_err(|_| {
                    TrashError::new(503, "trash_unreadable", "回收站条目暂时不可枚举")
                })? {
                    let file = file.map_err(|_| unexpected())?;
                    let kind = file.file_type().map_err(|_| unexpected())?;
                    // A whole trashed Grok session directory (WP-E) is the
                    // only directory the bin itself creates under `files/`.
                    if kind.is_symlink() || !(kind.is_file() || kind.is_dir()) {
                        return Err(unexpected());
                    }
                    let Some(file_name) = file.file_name().to_str().map(str::to_owned) else {
                        return Err(unexpected());
                    };
                    if !file_name_is_valid(&file_name) {
                        return Err(unexpected());
                    }
                    files.push((file.path(), kind.is_dir()));
                }
            } else {
                return Err(unexpected());
            }
        }
        let mut freed = 0;
        for (path, is_dir) in files {
            if is_dir {
                let size = manifest::directory_bytes(&path);
                fs::remove_dir_all(&path)
                    .map_err(|_| TrashError::new(500, "purge_failed", "回收站会话目录无法删除"))?;
                freed += size;
                continue;
            }
            let size = fs::symlink_metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            fs::remove_file(&path)
                .map_err(|_| TrashError::new(500, "purge_failed", "回收站文件无法删除"))?;
            freed += size;
        }
        if files_dir.exists() {
            fs::remove_dir(&files_dir)
                .map_err(|_| TrashError::new(500, "purge_failed", "回收站条目目录无法删除"))?;
        }
        for name in [MANIFEST_TEMP, MANIFEST_NAME] {
            match fs::remove_file(entry_dir.join(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => return Err(TrashError::new(500, "purge_failed", "回收站清单无法删除")),
            }
        }
        fs::remove_dir(&entry_dir)
            .map_err(|_| TrashError::new(500, "purge_failed", "回收站条目目录无法删除"))?;
        Ok(freed)
    }
}

fn unknown_explanation(reason: &str) -> &str {
    match reason {
        "no_runtime" => "未配置受控 host 目录，没有任何进程观察",
        "no_instance" => "没有受控实例记录此会话；外部 CLI 不在观察范围内",
        "host_unreachable" => "记录了实例但 host 未应答",
        "record_missing" => "实例记录消失而进程仍存在",
        "duplicate_host" => "多个 host 声明同一会话",
        "identity_unverifiable" => "进程身份无法核验",
        "platform_unsupported" => "当前平台没有进程表支持",
        other => other,
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

/// Rename never follows a symlink at either end; a destination that already
/// exists is a conflict, not something to replace. Cross-device moves are
/// refused because they cannot be atomic.
fn rename_same_filesystem(from: &Path, to: &Path) -> Result<(), TrashError> {
    match fs::symlink_metadata(to) {
        Ok(_) => {
            return Err(TrashError::new(
                409,
                "destination_exists",
                format!("目标路径已存在: {}", to.display()),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err(TrashError::new(503, "stat_failed", "目标路径暂时无法检查")),
    }
    fs::rename(from, to).map_err(|error| {
        if error.kind() == io::ErrorKind::CrossesDevices {
            TrashError::new(
                409,
                "cross_filesystem",
                "回收站目录与数据源不在同一文件系统，拒绝非原子的跨设备移动",
            )
        } else {
            TrashError::new(
                500,
                "rename_failed",
                format!("移动文件失败: {}", error.kind()),
            )
        }
    })
}

/// Recreate missing parents of `path` inside `root`, then re-walk so a link
/// introduced meanwhile is still rejected.
fn ensure_parent(root: &Path, path: &Path) -> Result<(), TrashError> {
    plan::trusted_within(root, path)?;
    let parent = path
        .parent()
        .ok_or_else(|| TrashError::new(403, "path_outside_root", "路径没有父目录"))?;
    if fs::symlink_metadata(parent).is_err() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(parent)
            .map_err(|_| TrashError::new(500, "restore_failed", "无法重建原始目录"))?;
    }
    plan::trusted_within(root, path)
}

/// Explicit private directory: absolute, no `..`, no symlink/reparse ancestor,
/// 0700 on Unix. Mirrors the audit/delivery directory checks.
pub fn validate_directory(path: &Path) -> io::Result<PathBuf> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "SESSIONDOCK_TRASH_DIR requires an explicit existing absolute directory without relative jumps",
        )
    };
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(invalid());
    }
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| invalid())?;
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        if metadata.file_type().is_symlink() || reparse {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "trash directory and its ancestors must not be symlinks or reparse points",
            ));
        }
        if !metadata.is_dir() {
            return Err(invalid());
        }
        #[cfg(unix)]
        if ancestor == path {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o777 != 0o700 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "trash directory requires private owner-only permissions (0700)",
                ));
            }
        }
    }
    path.canonicalize().map_err(|_| invalid())
}
