//! Metadata-only headless notifications refer to the selected SID's native
//! transcript. Never render their transport fields as conversational content.
use super::{Live, Turn, project, transport::ProcessConfig};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    io::SeekFrom,
    sync::Arc,
    time::SystemTime,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

fn records(bytes: &[u8]) -> impl DoubleEndedIterator<Item = Value> + '_ {
    // Ignore a trailing partial record; a later native write retries it.
    bytes
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|line| line.ends_with(b"\n"))
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
}

pub(super) async fn model(process: &ProcessConfig, sid: &str) -> Option<String> {
    const TAIL_BYTES: u64 = 65_536;
    let path = process.transcript_path(sid);
    let mut file = tokio::fs::File::open(&path).await.ok()?;
    let len = file.metadata().await.ok()?.len();
    if len > TAIL_BYTES {
        if file.seek(SeekFrom::Start(len - TAIL_BYTES)).await.is_ok() {
            let mut tail = Vec::with_capacity(TAIL_BYTES as usize);
            if file.read_to_end(&mut tail).await.is_ok() {
                let slice = match tail.iter().position(|&b| b == b'\n') {
                    Some(pos) => &tail[pos + 1..],
                    None => &[],
                };
                if let Some(found) = crate::sessions::agy::transcript_model(slice) {
                    return Some(found);
                }
            }
        }
        file.seek(SeekFrom::Start(0)).await.ok()?;
    }
    let mut bytes = Vec::with_capacity(len as usize);
    file.read_to_end(&mut bytes).await.ok()?;
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

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum StepKind {
    UserInput,
    SystemMessage,
    PlannerResponse,
}

impl StepKind {
    fn from_step_type(step_type: Option<&str>) -> Self {
        match step_type {
            Some("user_input") => Self::UserInput,
            Some("system_message") => Self::SystemMessage,
            _ => Self::PlannerResponse,
        }
    }

    fn from_transcript_type(record_type: Option<&str>) -> Option<Self> {
        match record_type {
            Some("USER_INPUT") => Some(Self::UserInput),
            Some("SYSTEM_MESSAGE") => Some(Self::SystemMessage),
            Some("PLANNER_RESPONSE") => Some(Self::PlannerResponse),
            _ => None,
        }
    }

    fn transcript_type(self) -> &'static str {
        match self {
            Self::UserInput => "USER_INPUT",
            Self::SystemMessage => "SYSTEM_MESSAGE",
            Self::PlannerResponse => "PLANNER_RESPONSE",
        }
    }
}

#[derive(Default)]
pub(super) struct Pending {
    steps: BTreeMap<u64, Value>,
    /// `(step_index, kind)` -> `(byte_offset, byte_len)` of the first
    /// completed transcript line for that step. Storing only 16-byte spans
    /// keeps memory tiny across long sessions while allowing re-resolution.
    spans: HashMap<(u64, StepKind), (u64, usize)>,
    scanned_offset: u64,
    /// `(line_offset, raw_part_len, fnv1a64)` of the last complete line scanned.
    last_anchor: Option<(u64, usize, u64)>,
    file_id: Option<(u64, u64)>,
    stamp: Option<(u64, Option<SystemTime>)>,
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn file_identity(meta: &std::fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        (0, 0)
    }
}

fn index_transcript_line(
    line: &[u8],
    line_offset: u64,
    out: &mut HashMap<(u64, StepKind), (u64, usize)>,
) {
    if line.is_empty() {
        return;
    }
    let Ok(record) = serde_json::from_slice::<Value>(line) else {
        return;
    };
    if record["status"] != "DONE" {
        return;
    }
    let Some(kind) = StepKind::from_transcript_type(record["type"].as_str()) else {
        return;
    };
    let Some(index) = record["step_index"].as_u64() else {
        return;
    };
    out.entry((index, kind))
        .or_insert((line_offset, line.len()));
}

fn index_complete_lines(
    complete: &[u8],
    mut offset: u64,
    out: &mut HashMap<(u64, StepKind), (u64, usize)>,
) -> Option<(u64, usize, u64)> {
    let mut last_anchor = None;
    for part in complete.split_inclusive(|&b| b == b'\n') {
        let line = part.strip_suffix(b"\n").unwrap_or(part);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        index_transcript_line(line, offset, out);
        last_anchor = Some((offset, part.len(), fnv1a64(part)));
        offset += part.len() as u64;
    }
    last_anchor
}

