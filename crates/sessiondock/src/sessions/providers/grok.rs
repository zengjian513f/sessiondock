//! Summary-derived Grok metadata. The transcript never overrides these fields,
//! and URL-decoded cwd is display metadata, not a filesystem authorization.

use chrono::Timelike;
use serde_json::{Value, json};
use std::path::Path;

use crate::json_compat::unquote;
use crate::sessions::text::{clip, normalized};

/// Sidebar title when Grok has not written `generated_title` or
/// `session_summary`. Matches the pending-session label so a freshly
/// launched native session does not jump to the first eight characters
/// of its UUID directory name. Python still uses those eight characters
/// (DELTA).
pub(super) const UNTITLED_TITLE: &str = "新建 Grok 会话";

pub(super) fn validate(summary: &Value) -> Result<(), String> {
    if !summary.is_object() {
        return Err("Grok summary.json 必须是 JSON 对象".to_owned());
    }
    if !summary["info"].is_null() && !summary["info"].is_object() {
        return Err("Grok summary.json 的 info 必须是对象或 null".to_owned());
    }
    for key in [
        "generated_title",
        "session_summary",
        "current_model_id",
        "agent_name",
    ] {
        if !summary[key].is_null() && !summary[key].is_string() {
            return Err(format!("Grok summary.json 的 {key} 必须是字符串或 null"));
        }
    }
    for key in ["id", "cwd"] {
        if !summary["info"][key].is_null() && !summary["info"][key].is_string() {
            return Err(format!(
                "Grok summary.json 的 info.{key} 必须是字符串或 null"
            ));
        }
    }
    Ok(())
}

fn nonempty(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !s.is_empty())
}

pub(super) fn metadata(path: &Path, summary: &Value, fallback: &str) -> Value {
    let directory = path.file_name().unwrap_or_default().to_string_lossy();
    let project = path
        .parent()
        .and_then(Path::file_name)
        .unwrap_or_default()
        .to_string_lossy();
    let sid = nonempty(&summary["info"]["id"]).unwrap_or(&directory);
    let title = nonempty(&summary["generated_title"])
        .or_else(|| nonempty(&summary["session_summary"]))
        .unwrap_or(UNTITLED_TITLE);
    let cwd = nonempty(&summary["info"]["cwd"])
        .map(str::to_owned)
        .unwrap_or_else(|| unquote(&project));
    // The mtime fallback uses _iso(...timespec='seconds'), independently
    // for created and updated. Do not substitute native message timestamps.
    let fallback = chrono::DateTime::parse_from_rfc3339(fallback)
        .ok()
        .and_then(|date| date.with_nanosecond(0))
        .map(|date| {
            date.to_utc()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        })
        .unwrap_or_else(|| fallback.to_owned());
    let mut created = normalized(&summary["created_at"]);
    let selected = if super::truthy(&summary["last_active_at"]) {
        &summary["last_active_at"]
    } else {
        &summary["updated_at"]
    };
    let mut updated = normalized(selected);
    if created.is_null() {
        created = json!(fallback);
    }
    if updated.is_null() {
        updated = json!(fallback);
    }
    json!({"sid":sid,"title":clip(title, 110),"cwd":cwd,"created":created,"updated":updated,
        "model":summary["current_model_id"],"branch":summary["agent_name"]})
}
