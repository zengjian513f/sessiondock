//! Metadata-only headless notifications refer to the selected SID's native
//! transcript. Never render their transport fields as conversational content.
use super::{Live, Turn, project, transport::ProcessConfig};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc, time::SystemTime};

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
            Some("agent_response" | "system_message")
        ) && step["state"] == "DONE"
            && !step["text_delta"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
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
        // Ignore a trailing partial record; a later native write retries it.
        for line in bytes
            .split_inclusive(|byte| *byte == b'\n')
            .filter(|line| line.ends_with(b"\n"))
        {
            let Ok(record) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            let Some(index) = record["step_index"].as_u64() else {
                continue;
            };
            let Some(step) = self.steps.get(&index) else {
                continue;
            };
            let expected = if step["step_type"] == "system_message" {
                "SYSTEM_MESSAGE"
            } else {
                "PLANNER_RESPONSE"
            };
            if record["type"] != expected || record["status"] != "DONE" {
                continue;
            }
            let mut step = self.steps.remove(&index).unwrap();
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
        assistant
    }
}
