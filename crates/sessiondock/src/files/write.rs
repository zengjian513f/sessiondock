//! Authenticated operator file mutations through checked parent handles.
//! Reads/writes resolve symlink parents; rename/trash preserve the named leaf.
//! Configured roots enable writes without imposing a directory jail. Default
//! conflicts never overwrite; explicit replace first preserves old data in the
//! scoped private trash. See `docs/files.md`.

use super::{
    FileError, FileScope, ResolvedTarget,
    boundary::{self, Identity, Root, identity, ordinary, unshared},
    jobs::{self, Job, JobState, Registry, Upload},
};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, DirBuilder, OpenOptions};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    ffi::{OsStr, OsString},
    io::{self, ErrorKind, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub const MAX_WRITE_ITEMS: usize = 2000;
pub const DEFAULT_UPLOAD_CHUNK_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_NAME_BYTES: usize = 255;
pub const UPLOAD_DIR: &str = ".sessiondock-upload";
/// Deleted entries move into this private subdirectory of the state directory
/// (`<SESSIONDOCK_STATE_DIR>/file-trash/<32 hex>/<name>` plus a manifest), like
/// Python's `file-manager/trash`, never into the project tree.
pub const FILE_TRASH_DIR: &str = "file-trash";
const MAX_KEEP_ATTEMPTS: u32 = 100_000;
const COPY_CHUNK: usize = 64 * 1024;

/// Upload sizes shared with the Python file manager.
#[derive(Clone, Debug)]
pub struct WriteLimits {
    pub max_job_bytes: u64,
    pub max_chunk_bytes: usize,
}
impl Default for WriteLimits {
    fn default() -> Self {
        Self {
            max_job_bytes: 1024 * 1024 * 1024 * 1024,
            max_chunk_bytes: DEFAULT_UPLOAD_CHUNK_BYTES,
        }
    }
}

/// Jobs are bound to the exact session scope that created them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScopeKey {
    pub uid: String,
    pub agent: Option<String>,
}
impl ScopeKey {
    pub fn new(scope: &FileScope<'_>) -> Self {
        Self {
            uid: scope.uid.to_owned(),
            agent: scope.agent.map(str::to_owned),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    Error,
    Keep,
    Skip,
    Replace,
}
impl Conflict {
    fn parse(raw: Option<&str>) -> Result<Self, FileError> {
        match raw.unwrap_or("") {
            "" | "error" => Ok(Self::Error),
            "keep" => Ok(Self::Keep),
            "skip" => Ok(Self::Skip),
            "replace" => Ok(Self::Replace),
            _ => Err(FileError::new(
                400,
                "file_conflict_invalid",
                "无效的重名处理方式",
            )),
        }
    }
    pub(super) fn wire(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Keep => "keep",
            Self::Skip => "skip",
            Self::Replace => "replace",
        }
    }
}

/// Legacy `files/action` body. Scope fields are consumed by transport.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ActionRequest {
    pub uid: String,
    pub agent: String,
    pub r#ref: String,
    pub action: String,
    pub paths: Vec<String>,
    pub destination: Option<String>,
    pub name: Option<String>,
    pub conflict: Option<String>,
    pub size: Option<u64>,
    pub modified: Option<f64>,
    pub sha256: Option<String>,
    pub job: Option<String>,
}

/// HTTP status plus the legacy `{job: …}` body (with `error`/`code` when the
/// whole request failed).
pub struct Outcome {
    pub status: u16,
    pub body: Value,
}
impl Outcome {
    fn ok(job: &Job) -> Self {
        Self {
            status: 200,
            body: json!({"job": job.public()}),
        }
    }
    fn failed(job: &Job, error: &FileError) -> Self {
        let mut body = json!({"job": job.public(), "error": error.message, "code": error.code});
        if let Some(details) = error.details.as_object() {
            for (key, value) in details {
                body[key] = value.clone();
            }
        }
        Self {
            status: error.status,
            body,
        }
    }
}

struct Located {
    target: ResolvedTarget,
    path: PathBuf,
}
struct Entry {
    dir: Located,
    name: String,
    path: PathBuf,
}

pub struct WriteService {
    private_root: Option<PathBuf>,
    /// `<state>/file-trash`, opened at startup; `None` keeps `delete` refused.
    trash: Option<Arc<Root>>,
    limits: WriteLimits,
    registry: Registry,
    /// Test seam invoked between path resolution and the filesystem change,
    /// so races can be reproduced deterministically.
    #[cfg(test)]
    pub(super) before_write: std::sync::Mutex<Option<Box<dyn FnMut() + Send>>>,
}
impl WriteService {
    /// Nonempty existing roots enable compatibility with prior configurations.
    /// `state_dir` receives private `file-trash`; without it delete/replace of
    /// existing targets is unavailable.
    pub fn open(
        _roots: Vec<PathBuf>,
        limits: WriteLimits,
        state_dir: Option<PathBuf>,
    ) -> Result<Self, FileError> {
        if limits.max_job_bytes == 0 || limits.max_chunk_bytes == 0 {
            return Err(FileError::new(
                400,
                "file_write_limits_invalid",
                "写入预算必须为正数",
            ));
        }
        let private_root = state_dir
            .as_ref()
            .map(|path| path.canonicalize().map_err(FileError::io))
            .transpose()?;
        let trash = state_dir
            .map(|state| {
                #[cfg(windows)]
                let state = attachment_directory_path(state);
                let state_root = Root::open(&state)?;
                private_subdir(&state_root.directory, FILE_TRASH_DIR)?;
                Root::open(&state_root.path.join(FILE_TRASH_DIR)).map(Arc::new)
            })
            .transpose()?;
        Ok(Self {
            private_root,
            trash,
            registry: Registry::new(),
            limits,
            #[cfg(test)]
            before_write: std::sync::Mutex::new(None),
        })
    }
    fn before_write(&self) {
        #[cfg(test)]
        if let Ok(mut hook) = self.before_write.lock()
            && let Some(hook) = hook.as_mut()
        {
            hook()
        }
    }

    pub fn limits(&self) -> &WriteLimits {
        &self.limits
    }

    /// Declared to the legacy frontend; absent on Python, so the page keeps
    /// its original defaults there.
    pub fn capabilities(&self) -> Value {
        let mut actions = vec!["upload", "cancel", "mkdir", "new-file", "rename", "move"];
        if self.trash.is_some() {
            actions.push("delete");
        }
        let mut conflicts = vec!["error", "keep", "skip"];
        if self.trash.is_some() {
            conflicts.push("replace");
        }
        json!({
            "actions": actions,
            "conflicts": conflicts,
            "delete": "trash",
            "chunk_bytes": self.limits.max_chunk_bytes,
            "job_bytes": self.limits.max_job_bytes,
            "max_items": MAX_WRITE_ITEMS,
        })
    }

    /// Display flag for listings: operator file writes are enabled.
    /// Not an authorization; every mutation re-resolves independently.
    pub fn writable(&self, _path: &Path) -> bool {
        true
    }

    pub fn jobs(&self, scope: &FileScope<'_>) -> Value {
        json!({"jobs": self.registry.list(&ScopeKey::new(scope))})
    }

    /// Completed upload lookup for attachment recording; the transport decides
    /// whether a metadata store exists.
    pub fn completed_upload(
        &self,
        scope: &FileScope<'_>,
        job_id: &str,
    ) -> Result<Value, FileError> {
        let handle = self.registry.find(&ScopeKey::new(scope), job_id)?;
        let job = jobs::lock(&handle)?;
        if job.action != "upload" || job.state != JobState::Completed {
            return Err(FileError::new(
                409,
                "file_job_not_completed",
                "只能登记已完成的上传任务",
            ));
        }
        let Some(path) = job.path.clone() else {
            return Err(FileError::new(
                409,
                "file_job_skipped",
                "该上传因重名被跳过，没有可登记的文件",
            ));
        };
        Ok(json!({
            "job": job.id, "path": path, "name": job.published_name.clone().unwrap_or_else(|| job.name.clone()),
            "size": job.total_bytes, "sha256": job.sha256, "destination": job.destination,
        }))
    }

    // ---- authorization -------------------------------------------------

    fn normalized(&self, _anchor: &ResolvedTarget, raw: &str) -> Result<PathBuf, FileError> {
        boundary::mutation_path(raw)
    }
    fn root_for(&self, path: &Path) -> Result<Arc<Root>, FileError> {
        boundary::volume_root(path)
    }

    /// Python Manager.guard: protect the filesystem root, home itself and the
    /// private file-manager state, including its ancestors and descendants.
    fn guard(&self, path: &Path) -> Result<(), FileError> {
        let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        if path.parent().is_none()
            || home.as_deref() == Some(path)
            || self.private_root.as_ref().is_some_and(|private| {
                resolved.starts_with(private) || private.starts_with(&resolved)
            })
        {
            return Err(FileError::new(
                403,
                "file_root_immutable",
                "不能修改根目录、用户主目录或文件管理器的数据目录",
            ));
        }
        Ok(())
    }

