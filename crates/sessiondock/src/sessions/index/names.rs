//! Codex `session_index.jsonl` names applied to summary rows (batch 34).
//!
//! Same explicit, bounded contract as `sessions::names` (docs/codex-names.md):
//! an explicitly configured absolute file, opened without following symlinks,
//! 4 MiB / 50 000 lines / 10 000 ids, `id` + `thread_name` truthy, titles
//! clipped to 110 characters, `renamed_at`/`renamed_to` only when the entry
//! carries an `updated_at`. The snapshot is reused while the file's stamp is
//! unchanged, so a warm list refresh costs one `stat` here.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use cap_fs_ext::{DirExt, FollowSymlinks, MetadataExt, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, Metadata, OpenOptions};
use serde_json::{Value, json};

use super::summary::{clip, norm_ts, truthy};
use super::{CandidateRef, graph};
use crate::sessions::SessionError;

const BYTE_LIMIT: u64 = 4 * 1024 * 1024;
const LINE_LIMIT: usize = 50_000;
const NAME_LIMIT: usize = 10_000;
const ID_LIMIT: usize = 256;
const TITLE_LIMIT: usize = 16 * 1024;

fn error(message: &str) -> SessionError {
    SessionError::new(503, format!("Codex 名称索引：{message}"))
}

