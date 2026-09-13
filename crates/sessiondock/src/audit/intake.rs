//! Request-shape validation and structured-metadata sanitization.
//!
//! Wire contract (compatible with the Python `_browser_audit` handler):
//! the body is one JSON object `{page_id|_page_id, uid, _build, _trace_id,
//! events:[{event, ts, uid, trace_id, request_id, connection_id, severity,
//! build, data, content}]}`. `content` and unknown keys are skipped by serde
//! without being allocated. Invalid events are skipped, not fatal, exactly as
//! in Python; only a structurally malformed body is rejected.

use std::{
    net::IpAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use serde_json::{Map, Value, json};

pub const EVENT_NAME_MAX: usize = 160;
pub const CLIENT_TS_MAX: usize = 64;
pub const UID_MAX: usize = 512;
pub const ID_MAX: usize = 128;
pub const SEVERITY_MAX: usize = 24;
/// Characters kept per string value inside `data`.
pub const STRING_MAX_CHARS: usize = 1024;
pub const DATA_DEPTH_MAX: usize = 8;
pub const ARRAY_MAX: usize = 256;
pub const OBJECT_KEYS_MAX: usize = 128;
/// Envelope bytes per event beyond the bounded `data` object: every id field
/// at its maximum plus JSON punctuation and server-added fields.
pub const EVENT_OVERHEAD: usize = 2048;

const SECRET_KEYS: [&str; 15] = [
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
    "token",
    "bearer",
    "credential",
    "credentials",
];

/// Structural rejections. Codes mirror the Python responses: a bad body is
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
    events: Vec<RawEvent>,
}

#[derive(Deserialize)]
struct RawEvent {
    #[serde(default)]
    event: Option<Value>,
    #[serde(default)]
    ts: Option<Value>,
    #[serde(default)]
    uid: Option<Value>,
    #[serde(default)]
    trace_id: Option<Value>,
    #[serde(default)]
    request_id: Option<Value>,
    #[serde(default)]
    connection_id: Option<Value>,
    #[serde(default)]
    severity: Option<Value>,
    #[serde(default)]
    build: Option<Value>,
    #[serde(default)]
    data: Option<Value>,
}

/// Optional string field: absent/null becomes empty, any other type or an
/// over-long value is an error.
fn text(value: &Option<Value>, max: usize) -> Result<&str, ()> {
    match value {
        None | Some(Value::Null) => Ok(""),
        Some(Value::String(text)) if text.len() <= max => Ok(text),
        _ => Err(()),
    }
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
    data_bytes: usize,
    sequence: &AtomicU64,
    now: SystemTime,
) -> Result<Prepared, Rejection> {
    let raw: RawBatch = serde_json::from_slice(body)
        .map_err(|_| Rejection::Malformed("需要包含 events 数组的 JSON 对象"))?;
    if raw.events.len() > max_events {
        return Err(Rejection::TooManyEvents);
    }
    let invalid = |_| Rejection::Malformed("批次字段类型或长度无效");
    let page_id = match text(&raw.page_id, ID_MAX).map_err(invalid)? {
        "" => text(&raw._page_id, ID_MAX).map_err(invalid)?,
        page => page,
    };
    let batch_uid = text(&raw.uid, UID_MAX).map_err(invalid)?;
    let batch_build = text(&raw._build, ID_MAX).map_err(invalid)?;
    let batch_trace = text(&raw._trace_id, ID_MAX).map_err(invalid)?;
    let received_at = rfc3339(now);
    let client = client.to_string();
    let mut bytes = Vec::new();
    let mut events = 0u32;
    let mut skipped = 0usize;
    let batch = Envelope {
        page_id,
        uid: batch_uid,
        build: batch_build,
        trace_id: batch_trace,
        received_at: &received_at,
        client: &client,
        data_bytes,
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
    data_bytes: usize,
}

fn record(item: &RawEvent, batch: &Envelope<'_>) -> Option<Value> {
    let name = match &item.event {
        Some(Value::String(name)) if valid_event_name(name) => name.as_str(),
        _ => return None,
    };
    let client_ts = match &item.ts {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(ts)) if ts.len() <= CLIENT_TS_MAX => Value::String(ts.clone()),
        _ => return None,
    };
    let uid = match text(&item.uid, UID_MAX).ok()? {
        "" => batch.uid,
        uid => uid,
    };
    let trace_id = match text(&item.trace_id, ID_MAX).ok()? {
        "" => batch.trace_id,
        trace => trace,
    };
    let request_id = text(&item.request_id, ID_MAX).ok()?;
    let connection_id = text(&item.connection_id, ID_MAX).ok()?;
    let build = match batch.build {
        "" => text(&item.build, ID_MAX).ok()?,
        build => build,
    };
    let severity = match text(&item.severity, SEVERITY_MAX).ok()? {
        "" => "info",
        severity => severity,
    };
    let data = match &item.data {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(data @ Value::Object(_)) => bounded_data(data, batch.data_bytes),
        _ => return None,
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

/// Sanitizes then enforces the serialized budget; an over-budget object is
/// replaced by a marker that keeps only its top-level key names.
pub fn bounded_data(data: &Value, data_bytes: usize) -> Value {
    let clean = sanitize(data, 0);
    let size = serde_json::to_vec(&clean)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    if size <= data_bytes {
        return clean;
    }
    let keys: Vec<&str> = clean
        .as_object()
        .map(|map| map.keys().take(32).map(String::as_str).collect())
        .unwrap_or_default();
    json!({"truncated": true, "bytes": size, "keys": keys})
}

/// Structured metadata only: redacts secret-looking keys, replaces absolute
/// filesystem paths, and bounds strings, arrays, objects and nesting depth.
pub fn sanitize(value: &Value, depth: usize) -> Value {
    if depth > DATA_DEPTH_MAX {
        return Value::String("<depth-limit>".into());
    }
    match value {
        Value::String(text) => Value::String(bounded_string(text)),
        Value::Array(items) => {
            let mut clean: Vec<Value> = items
                .iter()
                .take(ARRAY_MAX)
                .map(|item| sanitize(item, depth + 1))
                .collect();
            if items.len() > ARRAY_MAX {
                clean.push(json!({"truncated": true, "items": items.len()}));
            }
            Value::Array(clean)
        }
        Value::Object(map) => {
            let mut clean = Map::new();
            for (key, item) in map.iter().take(OBJECT_KEYS_MAX) {
                let key: String = key.chars().take(ID_MAX).collect();
                let item = if secret_key(&key) {
                    Value::String("<redacted>".into())
                } else {
                    sanitize(item, depth + 1)
                };
                clean.insert(key, item);
            }
            if map.len() > OBJECT_KEYS_MAX {
                clean.insert("<truncated-keys>".into(), json!(map.len()));
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

fn bounded_string(text: &str) -> String {
    if looks_like_absolute_path(text) {
        return "<path>".into();
    }
    let mut chars = text.char_indices();
    match chars.nth(STRING_MAX_CHARS) {
        None => text.to_owned(),
        Some((cut, _)) => format!("{}…", &text[..cut]),
    }
}

/// Whole-value heuristic for POSIX, home-relative, drive-letter and UNC paths.
/// A bare `/` or a page URL such as `/?sid=…` is not a filesystem path.
pub fn looks_like_absolute_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    match bytes {
        [b'/', next, ..] => !matches!(next, b'/' | b'?' | b'#' | b' '),
        [b'~', b'/', ..] => true,
        [b'\\', b'\\', ..] => true,
        [drive, b':', b'\\' | b'/', ..] => drive.is_ascii_alphabetic(),
        _ => false,
    }
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