    /// An existing directory resolved through its symlinks.
    fn directory(&self, _anchor: &ResolvedTarget, raw: &str) -> Result<Located, FileError> {
        let path = boundary::absolute_navigation(raw)?;
        let root = self.root_for(&path)?;
        let target = boundary::open_target(root.clone(), path.clone())?;
        if target.kind() != "directory" {
            return Err(FileError::new(
                400,
                "file_directory_required",
                "目标必须是目录",
            ));
        }
        Ok(Located { target, path })
    }
    /// A named entry; resolve the parent and preserve the leaf itself.
    fn entry(&self, anchor: &ResolvedTarget, raw: &str) -> Result<Entry, FileError> {
        let path = self.normalized(anchor, raw)?;
        let root = self.root_for(&path)?;
        self.guard(&path)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| FileError::new(400, "file_path_invalid", "路径组件无效"))?
            .to_owned();
        validate_name(&name)?;
        let parent = path.parent().unwrap_or(&root.path).to_path_buf();
        let target = boundary::open_target(root.clone(), parent.clone())?;
        if target.kind() != "directory" {
            return Err(FileError::new(
                400,
                "file_parent_not_directory",
                "路径的父组件不是目录",
            ));
        }
        Ok(Entry {
            dir: Located {
                target,
                path: parent,
            },
            name,
            path,
        })
    }

    fn destination_conflict(
        &self,
        destination: &ResolvedTarget,
        name: &str,
        key: &ScopeKey,
        conflict: Conflict,
    ) -> Result<Conflict, FileError> {
        self.guard(&destination.path().join(name))?;
        if conflict != Conflict::Replace {
            return Ok(conflict);
        }
        destination.verify_identity()?;
        let dir = destination.directory()?;
        match dir.symlink_metadata(name) {
            Ok(_) => {
                self.trash_named(dir, &destination.path().join(name), name, key)?;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(FileError::io(error)),
        }
        destination.verify_identity()?;
        Ok(Conflict::Error)
    }

    // ---- actions ---------------------------------------------------------

    pub fn action(
        &self,
        anchor: &ResolvedTarget,
        scope: &FileScope<'_>,
        request: ActionRequest,
    ) -> Result<Outcome, FileError> {
        if anchor.kind() != "directory" {
            return Err(FileError::new(
                400,
                "file_directory_anchor_required",
                "文件操作入口必须是会话提及的目录",
            ));
        }
        let key = ScopeKey::new(scope);
        match request.action.as_str() {
            "upload" => self.start_upload(anchor, key, request),
            "cancel" => self.cancel(&key, request.job.as_deref().unwrap_or("")),
            "mkdir" | "new-file" => self.create(anchor, key, request),
            "rename" => self.rename(anchor, key, request),
            "move" => self.relocate(anchor, key, request),
            "delete" => self.delete(anchor, key, request),
            "retry" | "copy" | "trash" | "restore" | "purge" | "compress" | "extract"
            | "bundle" => Err(FileError::new(
                501,
                "file_action_not_implemented",
                format!(
                    "写入服务尚未实现此文件操作：{}；不会伪造任务或成功结果",
                    request.action
                ),
            )),
            _ => Err(FileError::new(400, "file_action_invalid", "未知文件操作")),
        }
    }

    fn items(&self, anchor: &ResolvedTarget, raw: &[String]) -> Result<Vec<PathBuf>, FileError> {
        if raw.is_empty() {
            return Err(FileError::new(400, "file_paths_required", "请先选择项目"));
        }
        if raw.len() > MAX_WRITE_ITEMS {
            return Err(FileError::new(
                413,
                "file_items_limit",
                format!("一次最多操作 {MAX_WRITE_ITEMS} 个项目"),
            )
            .with_details(json!({"limit": MAX_WRITE_ITEMS})));
        }
        let mut paths: Vec<PathBuf> = Vec::new();
        for item in raw {
            let path = self.normalized(anchor, item)?;
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        // Selecting a parent and its child must not operate on the child twice.
        let all = paths.clone();
        paths.retain(|path| {
            !all.iter()
                .any(|other| other != path && path.starts_with(other))
        });
        Ok(paths)
    }

    fn finish(&self, mut job: Job) -> Outcome {
        job.items_done = job.completed.len();
        job.state = if job.errors.is_empty() {
            JobState::Completed
        } else {
            JobState::Failed
        };
        let outcome = if !job.errors.is_empty() && job.completed.is_empty() {
            let first = job.errors[0].clone();
            let error = FileError::new(
                first["status"].as_u64().unwrap_or(409) as u16,
                "file_action_failed",
                first["error"].as_str().unwrap_or("文件操作失败").to_owned(),
            );
            let mut outcome = Outcome::failed(&job, &error);
            outcome.body["code"] = first["code"].clone();
            outcome
        } else {
            Outcome::ok(&job)
        };
        self.registry.insert_finished(job);
        outcome
    }
    fn record(job: &mut Job, path: &Path, result: Result<Value, FileError>) {
        let wire = boundary::wire_path(path).unwrap_or_else(|_| path.display().to_string());
        match result {
            Ok(mut value) => {
                value["path"] = json!(wire);
                job.completed.push(value);
            }
            Err(error) => job.errors.push(json!({
                "path": wire, "error": error.message, "code": error.code, "status": error.status
            })),
        }
    }

    fn create(
        &self,
        anchor: &ResolvedTarget,
        key: ScopeKey,
        request: ActionRequest,
    ) -> Result<Outcome, FileError> {
        let name = required_name(request.name.as_deref())?;
        let conflict = Conflict::parse(request.conflict.as_deref())?;
        let destination = self.directory(
            anchor,
            required(request.destination.as_deref(), "destination")?,
        )?;
        let action: &'static str = if request.action == "mkdir" {
            "mkdir"
        } else {
            "new-file"
        };
        let mut job = Job::new(key, action, name.clone(), &destination.path, conflict, 1)?;
        let path = destination.path.join(&name);
        self.guard(&path)?;
        self.before_write();
        let result = (|| {
            destination.target.verify_identity()?;
            let dir = destination.target.directory()?;
            let conflict =
                self.destination_conflict(&destination.target, &name, &job.scope, conflict)?;
            let published = if action == "mkdir" {
                create_directory(dir, &name, conflict)?
            } else {
                create_empty_file(dir, &name, conflict)?
            };
            destination.target.verify_identity()?;
            Ok(match published {
                Some(created) => {
                    json!({"created": boundary::wire_path(&destination.path.join(created))?})
                }
                None => json!({"skipped": true}),
            })
        })();
        Self::record(&mut job, &path, result);
        Ok(self.finish(job))
    }

    fn rename(
        &self,
        anchor: &ResolvedTarget,
        key: ScopeKey,
        request: ActionRequest,
    ) -> Result<Outcome, FileError> {
        let name = required_name(request.name.as_deref())?;
        let conflict = Conflict::parse(request.conflict.as_deref())?;
        if request.paths.len() != 1 {
            return Err(FileError::new(
                400,
                "file_single_item_required",
                "请选择一个项目",
            ));
        }
        let source = self.entry(anchor, &request.paths[0])?;
        self.guard(&source.dir.path.join(&name))?;
        let mut job = Job::new(key, "rename", name.clone(), &source.dir.path, conflict, 1)?;
        self.before_write();
        let result = (|| {
            if source.name == name && conflict != Conflict::Keep {
                return Err(FileError::new(400, "file_same_path", "源路径和目标相同"));
            }
            source.dir.target.verify_identity()?;
            let dir = source.dir.target.directory()?;
            let conflict =
                self.destination_conflict(&source.dir.target, &name, &job.scope, conflict)?;
            let published = relocate_entry(dir, &source.name, dir, &name, conflict)?;
            source.dir.target.verify_identity()?;
            Ok(match published {
                Some(new_name) => {
                    json!({"renamed": boundary::wire_path(&source.dir.path.join(new_name))?})
                }
                None => json!({"skipped": true}),
            })
        })();
        Self::record(&mut job, &source.path, result);
        Ok(self.finish(job))
    }

    fn relocate(
        &self,
        anchor: &ResolvedTarget,
        key: ScopeKey,
        request: ActionRequest,
    ) -> Result<Outcome, FileError> {
        let conflict = Conflict::parse(request.conflict.as_deref())?;
        let paths = self.items(anchor, &request.paths)?;
        let destination = self.directory(
            anchor,
            required(request.destination.as_deref(), "destination")?,
        )?;
        let mut job = Job::new(
            key,
            "move",
            paths[0]
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_owned(),
            &destination.path,
            conflict,
            paths.len(),
        )?;
        for path in &paths {
            let result = (|| {
                let source = self.entry(
                    anchor,
                    path.to_str().ok_or_else(|| {
                        FileError::new(400, "file_path_encoding", "路径无法表示为 UTF-8")
                    })?,
                )?;
                if destination.path.starts_with(&source.path) {
                    return Err(FileError::new(
                        400,
                        "file_move_into_self",
                        "不能把目录放进自身或子目录",
                    ));
                }
                if destination.path == source.dir.path && conflict != Conflict::Keep {
                    return Err(FileError::new(400, "file_same_path", "源路径和目标相同"));
                }
                self.guard(&destination.path.join(&source.name))?;
                self.before_write();
                source.dir.target.verify_identity()?;
                destination.target.verify_identity()?;
                let conflict = self.destination_conflict(
                    &destination.target,
                    &source.name,
                    &job.scope,
                    conflict,
                )?;
                let published = match relocate_entry(
                    source.dir.target.directory()?,
                    &source.name,
                    destination.target.directory()?,
                    &source.name,
                    conflict,
                ) {
                    Err(error) if error.code == "file_move_cross_device" => {
                        let copied = copy_entry_publish(
                            source.dir.target.directory()?,
                            &source.name,
                            destination.target.directory()?,
                            &source.name,
                            conflict,
                        )?;
                        if copied.is_some() {
                            // Python keeps the original recoverable after publishing a
                            // cross-device copy. Trash also supports different devices.
                            source.dir.target.verify_identity()?;
                            destination.target.verify_identity()?;
                            self.trash(&source, &job.scope)?;
                        }
                        copied
                    }
                    other => other?,
                };
                source.dir.target.verify_identity()?;
                destination.target.verify_identity()?;
                Ok(match published {
                    Some(new_name) => {
                        json!({"moved": boundary::wire_path(&destination.path.join(new_name))?})
                    }
                    None => json!({"skipped": true}),
                })
            })();
            Self::record(&mut job, path, result);
        }
        Ok(self.finish(job))
    }

    fn delete(
        &self,
        anchor: &ResolvedTarget,
        key: ScopeKey,
        request: ActionRequest,
    ) -> Result<Outcome, FileError> {
        let paths = self.items(anchor, &request.paths)?;
        let mut job = Job::new(
            key.clone(),
            "delete",
            paths[0]
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_owned(),
            &anchor.root.path,
            Conflict::Error,
            paths.len(),
        )?;
        for path in &paths {
            let result = (|| {
                let source = self.entry(
                    anchor,
                    path.to_str().ok_or_else(|| {
                        FileError::new(400, "file_path_encoding", "路径无法表示为 UTF-8")
                    })?,
                )?;
                self.before_write();
                source.dir.target.verify_identity()?;
                let trash = self.trash(&source, &key)?;
                source.dir.target.verify_identity()?;
                Ok(json!({"trash": trash}))
            })();
            Self::record(&mut job, path, result);
        }
        Ok(self.finish(job))
    }

    /// Delete never unlinks: the entry is renamed into a fresh private
    /// `<state>/file-trash/<id>/` directory with a manifest.
    fn trash(&self, source: &Entry, key: &ScopeKey) -> Result<Value, FileError> {
        self.trash_named(
            source.dir.target.directory()?,
            &source.path,
            &source.name,
            key,
        )
    }

    fn trash_named(
        &self,
        dir: &Dir,
        path: &Path,
        name: &str,
        key: &ScopeKey,
    ) -> Result<Value, FileError> {
        let trash = self.trash.as_ref().ok_or_else(|| {
            FileError::new(
                501,
                "file_trash_unconfigured",
                "未配置 SESSIONDOCK_STATE_DIR，没有回收目录；写入服务不会直接删除",
            )
        })?;
        let before = dir.symlink_metadata(name).map_err(FileError::io)?;
        unshared(&before)?;
        let expected = identity(&before);
        trash.verify()?;
        let (item_id, item) = fresh_private_child(&trash.directory)?;
        let manifest = json!({
            "id": item_id, "path": boundary::wire_path(path)?, "name": name,
            "kind": if before.is_symlink() { "symlink" } else if before.is_dir() { "directory" } else { "file" },
            "deleted": jobs::now(), "uid": key.uid, "agent": key.agent,
        });
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        cap_std::fs::OpenOptionsExt::mode(&mut options, 0o600);
        item.open_with("manifest.json", &options)
            .and_then(|mut file| {
                file.write_all(manifest.to_string().as_bytes())?;
                file.sync_all()
            })
            .map_err(FileError::io)?;
        let renamed = match rename_noreplace(dir, name, &item, name) {
            Ok(()) => true,
            Err(error) if error.kind() == ErrorKind::CrossesDevices => {
                let copied = copy_entry(dir, name.as_ref(), &item, name.as_ref())?;
                copied.verify(dir, name.as_ref())?;
                copied.remove(dir, name.as_ref())?;
                false
            }
            Err(error) => {
                let _ = item.remove_file("manifest.json");
                let _ = trash.directory.remove_dir(&item_id);
                return Err(FileError::io(error));
            }
        };
        let after = item
            .symlink_metadata(name)
            .map_err(|_| FileError::changed())?;
        if renamed && identity(&after) != expected {
            return Err(FileError::changed());
        }
        Ok(json!({
            "id": item_id,
            "path": boundary::wire_path(&trash.path.join(&item_id).join(name))?,
        }))
    }

    // ---- uploads ---------------------------------------------------------

    fn start_upload(
        &self,
        anchor: &ResolvedTarget,
        key: ScopeKey,
        request: ActionRequest,
    ) -> Result<Outcome, FileError> {
        let name = required_name(request.name.as_deref())?;
        let conflict = Conflict::parse(request.conflict.as_deref())?;
        let size = request
            .size
            .ok_or_else(|| FileError::new(400, "file_upload_size_invalid", "无效的上传大小"))?;
        if size > self.limits.max_job_bytes {
            return Err(FileError::new(
                413,
                "file_job_too_large",
                format!("单个上传最多 {} 字节", self.limits.max_job_bytes),
            )
            .with_details(json!({"limit": self.limits.max_job_bytes})));
        }
        let declared = request.sha256.as_deref().map(parse_sha256).transpose()?;
        if request
            .modified
            .is_some_and(|value| !value.is_finite() || value < 0.0)
        {
            return Err(FileError::new(
                400,
                "file_upload_modified_invalid",
                "无效的修改时间",
            ));
        }
        let destination = self.directory(
            anchor,
            required(request.destination.as_deref(), "destination")?,
        )?;
        self.guard(&destination.path.join(&name))?;
        self.before_write();
        destination.target.verify_identity()?;
        let dir = destination.target.directory()?;
        if conflict == Conflict::Error && dir.symlink_metadata(&name).is_ok() {
            return Err(FileError::new(
                409,
                "file_exists",
                "目标已存在；写入服务不会覆盖，请改名、跳过或保留两份",
            ));
        }
        let staging_dir = private_subdir(dir, UPLOAD_DIR)?;
        let mut job = Job::new(key, "upload", name, &destination.path, conflict, 1)?;
        let staging_name = format!("{}.part", job.id);
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        cap_std::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = staging_dir
            .open_with(&staging_name, &options)
            .map_err(FileError::io)?
            .into_std();
        job.state = JobState::Uploading;
        job.total_bytes = size;
        job.upload_modified = request.modified;
        job.declared_sha256 = declared;
        job.upload = Some(Upload {
            dest: destination.target,
            staging_dir,
            staging_name,
            file,
            hasher: Default::default(),
        });
        let job = self.registry.insert_uploading(job)?;
        let mut guard = jobs::lock(&job)?;
        let job: &mut Job = &mut guard;
        if size == 0 {
            return Ok(self.finalize(job));
        }
        Ok(Outcome::ok(job))
    }

    fn cancel(&self, key: &ScopeKey, job_id: &str) -> Result<Outcome, FileError> {
        let handle = self.registry.find(key, job_id)?;
        let mut guard = jobs::lock(&handle)?;
        let job: &mut Job = &mut guard;
        if let Some(upload) = job.upload.take() {
            drop(upload.file);
            let _ = upload.staging_dir.remove_file(&upload.staging_name);
            job.state = JobState::Cancelled;
        }
        Ok(Outcome::ok(job))
    }

    /// `offset` must equal the bytes received so far. Re-sending exactly the
    /// last accepted chunk is idempotent; anything else is a 409 that names
    /// the expected offset. The final chunk publishes synchronously.
    pub fn upload(
        &self,
        _anchor: &ResolvedTarget,
        scope: &FileScope<'_>,
        job_id: &str,
        offset: u64,
        chunk: &[u8],
    ) -> Result<Outcome, FileError> {
        if chunk.len() > self.limits.max_chunk_bytes {
            return Err(FileError::new(
                413,
                "file_upload_chunk_too_large",
                format!("单个分块最多 {} 字节", self.limits.max_chunk_bytes),
            )
            .with_details(json!({"limit": self.limits.max_chunk_bytes})));
        }
        let handle = self.registry.find(&ScopeKey::new(scope), job_id)?;
        let mut guard = jobs::lock(&handle)?;
        let job: &mut Job = &mut guard;
        let length = chunk.len() as u64;
        let Some(upload) = job.upload.as_mut() else {
            if job.state == JobState::Completed
                && offset <= job.total_bytes
                && offset + length == job.total_bytes
            {
                return Ok(Outcome::ok(job));
            }
            return Err(
                FileError::new(409, "file_upload_finished", "上传已结束或已取消")
                    .with_details(json!({"state": job.state.wire()})),
            );
        };
        // Root replaced, a component swapped for a link, or the destination
        // directory moved: refuse before touching the staging file.
        self.before_write();
        upload.dest.verify_identity()?;
        let received = job.received;
        if offset == received {
            if received + length > job.total_bytes {
                return Err(
                    FileError::new(413, "file_upload_overflow", "分块超过声明的上传大小")
                        .with_details(json!({"limit": job.total_bytes, "expected": received})),
                );
            }
            let write = upload
                .file
                .seek(SeekFrom::Start(received))
                .and_then(|_| upload.file.write_all(chunk));
            if let Err(error) = write {
                let _ = upload.file.set_len(received);
                return Err(FileError::io(error));
            }
            use sha2::Digest;
            upload.hasher.update(chunk);
            job.last_offset = received;
            job.received = received + length;
        } else if offset == job.last_offset && offset + length == received && length > 0 {
            let mut previous = vec![0u8; chunk.len()];
            let same = upload
                .file
                .seek(SeekFrom::Start(offset))
                .and_then(|_| upload.file.read_exact(&mut previous))
                .is_ok()
                && previous == chunk;
            if !same {
                return Err(FileError::new(
                    409,
                    "file_upload_offset",
                    "重发的分块与已接收数据不一致，请刷新任务后从预期位置继续",
                )
                .with_details(json!({"expected": received})));
            }
        } else {
            return Err(FileError::new(
                409,
                "file_upload_offset",
                "上传位置不一致，请刷新任务后从预期位置继续",
            )
            .with_details(json!({"expected": received})));
        }
        job.updated = jobs::now();
        if job.received == job.total_bytes {
            return Ok(self.finalize(job));
        }
        Ok(Outcome::ok(job))
    }

    fn finalize(&self, job: &mut Job) -> Outcome {
        let Some(upload) = job.upload.take() else {
            return Outcome::ok(job);
        };
        let Upload {
            dest,
            staging_dir,
            staging_name,
            file,
            hasher,
        } = upload;
        use sha2::Digest;
        let digest: [u8; 32] = hasher.finalize().into();
        let computed = hex(&digest);
        let conflict = job.conflict;
        let name = job.name.clone();
        let result = (|| {
            if let Some(declared) = job.declared_sha256
                && declared != digest
            {
                return Err(FileError::new(
                    409,
                    "file_upload_checksum",
                    "上传数据的 SHA-256 与声明不符，已丢弃暂存数据",
                )
                .with_details(json!({"sha256": computed})));
            }
            file.sync_all().map_err(FileError::io)?;
            drop(file);
            dest.verify_identity()?;
            let dir = dest.directory()?;
            let conflict = self.destination_conflict(&dest, &name, &job.scope, conflict)?;
            let published = publish_file(&staging_dir, &staging_name, dir, &name, conflict)?;
            dest.verify_identity()?;
            Ok(published)
        })();
        // The staging name must not survive or leak partial bytes.
        let _ = staging_dir.remove_file(&staging_name);
        job.sha256 = Some(computed);
        job.updated = jobs::now();
        match result {
            Ok(Some(published)) => {
                let path = dest.path().join(&published);
                let wire =
                    boundary::wire_path(&path).unwrap_or_else(|_| path.display().to_string());
                job.path = Some(wire.clone());
                job.published_name = Some(published);
                job.completed.push(json!({"path": wire}));
                job.items_done = 1;
                job.state = JobState::Completed;
                Outcome::ok(job)
            }
            Ok(None) => {
                job.completed
                    .push(json!({"path": dest.path().join(&name).to_str(), "skipped": true}));
                job.items_done = 1;
                job.state = JobState::Completed;
                Outcome::ok(job)
            }
            Err(error) => {
                job.errors.push(json!({
                    "path": dest.path().join(&name).to_str(), "error": error.message,
                    "code": error.code, "status": error.status,
                }));
                job.state = JobState::Failed;
                Outcome::failed(job, &error)
            }
        }
    }
}

