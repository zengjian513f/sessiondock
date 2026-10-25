//! Projection of CodeBuddy's native transcript records. The wire shape is
//! OpenAI-Responses style: `message`, `reasoning`, `function_call`,
//! `function_call_result` plus bookkeeping kinds (docs/codebuddy.md).
use super::{Parser, clip, first_nonempty, normalized, string};
use crate::sessions::index::summary::codebuddy::real_user_text;
use serde_json::{Value, json};
use std::path::Path;

/// Native bookkeeping kinds that carry no conversation content.
const BOOKKEEPING: [&str; 4] = ["file-history-snapshot", "custom-title", "ai-title", "topic"];

pub(super) fn metadata(path: &Path, records: &[(Value, u64)], fallback: &str) -> Value {
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut sid = stem.clone();
    let mut sid_seen = false;
    let mut cwd = String::new();
    let mut created = Value::Null;
    let mut updated = Value::Null;
    let mut model = Value::Null;
    let mut first_user = String::new();
    let mut custom = String::new();
    let mut ai = String::new();
    for (record, _) in records {
        if !sid_seen && let Some(value) = record["sessionId"].as_str().filter(|s| !s.is_empty()) {
            sid = value.to_owned();
            sid_seen = true;
        }
        if cwd.is_empty() {
            cwd = string(&record["cwd"]);
        }
        let ts = normalized(&record["timestamp"]);
        if !ts.is_null() {
            if created.is_null() {
                created = ts.clone();
            }
            updated = ts;
        }
        let candidate = &record["providerData"]["model"];
        if crate::sessions::providers::valid_native_model(candidate)
            && (record["type"] != "message" || record["role"] == "assistant")
        {
            model = candidate.clone();
        }
        match record["type"].as_str().unwrap_or("") {
            "custom-title" => custom = string(&record["customTitle"]),
            "ai-title" => ai = string(&record["aiTitle"]),
            "message" if first_user.is_empty() => {
                if let Ok(Some(text)) = real_user_text(record) {
                    first_user = text;
                }
            }
            _ => {}
        }
    }
    let title = [custom, ai, first_user]
        .into_iter()
        .find(|title| !title.trim().is_empty())
        .unwrap_or(stem);
    if cwd.is_empty() {
        cwd = "(未知)".to_owned();
    }
    if created.is_null() {
        created = json!(fallback);
    }
    if updated.is_null() {
        updated = created.clone();
    }
    json!({"sid":sid,"title":clip(&title),"cwd":cwd,"created":created,"updated":updated,
        "model":model,"branch":Value::Null})
}

impl Parser<'_> {
    pub(super) fn codebuddy(&mut self, record: &Value, end: u64) -> Result<(), String> {
        let first = self.events.len();
        let result = self.codebuddy_record(record, end);
        // The record id is the identity a live SDK echo reports, so the SDK
        // view can cut the history exactly where the live projection begins.
        if let Some(id) = record["id"].as_str().filter(|id| !id.is_empty()) {
            for event in &mut self.events[first..] {
                event.message["native_id"] = json!(id);
            }
        }
        result
    }

    fn codebuddy_record(&mut self, record: &Value, end: u64) -> Result<(), String> {
        let kind = record["type"].as_str().unwrap_or("");
        let ts = normalized(&record["timestamp"]);
        match kind {
            "message" => {
                let role = match record["role"].as_str().unwrap_or("") {
                    "user" => "user",
                    "assistant" => "assistant",
                    "system" | "developer" => "system",
                    other => {
                        self.skipped.note("CodeBuddy 消息角色", other);
                        return Ok(());
                    }
                };
                if role == "user" {
                    let provider = &record["providerData"];
                    if provider["isMeta"] == true
                        || provider["skipRun"] == true
                        || provider["isCompactInternal"] == true
                        || provider["isCompacted"] == true
                        || provider["isSummary"] == true
                    {
                        return Ok(());
                    }
                    let flat = super::visible_text(&record["content"]);
                    if flat.trim_start().starts_with("<turn_aborted>") {
                        self.status(end, "aborted", &ts, json!({"reason":"用户中断"}));
                        return Ok(());
                    }
                    self.turn = format!("request:{}", string(&provider["conversationRequestId"]));
                }
                let phase = (role == "assistant").then_some("progress");
                self.content(end, role, &record["content"], &ts, phase)?;
            }
            "reasoning" => {
                let raw = record["rawContent"].as_array().into_iter().flatten();
                let text = raw
                    .filter_map(|block| block["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = if text.trim().is_empty() {
                    super::visible_text(&record["content"])
                } else {
                    text
                };
                if !text.trim().is_empty() {
                    self.emit(end, "thinking", text, &ts, json!({}));
                }
            }
            "function_call" => {
                let name = first_nonempty(&[record["name"].clone()], "tool");
                self.tool(
                    end,
                    &name,
                    &string(&record["callId"]),
                    &record["arguments"],
                    &ts,
                )?;
            }
            "function_call_result" => {
                let failed = record["status"].as_str().is_some_and(|s| s != "completed")
                    || record["output"]["isError"] == true;
                self.output(
                    end,
                    &string(&record["callId"]),
                    &record["output"],
                    failed,
                    &ts,
                )?;
            }
            kind if BOOKKEEPING.contains(&kind) => {}
            other => self.skipped.note("CodeBuddy 记录类型", other),
        }
        Ok(())
    }

    /// The last assistant text of a turn with no tool call after it is the
    /// answer; every earlier assistant text is progress narration.
    pub(super) fn codebuddy_phases(&mut self) {
        let mut later_tool = false;
        let mut final_seen = false;
        for event in self.events.iter_mut().rev() {
            match event.message["role"].as_str().unwrap_or("") {
                "user" => {
                    later_tool = false;
                    final_seen = false;
                }
                "tool" | "question" => later_tool = true,
                "assistant" => {
                    let last = !later_tool && !final_seen;
                    event.message["phase"] = json!(if last { "final" } else { "progress" });
                    final_seen |= last;
                }
                _ => {}
            }
        }
    }
}
