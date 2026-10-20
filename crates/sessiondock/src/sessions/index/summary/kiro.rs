//! Kiro session summary (`session.json` + `messages.jsonl` for V3 KAS, and
//! `cli/<uuid>.json` + `cli/<uuid>.jsonl` for V2 legacy CLI).

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{
    AgentMeta, CLAUDE_HEAD_LINES, Input, Records, RowSummary, SidecarBytes, clip, committed_end,
    flatten_text, is_injected, iso_seconds, norm_ts, py_strip, skipped_warnings, title_from_text,
};
use crate::sessions::scope::kiro_native_identity;

/// Locate a Kiro session metadata file (`session.json` or `<uuid>.json`) under `kiro_root`
/// by its session ID.
pub fn locate_session_file(kiro_root: &Path, session_id: &str) -> Option<PathBuf> {
    let sid = session_id.trim();
    if sid.is_empty() || sid.contains('/') || sid.contains('\\') || sid == "." || sid == ".." {
        return None;
    }

    // 1. Direct V3: <root>/<sid>/session.json
    let direct_v3 = kiro_root.join(sid).join("session.json");
    if direct_v3.is_file() {
        return Some(direct_v3);
    }

    // 2. V2 CLI subdir: <root>/cli/<sid>.json
    let v2_cli = kiro_root.join("cli").join(format!("{sid}.json"));
    if v2_cli.is_file() {
        return Some(v2_cli);
    }

    // 3. Direct V2: <root>/<sid>.json
    let direct_v2 = kiro_root.join(format!("{sid}.json"));
    if direct_v2.is_file() {
        return Some(direct_v2);
    }

    // 4. Bucket V3: <root>/<bucket>/<sid>/session.json or fallback JSON scan
    let entries = std::fs::read_dir(kiro_root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let candidate = path.join(sid).join("session.json");
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // 5. Fallback scan for non-matching directory/file names whose JSON declares `id`/`session_id`
    if let Ok(entries) = std::fs::read_dir(kiro_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let v3 = path.join("session.json");
                if v3.is_file() && json_declares_id(&v3, sid) {
                    return Some(v3);
                }
                if let Ok(children) = std::fs::read_dir(&path) {
                    for child in children.flatten() {
                        let child_path = child.path();
                        if child_path.is_dir() {
                            let nested = child_path.join("session.json");
                            if nested.is_file() && json_declares_id(&nested, sid) {
                                return Some(nested);
                            }
                        } else if child_path.extension().and_then(|e| e.to_str()) == Some("json")
                            && json_declares_id(&child_path, sid)
                        {
                            return Some(child_path);
                        }
                    }
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("json")
                && json_declares_id(&path, sid)
            {
                return Some(path);
            }
        }
    }

    None
}

/// Extract native session ID from a Kiro `session.json` or `<uuid>.json` path.
pub fn sid_for_path(path: &Path) -> Option<String> {
    if let Ok(bytes) = std::fs::read(path)
        && let Ok(val) = serde_json::from_slice::<Value>(&bytes)
        && let Ok(sid) = kiro_native_identity(Some(&val)).0
        && !sid.is_empty()
    {
        return Some(sid);
    }
    let fallback = infer_default_sid(path);
    (!fallback.is_empty()).then_some(fallback)
}

/// Resolve either a session ID (`<sid>`) or an index UID (`kiro:<16-hex-sha1>`)
/// under `kiro_root`, returning `(session_metadata_path, native_sid)`.
pub fn locate_session_file_by_uid_or_sid(
    kiro_root: &Path,
    uid_or_sid: &str,
) -> Option<(PathBuf, String)> {
    let trimmed = uid_or_sid.trim();
    if trimmed.is_empty() {
        return None;
    }
    let raw = trimmed.strip_prefix("kiro:").unwrap_or(trimmed);
    if let Some(path) = locate_session_file(kiro_root, raw) {
        let sid = sid_for_path(&path).unwrap_or_else(|| raw.to_owned());
        return Some((path, sid));
    }
    let expected_uid = if trimmed.starts_with("kiro:") {
        trimmed.to_owned()
    } else {
        format!("kiro:{trimmed}")
    };
    let matches_uid = |candidate: &Path| -> bool {
        if crate::sessions::uid_for("kiro", candidate) == expected_uid {
            return true;
        }
        if let Ok(canon) = candidate.canonicalize()
            && crate::sessions::uid_for("kiro", &canon) == expected_uid
        {
            return true;
        }
        false
    };
    let entries = std::fs::read_dir(kiro_root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let direct_v3 = path.join("session.json");
            if direct_v3.is_file() && matches_uid(&direct_v3) {
                let sid = sid_for_path(&direct_v3).unwrap_or_default();
                return Some((direct_v3, sid));
            }
            if let Ok(children) = std::fs::read_dir(&path) {
                for child in children.flatten() {
                    let child_path = child.path();
                    if child_path.is_dir() {
                        let nested = child_path.join("session.json");
                        if nested.is_file() && matches_uid(&nested) {
                            let sid = sid_for_path(&nested).unwrap_or_default();
                            return Some((nested, sid));
                        }
                    } else if child_path.extension().and_then(|e| e.to_str()) == Some("json")
                        && child_path.file_name().and_then(|n| n.to_str()) != Some("session.json")
                        && child_path.file_name().and_then(|n| n.to_str()) != Some("index.json")
                        && matches_uid(&child_path)
                    {
                        let sid = sid_for_path(&child_path).unwrap_or_default();
                        return Some((child_path, sid));
                    }
                }
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("json")
            && path.file_name().and_then(|n| n.to_str()) != Some("session.json")
            && path.file_name().and_then(|n| n.to_str()) != Some("index.json")
            && matches_uid(&path)
        {
            let sid = sid_for_path(&path).unwrap_or_default();
            return Some((path, sid));
        }
    }
    None
}

fn json_declares_id(path: &Path, target_sid: &str) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(val) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    val.get("id")
        .and_then(Value::as_str)
        .or_else(|| val.get("sessionId").and_then(Value::as_str))
        .or_else(|| val.get("session_id").and_then(Value::as_str))
        .is_some_and(|id| id.trim() == target_sid)
}

pub fn summarize(input: &Input<'_>) -> RowSummary {
    let default_sid = infer_default_sid(input.path);
    let (summary, summary_stamp) = match &input.sidecar {
        Some(SidecarBytes::Bytes { bytes, stamp }) => match serde_json::from_slice::<Value>(bytes) {
            Ok(Value::Object(map)) => (Value::Object(map), *stamp),
            Ok(_) => {
                return unsupported(
                    default_sid,
                    input.data.as_ref().map(|d| d.stamp).unwrap_or(*stamp),
                    "Kiro session.json 顶层不是 JSON 对象".to_owned(),
                );
            }
            Err(error) => {
                return unsupported(
                    default_sid,
                    input.data.as_ref().map(|d| d.stamp).unwrap_or(*stamp),
                    format!("Kiro session.json 解析失败: {error}"),
                );
            }
        },
        Some(SidecarBytes::Failed { stamp, reason }) => {
            let fallback_stamp = input
                .data
                .as_ref()
                .map(|d| d.stamp)
                .or(*stamp)
                .unwrap_or(super::Stamp {
                    size: 0,
                    mtime_ns: 0,
                    dev: 0,
                    ino: 0,
                });
            return unsupported(default_sid, fallback_stamp, reason.clone());
        }
        None => {
            let fallback_stamp = input.data.as_ref().map(|d| d.stamp).unwrap_or(super::Stamp {
                size: 0,
                mtime_ns: 0,
                dev: 0,
                ino: 0,
            });
            return unsupported(
                default_sid,
                fallback_stamp,
                "Kiro session.json 缺失".to_owned(),
            );
        }
    };

    let (native_id, declared_ids) = kiro_native_identity(Some(&summary));
    let sid = native_id
        .as_ref()
        .ok()
        .cloned()
        .unwrap_or_else(|| default_sid.clone());

    let is_v2 = summary.get("session_state").is_some()
        || (summary.get("session_id").is_some() && summary.get("id").is_none());

    let mut first_user_title = String::new();
    let mut first_turn_ts = String::new();
    let mut last_turn_ts = String::new();
    let mut committed = None;
    let mut cursor_head = None;
    let mut warnings = Vec::new();
    let mut size = 0u64;

    if let Some(data) = &input.data {
        size = data.stamp.size;
        let records = Records::parse(data, CLAUDE_HEAD_LINES);
        for record in records.all() {
            let v = &record.value;
            if let Some(ts) = extract_turn_timestamp(v, is_v2) {
                if first_turn_ts.is_empty() {
                    first_turn_ts = ts.clone();
                }
                last_turn_ts = ts;
            }
            if first_user_title.is_empty()
                && let Some(text) = extract_user_prompt_text(v, is_v2)
            {
                let candidate = title_from_text(&text);
                if !candidate.is_empty() {
                    first_user_title = candidate;
                }
            }
        }
        committed = committed_end(data);
        cursor_head = committed.and_then(|end| super::cursor_head(data, end));
        warnings = skipped_warnings("kiro", &records);
    }

    if let Some(hist) = summary.get("history").and_then(Value::as_array) {
        for entry in hist {
            if let Some(ts) = extract_turn_timestamp(entry, true) {
                if first_turn_ts.is_empty() {
                    first_turn_ts = ts.clone();
                }
                if last_turn_ts.is_empty() {
                    last_turn_ts = ts;
                }
            }
            if first_user_title.is_empty()
                && let Some(text) = extract_user_prompt_text(entry, true)
            {
                let candidate = title_from_text(&text);
                if !candidate.is_empty() {
                    first_user_title = candidate;
                }
            }
        }
    }

    let created_raw = summary
        .get("createdAt")
        .or_else(|| summary.get("created_at"))
        .and_then(norm_ts);
    let created = created_raw
        .or_else(|| (!first_turn_ts.is_empty()).then(|| first_turn_ts.clone()))
        .unwrap_or_else(|| iso_seconds(summary_stamp.mtime_ns));

    if last_turn_ts.is_empty() && is_v2 {
        last_turn_ts = extract_v2_turn_metadata_ts(&summary);
    }

    // BUG-20261020-085512-56b058: prioritize the last actual conversation turn timestamp;
    // when no conversation turns exist yet, fall back to `created` (never `lastModifiedAt` / `mtime`).
    let updated = if !last_turn_ts.is_empty() {
        last_turn_ts
    } else {
        created.clone()
    };

    let explicit_title = summary
        .get("title")
        .and_then(Value::as_str)
        .or_else(|| summary.get("customTitle").and_then(Value::as_str))
        .or_else(|| summary.get("name").and_then(Value::as_str))
        .map(py_strip)
        .unwrap_or("")
        .to_owned();

    let title = if explicit_title.is_empty()
        || explicit_title == "New Session"
        || explicit_title == "(no title)"
    {
        if !first_user_title.is_empty() {
            first_user_title
        } else {
            "新建 Kiro 会话".to_owned()
        }
    } else {
        clip(&explicit_title, 90)
    };

    let cwd = extract_cwd(&summary, is_v2);
    let model = extract_model(&summary, is_v2);
    let branch = extract_branch(&summary);
    let agent = extract_agent(&summary, &sid, &title, is_v2);

    let mut out = RowSummary::blank(sid, title, cwd, created);
    out.updated = updated;
    out.size = size;
    out.model = model;
    out.branch = branch;
    out.agent = agent;
    out.native_id = native_id;
    out.declared_ids = declared_ids;
    out.committed = committed;
    out.cursor_head = cursor_head;
    out.warnings = warnings;
    out
}

fn infer_default_sid(path: &Path) -> String {
    if path.file_name().and_then(|n| n.to_str()) == Some("session.json") {
        path.parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_owned()
    } else {
        path.file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_owned()
    }
}

fn extract_cwd(summary: &Value, is_v2: bool) -> String {
    for key in ["cwd", "workspaceDirectory", "workingDirectory", "directory"] {
        let candidate = summary
            .get(key)
            .and_then(Value::as_str)
            .map(py_strip)
            .unwrap_or("")
            .to_owned();
        if !candidate.is_empty() {
            return candidate;
        }
    }
    if is_v2
        && let Some(turns) = summary
            .pointer("/session_state/conversation_metadata/user_turn_metadatas")
            .and_then(Value::as_array)
    {
        for turn in turns.iter().rev() {
            let candidate = turn
                .get("cwd")
                .and_then(Value::as_str)
                .or_else(|| turn.get("working_directory").and_then(Value::as_str))
                .map(py_strip)
                .unwrap_or("")
                .to_owned();
            if !candidate.is_empty() {
                return candidate;
            }
        }
    }
    "unknown".to_owned()
}

fn extract_model(summary: &Value, is_v2: bool) -> Value {
    for key in ["modelId", "model_id", "model"] {
        if let Some(m) = summary
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Value::String(m.to_owned());
        }
    }
    if is_v2 {
        for ptr in [
            "/session_state/rts_model_state/model_info/model_id",
            "/session_state/rts_model_state/model_info/modelId",
            "/session_state/rts_model_state/model_id",
            "/session_state/rts_model_state/modelId",
        ] {
            if let Some(m) = summary
                .pointer(ptr)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Value::String(m.to_owned());
            }
        }
        if let Some(turns) = summary
            .pointer("/session_state/conversation_metadata/user_turn_metadatas")
            .and_then(Value::as_array)
        {
            for turn in turns.iter().rev() {
                for key in ["model_id", "modelId", "model"] {
                    if let Some(m) = turn
                        .get(key)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        return Value::String(m.to_owned());
                    }
                }
            }
        }
    }
    Value::Null
}

