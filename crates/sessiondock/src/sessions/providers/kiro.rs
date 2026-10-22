//! Render normalized items from SessionDock's ACP projection only.
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
        let text = string(&item["text"]);
        let mut extra = item.clone();
        if let Some(object) = extra.as_object_mut() {
            object.remove("role");
            object.remove("text");
            object.remove("content");
        }
        self.emit(end, role, text, &item["timestamp"], extra);
        Ok(())
    }
}
