//! Agy's complete, system-generated transcript (not the truncated transcript).
use serde_json::{Value, json};

use super::{Parser, clip, normalized, string};

pub(super) fn metadata(summary: &Value, records: &[(Value, u64)], fallback: &str) -> Value {
    let session = &summary["session"];
    let title = session["title"]
        .as_str()
        .filter(|v| !v.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            records
                .iter()
                .find(|(v, _)| v["type"] == "USER_INPUT")
                .and_then(|(v, _)| v["content"].as_str())
                .map(crate::sessions::agy::user_text)
        })
        .unwrap_or_else(|| crate::sessions::index::summary::agy::UNTITLED.to_owned());
    let stamp = |v: &Value| match normalized(v) {
        Value::Null => json!(fallback),
        v => v,
    };
    json!({"sid": session["id"], "title": clip(&title), "cwd": session["directory"],
        "created": stamp(&session["time_created"]), "updated": stamp(&session["time_updated"]),
        "model": session["model"]["id"], "branch": session["agent"]})
}

impl Parser<'_> {
    pub(super) fn agy(&mut self, record: &Value, end: u64) -> Result<(), String> {
        let ts = normalized(&record["created_at"]);
        // The native format references media files by URI; it never embeds
        // image bytes. Keep normal selected-view file grants and validation.
        let mut media = Vec::new();
        for item in record["media"].as_array().into_iter().flatten() {
            if item["mime_type"]
                .as_str()
                .is_some_and(|mime| mime.starts_with("image/"))
            {
                if let Some(uri) = item["uri"].as_str()
                    && let Some(image) = crate::media::NativeImage::from_block(
                        &json!({"type":"image_url","image_url":{"url":uri}}),
                    )?
                {
                    media.push(image);
                }
            } else {
                self.skipped
                    .note("Agy 媒体类型", item["mime_type"].as_str().unwrap_or(""));
            }
        }
        match record["type"].as_str().unwrap_or("") {
            "USER_INPUT" => {
                self.turn = format!("step:{}", record["step_index"]);
                let text = crate::sessions::agy::user_text(&string(&record["content"]));
                if !text.trim().is_empty() || !media.is_empty() {
                    self.emit_media(end, "user", text, &ts, json!({}), media)?;
                }
            }
            "PLANNER_RESPONSE" => {
                let calls = record["tool_calls"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let thinking = string(&record["thinking"]);
                if !thinking.trim().is_empty() {
                    self.emit(end, "thinking", thinking, &ts, json!({}));
                }
                let text = string(&record["content"]);
                if !text.trim().is_empty() || !media.is_empty() {
                    self.emit_media(
                        end,
                        "assistant",
                        text,
                        &ts,
                        json!({"phase":if calls.is_empty() && record["status"] == "DONE" { "final" } else { "progress" }}),
                        media,
                    )?;
                }
                // The complete transcript documents tool_calls as native JSON.
                // Retain all fields instead of inventing call/result IDs or
                // interpreting provider-specific argument layouts as edits.
                for call in calls {
                    let name = call["name"].as_str().unwrap_or("Agy 工具调用");
                    self.emit(
                        end,
                        "tool",
                        string(call),
                        &ts,
                        json!({"name":name,"changes":null}),
                    );
                }
            }
            "ERROR_MESSAGE" => {
                let text = record["content"]
                    .as_str()
                    .or(record["error"].as_str())
                    .unwrap_or("");
                if !text.is_empty() {
                    self.emit(
                        end,
                        "assistant",
                        text,
                        &ts,
                        json!({"phase":"progress","error":true}),
                    );
                }
            }
            other => {
                let content = string(&record["content"]);
                if !content.is_empty() || !media.is_empty() {
                    // Tool and system steps publish their own content. The
                    // transcript has no documented cross-step call ID, so do
                    // not attach a result to an unrelated call by position.
                    self.emit_media(end, "tool_result", content, &ts,
                        json!({"name":other,"error":record["status"]=="ERROR" || record["error"].is_string()}), media)?;
                } else {
                    self.skipped.note("Agy 记录类型", other);
                }
            }
        }
        Ok(())
    }
}
