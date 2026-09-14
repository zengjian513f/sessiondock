//! Request-shape validation and structured-metadata sanitization.
//!
//! Wire contract:
//! the body is one JSON object `{page_id|_page_id, uid, _build, _trace_id,
//! events:[{event, ts, uid, trace_id, request_id, connection_id, severity,
//! build, data, content}]}`. `content` and unknown keys are not retained.
//! Invalid events are skipped, not fatal; only a
//! structurally malformed body is rejected.

use std::{
    net::IpAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use serde_json::{Map, Value, json};

pub const EVENT_NAME_MAX: usize = 160;
pub const UID_MAX: usize = 512;
pub const ID_MAX: usize = 128;
pub const SEVERITY_MAX: usize = 24;
pub const DATA_DEPTH_MAX: usize = 12;

const SECRET_KEYS: [&str; 11] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "api-key",
    "apikey",
    "password",
    "passwd",
    "secret",
    "access-token",
    "refresh-token",
];

/// Structural rejections. A bad body is
/// `400`, too many events is `413`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    Malformed(&'static str),
    TooManyEvents,
}

/// Immutable JSONL snapshot handed to the writer. `bytes` already ends with a
/// newline per event and is never modified after construction.
pub struct Batch {
    pub bytes: Box<[u8]>,
    pub events: u32,
}

pub struct Prepared {
    pub batch: Option<Batch>,
    pub skipped: usize,
}

#[derive(Deserialize)]
struct RawBatch {
    #[serde(default)]
    page_id: Option<Value>,
    #[serde(default)]
    _page_id: Option<Value>,
    #[serde(default)]
    uid: Option<Value>,
    #[serde(default)]
    _build: Option<Value>,
    #[serde(default)]
    _trace_id: Option<Value>,
    events: Vec<Value>,
}

fn text(value: Option<&Value>, max: usize) -> String {
    let text = match value {
        None | Some(Value::Null | Value::Bool(false)) => String::new(),
        Some(Value::Bool(true)) => "True".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    };
    text.chars().take(max).collect()
}

pub fn valid_event_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= EVENT_NAME_MAX
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-'))
}

pub fn prepare(
    body: &[u8],
    client: IpAddr,
    max_events: usize,
    sequence: &AtomicU64,
    now: SystemTime,
) -> Result<Prepared, Rejection> {
    let raw: RawBatch = serde_json::from_slice(body)
        .map_err(|_| Rejection::Malformed("需要包含 events 数组的 JSON 对象"))?;
    if raw.events.len() > max_events {
        return Err(Rejection::TooManyEvents);
    }
    let page = text(raw.page_id.as_ref(), ID_MAX);
    let page_id = if page.is_empty() {
        text(raw._page_id.as_ref(), ID_MAX)
    } else {
        page
    };
    let batch_uid = text(raw.uid.as_ref(), UID_MAX);
    let batch_build = text(raw._build.as_ref(), ID_MAX);
    let batch_trace = text(raw._trace_id.as_ref(), ID_MAX);
    let received_at = rfc3339(now);
    let client = client.to_string();
    let mut bytes = Vec::new();
    let mut events = 0u32;
    let mut skipped = 0usize;
    let batch = Envelope {
        page_id: &page_id,
        uid: &batch_uid,
        build: &batch_build,
        trace_id: &batch_trace,
        received_at: &received_at,
        client: &client,
    };
    for item in &raw.events {
        let Some(mut record) = record(item, &batch) else {
            skipped += 1;
            continue;
        };
        record["seq"] = json!(sequence.fetch_add(1, Ordering::Relaxed));
        serde_json::to_writer(&mut bytes, &record).expect("Value serializes");
        bytes.push(b'\n');
        events += 1;
    }
    Ok(Prepared {
        batch: (events > 0).then(|| Batch {
            bytes: bytes.into_boxed_slice(),
            events,
        }),
        skipped,
    })
}

/// Batch-level fields every event inherits when it does not carry its own.
struct Envelope<'a> {
    page_id: &'a str,
    uid: &'a str,
    build: &'a str,
    trace_id: &'a str,
    received_at: &'a str,
    client: &'a str,
}

fn record(item: &Value, batch: &Envelope<'_>) -> Option<Value> {
    let item = item.as_object()?;
    let name = text(item.get("event"), EVENT_NAME_MAX);
    if !valid_event_name(&name) {
        return None;
    }
    let client_ts = item.get("ts").cloned().unwrap_or(Value::Null);
    let own_uid = text(item.get("uid"), UID_MAX);
    let uid = if own_uid.is_empty() {
        batch.uid
    } else {
        &own_uid
    };
    let own_trace = text(item.get("trace_id"), ID_MAX);
    let trace_id = if own_trace.is_empty() {
        batch.trace_id
    } else {
        &own_trace
    };
    let request_id = text(item.get("request_id"), ID_MAX);
    let connection_id = text(item.get("connection_id"), ID_MAX);
    let build = match batch.build {
        "" => text(item.get("build"), ID_MAX),
        build => build.to_owned(),
    };
    let severity = match text(item.get("severity"), SEVERITY_MAX) {
        value if value.is_empty() => "info".to_owned(),
        value => value,
    };
    let data = match item.get("data") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(data @ Value::Object(_)) => sanitize(data, 0),
        _ => Value::Object(Map::new()),
    };
    Some(json!({
        "seq": 0,
        "received_at": batch.received_at,
        "event": format!("browser.{name}"),
        "severity": severity,
        "client_ts": client_ts,
        "page_id": batch.page_id,
        "uid": uid,
        "source": uid.split_once(':').map_or("", |(source, _)| source),
        "trace_id": trace_id,
        "request_id": request_id,
        "connection_id": connection_id,
        "build": build,
        "client": batch.client,
        "data": data,
    }))
}

/// Redact secret keys and stop recursion after depth 12.
pub fn sanitize(value: &Value, depth: usize) -> Value {
    if depth > DATA_DEPTH_MAX {
        return Value::String("<depth-limit>".into());
    }
    match value {
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| sanitize(item, depth + 1)).collect())
        }
        Value::Object(map) => {
            let mut clean = Map::new();
            for (key, item) in map {
                let item = if secret_key(key) {
                    Value::String("<redacted>".into())
                } else {
                    sanitize(item, depth + 1)
                };
                clean.insert(key.clone(), item);
            }
            Value::Object(clean)
        }
        other => other.clone(),
    }
}

fn secret_key(key: &str) -> bool {
    let folded: String = key
        .chars()
        .map(|c| {
            if c == '_' {
                '-'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    SECRET_KEYS.contains(&folded.as_str())
}

fn rfc3339(now: SystemTime) -> String {
    let nanos = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    chrono::DateTime::from_timestamp(
        (nanos / 1_000_000_000) as i64,
        (nanos % 1_000_000_000) as u32,
    )
    .unwrap_or_default()
    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn utc_date(now: SystemTime) -> String {
    let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    chrono::DateTime::from_timestamp(seconds as i64, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d")
        .to_string()
}
