//! Agy's complete, system-generated transcript (not the truncated transcript).
use serde_json::{Value, json};

use super::{Parser, clip, envelopes, normalized, string};

/// Agy's exact system injection envelope; ordinary user/code tags are never
/// passed here. Unrecognized or incomplete layouts stay verbatim.
fn system_message(text: &str) -> (String, Value) {
    const INTRO: &str = "The following is a <SYSTEM_MESSAGE> not actually sent by the user. It is provided by the system as important information to pay attention to.\n\n";
    let wrapped = text.strip_prefix(INTRO).unwrap_or(text);
    let body = envelopes::whole(wrapped, "SYSTEM_MESSAGE")
        .map(str::trim)
        .unwrap_or(text);
    let header = body
        .strip_prefix("[Message] timestamp=")
        .and_then(|rest| rest.split_once(" sender="))
        .and_then(|(timestamp, rest)| {
            rest.split_once(" priority=")
                .map(|(sender, rest)| (timestamp, sender, rest))
        })
        .and_then(|(timestamp, sender, rest)| {
            rest.split_once(" content=")
                .map(|(priority, content)| (timestamp, sender, priority, content))
        });
    match header {
        Some((timestamp, sender, priority, content)) => (
            content.to_owned(),
            json!({
                "native_type":"SYSTEM_MESSAGE", "counted":false, "system_timestamp":timestamp,
                "system_sender":sender, "system_priority":priority,
            }),
        ),
        None => (
            body.to_owned(),
            json!({"native_type":"SYSTEM_MESSAGE", "counted":false}),
        ),
    }
}

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
                let content = string(&record["content"]);
                let (text, tail) =
                    crate::sessions::agy::user_envelope(&content).unwrap_or((&content, ""));
                let parts = envelopes::fields(tail);
                let metadata: Vec<_> = parts
                    .iter()
                    .flatten()
                    .filter(|(tag, _)| *tag == "ADDITIONAL_METADATA")
                    .map(|(tag, body)| json!({"tag":tag,"text":body.trim()}))
                    .collect();
                if !text.trim().is_empty() || !media.is_empty() {
                    self.emit_media(
                        end,
                        "user",
                        text.to_owned(),
                        &ts,
                        json!({"native_type":"USER_INPUT", "native_metadata":metadata}),
                        media,
                    )?;
                }
                if let Some(parts) = parts {
                    for (tag, body) in parts {
                        if tag != "ADDITIONAL_METADATA" && !body.trim().is_empty() {
                            self.emit(
                                end,
                                "system",
                                body.trim(),
                                &ts,
                                json!({"native_type":tag,"counted":false}),
                            );
                        }
                    }
                } else if !tail.trim().is_empty() {
                    self.emit(
                        end,
                        "system",
                        tail.trim(),
                        &ts,
                        json!({"native_type":"AGY_USER_METADATA","counted":false}),
                    );
                }
            }
            "SYSTEM_MESSAGE" => {
                let (text, extra) = system_message(&string(&record["content"]));
                if !text.trim().is_empty() || !media.is_empty() {
                    self.emit_media(end, "system", text, &ts, extra, media)?;
                }
            }
            "PLANNER_RESPONSE" => {
                let calls = record["tool_calls"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let thinking = string(&record["thinking"]);
                if !thinking.trim().is_empty() {
                    self.emit(
                        end,
                        "thinking",
                        thinking,
                        &ts,
                        json!({"native_type":"PLANNER_RESPONSE"}),
                    );
                }
                let text = string(&record["content"]);
                if !text.trim().is_empty() || !media.is_empty() {
                    self.emit_media(
                        end,
                        "assistant",
                        text,
                        &ts,
                        json!({"native_type":"PLANNER_RESPONSE", "phase":if calls.is_empty() && record["status"] == "DONE" { "final" } else { "progress" }}),
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
                        json!({"native_type":"PLANNER_RESPONSE", "name":name,"changes":null}),
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
                        json!({"native_type":"ERROR_MESSAGE", "phase":"progress","error":true}),
                    );
                }
            }
            other => {
                let content = string(&record["content"]);
                if !content.is_empty() || !media.is_empty() {
                    // Generic/unknown steps publish their own content. The
                    // transcript has no documented cross-step call ID, so do
                    // not attach a result to an unrelated call by position.
                    self.emit_media(end, "tool_result", content, &ts,
                        json!({"native_type":other,"name":other,"error":record["status"]=="ERROR" || record["error"].is_string()}), media)?;
                } else {
                    self.skipped.note("Agy 记录类型", other);
                }
            }
        }
        Ok(())
    }
}