// ---- helpers -------------------------------------------------------------

fn required<'a>(value: Option<&'a str>, field: &str) -> Result<&'a str, FileError> {
    value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| FileError::new(400, "file_field_required", format!("缺少必需字段 {field}")))
}
fn required_name(value: Option<&str>) -> Result<String, FileError> {
    let name = required(value, "name")?;
    validate_name(name)?;
    Ok(name.to_owned())
}
/// One component: no separators, no `.`/`..`, no reserved prefix, platform
/// path rules (Windows reserved names, foreign separators) included.
pub fn validate_name(name: &str) -> Result<(), FileError> {
    let invalid = || {
        FileError::new(
            400,
            "file_name_invalid",
            "名称无效：须为单个路径组件，不能包含 /、\\、控制字符或为 . 与 ..",
        )
    };
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(invalid());
    }
    if name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(invalid());
    }
    if Path::new(name).components().count() != 1
        || !matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err(invalid());
    }
    Ok(())
}
fn parse_sha256(raw: &str) -> Result<[u8; 32], FileError> {
    let invalid = || FileError::new(400, "file_sha256_invalid", "sha256 须为 64 位十六进制");
    if raw.len() != 64 {
        return Err(invalid());
    }
    let mut out = [0u8; 32];
    for (index, pair) in raw.as_bytes().chunks(2).enumerate() {
        let text = std::str::from_utf8(pair).map_err(|_| invalid())?;
        out[index] = u8::from_str_radix(text, 16).map_err(|_| invalid())?;
    }
    Ok(out)
}
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn numbered(name: &str, attempt: u32) -> String {
    let path = Path::new(name);
    match (
        path.file_stem().and_then(|s| s.to_str()),
        path.extension().and_then(|e| e.to_str()),
    ) {
        (Some(stem), Some(extension)) => format!("{stem} ({attempt}).{extension}"),
        _ => format!("{name} ({attempt})"),
    }
}
fn exists_error() -> FileError {
    FileError::new(
        409,
        "file_exists",
        "目标已存在；写入服务不会覆盖，请改名、跳过或保留两份",
    )
}
fn keep_exhausted() -> FileError {
    FileError::new(409, "file_keep_exhausted", "无法生成唯一名称")
}

