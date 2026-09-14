//! Per-entry manifest: the only record of where trashed files came from.
//!
//! A manifest is written before the first rename (`moving`), rewritten once
//! every named file sits inside the entry (`trashed`), and marked `partial` if
//! a rollback after a mid-batch failure could not return every file. Restore
//! uses the recorded file locations, including recoverable partial entries.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::{TrashError, entry_id_is_valid};

pub const MANIFEST_VERSION: u32 = 1;
pub const MANIFEST_NAME: &str = "manifest.json";
pub const MANIFEST_TEMP: &str = "manifest.json.tmp";
pub const FILES_DIR: &str = "files";

/// Size, modification time and file identity captured for manifest reporting.
/// The recorded origin determines restoration; later edits do not revoke it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub size: u64,
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
    pub identity: String,
}

impl Stamp {
    /// Capture the named entry itself so a moved link remains a link.
    pub fn capture(path: &Path) -> Result<Self, TrashError> {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                TrashError::new(
                    409,
                    "changed_since_inventory",
                    "会话文件已不在索引记录的位置",
                )
            } else {
                TrashError::new(503, "stat_failed", "会话文件暂时不可检查")
            }
        })?;
        if !metadata.file_type().is_symlink() && !metadata.is_file() {
            return Err(TrashError::new(
                403,
                "not_regular_file",
                "索引命名的路径不是文件",
            ));
        }
        Ok(Self::from_metadata(&metadata))
    }

    /// Capture a whole Grok session directory or its named link for reporting.
    pub fn capture_directory(path: &Path) -> Result<Self, TrashError> {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                TrashError::new(
                    409,
                    "changed_since_inventory",
                    "会话目录已不在索引记录的位置",
                )
            } else {
                TrashError::new(503, "stat_failed", "会话目录暂时不可检查")
            }
        })?;
        if !metadata.file_type().is_symlink() && !metadata.is_dir() {
            return Err(TrashError::new(
                403,
                "not_directory",
                "索引命名的路径不是目录",
            ));
        }
        let mut stamp = Self::from_metadata(&metadata);
        stamp.size = directory_bytes(path);
        Ok(stamp)
    }

    pub fn from_metadata(metadata: &fs::Metadata) -> Self {
        let modified = metadata
            .modified()
            .unwrap_or(UNIX_EPOCH)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            format!("{}:{}", metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let identity = format!("{:?}:{}", metadata.created().ok(), metadata.len());
        Self {
            size: metadata.len(),
            mtime_secs: modified.as_secs(),
            mtime_nanos: modified.subsec_nanos(),
            identity,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    Moving,
    Trashed,
    Partial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    /// The session transcript (Claude/Codex JSONL; older entries also Grok
    /// `chat_history.jsonl`).
    Data,
    /// Grok `summary.json` (older entries).
    Summary,
    /// A subagent transcript owned by the session.
    Agent,
    /// A Claude subagent `.meta.json` sidecar.
    AgentMeta,
    /// A whole Grok session directory (Python moves the directory).
    Directory,
}

/// Sum regular-file bytes for recycle-bin reporting without traversal quotas.
pub fn directory_bytes(path: &Path) -> u64 {
    let mut pending = vec![path.to_path_buf()];
    let mut total = 0u64;
    while let Some(directory) = pending.pop() {
        let Ok(children) = fs::read_dir(directory) else {
            continue;
        };
        for child in children.flatten() {
            let Ok(kind) = child.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(child.path());
            } else if kind.is_file()
                && let Ok(metadata) = child.metadata()
            {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    /// File name inside the entry's `files/` directory.
    pub name: String,
    /// Absolute original path, exactly as the published index row named it.
    pub origin: PathBuf,
    pub role: FileRole,
    pub stamp: Stamp,
    /// True once the file has been renamed into the entry.
    pub in_trash: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStateNote {
    pub state: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub entry_id: String,
    pub uid: String,
    pub source: String,
    pub sid: String,
    pub title: String,
    pub cwd: String,
    pub created: String,
    pub updated: String,
    /// The session-level path from the inventory row: a file for Claude and
    /// Codex, the session directory for Grok.
    pub origin: PathBuf,
    /// Configured native root the origin belonged to.
    pub root: PathBuf,
    pub deleted_at: String,
    pub deleted_at_unix: u64,
    pub state: EntryState,
    pub forced: bool,
    pub run_state: RunStateNote,
    /// Sum of the named files' sizes at inventory time.
    pub bytes: u64,
    pub files: Vec<FileRecord>,
}

impl Manifest {
    pub fn kind(&self) -> &'static str {
        if self.source == "grok" { "dir" } else { "file" }
    }

    pub fn path(entry_dir: &Path) -> PathBuf {
        entry_dir.join(MANIFEST_NAME)
    }

    /// Atomic replace: write a private temp file, then rename over the name.
    pub fn write(&self, entry_dir: &Path) -> Result<(), TrashError> {
        let temp = entry_dir.join(MANIFEST_TEMP);
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|_| TrashError::new(500, "manifest_encoding", "回收站清单无法编码"))?;
        {
            let mut options = fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&temp)
                .map_err(|_| TrashError::new(500, "manifest_write", "回收站清单无法写入"))?;
            use std::io::Write;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| TrashError::new(500, "manifest_write", "回收站清单无法写入"))?;
        }
        fs::rename(&temp, Self::path(entry_dir))
            .map_err(|_| TrashError::new(500, "manifest_write", "回收站清单无法替换"))
    }

    pub fn read(entry_dir: &Path) -> Result<Self, TrashError> {
        let path = Self::path(entry_dir);
        let metadata = fs::metadata(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                TrashError::new(404, "manifest_missing", "回收站条目缺少清单")
            } else {
                TrashError::new(503, "manifest_unreadable", "回收站清单暂时不可读取")
            }
        })?;
        if !metadata.is_file() {
            return Err(TrashError::new(
                409,
                "manifest_invalid",
                "回收站清单不是普通文件",
            ));
        }
        let bytes = fs::read(&path)
            .map_err(|_| TrashError::new(503, "manifest_unreadable", "回收站清单暂时不可读取"))?;
        let manifest: Self = serde_json::from_slice(&bytes)
            .map_err(|_| TrashError::new(409, "manifest_invalid", "回收站清单损坏"))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), TrashError> {
        let invalid = |message: &str| TrashError::new(409, "manifest_invalid", message);
        if self.version != MANIFEST_VERSION {
            return Err(invalid("回收站清单版本不受支持"));
        }
        if !entry_id_is_valid(&self.entry_id) {
            return Err(invalid("回收站清单条目 ID 无效"));
        }
        if !matches!(self.source.as_str(), "claude" | "codex" | "grok") {
            return Err(invalid("回收站清单来源无效"));
        }
        if self.uid.is_empty() || !self.uid.starts_with(&format!("{}:", self.source)) {
            return Err(invalid("回收站清单 UID 无效"));
        }
        if self.files.is_empty() {
            return Err(invalid("回收站清单文件数量无效"));
        }
        let mut names = std::collections::BTreeSet::new();
        for file in &self.files {
            if !super::file_name_is_valid(&file.name) || !names.insert(&file.name) {
                return Err(invalid("回收站清单文件名无效或重复"));
            }
        }
        Ok(())
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn rfc3339(unix: u64) -> String {
    chrono::DateTime::from_timestamp(i64::try_from(unix).unwrap_or(0), 0)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