fn io_error(io: std::io::Error) -> SessionError {
    match io.kind() {
        std::io::ErrorKind::PermissionDenied => error("文件或目录不可读"),
        std::io::ErrorKind::NotFound => error("显式配置的文件或目录不存在"),
        _ => error("文件访问失败，请检查显式配置；没有回退到旧名称"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    identity: (u64, u64),
    size: u64,
    modified: Option<cap_std::time::SystemTime>,
}

fn stamp(meta: &Metadata) -> Stamp {
    Stamp {
        identity: (meta.dev(), meta.ino()),
        size: meta.len(),
        modified: meta.modified().ok(),
    }
}

fn ordinary(meta: &Metadata, directory: bool) -> Result<(), SessionError> {
    if meta.is_symlink() || (directory && !meta.is_dir()) || (!directory && !meta.is_file()) {
        return Err(error(
            "只接受普通文件和目录，不跟随符号链接、重解析点或特殊文件",
        ));
    }
    if !directory && meta.nlink() != 1 {
        return Err(error("不接受具有多个硬链接的名称索引"));
    }
    if !directory && meta.len() > BYTE_LIMIT {
        return Err(SessionError::new(413, "Codex 名称索引超过 4 MiB 字节预算"));
    }
    Ok(())
}

struct Opened {
    file: cap_std::fs::File,
    stamp: Stamp,
}

impl Opened {
    /// Walk every component of the configured path without following links.
    fn new(path: &Path) -> Result<Self, SessionError> {
        if !path.is_absolute()
            || path
                .to_str()
                .is_none_or(|s| s.len() > 4096 || s.contains('\0'))
        {
            return Err(SessionError::new(
                400,
                "Codex 名称索引需要显式、标准的绝对 UTF-8 文件路径",
            ));
        }
        let mut base = PathBuf::new();
        let mut parts = Vec::new();
        for part in path.components() {
            match part {
                Component::Prefix(_) | Component::RootDir => base.push(part.as_os_str()),
                Component::Normal(name) => parts.push(name.to_owned()),
                _ => return Err(SessionError::new(400, "Codex 名称索引路径不能包含相对跳转")),
            }
        }
        let name = parts.pop().ok_or_else(|| error("配置必须指向文件"))?;
        let mut parent = Dir::open_ambient_dir(base, ambient_authority()).map_err(io_error)?;
        for name in parts {
            let before = parent.symlink_metadata(&name).map_err(io_error)?;
            ordinary(&before, true)?;
            parent = parent.open_dir_nofollow(&name).map_err(io_error)?;
        }
        let before = parent.symlink_metadata(&name).map_err(io_error)?;
        ordinary(&before, false)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let file = parent.open_with(&name, &options).map_err(io_error)?;
        let after = file.metadata().map_err(io_error)?;
        ordinary(&after, false)?;
        if stamp(&before) != stamp(&after) {
            return Err(error("读取期间文件或父目录身份改变，请重试"));
        }
        Ok(Self {
            file,
            stamp: stamp(&after),
        })
    }
}

#[derive(Debug)]
struct Name {
    title: String,
    updated: Value,
}

pub struct NameIndex {
    stamp: Stamp,
    names: BTreeMap<String, Name>,
}

/// Whether the file's stamp differs from the loaded snapshot (one `stat`).
pub fn changed(path: &Path, previous: Option<&NameIndex>) -> Result<bool, SessionError> {
    let opened = Opened::new(path)?;
    Ok(previous.is_none_or(|old| old.stamp != opened.stamp))
}

pub fn load(
    path: &Path,
    previous: Option<&Arc<NameIndex>>,
) -> Result<Arc<NameIndex>, SessionError> {
    let mut opened = Opened::new(path)?;
    if let Some(old) = previous.filter(|old| old.stamp == opened.stamp) {
        return Ok(old.clone());
    }
    let mut bytes = Vec::with_capacity(opened.stamp.size as usize);
    (&mut opened.file)
        .take(BYTE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > BYTE_LIMIT {
        return Err(SessionError::new(413, "Codex 名称索引超过 4 MiB 字节预算"));
    }
    if bytes.len() as u64 != opened.stamp.size {
        return Err(error("读取期间文件或父目录身份改变，请重试"));
    }
    let names = parse(&bytes)?;
    let after = opened.file.metadata().map_err(io_error)?;
    if stamp(&after) != opened.stamp {
        return Err(error("读取期间文件或父目录身份改变，请重试"));
    }
    Ok(Arc::new(NameIndex {
        stamp: opened.stamp,
        names,
    }))
}

fn parse(bytes: &[u8]) -> Result<BTreeMap<String, Name>, SessionError> {
    let mut names = BTreeMap::new();
    for (index, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
        if index >= LINE_LIMIT {
            return Err(SessionError::new(413, "Codex 名称索引超过 50000 行预算"));
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let row: Value =
            serde_json::from_slice(line).map_err(|_| error("包含无效或未完成的 JSON 行"))?;
        if !row.is_object() {
            return Err(error("名称索引每一行必须是 JSON 对象"));
        }
        if !truthy(&row["id"]) || !truthy(&row["thread_name"]) {
            continue;
        }
        let sid = row["id"].as_str().ok_or_else(|| error("id 必须是字符串"))?;
        let title = row["thread_name"]
            .as_str()
            .ok_or_else(|| error("thread_name 必须是字符串"))?;
        if sid.len() > ID_LIMIT || title.len() > TITLE_LIMIT {
            return Err(SessionError::new(
                413,
                "Codex 名称索引 id 或名称字段超过预算",
            ));
        }
        names.insert(
            sid.to_owned(),
            Name {
                title: title.to_owned(),
                updated: norm_ts(&row["updated_at"])
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            },
        );
        if names.len() > NAME_LIMIT {
            return Err(SessionError::new(
                413,
                "Codex 名称索引超过 10000 个 ID 预算",
            ));
        }
    }
    Ok(names)
}

impl NameIndex {
    /// Python `finalize_sessions` titles over summary rows: own name first,
    /// otherwise the first named ancestor along the `forked_from_id`
    /// lineage (`graph::lineage`), otherwise the root's base title the
    /// graph already set.
    pub fn enrich(&self, rows: &mut [Value], entries: &BTreeMap<String, CandidateRef>) {
        let mains = graph::mains_by_sid(entries);
        for row in rows {
            let Some(entry) = row["uid"].as_str().and_then(|uid| entries.get(uid)) else {
                continue;
            };
            if entry.source != "codex" || entry.summary.agent.is_some() {
                continue;
            }
            let own = self.names.get(entry.summary.sid.as_str());
            row["renamed_at"] = own.map(|name| name.updated.clone()).unwrap_or(Value::Null);
            row["renamed_to"] = own
                .filter(|name| !name.updated.is_null())
                .map(|name| json!(name.title))
                .unwrap_or(Value::Null);
            let named = own.or_else(|| {
                graph::lineage(entry, &mains)
                    .iter()
                    .find_map(|parent| self.names.get(parent.summary.sid.as_str()))
            });
            if let Some(name) = named {
                row["title"] = json!(clip(&name.title, 110));
            }
        }
    }
}

#[cfg(test)]
mod tests;