/// Ensures `<root>/<name>` exists as a private (0700) real directory owned by
/// this service and returns its handle. Links and lax permissions are refused.
pub(super) fn private_subdir(root: &Arc<Dir>, name: &str) -> Result<Arc<Dir>, FileError> {
    if root.symlink_metadata(name).is_err() {
        #[allow(unused_mut)]
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        cap_std::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match root.create_dir_with(name, &builder) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(FileError::io(error)),
        }
    }
    let before = root.symlink_metadata(name).map_err(FileError::io)?;
    ordinary(&before)?;
    if !before.is_dir() {
        return Err(FileError::new(
            403,
            "file_private_dir_invalid",
            format!("{name} 必须是目录"),
        ));
    }
    let dir = Arc::new(root.open_dir_nofollow(name).map_err(FileError::io)?);
    let metadata = dir.dir_metadata().map_err(FileError::io)?;
    if identity(&metadata) != identity(&before) {
        return Err(FileError::changed());
    }
    #[cfg(unix)]
    if cap_std::fs::MetadataExt::mode(&metadata) & 0o077 != 0 {
        return Err(FileError::new(
            403,
            "file_private_dir_permissions",
            format!("{name} 目录必须仅所有者可访问 (0700)"),
        ));
    }
    Ok(dir)
}
fn fresh_private_child(parent: &Dir) -> Result<(String, Dir), FileError> {
    for _ in 0..4 {
        let id = jobs::random_id()?;
        #[allow(unused_mut)]
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        cap_std::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match parent.create_dir_with(&id, &builder) {
            Ok(()) => {
                let dir = parent.open_dir_nofollow(&id).map_err(FileError::io)?;
                return Ok((id, dir));
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(FileError::io(error)),
        }
    }
    Err(FileError::new(503, "file_trash_id", "无法分配回收目录编号"))
}

fn create_directory(
    dir: &Dir,
    name: &str,
    conflict: Conflict,
) -> Result<Option<String>, FileError> {
    for attempt in 0..MAX_KEEP_ATTEMPTS {
        let candidate = if attempt == 0 {
            name.to_owned()
        } else {
            numbered(name, attempt)
        };
        match dir.create_dir(&candidate) {
            Ok(()) => return Ok(Some(candidate)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => match conflict {
                Conflict::Error | Conflict::Replace => return Err(exists_error()),
                Conflict::Skip => return Ok(None),
                Conflict::Keep => continue,
            },
            Err(error) => return Err(FileError::io(error)),
        }
    }
    Err(keep_exhausted())
}
fn create_empty_file(
    dir: &Dir,
    name: &str,
    conflict: Conflict,
) -> Result<Option<String>, FileError> {
    for attempt in 0..MAX_KEEP_ATTEMPTS {
        let candidate = if attempt == 0 {
            name.to_owned()
        } else {
            numbered(name, attempt)
        };
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        cap_std::fs::OpenOptionsExt::mode(&mut options, 0o644);
        match dir.open_with(&candidate, &options) {
            Ok(_) => return Ok(Some(candidate)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => match conflict {
                Conflict::Error | Conflict::Replace => return Err(exists_error()),
                Conflict::Skip => return Ok(None),
                Conflict::Keep => continue,
            },
            // Windows reports AccessDenied when O_EXCL targets an existing
            // directory. Python treats every existing named entry alike.
            Err(_) if dir.symlink_metadata(&candidate).is_ok() => match conflict {
                Conflict::Error | Conflict::Replace => return Err(exists_error()),
                Conflict::Skip => return Ok(None),
                Conflict::Keep => continue,
            },
            Err(error) => return Err(FileError::io(error)),
        }
    }
    Err(keep_exhausted())
}

/// Regular file: `linkat` into the destination (fails on an existing name),
/// re-check the linked inode, then unlink the source name. Filesystems that
/// refuse the link fall back to an `O_EXCL` copy.
fn publish_file(
    src_dir: &Dir,
    src_name: &str,
    dst_dir: &Dir,
    name: &str,
    conflict: Conflict,
) -> Result<Option<String>, FileError> {
    let before = src_dir.symlink_metadata(src_name).map_err(FileError::io)?;
    unshared(&before)?;
    if !before.is_file() {
        return Err(FileError::new(400, "file_required", "此操作需要普通文件"));
    }
    let expected = identity(&before);
    for attempt in 0..MAX_KEEP_ATTEMPTS {
        let candidate = if attempt == 0 {
            name.to_owned()
        } else {
            numbered(name, attempt)
        };
        match src_dir.hard_link(src_name, dst_dir, &candidate) {
            Ok(()) => {
                let linked = dst_dir
                    .symlink_metadata(&candidate)
                    .map_err(|_| FileError::changed())?;
                if linked.is_symlink() || identity(&linked) != expected {
                    let _ = dst_dir.remove_file(&candidate);
                    return Err(FileError::changed());
                }
                src_dir.remove_file(src_name).map_err(|error| {
                    FileError::new(
                        503,
                        "file_publish_unlink",
                        format!("已发布 {candidate}，但无法移除源名称：{error}"),
                    )
                })?;
                return Ok(Some(candidate));
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => match conflict {
                Conflict::Error | Conflict::Replace => return Err(exists_error()),
                Conflict::Skip => return Ok(None),
                Conflict::Keep => continue,
            },
            Err(error) if hard_links_unavailable(&error) => {
                return copy_publish(src_dir, src_name, dst_dir, name, conflict, attempt);
            }
            Err(error) => return Err(FileError::io(error)),
        }
    }
    Err(keep_exhausted())
}
fn hard_links_unavailable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::CrossesDevices | ErrorKind::PermissionDenied | ErrorKind::Unsupported
    ) || error.raw_os_error().is_some_and(|code| {
        // EPERM (1), EXDEV (18), EMLINK (31), ENOTSUP/EOPNOTSUPP (95).
        matches!(code, 1 | 18 | 31 | 95)
    })
}
/// `O_EXCL` copy fallback: never truncates an existing destination, verifies
/// the source did not change while copying, then unlinks the source name.
fn copy_publish(
    src_dir: &Dir,
    src_name: &str,
    dst_dir: &Dir,
    name: &str,
    conflict: Conflict,
    start: u32,
) -> Result<Option<String>, FileError> {
    let mut read = OpenOptions::new();
    read.read(true).follow(FollowSymlinks::No).nonblock(true);
    let mut source = src_dir.open_with(src_name, &read).map_err(FileError::io)?;
    let before = source.metadata().map_err(FileError::io)?;
    unshared(&before)?;
    if !before.is_file() {
        return Err(FileError::new(400, "file_required", "此操作需要普通文件"));
    }
    let stamp = boundary::Stamp::new(&before);
    for attempt in start..MAX_KEEP_ATTEMPTS {
        let candidate = if attempt == 0 {
            name.to_owned()
        } else {
            numbered(name, attempt)
        };
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        cap_std::fs::OpenOptionsExt::mode(&mut options, 0o644);
        let mut target = match dst_dir.open_with(&candidate, &options) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => match conflict {
                Conflict::Error | Conflict::Replace => return Err(exists_error()),
                Conflict::Skip => return Ok(None),
                Conflict::Keep => continue,
            },
            Err(error) => return Err(FileError::io(error)),
        };
        let copied = (|| -> Result<(), FileError> {
            source.seek(SeekFrom::Start(0)).map_err(FileError::io)?;
            let mut remaining = before.len();
            let mut buffer = vec![0u8; COPY_CHUNK];
            while remaining > 0 {
                let want = remaining.min(COPY_CHUNK as u64) as usize;
                let count = source.read(&mut buffer[..want]).map_err(FileError::io)?;
                if count == 0 {
                    return Err(FileError::changed());
                }
                target.write_all(&buffer[..count]).map_err(FileError::io)?;
                remaining -= count as u64;
            }
            let after = source.metadata().map_err(FileError::io)?;
            if boundary::Stamp::new(&after) != stamp {
                return Err(FileError::changed());
            }
            target.sync_all().map_err(FileError::io)
        })();
        if let Err(error) = copied {
            let _ = dst_dir.remove_file(&candidate);
            return Err(error);
        }
        drop(target);
        drop(source);
        src_dir.remove_file(src_name).map_err(|error| {
            FileError::new(
                503,
                "file_publish_unlink",
                format!("已复制到 {candidate}，但无法移除源名称：{error}"),
            )
        })?;
        return Ok(Some(candidate));
    }
    Err(keep_exhausted())
}
/// Snapshot of the copied source. Retained directory handles and no-follow leaf
/// operations keep recursive copying/removal from walking through substituted links.
struct CopiedEntry {
    stamp: boundary::Stamp,
    metadata: cap_std::fs::Metadata,
    /// A real directory whose children were copied and must be verified and
    /// removed recursively. Directory symlinks are deliberately false.
    recurse_directory: bool,
    /// Windows removes a directory symlink with directory removal semantics,
    /// even though the link itself must never be traversed.
    remove_as_directory: bool,
    children: Vec<(OsString, CopiedEntry)>,
}
impl CopiedEntry {
    fn verify(&self, parent: &Dir, name: &OsStr) -> Result<(), FileError> {
        let current = parent.symlink_metadata(name).map_err(FileError::io)?;
        if boundary::Stamp::new(&current) != self.stamp {
            return Err(FileError::changed());
        }
        if self.recurse_directory {
            let directory = parent.open_dir_nofollow(name).map_err(FileError::io)?;
            if boundary::Stamp::new(&directory.dir_metadata().map_err(FileError::io)?) != self.stamp
            {
                return Err(FileError::changed());
            }
            for (name, child) in &self.children {
                child.verify(&directory, name)?;
            }
        }
        Ok(())
    }
    fn remove(&self, parent: &Dir, name: &OsStr) -> Result<(), FileError> {
        self.verify(parent, name)?;
        self.remove_verified(parent, name)
    }
    fn remove_verified(&self, parent: &Dir, name: &OsStr) -> Result<(), FileError> {
        let current = parent.symlink_metadata(name).map_err(FileError::io)?;
        // Removing another hard-link alias changes ctime/nlink on this inode.
        // Recheck content and identity without rejecting our own prior unlink.
        if identity(&current) != identity(&self.metadata)
            || current.len() != self.metadata.len()
            || current.modified().ok() != self.metadata.modified().ok()
        {
            return Err(FileError::changed());
        }
        if self.recurse_directory {
            let directory = parent.open_dir_nofollow(name).map_err(FileError::io)?;
            let expected = identity(&directory.dir_metadata().map_err(FileError::io)?);
            for (name, child) in &self.children {
                child.remove_verified(&directory, name)?;
            }
            if identity(&parent.symlink_metadata(name).map_err(FileError::io)?) != expected {
                return Err(FileError::changed());
            }
            parent.remove_dir(name).map_err(FileError::io)
        } else if self.remove_as_directory {
            parent.remove_dir(name).map_err(FileError::io)
        } else {
            parent.remove_file(name).map_err(FileError::io)
        }
    }
}

#[cfg(windows)]
fn directory_handle_path(directory: &Dir) -> Result<PathBuf, FileError> {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let mut buffer = vec![0u16; 260];
    loop {
        let length = unsafe {
            GetFinalPathNameByHandleW(
                directory.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                0,
            )
        };
        if length == 0 {
            return Err(FileError::io(io::Error::last_os_error()));
        }
        if length as usize >= buffer.len() {
            buffer.resize(length as usize + 1, 0);
            continue;
        }
        return Ok(PathBuf::from(std::ffi::OsString::from_wide(
            &buffer[..length as usize],
        )));
    }
}

/// Copy ordinary files/directories and the link itself, never a link's target.
fn copy_entry(
    src: &Dir,
    source: &OsStr,
    dst: &Dir,
    destination: &OsStr,
) -> Result<CopiedEntry, FileError> {
    let before = src.symlink_metadata(source).map_err(FileError::io)?;
    unshared(&before)?;
    #[cfg(windows)]
    let remove_as_directory = {
        use cap_std::fs::MetadataExt;
        before.file_attributes() & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY
            != 0
    };
    #[cfg(not(windows))]
    let remove_as_directory = before.is_dir();
    let mut copied = CopiedEntry {
        stamp: boundary::Stamp::new(&before),
        metadata: before.clone(),
        recurse_directory: !before.is_symlink() && before.is_dir(),
        remove_as_directory,
        children: Vec::new(),
    };
    if before.is_symlink() {
        let target = src.read_link_contents(source).map_err(FileError::io)?;
        #[cfg(not(windows))]
        dst.symlink_contents(target, destination)
            .map_err(FileError::io)?;
        #[cfg(windows)]
        {
            // Python copies the link contents even when the target is absolute.
            // cap-fs-ext's symlink methods reject such targets as an escape.
            let destination = directory_handle_path(dst)?.join(destination);
            if remove_as_directory {
                std::os::windows::fs::symlink_dir(target, destination).map_err(FileError::io)?;
            } else {
                std::os::windows::fs::symlink_file(target, destination).map_err(FileError::io)?;
            }
        }
    } else if before.is_dir() {
        let source_dir = src.open_dir_nofollow(source).map_err(FileError::io)?;
        if boundary::Stamp::new(&source_dir.dir_metadata().map_err(FileError::io)?) != copied.stamp
        {
            return Err(FileError::changed());
        }
        dst.create_dir(destination).map_err(FileError::io)?;
        let destination_dir = dst.open_dir_nofollow(destination).map_err(FileError::io)?;
        for entry in source_dir.entries().map_err(FileError::io)? {
            let name = entry.map_err(FileError::io)?.file_name();
            let child = copy_entry(&source_dir, &name, &destination_dir, &name)?;
            copied.children.push((name, child));
        }
        destination_dir
            .set_permissions(".", before.permissions())
            .map_err(FileError::io)?;
    } else {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let mut input = src.open_with(source, &options).map_err(FileError::io)?;
        if boundary::Stamp::new(&input.metadata().map_err(FileError::io)?) != copied.stamp {
            return Err(FileError::changed());
        }
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        let mut output = dst
            .open_with(destination, &options)
            .map_err(FileError::io)?;
        let count = io::copy(
            &mut Read::by_ref(&mut input).take(before.len()),
            &mut output,
        )
        .map_err(FileError::io)?;
        if count != before.len()
            || boundary::Stamp::new(&input.metadata().map_err(FileError::io)?) != copied.stamp
        {
            return Err(FileError::changed());
        }
        output
            .set_permissions(before.permissions())
            .map_err(FileError::io)?;
        output.sync_all().map_err(FileError::io)?;
    }
    dst.set_symlink_times(
        destination,
        before
            .accessed()
            .ok()
            .map(cap_fs_ext::SystemTimeSpec::Absolute),
        before
            .modified()
            .ok()
            .map(cap_fs_ext::SystemTimeSpec::Absolute),
    )
    .map_err(FileError::io)?;
    copied.verify(src, source)?;
    Ok(copied)
}

/// Stage the complete cross-device tree on the destination filesystem, then
/// publish atomically. The caller recycles the original only after success.
fn copy_entry_publish(
    src: &Dir,
    source: &str,
    dst: &Dir,
    name: &str,
    conflict: Conflict,
) -> Result<Option<String>, FileError> {
    let (temporary_name, temporary) = fresh_private_child(dst)?;
    let result = (|| {
        let copied = copy_entry(src, source.as_ref(), &temporary, "data".as_ref())?;
        for attempt in 0..MAX_KEEP_ATTEMPTS {
            let candidate = if attempt == 0 {
                name.to_owned()
            } else {
                numbered(name, attempt)
            };
            copied.verify(src, source.as_ref())?;
            match rename_noreplace(&temporary, "data", dst, &candidate) {
                Ok(()) => return Ok(Some(candidate)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => match conflict {
                    Conflict::Error | Conflict::Replace => return Err(exists_error()),
                    Conflict::Skip => return Ok(None),
                    Conflict::Keep => continue,
                },
                Err(error) => return Err(FileError::io(error)),
            }
        }
        Err(keep_exhausted())
    })();
    let _ = dst.remove_dir_all(&temporary_name);
    result
}

#[cfg(all(test, windows))]
pub(super) fn copy_then_remove_entry_for_test(
    source: &Path,
    destination: &Path,
) -> Result<(), FileError> {
    let src = Dir::open_ambient_dir(
        source.parent().unwrap_or_else(|| Path::new(".")),
        cap_std::ambient_authority(),
    )
    .map_err(FileError::io)?;
    let dst = Dir::open_ambient_dir(
        destination.parent().unwrap_or_else(|| Path::new(".")),
        cap_std::ambient_authority(),
    )
    .map_err(FileError::io)?;
    let source = source
        .file_name()
        .ok_or_else(|| FileError::new(400, "file_path_invalid", "源路径缺少名称"))?;
    let destination = destination
        .file_name()
        .ok_or_else(|| FileError::new(400, "file_path_invalid", "目标路径缺少名称"))?;
    let copied = copy_entry(&src, source, &dst, destination)?;
    copied.remove(&src, source)
}

/// Move a recycle-bin entry using the same retained-parent operations as the
/// file manager. A complete copy is published before a cross-device source
/// is removed; a failed removal leaves that recoverable copy in place.
pub(crate) fn move_recycle_entry(source: &Path, destination: &Path) -> Result<(), FileError> {
    let parent = |path: &Path| -> Result<Dir, FileError> {
        Dir::open_ambient_dir(
            path.parent().unwrap_or_else(|| Path::new(".")),
            cap_std::ambient_authority(),
        )
        .map_err(FileError::io)
    };
    let name = |path: &Path| -> Result<String, FileError> {
        path.file_name()
            .and_then(OsStr::to_str)
            .map(str::to_owned)
            .ok_or_else(|| FileError::new(400, "file_path_encoding", "路径无法表示为 UTF-8"))
    };
    let src = parent(source)?;
    let dst = parent(destination)?;
    let source = name(source)?;
    let destination = name(destination)?;
    match rename_noreplace(&src, &source, &dst, &destination) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == ErrorKind::CrossesDevices => {}
        Err(error) => return Err(FileError::io(error)),
    }
    let (temporary_name, temporary) = fresh_private_child(&dst)?;
    let result = (|| {
        let copied = copy_entry(&src, source.as_ref(), &temporary, "data".as_ref())?;
        copied.verify(&src, source.as_ref())?;
        rename_noreplace(&temporary, "data", &dst, &destination).map_err(FileError::io)?;
        copied
            .remove_verified(&src, source.as_ref())
            .map_err(|error| {
                FileError::new(
                    error.status,
                    "file_move_source_cleanup",
                    format!("目标已完整发布，但无法移除源名称：{}", error.message),
                )
            })
    })();
    let _ = dst.remove_dir_all(&temporary_name);
    result
}

/// Atomic no-clobber rename on platforms with the corresponding syscall.
fn rename_noreplace(src: &Dir, source: &str, dst: &Dir, destination: &str) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
    {
        rustix::fs::renameat_with(
            src,
            source,
            dst,
            destination,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::from)
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        };
        use windows_sys::{
            Wdk::Storage::FileSystem::{
                FILE_RENAME_INFORMATION, FileRenameInformation, NtSetInformationFile,
            },
            Win32::{Foundation::RtlNtStatusToDosError, System::IO::IO_STATUS_BLOCK},
        };
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        options.access_mode(DELETE | FILE_READ_ATTRIBUTES);
        // Open the named link itself and allow directory handles. The capability
        // walk still checks its parent; the operation then uses this held handle.
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        let source = src.open_with(source, &options)?;
        let name: Vec<u16> = OsStr::new(destination).encode_wide().collect();
        let name_bytes = name
            .len()
            .checked_mul(2)
            .ok_or_else(|| io::Error::from(ErrorKind::InvalidInput))?;
        let bytes = std::mem::size_of::<FILE_RENAME_INFORMATION>()
            .checked_add(name_bytes)
            .ok_or_else(|| io::Error::from(ErrorKind::InvalidInput))?;
        let length = u32::try_from(bytes).map_err(|_| io::Error::from(ErrorKind::InvalidInput))?;
        // A usize allocation supplies FILE_RENAME_INFORMATION's pointer alignment,
        // including the variable-length UTF-16 tail. Zero means no replacement.
        let mut buffer = vec![
            0usize;
            bytes
                .max(std::mem::size_of::<FILE_RENAME_INFORMATION>())
                .div_ceil(std::mem::size_of::<usize>())
        ];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        // SAFETY: buffer is aligned, initialized and large enough for the header
        // and filename. Both directory/source handles remain alive for the call.
        // RootDirectory resolves only the validated single-component leaf name.
        // Use the native operation: the Win32 wrapper rejects this relative
        // directory-handle form with ERROR_INVALID_PARAMETER on Windows.
        // The source is opened synchronously (no FILE_FLAG_OVERLAPPED).
        let mut io_status = IO_STATUS_BLOCK::default();
        let status = unsafe {
            (*info).RootDirectory = dst.as_raw_handle();
            (*info).FileNameLength = name_bytes as u32;
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                name.len(),
            );
            NtSetInformationFile(
                source.as_raw_handle(),
                &mut io_status,
                info.cast(),
                length,
                FileRenameInformation,
            )
        };
        if status < 0 {
            // SAFETY: translating the returned status has no pointer arguments.
            Err(io::Error::from_raw_os_error(
                unsafe { RtlNtStatusToDosError(status) } as i32,
            ))
        } else {
            Ok(())
        }
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos", target_os = "ios")))]
    {
        src.rename(source, dst, destination)
    }
}

