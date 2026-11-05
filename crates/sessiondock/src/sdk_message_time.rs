//! Message times from native fields or the earliest backend transport receipt.
use serde_json::Value;

pub(crate) fn now() -> String {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    chrono::DateTime::from_timestamp(elapsed.as_secs() as i64, elapsed.subsec_nanos())
        .expect("current clock")
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub(crate) fn hosted(client: &sdkhost_client::Client, end: u64, live_start: u64) -> Option<String> {
    client
        .received_at(sdkhost_client::Stream::Stdout, end)
        .ok()
        .flatten()
        .and_then(|ms| chrono::DateTime::from_timestamp_millis(ms as i64))
        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        // An already running old host has no receipt index. Timestamp only
        // output produced after attachment, never its buffered history.
        .or_else(|| (end > live_start).then(now))
}

#[derive(Clone, Default)]
pub(crate) struct MessageTime {
    native: Option<String>,
    pub received: Option<String>,
}

impl MessageTime {
    pub fn new(frame: &Value, received: Option<String>) -> Self {
        Self {
            native: ["/ts", "/timestamp", "/created_at", "/_meta/kiro/timestamp"]
                .iter()
                .find_map(|key| frame.pointer(key).and_then(crate::sessions::text::norm_ts)),
            received,
        }
    }

    pub fn stamp(&self, item: &mut Value) {
        if item["ts"].is_null()
            && let Some(at) = &self.native
        {
            item["ts"] = Value::String(at.clone());
        }
        if item["received_at"].is_null()
            && let Some(at) = &self.received
        {
            item["received_at"] = Value::String(at.clone());
        }
    }
}

pub(crate) fn preserve(item: &mut Value, previous: &Value) {
    for key in ["ts", "timestamp", "received_at"] {
        if (key == "received_at" || item[key].is_null()) && !previous[key].is_null() {
            item[key] = previous[key].clone();
        }
    }
}
