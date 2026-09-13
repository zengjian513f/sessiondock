//! Deletion planning from one published list snapshot.
//!
//! Every path comes from the published session rows (which the lazy index
//! derived from its own directory walk), never from the request. The fork
//! protection set is computed once from the same rows so that a batch cannot
//! delete a child first and then its now "childless" parent.

use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{
    TrashError,
    manifest::{FileRole, Stamp},
};
use crate::sessions::SessionRoots;

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or("")
}

/// UIDs of rows that some other row of the same source names as its fork
/// parent. A row with a Codex `history_base` object asserts a fixed-prefix
/// dependency on `thread_id` (or `forked_from_id` when the object has no
/// thread) whether or not the child currently renders; a row without one
/// asserts `forked_from_id` only while it is supported, so unsupported orphan
/// subagent rows (whose `forked_from_id` is ownership, not history) cannot
/// pin each other. This matches the rows the UI marks as fork parents, plus
/// every fixed-prefix parent the Rust read model actually depends on.
pub fn protection_set(rows: &[Value]) -> BTreeSet<String> {
    let mut edges: BTreeSet<(String, String)> = BTreeSet::new();
    for row in rows {
        let source = text(row, "source");
        let base = &row["history_base"];
        let parent = if base.is_object() {
            let declared = text(base, "thread_id");
            if declared.is_empty() {
                text(row, "forked_from_id")
            } else {
                declared
            }
        } else if row["supported"] != false {
            text(row, "forked_from_id")
        } else {
            ""
        };
        if !parent.is_empty() && !source.is_empty() {
            edges.insert((source.to_owned(), parent.to_owned()));
        }
    }
    rows.iter()
        .filter(|row| {
            let sid = text(row, "sid");
            !sid.is_empty() && edges.contains(&(text(row, "source").to_owned(), sid.to_owned()))
        })
        .filter_map(|row| row["uid"].as_str().filter(|uid| !uid.is_empty()))
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedFile {
    pub origin: PathBuf,
    pub role: FileRole,
    pub stamp: Stamp,
}

/// Everything a delete needs, captured from one row before any rename.
#[derive(Clone, Debug)]
pub struct Plan {
    pub uid: String,
    pub source: String,
    pub sid: String,
    pub title: String,
    pub cwd: String,
    pub created: String,
    pub updated: String,
    pub origin: PathBuf,
    pub root: PathBuf,
    pub files: Vec<PlannedFile>,
}

impl Plan {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|file| file.stamp.size).sum()
    }

    /// Derive the file set for one published row. Paths are accepted only when
    /// they stay inside the configured root for the row's source, with no
    /// symlink anywhere on the way (checked component by component, so an
    /// aliased directory cannot redirect a rename either).
    pub fn derive(row: &Value, roots: &SessionRoots) -> Result<Self, TrashError> {
        let uid = text(row, "uid").to_owned();
        let source = text(row, "source").to_owned();
        let root = match source.as_str() {
            "claude" => roots.claude.as_ref(),
            "codex" => roots.codex.as_ref(),
            "grok" => roots.grok.as_ref(),
            _ => None,
        }
        .ok_or_else(|| {
            TrashError::new(409, "root_unconfigured", "该会话来源没有配置的数据源目录")
        })?;
        let origin = PathBuf::from(text(row, "path"));
        if origin.as_os_str().is_empty() {
            return Err(TrashError::new(
                409,
                "path_missing",
                "索引行没有记录原生路径",
            ));
        }
        let mut files = Vec::new();
        let mut push = |path: PathBuf, role: FileRole| -> Result<(), TrashError> {
            if files.len() >= super::FILES_PER_ENTRY_LIMIT {
                return Err(TrashError::new(
                    413,
                    "too_many_files",
                    "会话文件数量超过回收站单条目限制",
                ));
            }
            trusted_within(root, &path)?;
            let stamp = if role == FileRole::Directory {
                Stamp::capture_directory(&path)?
            } else {
                Stamp::capture(&path)?
            };
            files.push(PlannedFile {
                origin: path,
                role,
                stamp,
            });
            Ok(())
        };
        match source.as_str() {
            "claude" | "codex" => {
                push(origin.clone(), FileRole::Data)?;
                for item in row["agent_items"].as_array().into_iter().flatten() {
                    let path = PathBuf::from(text(item, "path"));
                    if path.as_os_str().is_empty() {
                        return Err(TrashError::new(
                            409,
                            "path_missing",
                            "子代理索引项没有记录原生路径",
                        ));
                    }
                    push(path.clone(), FileRole::Agent)?;
                    if source == "claude" {
                        // Same rule as the inventory: an `agent-<id>.meta.json`
                        // sidecar next to the transcript belongs to it.
                        let sidecar = path.with_extension("meta.json");
                        match fs::symlink_metadata(&sidecar) {
                            Ok(_) => push(sidecar, FileRole::AgentMeta)?,
                            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                            Err(_) => {
                                return Err(TrashError::new(
                                    503,
                                    "stat_failed",
                                    "子代理元数据暂时不可检查",
                                ));
                            }
                        }
                    }
                }
            }
            "grok" => {
                // WP-E, Python parity: the whole session directory moves
                // (events/updates/prompt_context/... included), not only the
                // two files the index reads. The summary must still be there:
                // it is what identifies the directory as this session.
                Stamp::capture(&origin.join("summary.json"))?;
                push(origin.clone(), FileRole::Directory)?;
            }
            _ => return Err(TrashError::new(409, "unknown_source", "未知的会话来源")),
        }
        Ok(Self {
            uid,
            sid: text(row, "sid").to_owned(),
            title: text(row, "title").to_owned(),
            cwd: text(row, "cwd").to_owned(),
            created: text(row, "created").to_owned(),
            updated: text(row, "updated").to_owned(),
            origin,
            root: root.clone(),
            source,
            files,
        })
    }
}

/// `path` must be absolute, lexically inside `root` (no `..`), and every
/// existing component from the root down must be a non-symlink directory
/// (the final component may be absent or a regular file).
pub fn trusted_within(root: &Path, path: &Path) -> Result<(), TrashError> {
    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || !path.starts_with(root)
        || path == root
    {
        return Err(TrashError::new(
            403,
            "path_outside_root",
            "路径不在配置的数据源目录内",
        ));
    }
    for ancestor in path.ancestors() {
        if !ancestor.starts_with(root) {
            break;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(TrashError::new(
                    403,
                    "symlink_rejected",
                    "路径经过符号链接，拒绝跟随",
                ));
            }
            Ok(metadata) if ancestor != path && !metadata.is_dir() => {
                return Err(TrashError::new(
                    403,
                    "path_outside_root",
                    "路径的父级不是目录",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound && ancestor == path => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // A missing parent is acceptable only for restore, which
                // recreates it; the caller re-walks after creation.
            }
            Err(_) => return Err(TrashError::new(503, "stat_failed", "无法确认路径的父目录")),
        }
    }
    Ok(())
}