/// Rename/move of an existing ordinary entry without replacing anything.
fn relocate_entry(
    src_dir: &Dir,
    src_name: &str,
    dst_dir: &Dir,
    name: &str,
    conflict: Conflict,
) -> Result<Option<String>, FileError> {
    let before = src_dir.symlink_metadata(src_name).map_err(FileError::io)?;
    unshared(&before)?;
    let expected: Identity = identity(&before);
    for attempt in 0..MAX_KEEP_ATTEMPTS {
        let candidate = if attempt == 0 {
            name.to_owned()
        } else {
            numbered(name, attempt)
        };
        match dst_dir.symlink_metadata(&candidate) {
            Ok(_) => match conflict {
                Conflict::Error | Conflict::Replace => return Err(exists_error()),
                Conflict::Skip => return Ok(None),
                Conflict::Keep => continue,
            },
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(FileError::io(error)),
        }
        match rename_noreplace(src_dir, src_name, dst_dir, &candidate) {
            Ok(()) => {
                let moved = dst_dir
                    .symlink_metadata(&candidate)
                    .map_err(|_| FileError::changed())?;
                if identity(&moved) != expected {
                    return Err(FileError::changed());
                }
                return Ok(Some(candidate));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::AlreadyExists
                        | ErrorKind::DirectoryNotEmpty
                        | ErrorKind::NotADirectory
                        | ErrorKind::IsADirectory
                ) =>
            {
                return Err(exists_error());
            }
            Err(error) if error.kind() == ErrorKind::CrossesDevices => {
                return Err(FileError::new(
                    409,
                    "file_move_cross_device",
                    "需要跨文件系统复制后回收源项目",
                ));
            }
            Err(error) => return Err(FileError::io(error)),
        }
    }
    Err(keep_exhausted())
}

