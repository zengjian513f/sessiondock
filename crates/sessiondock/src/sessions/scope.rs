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

/// OpenCode records carry no session id; the mirrored session row does.
pub(super) fn summary_native_identity(
    id: Option<&Value>,
) -> (Result<String, SessionError>, Vec<String>) {
    match id
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
    {
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

/// Kiro identity in the SessionDock-owned catalog projection.
pub(super) fn kiro_native_identity(
    summary: Option<&Value>,
) -> (Result<String, SessionError>, Vec<String>) {
    summary_native_identity(
        summary
            .filter(|s| s["format"] == super::kiro::FORMAT)
            .map(|s| &s["session"]["id"]),
    )
}

/// The scope rules over declared ID values from the records seen.
pub(crate) fn declared_identity<'a>(
    records: impl Iterator<Item = &'a Value>,
) -> (Result<String, SessionError>, Vec<String>) {
    let mut identity: Option<String> = None;
    let mut declared = BTreeSet::new();
    let mut error = None;
    for value in records {
        let Some(id) = value.as_str().filter(|id| !id.trim().is_empty()) else {
            error.get_or_insert_with(|| {
                SessionError::new(501, "原生会话 ID 无效，不能确定操作范围")
            });
            continue;
        };
        declared.insert(id.to_owned());
        if identity.as_deref().is_some_and(|previous| previous != id) {
            error.get_or_insert_with(|| SessionError::new(409, "原生记录包含冲突的会话 ID"));
        }
        identity.get_or_insert_with(|| id.to_owned());
    }
    let identity = match error {
        Some(error) => Err(error),
        None => identity.ok_or_else(|| {
            SessionError::new(501, "原生记录缺少明确会话 ID，不能用文件名或显示名称推断")
        }),
    };
    (identity, declared.into_iter().collect())
}

pub(super) fn native_identity(
    source: &str,
    records: &[(Value, u64)],
) -> (Result<String, SessionError>, Vec<String>) {
    match source {
        "claude" => declared_identity(records.iter().filter_map(|(r, _)| r.get("sessionId"))),
        "codex" => declared_identity(
            records
                .iter()
                .find(|(r, _)| r["type"] == "session_meta")
                .and_then(|(r, _)| r["payload"].get("id"))
                .into_iter(),
        ),
        "kiro" => declared_identity(records.iter().filter_map(|(r, _)| {
            r.get("session_id")
                .or_else(|| r.get("sessionId"))
                .or_else(|| r.get("payload").and_then(|p| p.get("sessionId")))
        })),
        _ => (
            Err(SessionError::new(501, "此数据源尚不支持原生操作范围")),
            Vec::new(),
        ),
    }
}
