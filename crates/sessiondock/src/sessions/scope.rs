//! Identity provenance captured once from already parsed, committed records.
//! Display SID fallback and agent labels are deliberately not evidence here.

use super::{NativeScope, SessionError};
use serde_json::Value;
use std::collections::BTreeSet;

/// Crate-private provenance transfer. Public display rows/NativeScope literals
/// cannot construct a runtime catalog marked as verified.
pub(crate) struct CatalogEntry {
    pub uid: String,
    pub source: String,
    pub declared_ids: Vec<String>,
    pub scope: Result<NativeScope, SessionError>,
    pub subagent: bool,
}

/// A Grok main session's identity is the validated `summary.json`
/// `info.id` (the value `grok --session-id`/`--resume` name); the chat file
/// declares nothing. The index summary derives the same value.
pub(super) fn grok_native_identity(
    summary: Option<&Value>,
) -> (Result<String, SessionError>, Vec<String>) {
    let id = summary
        .map(|summary| &summary["info"]["id"])
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty());
    match id {
        Some(id) => (Ok(id.to_owned()), vec![id.to_owned()]),
        None => (
            Err(SessionError::new(
                501,
                "原生记录缺少明确会话 ID，不能用文件名或显示名称推断",
            )),
            Vec::new(),
        ),
    }
}

pub(super) fn native_identity(
    source: &str,
    records: &[(Value, u64)],
) -> (Result<String, SessionError>, Vec<String>) {
    if !matches!(source, "claude" | "codex") {
        return (
            Err(SessionError::new(501, "此数据源尚不支持原生操作范围")),
            Vec::new(),
        );
    }
    let mut identity = None;
    let mut declared = BTreeSet::new();
    let mut error = None;
    let mut codex_meta_seen = false;
    for (record, _) in records {
        let value = if source == "claude" {
            record.get("sessionId")
        } else if record["type"] == "session_meta" {
            // The first session_meta is
            // the only identity; old-style forks and subagent rollouts copy
            // their ancestors' metas after it, and those ids are not this file's.
            if codex_meta_seen {
                continue;
            }
            codex_meta_seen = true;
            record["payload"].get("id")
        } else {
            continue;
        };
        let Some(value) = value else { continue };
        let Some(id) = value.as_str().filter(|id| !id.trim().is_empty()) else {
            error.get_or_insert_with(|| {
                SessionError::new(501, "原生会话 ID 无效，不能确定操作范围")
            });
            continue;
        };
        declared.insert(id.to_owned());
        if identity.is_some_and(|previous| previous != id) {
            error.get_or_insert_with(|| SessionError::new(409, "原生记录包含冲突的会话 ID"));
        }
        identity.get_or_insert(id);
    }
    let identity = match error {
        Some(error) => Err(error),
        None => identity.map(str::to_owned).ok_or_else(|| {
            SessionError::new(501, "原生记录缺少明确会话 ID，不能用文件名或显示名称推断")
        }),
    };
    (identity, declared.into_iter().collect())
}
