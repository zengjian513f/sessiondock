//! Codex `session_index.jsonl` names applied to summary rows (batch 34).
//!
//! Same contract as Python's `_thread_names`: a missing index is an empty
//! index, malformed lines are skipped, `id` + `thread_name` must be truthy,
//! titles are clipped to 110 characters, and `renamed_at`/`renamed_to` only
//! appear when the entry carries an `updated_at`. The snapshot is reused while
//! the file's stamp is unchanged, so a warm list refresh costs one `stat` here.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use cap_fs_ext::MetadataExt;
use cap_std::fs::Metadata;
use serde_json::{Value, json};

use super::summary::{clip, norm_ts, truthy};
use super::{CandidateRef, graph};
use crate::sessions::SessionError;

fn error(message: &str) -> SessionError {
    SessionError::new(503, format!("Codex 名称索引：{message}"))
}

fn io_error(io: std::io::Error) -> SessionError {
    match io.kind() {
        std::io::ErrorKind::PermissionDenied => error("文件或目录不可读"),
        _ => error("文件访问失败，请检查名称索引"),
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

struct Opened {
    file: Option<std::fs::File>,
    stamp: Option<Stamp>,
}

impl Opened {
    fn new(path: &Path) -> Result<Self, SessionError> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(io) if io.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    file: None,
                    stamp: None,
                });
            }
            Err(io) => return Err(io_error(io)),
        };
        let after = Metadata::from_file(&file).map_err(io_error)?;
        if !after.is_file() {
            return Err(error("配置必须指向文件"));
        }
        Ok(Self {
            file: Some(file),
            stamp: Some(stamp(&after)),
        })
    }
}

#[derive(Debug)]
struct Name {
    title: String,
    updated: Value,
}

pub struct NameIndex {
    stamp: Option<Stamp>,
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
    let Some(mut file) = opened.file.take() else {
        return Ok(Arc::new(NameIndex {
            stamp: None,
            names: BTreeMap::new(),
        }));
    };
    let expected = opened.stamp.as_ref().expect("opened file has a stamp");
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io_error)?;
    if bytes.len() as u64 != expected.size {
        return Err(error("读取期间文件或父目录身份改变，请重试"));
    }
    let names = parse(&bytes)?;
    let after = Metadata::from_file(&file).map_err(io_error)?;
    if stamp(&after) != *expected {
        return Err(error("读取期间文件或父目录身份改变，请重试"));
    }
    Ok(Arc::new(NameIndex {
        stamp: opened.stamp,
        names,
    }))
}

fn parse(bytes: &[u8]) -> Result<BTreeMap<String, Name>, SessionError> {
    let mut names = BTreeMap::new();
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(row) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let Some(row) = row.as_object() else {
            continue;
        };
        let Some(id) = row.get("id").filter(|value| truthy(value)) else {
            continue;
        };
        let Some(thread_name) = row.get("thread_name").filter(|value| truthy(value)) else {
            continue;
        };
        let Some(sid) = id.as_str() else {
            // Non-string Python dictionary keys cannot match a session SID.
            continue;
        };
        let title = match thread_name {
            Value::String(value) => value.clone(),
            Value::Bool(value) => if *value { "True" } else { "False" }.to_owned(),
            value => value.to_string(),
        };
        names.insert(
            sid.to_owned(),
            Name {
                title,
                updated: row
                    .get("updated_at")
                    .and_then(norm_ts)
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            },
        );
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