async fn read_validated_record(
    file: &mut tokio::fs::File,
    index: u64,
    kind: StepKind,
    offset: u64,
    len: usize,
) -> Option<Value> {
    file.seek(SeekFrom::Start(offset)).await.ok()?;
    let mut line = vec![0u8; len];
    file.read_exact(&mut line).await.ok()?;
    let record = serde_json::from_slice::<Value>(&line).ok()?;
    if record["status"] != "DONE"
        || record["step_index"].as_u64() != Some(index)
        || record["type"].as_str() != Some(kind.transcript_type())
    {
        return None;
    }
    Some(record)
}

impl Pending {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    fn reset_index(&mut self) {
        self.scanned_offset = 0;
        self.spans.clear();
        self.last_anchor = None;
    }

    async fn verify_anchor(&self, file: &mut tokio::fs::File) -> bool {
        let Some((offset, len, expected_hash)) = self.last_anchor else {
            return self.scanned_offset == 0;
        };
        if file.seek(SeekFrom::Start(offset)).await.is_err() {
            return false;
        }
        let mut buf = vec![0u8; len];
        if file.read_exact(&mut buf).await.is_err() {
            return false;
        }
        fnv1a64(&buf) == expected_hash
    }

    async fn scan_from_current_offset(&mut self, file: &mut tokio::fs::File) -> bool {
        if file
            .seek(SeekFrom::Start(self.scanned_offset))
            .await
            .is_err()
        {
            return false;
        }
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).await.is_err() {
            return false;
        }
        // Ignore a trailing partial record; a later native write changes len and retries it.
        if let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') {
            let complete = &buf[..=last_nl];
            let base_offset = self.scanned_offset;
            self.scanned_offset += complete.len() as u64;
            if let Some(anchor) = index_complete_lines(complete, base_offset, &mut self.spans) {
                self.last_anchor = Some(anchor);
            }
        }
        true
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
        let Ok(mut file) = tokio::fs::File::open(&path).await else {
            return false;
        };
        let Ok(meta) = file.metadata().await else {
            return false;
        };
        let id = file_identity(&meta);
        let stamp = (meta.len(), meta.modified().ok());
        if self.file_id == Some(id) && self.stamp == Some(stamp) {
            return false;
        }
        if self.file_id != Some(id) || meta.len() < self.scanned_offset {
            self.file_id = Some(id);
            self.reset_index();
        } else if self.scanned_offset > 0 && !self.verify_anchor(&mut file).await {
            self.reset_index();
        }
        if meta.len() > self.scanned_offset && !self.scan_from_current_offset(&mut file).await {
            return false;
        }
        self.stamp = Some(stamp);
        let mut assistant = false;
        let mut model = None;
        let mut rescanned = false;
        loop {
            let candidates: Vec<(u64, StepKind, u64, usize)> = self
                .steps
                .iter()
                .filter_map(|(&index, step)| {
                    let kind = StepKind::from_step_type(step["step_type"].as_str());
                    let &(offset, len) = self.spans.get(&(index, kind))?;
                    Some((index, kind, offset, len))
                })
                .collect();
            let mut need_rescan = false;
            for (index, kind, offset, len) in candidates {
                let Some(record) = read_validated_record(&mut file, index, kind, offset, len).await
                else {
                    need_rescan = true;
                    break;
                };
                let mut step = self.steps.remove(&index).unwrap();
                if kind == StepKind::UserInput {
                    if let Some(value) = crate::sessions::agy::record_model(&record) {
                        model = Some(value);
                    }
                    continue;
                }
                let text = record["content"].as_str().unwrap_or_default();
                if kind == StepKind::SystemMessage {
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
            if need_rescan && !rescanned {
                rescanned = true;
                self.reset_index();
                if !self.scan_from_current_offset(&mut file).await {
                    self.stamp = None;
                    break;
                }
                continue;
            }
            if need_rescan {
                self.stamp = None;
            }
            break;
        }
        live.observe_model(model);
        assistant
    }
}
