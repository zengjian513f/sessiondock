//! Render normalized items from SessionDock's ACP and native-history projections.
use super::{Parser, string};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn metadata(
    _path: &Path,
    summary: &Value,
    _records: &[(Value, u64)],
    fallback: &str,
) -> Value {
    let session = &summary["session"];
    json!({"sid":session["id"],"title":session["title"],"cwd":session["cwd"],
        "forked_from_id":summary["native"]["forked_from_id"],
        "model":session["model"],"created":session["created"].as_str().unwrap_or(fallback),
        "updated":session["updated"].as_str().unwrap_or(fallback),
        "history_coverage":if summary["history_synced"] == true { "cached" } else { "not_synced" }})
}
impl Parser<'_> {
    pub(super) fn kiro(&mut self, record: &Value, end: u64) -> Result<(), String> {
        if record["format"] != crate::sessions::kiro::FORMAT {
            return Err("未知 Kiro 投影格式".into());
        }
        let item = &record["item"];
        let role = item["role"].as_str().unwrap_or("system");
        let mut text = string(&item["text"]);
        let mut extra = item.clone();
        if let Some(object) = extra.as_object_mut() {
            object.remove("role");
            object.remove("text");
            object.remove("content");
        }
        if role == "tool" {
            let input = item
                .get("input")
                .filter(|v| !v.is_null())
                .or_else(|| item.get("tool_input"));
            if let Some(input) = input {
                text = string(input);
            }
            let result = item
                .get("result")
                .filter(|v| !v.is_null())
                .or_else(|| item.get("tool_result"))
                .filter(|v| !v.is_null());
            if let Some(result) = result {
                extra["result"] = if result["role"] == "tool_result" {
                    result.clone()
                } else {
                    json!({"role":"tool_result","text":string(result),
                        "error":item["error"] == true || item["status"] == "failed"})
                };
            }
        }
        self.emit(end, role, text, &item["timestamp"], extra);
        Ok(())
    }
}