// ---- Bug-report attachments ----------------------------------------

/// Python `ATTACHMENT_MAX_BYTES`; the API derives its body cap from this value.
const BUG_REPORT_ATTACHMENT_MAX_BYTES: usize = 512 * 1024 * 1024;

/// Python `_attachment_name`: keep a readable file name that can neither
/// take part in path resolution nor exceed filesystem limits.
fn attachment_name(raw: &str) -> String {
    let base = raw
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim_matches([' ', '.'])
        .to_owned();
    let mut cleaned = String::new();
    let mut underscore = false;
    for ch in base.chars() {
        if ch.is_control() || ch == '/' || ch == '\\' {
            if !underscore {
                cleaned.push('_');
                underscore = true;
            }
        } else {
            underscore = false;
            cleaned.push(ch);
        }
    }
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = if collapsed.is_empty() || collapsed == "." || collapsed == ".." {
        "attachment".to_owned()
    } else {
        collapsed
    };
    let (stem, suffix) = match name.rfind('.') {
        Some(index) if index > 0 => {
            let suffix: String = name[index..].chars().take(20).collect();
            (name[..index].to_owned(), suffix)
        }
        _ => (name.clone(), String::new()),
    };
    let mut stem = stem;
    while stem.len() > 150 {
        stem.pop();
    }
    let stem = if stem.is_empty() {
        "attachment".to_owned()
    } else {
        stem
    };
    format!("{stem}{suffix}")
}

