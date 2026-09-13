//! Session-reference-scoped file reads and authenticated directory browsing.
//! Configured roots enable the service; grants permit operator navigation up
//! to the OS volume root. Transport validates each selected session first and
//! runs blocking filesystem work off the async reactor. See `docs/files.md`.

mod boundary;
mod grants;
mod info;
mod jobs;
mod media;
mod references;
mod response;
mod write;

use serde_json::{Value, json};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

pub(crate) use boundary::FileVersion;
pub use boundary::ResolvedTarget;
pub(crate) use media::ScopedFiles;
pub use references::clean_ref;
pub(crate) use references::normalize_media_ref;
pub(crate) use response::CheckedImage;
pub use response::{CheckedReader, FileBody, FileResponse, ReadOptions};
pub(crate) use write::move_recycle_entry;
pub use write::{
    ActionRequest, Conflict, DEFAULT_UPLOAD_CHUNK_BYTES, FILE_TRASH_DIR, MAX_WRITE_ITEMS, Outcome,
    ScopeKey, UPLOAD_DIR, WriteLimits, WriteService, validate_name,
};

pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_PREVIEW_BYTES: usize = 1024 * 1024;
pub const MAX_RAW_BYTES: u64 = 32 * 1024 * 1024;
pub const STREAM_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct FileError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    /// Explicit limits/expectations (`limit`, `expected`, …) merged into the
    /// JSON error body by the write transport; `Null` for ordinary errors.
    pub details: Value,
}
impl FileError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: Value::Null,
        }
    }
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }
    pub fn unsupported(mode: &str) -> Self {
        let _ = mode; // Do not reflect untrusted mode text into diagnostics.
        Self::new(501, "file_mode_not_implemented", "文件服务尚未实现此模式")
    }
    pub(super) fn changed() -> Self {
        Self::new(
            409,
            "file_changed",
            "文件或父目录在读取期间已变化，请刷新后重试",
        )
    }
    pub(super) fn io(error: std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::NotFound => Self::new(404, "file_not_found", "文件或目录不存在"),
            std::io::ErrorKind::PermissionDenied => {
                Self::new(403, "file_forbidden", "操作系统不允许访问此文件或目录")
            }
            _ => Self::new(503, "file_io", "文件访问失败；未忽略错误或返回空内容"),
        }
    }
}
impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for FileError {}

/// Values must come from the complete, selected semantic SessionStore view.
/// Neither cwd nor a raw client UID establishes any filesystem authority.
pub struct FileScope<'a> {
    pub uid: &'a str,
    pub agent: Option<&'a str>,
    pub cwd: &'a str,
    pub messages: &'a [Value],
}

#[derive(Debug, Clone)]
pub struct ListOptions {
    pub offset: usize,
    pub limit: usize,
    pub sort: String,
    pub order: String,
    pub hidden: bool,
}
impl Default for ListOptions {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 500,
            sort: "name".into(),
            order: "asc".into(),
            hidden: true,
        }
    }
}