fn extract_branch(summary: &Value) -> Value {
    for key in ["gitBranch", "git_branch", "branch"] {
        if let Some(b) = summary
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Value::String(b.to_owned());
        }
    }
    Value::Null
}

fn extract_agent(summary: &Value, sid: &str, title: &str, is_v2: bool) -> Option<AgentMeta> {
    let parent_sid = summary
        .get("parentSessionId")
        .or_else(|| summary.get("parent_session_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    if is_v2 {
        // V2: ONLY mark as subagent if `parent_session_id` is non-empty!
        let parent_sid = parent_sid?;
        return Some(AgentMeta {
            id: sid.to_owned(),
            parent_sid: Some(parent_sid),
            title: title.to_owned(),
            kind: "subagent".to_owned(),
            open_turn: false,
            turn_at: None,
        });
    }

    // V3: `parentSessionId` is non-empty OR `createdReason == "subagent"`
    let created_reason_subagent = summary
        .get("createdReason")
        .or_else(|| summary.get("session_created_reason"))
        .and_then(Value::as_str)
        .is_some_and(|r| r.trim() == "subagent");

    if parent_sid.is_none() && !created_reason_subagent {
        return None;
    }

    let kind = summary
        .get("agentName")
        .or_else(|| summary.get("subagentType"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("subagent")
        .to_owned();

    Some(AgentMeta {
        id: sid.to_owned(),
        parent_sid,
        title: title.to_owned(),
        kind,
        open_turn: false,
        turn_at: None,
    })
}

fn extract_turn_timestamp(record: &Value, is_v2: bool) -> Option<String> {
    let msg = record.get("message").unwrap_or(record);
    let kind = msg
        .get("kind")
        .or_else(|| msg.get("type"))
        .or_else(|| record.get("kind"))
        .or_else(|| record.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let is_turn_record = if is_v2 {
        matches!(
            kind,
            "Prompt" | "AssistantMessage" | "ToolResults" | "user" | "assistant" | "tool_result"
        )
    } else {
        match kind {
            "user" | "assistant" | "tool_call" | "tool_result" | "error" => true,
            "session_start" => {
                let data = msg.get("data").unwrap_or(msg);
                !data
                    .get("forcedRole")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            }
            _ => false,
        }
    };
    if !is_turn_record {
        return None;
    }

    for candidate in [
        record.get("timestamp"),
        record.get("createdAt"),
        record.get("created_at"),
        msg.get("timestamp"),
        msg.get("createdAt"),
        msg.get("created_at"),
        msg.get("data").and_then(|d| d.get("timestamp")),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(ts) = norm_ts(candidate) {
            return Some(ts);
        }
    }
    None
}

fn extract_v2_turn_metadata_ts(summary: &Value) -> String {
    let Some(turns) = summary
        .pointer("/session_state/conversation_metadata/user_turn_metadatas")
        .and_then(Value::as_array)
    else {
        return String::new();
    };
    for turn in turns.iter().rev() {
        for key in [
            "timestamp",
            "ended_at",
            "completed_at",
            "updated_at",
            "started_at",
            "created_at",
        ] {
            if let Some(val) = turn.get(key)
                && let Some(ts) = norm_ts(val)
            {
                return ts;
            }
        }
    }
    String::new()
}

pub(crate) fn extract_user_prompt_text(record: &Value, is_v2: bool) -> Option<String> {
    let msg = record.get("message").unwrap_or(record);
    let kind = msg
        .get("kind")
        .or_else(|| msg.get("type"))
        .or_else(|| record.get("kind"))
        .or_else(|| record.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let data = msg.get("data").unwrap_or(msg);

    if is_v2 || kind == "Prompt" {
        if kind == "Prompt" || kind == "user" {
            let text = extract_content_text(data);
            if !text.is_empty() && !is_injected(&text) {
                return Some(text);
            }
        } else if kind.is_empty() {
            // Check V2 embedded history turn object (`user.content.Prompt.prompt` or `user` / `prompt`)
            if let Some(u) = record.get("user") {
                let text = extract_v2_embedded_user_text(u);
                if !text.is_empty() && !is_injected(&text) {
                    return Some(text);
                }
            }
            if let Some(p) = record.get("prompt").and_then(Value::as_str) {
                let t = p.trim();
                if !t.is_empty() && !is_injected(t) {
                    return Some(t.to_owned());
                }
            }
        }
        return None;
    }

    match kind {
        "user" => {
            let text = extract_content_text(data);
            if !text.is_empty() && !is_injected(&text) {
                return Some(text);
            }
            None
        }
        "session_start" => {
            if data
                .get("forcedRole")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return None;
            }
            let text = extract_content_text(data);
            if !text.is_empty() && !is_injected(&text) {
                return Some(text);
            }
            None
        }
        _ => None,
    }
}

pub(crate) fn extract_content_text(data: &Value) -> String {
    if let Some(content) = data.get("content") {
        if let Some(s) = content.as_str() {
            return s.trim().to_owned();
        }
        if let Some(arr) = content.as_array() {
            let mut parts = Vec::new();
            for item in arr {
                if let Some(s) = item.as_str() {
                    let t = s.trim();
                    if !t.is_empty() {
                        parts.push(t.to_owned());
                    }
                    continue;
                }
                let item_kind = item
                    .get("kind")
                    .or_else(|| item.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if matches!(item_kind, "text" | "Text" | "") {
                    let t = item
                        .get("data")
                        .and_then(Value::as_str)
                        .or_else(|| item.get("text").and_then(Value::as_str))
                        .or_else(|| {
                            item.get("data")
                                .and_then(|d| d.get("text"))
                                .and_then(Value::as_str)
                        })
                        .unwrap_or("")
                        .trim();
                    if !t.is_empty() {
                        parts.push(t.to_owned());
                    }
                }
            }
            if !parts.is_empty() {
                return parts.join("\n\n");
            }
        }
        if let Ok(flat) = flatten_text(content) {
            let trimmed = flat.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
    }
    for key in ["text", "prompt", "message"] {
        if let Some(s) = data.get(key).and_then(Value::as_str) {
            let t = s.trim();
            if !t.is_empty() {
                return t.to_owned();
            }
        }
    }
    String::new()
}

pub(crate) fn extract_v2_embedded_user_text(u: &Value) -> String {
    if let Some(s) = u.as_str() {
        return s.trim().to_owned();
    }
    if let Some(p) = u.pointer("/content/Prompt/prompt").and_then(Value::as_str) {
        return p.trim().to_owned();
    }
    let t = extract_content_text(u);
    if !t.is_empty() {
        return t;
    }
    String::new()
}

fn unsupported(sid: String, stamp: super::Stamp, reason: String) -> RowSummary {
    let mut out = RowSummary::blank(
        sid,
        "不支持的会话格式".to_owned(),
        "unknown".to_owned(),
        iso_seconds(stamp.mtime_ns),
    );
    out.size = stamp.size;
    out.unsupported = Some(reason);
    out
}
