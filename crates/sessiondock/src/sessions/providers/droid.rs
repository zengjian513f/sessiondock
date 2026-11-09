//! Factory JSONL: session_start followed by messages with native block content.
use super::{Parser, normalized, string};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn metadata(path: &Path, records: &[(Value, u64)], fallback: &str) -> Value {
    let head = records
        .iter()
        .map(|(v, _)| v)
        .find(|v| v["type"] == "session_start");
    let empty = Value::Null;
    let head = head.unwrap_or(&empty);
    let sid = head["id"]
        .as_str()
        .or_else(|| head["sessionId"].as_str())
        .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or(""));
    let mut model = Value::Null;
    let mut created = json!(fallback);
    let mut updated = created.clone();
    for (v, _) in records {
        let ts = normalized(&v["timestamp"]);
        if !ts.is_null() {
            if created == fallback {
                created = ts.clone();
            }
            updated = ts;
        }
        if let Some(m) = v["message"]["modelId"].as_str() {
            model = json!(m);
        }
    }
    json!({"sid":sid,"title":head["title"].as_str().unwrap_or("新建 Droid 会话"),"cwd":head["cwd"].as_str().unwrap_or("(未知)"),"created":created,"updated":updated,"model":model,"branch":null})
}
impl Parser<'_> {
    pub(super) fn droid(&mut self, record: &Value, end: u64) -> Result<(), String> {
        if record["type"] == "session_start" {
            return Ok(());
        }
        let first = self.events.len();
        let ts = normalized(&record["timestamp"]);
        if record["type"] == "message" {
            let message = &record["message"];
            let role = if message["visibility"] == "llm_only" {
                "system"
            } else {
                message["role"].as_str().unwrap_or("system")
            };
            let mut content = message["content"].clone();
            for block in content.as_array_mut().into_iter().flatten() {
                if block["type"] == "tool_result" {
                    if let Some(id) = block.get("toolUseId").cloned() {
                        block["tool_use_id"] = id;
                    }
                    if let Some(error) = block.get("isError").cloned() {
                        block["is_error"] = error;
                    }
                }
            }
            if role == "user" {
                self.turn = format!("request:{}", string(&record["id"]));
            }
            let role = if matches!(role, "user" | "assistant" | "system") {
                role
            } else {
                "system"
            };
            self.content(
                end,
                role,
                &content,
                &ts,
                (role == "assistant").then_some(if message["openaiPhase"] == "final_answer" {
                    "final"
                } else {
                    "progress"
                }),
            )?;
        } else {
            // Unknown native metadata is visible without pretending it is an assistant/tool result.
            self.emit(
                end,
                "system",
                record.to_string(),
                &ts,
                json!({"native_type":record["type"]}),
            );
        }
        for event in &mut self.events[first..] {
            event.message["native_id"] = record["id"].clone();
            event.message["native_type"] = record["type"].clone();
        }
        Ok(())
    }
}