fn attachment_id_ok(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 9
        && !text.starts_with('0')
        && text.bytes().all(|b| b.is_ascii_digit())
}

/// Windows canonicalization adds a verbatim drive prefix. Keep the ordinary
/// drive spelling for configured-root comparison and returned attachment paths.
#[cfg(windows)]
fn attachment_directory_path(canonical: PathBuf) -> PathBuf {
    let mut components = canonical.components();
    if let Some(Component::Prefix(prefix)) = components.next()
        && let std::path::Prefix::VerbatimDisk(letter) = prefix.kind()
    {
        let mut plain = PathBuf::from(format!("{}:\\", char::from(letter)));
        for component in components {
            if let Component::Normal(name) = component {
                plain.push(name);
            }
        }
        return plain;
    }
    canonical
}

fn same_content(dir: &Dir, name: &str, bytes: &[u8]) -> bool {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No).nonblock(true);
    let Ok(mut file) = dir.open_with(name, &options) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if metadata.len() != bytes.len() as u64 {
        return false;
    }
    let mut existing = Vec::with_capacity(bytes.len());
    file.read_to_end(&mut existing).is_ok() && existing == bytes
}

impl WriteService {
    pub const BUG_REPORT_ATTACHMENT_MAX_BYTES: usize = BUG_REPORT_ATTACHMENT_MAX_BYTES;

