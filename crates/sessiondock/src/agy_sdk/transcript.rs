//! Metadata-only headless notifications refer to the selected SID's native
//! transcript. Never render their transport fields as conversational content.
use super::{Live, Turn, project, transport::ProcessConfig};
use serde_json::Value;
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
