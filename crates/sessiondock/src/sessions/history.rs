//! View identity helpers shared by `views` (`native_identity`, `history_link`,
//! `inherited_identity`). Ownership/fork rules live in `index/graph.rs` over
//! summaries.
//!
//! No paths from requests or `history_base` are opened here. Prefixes are
//! reparsed independently: parsing a parent's full tail and filtering offsets
//! would incorrectly retain later rollback/interrupt edits to earlier messages.

use serde_json::{Value, json};

use super::{Parsed, SessionError, hash};

/// Must match the ordinary physical cursor produced by the inventory parser.
/// In particular Codex agent parsing does not need an HTTP `agent` argument.
pub(super) fn native_identity(parsed: &Parsed, agent: &str) -> String {
    let agent = if parsed.meta["_is_subagent"] == true {
        text(&parsed.meta, "sid")
    } else {
        agent
    };
    hash(
        &serde_json::to_vec(&json!([
            "native-view-v1",
            parsed.candidate.source,
            text(&parsed.meta, "uid"),
            agent
        ]))
        .expect("primitive JSON serialization"),
    )
}

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or("")
}

fn unsupported(message: impl Into<String>) -> SessionError {
    SessionError::new(501, message)
}

/// The fixed-prefix parent a Codex transcript declares: `(thread_id, cut)`.
/// The physical parent is `history_base.thread_id`
/// (falling back to `forked_from_id`), the cut is `end_byte_offset`. A null
/// `history_base` inherits nothing — old-style forks and subagent rollouts
/// are self-contained files whose `forked_from_id` is only the logical
/// parent, and a rewind past the parent's own fork point legitimately names
/// a `thread_id` other than `forked_from_id`. `index/graph.rs` carries the
/// same function over summaries; the two must agree.
pub(super) fn history_link<'a>(
    source: &str,
    meta: &'a Value,
) -> Result<Option<(&'a str, usize)>, SessionError> {
    if source != "codex" {
        return Ok(None);
    }
    let base = &meta["history_base"];
    if base.is_null() {
        return Ok(None);
    }
    if !base.is_object() {
        return Err(unsupported("history_base 必须是对象"));
    }
    let declared = text(base, "thread_id");
    let parent = if declared.is_empty() {
        text(meta, "forked_from_id")
    } else {
        declared
    };
    if parent.is_empty() {
        return Err(unsupported("history_base 缺少父线程 ID"));
    }
    let cut = base["end_byte_offset"]
        .as_u64()
        .and_then(|cut| usize::try_from(cut).ok())
        .ok_or_else(|| unsupported("history_base 前缀偏移必须是非负整数"))?;
    Ok(Some((parent, cut)))
}

pub(super) fn inherited_identity(native: String, digests: Vec<Value>) -> String {
    if digests.is_empty() {
        native
    } else {
        hash(
            &serde_json::to_vec(&json!(["inherited-view-v1", native, digests]))
                .expect("primitive JSON serialization"),
        )
    }
}