    /// Conversation upload destination comes from the selected native view,
    /// never from a client-supplied cwd or file-browser navigation grant.
    pub fn session_attachment_upload(
        &self,
        scope: &FileScope<'_>,
        requested_id: Option<&str>,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<Value, FileError> {
        super::references::validate_scope(scope)?;
        let cwd = boundary::absolute_navigation(scope.cwd)?;
        self.guard(&cwd.join(crate::bug_report::ATTACHMENT_DIR))?;
        self.bug_report_upload(&cwd, requested_id, name, mime, bytes)
    }

    /// Python `_attachment_name`, exposed for the transport's tests.
    pub fn attachment_name(raw: &str) -> String {
        attachment_name(raw)
    }

    /// Python `_upload_attachment` for the `bug-report` scope: one raw body
    /// into `<repository>/sessiondock_attachments/<id>/<name>` where the
    /// repository lies inside a write root. The batch directory is the
    /// requested id or the next free number; the file keeps its name, reuses
    /// an identical existing file, or takes `stem__N.suffix`; nothing is ever
    /// overwritten. Returns Python's upload document.
    pub fn bug_report_upload(
        &self,
        repository: &Path,
        requested_id: Option<&str>,
        raw_name: &str,
        supplied_mime: &str,
        bytes: &[u8],
    ) -> Result<Value, FileError> {
        if bytes.is_empty() {
            return Err(FileError::new(
                400,
                "file_upload_empty",
                "附件为空或缺少 Content-Length",
            ));
        }
        if bytes.len() > BUG_REPORT_ATTACHMENT_MAX_BYTES {
            return Err(FileError::new(
                413,
                "file_upload_too_large",
                format!(
                    "单个附件不能超过 {} MB",
                    BUG_REPORT_ATTACHMENT_MAX_BYTES / (1024 * 1024)
                ),
            )
            .with_details(json!({"limit": BUG_REPORT_ATTACHMENT_MAX_BYTES})));
        }
        if let Some(id) = requested_id
            && !attachment_id_ok(id)
        {
            return Err(FileError::new(
                400,
                "file_attachment_id",
                "附件目录编号无效",
            ));
        }
        let repository = repository.canonicalize().map_err(FileError::io)?;
        let attachment_dir = crate::bug_report::ATTACHMENT_DIR;
        let attachment_path = repository.join(attachment_dir);
        let root = self.root_for(&attachment_path)?;
        // An operator may authorize only cwd/sessiondock_attachments, without
        // making the rest of a home/project directory writable.
        let (checked_repository, attachments) = if repository.starts_with(&root.path) {
            let checked = boundary::open_target(root.clone(), repository.clone())?;
            checked.verify_identity()?;
            let attachments =
                private_subdir(checked.directory()?, attachment_dir).map_err(|_| {
                    FileError::new(
                        409,
                        "file_attachment_dir",
                        format!("{attachment_dir} 不是安全目录"),
                    )
                })?;
            (checked, attachments)
        } else {
            let checked = boundary::open_target(root.clone(), attachment_path)?;
            checked.verify_identity()?;
            let attachments = checked.directory()?.clone();
            (checked, attachments)
        };
        let (attachment_id, batch) = match requested_id {
            Some(id) => (
                id.to_owned(),
                private_subdir(&attachments, id).map_err(|_| {
                    FileError::new(409, "file_attachment_dir", "附件编号对应的不是安全目录")
                })?,
            ),
            None => {
                let mut used = 0u64;
                for entry in attachments.entries().map_err(FileError::io)?.flatten() {
                    let name = entry.file_name();
                    if let Some(name) = name.to_str()
                        && attachment_id_ok(name)
                        && entry.file_type().is_ok_and(|kind| kind.is_dir())
                    {
                        used = used.max(name.parse().unwrap_or(0));
                    }
                }
                let mut allocated = None;
                for offset in 1..=32u64 {
                    let id = (used + offset).to_string();
                    #[allow(unused_mut)]
                    let mut builder = DirBuilder::new();
                    #[cfg(unix)]
                    cap_std::fs::DirBuilderExt::mode(&mut builder, 0o700);
                    match attachments.create_dir_with(&id, &builder) {
                        Ok(()) => {
                            allocated = Some((id.clone(), private_subdir(&attachments, &id)?));
                            break;
                        }
                        Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                        Err(error) => return Err(FileError::io(error)),
                    }
                }
                allocated.ok_or_else(|| {
                    FileError::new(503, "file_attachment_id", "无法分配附件目录编号")
                })?
            }
        };
        let checked_batch =
            boundary::open_target(root, repository.join(attachment_dir).join(&attachment_id))?;
        if identity(&batch.dir_metadata().map_err(FileError::io)?)
            != checked_batch.verify_identity()?
        {
            return Err(FileError::changed());
        }
        let original = attachment_name(raw_name);
        let stamp = format!(".{}-{}.upload", jobs::random_id()?, std::process::id());
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        cap_std::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let written = (|| -> Result<(), FileError> {
            let mut temp = batch.open_with(&stamp, &options).map_err(FileError::io)?;
            temp.write_all(bytes).map_err(FileError::io)?;
            temp.sync_data().map_err(FileError::io)
        })();
        if let Err(error) = written {
            let _ = batch.remove_file(&stamp);
            return Err(error);
        }
        let (stem, suffix) = match original.rfind('.') {
            Some(index) if index > 0 => {
                (original[..index].to_owned(), original[index..].to_owned())
            }
            _ => (original.clone(), String::new()),
        };
        let mut target = None;
        let mut reused = false;
        for number in 0..10_000u32 {
            if let Err(error) = checked_repository
                .verify_identity()
                .and_then(|_| checked_batch.verify_identity())
            {
                let _ = batch.remove_file(&stamp);
                return Err(error);
            }
            let candidate = if number == 0 {
                original.clone()
            } else {
                format!("{stem}__{number}{suffix}")
            };
            match batch.symlink_metadata(&candidate) {
                Ok(metadata) if metadata.is_symlink() => continue,
                Ok(metadata) if metadata.is_file() && same_content(&batch, &candidate, bytes) => {
                    target = Some(candidate);
                    reused = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => {}
            }
            match batch.hard_link(&stamp, &batch, &candidate) {
                Ok(()) => {
                    target = Some(candidate);
                    break;
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) if hard_links_unavailable(&error) => {
                    // Same O_EXCL publish as the job path on link-less filesystems.
                    let mut create = OpenOptions::new();
                    create
                        .write(true)
                        .create_new(true)
                        .follow(FollowSymlinks::No);
                    #[cfg(unix)]
                    cap_std::fs::OpenOptionsExt::mode(&mut create, 0o600);
                    match batch.open_with(&candidate, &create) {
                        Ok(mut file) => {
                            if let Err(error) =
                                file.write_all(bytes).and_then(|()| file.sync_data())
                            {
                                let _ = batch.remove_file(&candidate);
                                let _ = batch.remove_file(&stamp);
                                return Err(FileError::io(error));
                            }
                            target = Some(candidate);
                            break;
                        }
                        Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                        Err(error) => {
                            let _ = batch.remove_file(&stamp);
                            return Err(FileError::io(error));
                        }
                    }
                }
                Err(error) => {
                    let _ = batch.remove_file(&stamp);
                    return Err(FileError::io(error));
                }
            }
        }
        let _ = batch.remove_file(&stamp);
        let name = target.ok_or_else(|| {
            FileError::new(409, "file_keep_exhausted", "同名附件过多，无法分配文件名")
        })?;
        let guessed = mime_guess::from_path(&original).first_raw();
        let supplied = supplied_mime
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let supplied_ok = supplied.split_once('/').is_some_and(|(kind, sub)| {
            let ok = |text: &str| {
                !text.is_empty()
                    && text
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
            };
            ok(kind) && ok(sub)
        });
        let mime = guessed
            .map(str::to_owned)
            .or_else(|| supplied_ok.then_some(supplied))
            .unwrap_or_else(|| "application/octet-stream".into());
        let kind = match mime.split('/').next().unwrap_or("") {
            kind @ ("image" | "video" | "audio") => kind,
            _ => "file",
        };
        let path = repository
            .join(attachment_dir)
            .join(&attachment_id)
            .join(&name);
        #[cfg(windows)]
        let path = attachment_directory_path(path);
        let relative = Path::new(attachment_dir).join(&attachment_id).join(&name);
        Ok(json!({
            "ok": true, "name": name, "original_name": original, "path": path,
            "relative_path": relative, "attachment_id": attachment_id, "mime": mime,
            "kind": kind, "size": bytes.len(), "reused": reused, "media": Value::Null,
            "path_style": if cfg!(windows) { "windows" } else { "posix" },
        }))
    }
}

#[cfg(all(test, windows))]
mod windows_rename_tests {
    use super::*;
    #[test]
    fn kernel_no_replace_rejects_existing_files_and_directories() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let destination = temporary.path().join("destination");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(source.join("entry"), b"new bytes").unwrap();
        std::fs::write(destination.join("entry"), b"existing bytes").unwrap();
        let src = Dir::open_ambient_dir(&source, cap_std::ambient_authority()).unwrap();
        let dst = Dir::open_ambient_dir(&destination, cap_std::ambient_authority()).unwrap();
        // Call the primitive directly: no preliminary existence check can hide
        // accidental REPLACE_EXISTING behavior in the platform implementation.
        assert_eq!(
            rename_noreplace(&src, "entry", &dst, "entry")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists,
        );
        assert_eq!(std::fs::read(source.join("entry")).unwrap(), b"new bytes");
        assert_eq!(
            std::fs::read(destination.join("entry")).unwrap(),
            b"existing bytes"
        );
        rename_noreplace(&src, "entry", &dst, "moved").unwrap();
        assert!(!source.join("entry").exists());
        assert_eq!(
            std::fs::read(destination.join("moved")).unwrap(),
            b"new bytes"
        );
        std::fs::create_dir(source.join("folder")).unwrap();
        std::fs::create_dir(destination.join("folder")).unwrap();
        assert_eq!(
            rename_noreplace(&src, "folder", &dst, "folder")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists,
        );
        assert!(source.join("folder").is_dir() && destination.join("folder").is_dir());
        rename_noreplace(&src, "folder", &dst, "moved-folder").unwrap();
        assert!(destination.join("moved-folder").is_dir() && !source.join("folder").exists());
        // Exercise the shortest tail as well as multi-byte / surrogate UTF-16 names.
        for leaf in ["x", "中", "🦀"] {
            std::fs::write(source.join("short-source"), b"short-name bytes").unwrap();
            rename_noreplace(&src, "short-source", &dst, leaf).unwrap();
            assert_eq!(
                std::fs::read(destination.join(leaf)).unwrap(),
                b"short-name bytes"
            );
        }
    }
}
