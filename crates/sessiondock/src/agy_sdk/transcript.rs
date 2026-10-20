//! Metadata-only headless notifications refer to the selected SID's native
//! transcript. Never render their transport fields as conversational content.
use super::{Live, Turn, project, transport::ProcessConfig};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::SystemTime};

fn records(bytes: &[u8]) -> impl DoubleEndedIterator<Item = Value> + '_ {
    // Ignore a trailing partial record; a later native write retries it.
    bytes
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|line| line.ends_with(b"\n"))
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
}

pub(super) async fn model(process: &ProcessConfig, sid: &str) -> Option<String> {
    let bytes = tokio::fs::read(process.transcript_path(sid)).await.ok()?;
    crate::sessions::agy::transcript_model(&bytes)
}

/// Agy can retain an earlier stream error in its terminal result even after
/// native recovery produced a complete response. This is conflicting evidence,
/// not proof of SUCCESS and not a failed delivery.
pub(super) async fn conflicting_result(
    process: &ProcessConfig,
    live: &Live,
    turn: &Turn,
    result: &Value,
) -> Value {
    const STREAM_ERROR: &str =
        "The stream was interrupted. Please continue the task you were working on.";
    if result["status"] != "ERROR" || result["error"] != STREAM_ERROR {
        return json!({"conflict":false,"reason":"not_stream_error"});
    }
    let user_index = {
        let data = live.data.lock().unwrap();
        data.items
            .iter()
            .find(|item| item["request_id"] == turn.id && item["role"] == "user")
            .and_then(|item| item["native"]["step_index"].as_u64())
    };
    let Some(user_index) = user_index else {
        return json!({"conflict":false,"reason":"missing_user_step"});
    };
    let bytes = match tokio::fs::read(process.transcript_path(&live.sid)).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return json!({"conflict":false,"reason":"transcript_read_failed",
            "user_step":user_index,"io_kind":format!("{:?}",error.kind())});
        }
    };
    let mut error_seen = false;
    let mut final_response = None;
    let mut error_steps = Vec::new();
    let mut prior_error_step = None;
    let mut other_error_seen = false;
    let mut last_step = None;
    for record in records(&bytes) {
        let Some(index) = record["step_index"].as_u64() else {
            continue;
        };
        if index <= user_index {
            if record["type"] == "ERROR_MESSAGE" {
                prior_error_step = (record["error"] == STREAM_ERROR).then_some(index);
            }
            continue;
        }
        // Never borrow a later input's answer to resolve this turn.
        if record["type"] == "USER_INPUT" {
            return json!({"conflict":false,"reason":"later_user_input",
                "user_step":user_index,"later_user_step":record["step_index"].as_u64()});
        }
        last_step = record["step_index"].as_u64();
        final_response = None;
        if record["type"] == "ERROR_MESSAGE" && record["error"] == STREAM_ERROR {
            error_seen = true;
            error_steps.push(record["step_index"].as_u64());
        } else if record["type"] == "ERROR_MESSAGE" {
            other_error_seen = true;
        } else if (error_seen || prior_error_step.is_some())
            && record["type"] == "PLANNER_RESPONSE"
            && record["status"] == "DONE"
        {
            final_response = record["content"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned);
        }
    }
    let matches = final_response.as_ref().is_some_and(|text| {
        result["response"]
            .as_str()
            .is_some_and(|response| response.trim_end().ends_with(text.trim_end()))
    });
    let historical_error =
        matches && !error_seen && !other_error_seen && prior_error_step.is_some();
    json!({"conflict":matches && error_seen && !other_error_seen,
        "historical_error":historical_error,
        "reason":if other_error_seen {"different_current_error"}
        else if historical_error {"historical_error_after_completed_response"}
        else if matches {"recovered_response_with_stale_error"}
        else if !error_seen && prior_error_step.is_none() {"missing_native_error"}
        else if final_response.is_none() {"missing_final_response"}
        else {"response_mismatch"},
        "user_step":user_index,"error_steps":error_steps,"prior_error_step":prior_error_step,"last_step":last_step,
        "final_response_bytes":final_response.as_ref().map(String::len),
        "response_suffix_matches":matches})
}

#[derive(Default)]
pub(super) struct Pending {
    steps: BTreeMap<u64, Value>,
    stamp: Option<(u64, Option<SystemTime>)>,
}

impl Pending {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn observe(&mut self, step: &Value) {
        if matches!(
            step["step_type"].as_str(),
            Some("user_input" | "agent_response" | "system_message")
        ) && step["state"] == "DONE"
            && (step["step_type"] == "user_input"
                || !step["text_delta"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()))
            && let Some(index) = step["step_index"].as_u64()
        {
            self.steps.insert(index, step.clone());
            self.stamp = None;
        }
    }

    pub async fn refresh(
        &mut self,
        process: &Arc<ProcessConfig>,
        live: &Live,
        turn: &Turn,
    ) -> bool {
        if self.steps.is_empty() {
            return false;
        }
        let path = process.transcript_path(&live.sid);
        let Ok(meta) = tokio::fs::metadata(&path).await else {
            return false;
        };
        let stamp = (meta.len(), meta.modified().ok());
        if self.stamp == Some(stamp) {
            return false;
        }
        let Ok(bytes) = tokio::fs::read(&path).await else {
            return false;
        };
        self.stamp = Some(stamp);
        let mut assistant = false;
        let mut model = None;
        for record in records(&bytes) {
            let Some(index) = record["step_index"].as_u64() else {
                continue;
            };
            let Some(step) = self.steps.get(&index) else {
                continue;
            };
            let expected = match step["step_type"].as_str() {
                Some("user_input") => "USER_INPUT",
                Some("system_message") => "SYSTEM_MESSAGE",
                _ => "PLANNER_RESPONSE",
            };
            if record["type"] != expected || record["status"] != "DONE" {
                continue;
            }
            let mut step = self.steps.remove(&index).unwrap();
            if expected == "USER_INPUT" {
                if let Some(value) = crate::sessions::agy::record_model(&record) {
                    model = Some(value);
                }
                continue;
            }
            let text = record["content"].as_str().unwrap_or_default();
            if expected == "SYSTEM_MESSAGE" {
                let (text, fields) = crate::sessions::agy::system_message(text);
                step["text"] = text.into();
                step["system_fields"] = fields;
            } else {
                assistant |= !text.is_empty();
                step["text"] = text.into();
            }
            step["transcript"] = record;
            project(live, turn, &step);
        }
        live.observe_model(model);
        assistant
    }
}
