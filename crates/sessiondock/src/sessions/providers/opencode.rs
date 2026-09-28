//! OpenCode 2 message rows from the SessionDock mirror (`sessions::opencode`).
//!
//! `user` rows carry the prompt (the mirror moves inline images into a
//! `content` array); `assistant` rows carry ordered `reasoning`, `text` and
//! `tool` items, a tool's call and result together in `state`. `synthetic`,
//! `idle`, `model-switched`, `agent-switched`, `location-switched` and
//! `compaction` rows are OpenCode bookkeeping.

use std::path::Path;

use serde_json::{Value, json};

use super::{Parser, clip, image_content, normalized, string};

/// View metadata from the mirrored session row; an untitled session is named
/// by its first prompt, like its list row (`index::summary::opencode`).
pub(super) fn metadata(
    path: &Path,
    summary: &Value,
    records: &[(Value, u64)],
    fallback: &str,
) -> Value {
    let session = &summary["session"];
    let nonempty = |value: &Value| {
        value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let sid = nonempty(&session["id"]).unwrap_or_else(|| {
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let title = nonempty(&session["title"])
        .or_else(|| {
            records
                .iter()
                .find(|(record, _)| record["type"] == "user")
                .and_then(|(record, _)| nonempty(&record["data"]["text"]))
        })
        .unwrap_or_else(|| crate::sessions::index::summary::opencode::UNTITLED.to_owned());
    let stamp = |value: &Value| match normalized(value) {
        Value::Null => json!(fallback),
        value => value,
    };
    json!({"sid": sid, "title": clip(&title), "cwd": session["directory"],
        "created": stamp(&session["time_created"]), "updated": stamp(&session["time_updated"]),
        "model": session["model"]["id"], "branch": session["agent"]})
}

impl Parser<'_> {
    pub(super) fn opencode(&mut self, record: &Value, end: u64) -> Result<(), String> {
        let kind = record["type"].as_str().unwrap_or("");
        let data = &record["data"];
        let ts = match normalized(&data["time"]["created"]) {
            Value::Null => normalized(&record["time_created"]),
            ts => ts,
        };
        match kind {
            "user" => {
                let (text, media) = image_content::parts_with_media(
                    &record["content"],
                    self.media.as_ref(),
                    &mut self.skipped,
                )?;
                self.turn = format!("message:{}", string(&record["id"]));
                if !text.trim().is_empty() || !media.is_empty() {
                    self.emit_media(end, "user", text, &ts, json!({}), media)?;
                }
            }
            "assistant" => {
                let items = data["content"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                let last_text = items.iter().rposition(|item| item["type"] == "text");
                let finished = data["finish"] == "stop";
                for (index, item) in items.iter().enumerate() {
                    match item["type"].as_str().unwrap_or("") {
                        "reasoning" => {
                            let text = string(&item["text"]);
                            if !text.trim().is_empty() {
                                self.emit(end, "thinking", text, &ts, json!({}));
                            }
                        }
                        "text" => {
                            let text = string(&item["text"]);
                            if !text.trim().is_empty() {
                                let phase = if finished && Some(index) == last_text {
                                    "final"
                                } else {
                                    "progress"
                                };
                                self.emit(end, "assistant", text, &ts, json!({"phase": phase}));
                            }
                        }
                        "tool" => self.opencode_tool(end, item, &ts)?,
                        other => self.skipped.note("OpenCode 内容类型", other),
                    }
                }
                // A user interruption keeps what was said so far as the
                // turn's last progress, marked interrupted (like Codex's
                // `turn_aborted`); it is not an error to show.
                if data["finish"] == "error" && data["error"]["type"] == "aborted" {
                    for event in self.events.iter_mut().rev() {
                        if event.message["role"] == "assistant"
                            && event.message["turn_id"] == self.turn
                        {
                            if event.message["phase"] != "final" {
                                event.message["interrupted"] = json!(true);
                                event.message["interrupt_reason"] = json!("本轮在最终答复前被中断");
                            }
                            break;
                        }
                    }
                }
                // A provider failure ends the turn with no content.
                if data["finish"] == "error"
                    && data["error"]["type"] != "aborted"
                    && let Some(message) = data["error"]["message"].as_str()
                {
                    self.emit(
                        end,
                        "assistant",
                        format!("[OpenCode 错误] {message}"),
                        &ts,
                        json!({"phase": "final"}),
                    );
                }
            }
            // A turn that failed before any reply leaves only this marker; a
            // failure the assistant row already explained is not repeated.
            "idle"
                if data["outcome"] == "failed"
                    && !self.events.last().is_some_and(|event| {
                        event.message["text"]
                            .as_str()
                            .is_some_and(|text| text.starts_with("[OpenCode"))
                    }) =>
            {
                self.emit(
                    end,
                    "assistant",
                    "[OpenCode] 本轮失败，没有产生回复",
                    &ts,
                    json!({"phase": "final"}),
                )
            }
            "synthetic" | "idle" | "model-switched" | "agent-switched" | "location-switched"
            | "compaction" => {}
            other => self.skipped.note("OpenCode 记录类型", other),
        }
        Ok(())
    }

    fn opencode_tool(&mut self, end: u64, item: &Value, ts: &Value) -> Result<(), String> {
        let id = string(&item["id"]);
        let name = match item["name"].as_str() {
            Some(name) if !name.is_empty() => name,
            _ => "tool",
        };
        let state = &item["state"];
        self.tool(end, name, &id, &state["input"], ts)?;
        let error = state["status"] == "error";
        let content = match &state["content"] {
            Value::Array(items) if !items.is_empty() => Value::Array(items.clone()),
            _ if error => json!([{"type": "text", "text": string(&state["error"])}]),
            _ => return Ok(()),
        };
        self.output(
            end,
            &id,
            &json!({"content": content, "isError": error}),
            error,
            ts,
        )
    }
}