pub struct FileService {
    grants: grants::Grants,
    /// Media reference indexes keyed by view revision (`files/media.rs`).
    media_indexes: std::sync::Mutex<Vec<(String, std::sync::Arc<references::ReferenceIndex>)>>,
}
impl FileService {
    pub fn open(_roots: Vec<PathBuf>) -> Result<Self, FileError> {
        Ok(Self {
            grants: grants::Grants::default(),
            media_indexes: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn candidate(&self, scope: &FileScope<'_>, raw: &str) -> Result<ResolvedTarget, FileError> {
        let path = boundary::absolute_path(raw, scope.cwd)?;
        boundary::open_target(boundary::volume_root(&path)?, path)
    }

    /// Resolve navigation only after transport has validated the session and
    /// its durable directory grant. Cwd or a client path alone is no grant.
    pub fn navigation(&self, raw: &str) -> Result<ResolvedTarget, FileError> {
        let path = boundary::absolute_navigation(raw)?;
        boundary::open_target(boundary::volume_root(&path)?, path)
    }

    fn probe(
        &self,
        scope: &FileScope<'_>,
        raw: &str,
        probes: &Cell<usize>,
    ) -> Result<ResolvedTarget, FileError> {
        probes.set(probes.get() + 1);
        self.candidate(scope, raw)
    }
    fn directories(
        &self,
        scope: &FileScope<'_>,
        index: &references::ReferenceIndex,
        needed: bool,
        probes: &Cell<usize>,
    ) -> Result<Vec<PathBuf>, FileError> {
        let mut directories = BTreeSet::new();
        if needed {
            for raw in &index.directories {
                match self.probe(scope, raw, probes) {
                    Ok(target) if target.kind() == "directory" => {
                        directories.insert(target.path().to_path_buf());
                    }
                    Err(error) if error.status == 413 => return Err(error),
                    _ => {}
                }
            }
        }
        Ok(directories.into_iter().collect())
    }

    fn resolve(
        &self,
        scope: &FileScope<'_>,
        index: &references::ReferenceIndex,
        raw: &str,
        directories: &[PathBuf],
        probes: &Cell<usize>,
    ) -> Result<ResolvedTarget, FileError> {
        let reference = clean_ref(raw)?;
        self.resolve_reference(scope, index, &reference, directories, probes)
    }

    fn resolve_reference(
        &self,
        scope: &FileScope<'_>,
        index: &references::ReferenceIndex,
        reference: &str,
        directories: &[PathBuf],
        probes: &Cell<usize>,
    ) -> Result<ResolvedTarget, FileError> {
        if !index.refs.contains(reference) {
            return Err(FileError::new(
                404,
                "file_not_referenced",
                "该路径未出现在所选会话分支中",
            ));
        }
        let basename = !reference.contains(['/', '\\']);
        if !basename {
            return self.probe(scope, reference, probes);
        }
        let mut candidates: BTreeMap<PathBuf, ResolvedTarget> = BTreeMap::new();
        // Basenames require all branch references: never guess the first match.
        for other in index.basenames.get(reference).into_iter().flatten() {
            if let Ok(target) = self.probe(scope, other, probes) {
                candidates.insert(target.path().to_path_buf(), target);
                if candidates.len() > 1 {
                    return Err(FileError::new(
                        409,
                        "file_ambiguous",
                        "会话中有多个同名文件，请点击完整路径",
                    ));
                }
            }
        }
        for directory in directories {
            let child = directory.join(reference);
            if let Some(child) = child.to_str()
                && let Ok(target) = self.probe(scope, child, probes)
            {
                candidates.insert(target.path().to_path_buf(), target);
                if candidates.len() > 1 {
                    return Err(FileError::new(
                        409,
                        "file_ambiguous",
                        "会话中有多个同名文件，请点击完整路径",
                    ));
                }
            }
        }
        candidates.into_values().next().ok_or_else(|| {
            FileError::new(404, "file_not_found", "文件不存在，或会话未记录其完整路径")
        })
    }

    /// Direct file references are resolved against the current selected view.
    pub fn target(
        &self,
        scope: &FileScope<'_>,
        reference: &str,
        navigation: Option<&str>,
    ) -> Result<ResolvedTarget, FileError> {
        references::validate_scope(scope)?;
        let index = references::ReferenceIndex::new(scope.messages)?;
        let probes = Cell::new(0);
        let directories = self.directories(
            scope,
            &index,
            !clean_ref(reference)?.contains(['/', '\\']),
            &probes,
        )?;
        let anchor = self.resolve(scope, &index, reference, &directories, &probes)?;
        let Some(raw) = navigation.filter(|value| !value.is_empty()) else {
            return Ok(anchor);
        };
        if anchor.kind() != "directory" {
            return Err(FileError::new(
                400,
                "file_directory_anchor_required",
                "文件浏览入口必须是会话提及的目录",
            ));
        }
        anchor.verify()?;
        self.navigation(raw)
    }

    pub fn resolve_many(
        &self,
        scope: &FileScope<'_>,
        requested: &[String],
    ) -> Result<Value, FileError> {
        references::validate_scope(scope)?;
        if requested.len() > 256 {
            return Err(FileError::new(
                400,
                "file_reference_limit",
                "一次最多解析 256 个文件引用",
            ));
        }
        for reference in requested {
            clean_ref(reference)?;
        }
        if requested.is_empty() {
            return Ok(
                json!({"resolved":{},"targets":[],"file_browser":true,"errors":[],"incomplete":false,"writable":false}),
            );
        }
        let index = references::ReferenceIndex::new(scope.messages)?;
        let probes = Cell::new(0);
        let directories = self.directories(
            scope,
            &index,
            requested
                .iter()
                .any(|reference| !reference.contains(['/', '\\'])),
            &probes,
        )?;
        let mut resolved = serde_json::Map::new();
        let mut targets = Vec::new();
        let mut errors = Vec::new();
        let mut seen = BTreeSet::new();
        for reference in requested {
            if !seen.insert(reference) {
                continue;
            }
            match self.resolve(scope, &index, reference, &directories, &probes) {
                Ok(target) => {
                    let path = boundary::wire_path(target.path())?;
                    resolved.insert(reference.clone(), json!(path));
                    targets.push(json!({"ref":reference,"path":path,"kind":target.kind()}));
                }
                Err(error) if error.status == 413 => return Err(error),
                Err(error) => errors.push(json!({"ref":reference,"status":error.status,"code":error.code,"error":error.message})),
            }
        }
        Ok(
            json!({"resolved":resolved,"targets":targets,"file_browser":true,"errors":errors,"incomplete":!errors.is_empty(),"writable":false}),
        )
    }

    pub fn list(&self, target: &ResolvedTarget, options: &ListOptions) -> Result<Value, FileError> {
        boundary::list(target, options)
    }
    pub fn describe(&self, target: &ResolvedTarget) -> Result<Value, FileError> {
        response::describe(target)
    }
    pub fn read(
        &self,
        target: ResolvedTarget,
        options: &ReadOptions,
    ) -> Result<FileResponse, FileError> {
        response::read(target, options)
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod write_tests;
