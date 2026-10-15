//! Structured server log: one JSON object per line on stderr (journald keeps
//! it under the service unit). See `docs/logging.md`.
//!
//! Lines carry `ts`, `level`, `event` and the caller's fields. Callers pass
//! metadata only: never credentials, conversation text, native records or
//! request bodies. The request layer logs method, path without the query
//! string, status, duration and the page's trace/page ids.

use std::{
    io::Write,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Instant, SystemTime},
};

use axum::{extract::Request, http::HeaderMap, middleware::Next, response::Response};
use serde_json::{Map, Value, json};

/// Which requests the request layer logs (`SESSIONDOCK_LOG_REQUESTS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestLog {
    /// Every `/api` request.
    All,
    /// 5xx answers and requests slower than [`SLOW_MS`] (the default).
    Errors,
    Off,
}

/// A request at least this slow is logged in `errors` mode.
pub const SLOW_MS: u128 = 2000;

impl RequestLog {
    pub fn from_env() -> Self {
        match std::env::var("SESSIONDOCK_LOG_REQUESTS").as_deref() {
            Ok("all") => Self::All,
            Ok("off") => Self::Off,
            _ => Self::Errors,
        }
    }
}

fn request_mode() -> RequestLog {
    static MODE: OnceLock<RequestLog> = OnceLock::new();
    *MODE.get_or_init(RequestLog::from_env)
}

const QUEUE_BYTES: usize = 8 * 1024 * 1024;
#[derive(Default)]
struct LogState {
    bytes: AtomicUsize,
    dropped: AtomicU64,
    errors: AtomicU64,
    written: AtomicU64,
    stop: AtomicBool,
    done: AtomicBool,
}
struct Logger {
    tx: mpsc::SyncSender<Vec<u8>>,
    state: Arc<LogState>,
}
static LOGGER: OnceLock<Logger> = OnceLock::new();

fn logger() -> &'static Logger {
    LOGGER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(1024);
        let state = Arc::new(LogState::default());
        let worker = state.clone();
        let spawned = std::thread::Builder::new().name("sessiondock-log".into()).spawn(move || {
            let mut last_summary = Instant::now();
            let mut reported = (0, 0);
            loop {
                match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(bytes) => {
                        worker.bytes.fetch_sub(bytes.len(), Ordering::Relaxed);
                        if std::io::stderr().lock().write_all(&bytes).is_err() {
                            worker.errors.fetch_add(1, Ordering::Relaxed);
                        } else { worker.written.fetch_add(1, Ordering::Relaxed); }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {},
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                let totals = (worker.dropped.load(Ordering::Relaxed), worker.errors.load(Ordering::Relaxed));
                if totals != reported && last_summary.elapsed().as_secs() >= 60 {
                    last_summary = Instant::now();
                    let summary = format!("{}\n", json!({"ts": crate::audit::query::rfc3339(SystemTime::now()),
                        "level":"warn", "event":"log.delivery", "dropped": totals.0, "write_errors": totals.1,
                        "cumulative": true}));
                    // Only acknowledge the cumulative summary after a successful write.
                    if std::io::stderr().lock().write_all(summary.as_bytes()).is_ok() { reported = totals; }
                    else { worker.errors.fetch_add(1, Ordering::Relaxed); }
                }
                if worker.stop.load(Ordering::Acquire) && worker.bytes.load(Ordering::Relaxed) == 0 { break; }
            }
            worker.done.store(true, Ordering::Release);
        });
        if spawned.is_err() { state.done.store(true, Ordering::Release); }
        Logger {tx, state}
    })
}

pub fn snapshot() -> Value {
    let Some(logger) = LOGGER.get() else {
        return json!({"enabled": false});
    };
    let state = &logger.state;
    json!({"enabled":true, "queued_bytes":state.bytes.load(Ordering::Relaxed),
        "dropped":state.dropped.load(Ordering::Relaxed), "write_errors":state.errors.load(Ordering::Relaxed),
        "written":state.written.load(Ordering::Relaxed)})
}

/// No join: a blocked external stderr sink must never own process shutdown.
pub async fn shutdown() -> bool {
    let Some(logger) = LOGGER.get() else {
        return true;
    };
    logger.state.stop.store(true, Ordering::Release);
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !logger.state.done.load(Ordering::Acquire) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok()
}

/// Write one line. `fields` must be an object; other values are wrapped.
pub fn emit(level: &str, event: &str, fields: Value) {
    let mut line = Map::new();
    line.insert(
        "ts".into(),
        json!(crate::audit::query::rfc3339(SystemTime::now())),
    );
    line.insert("level".into(), json!(level));
    line.insert("event".into(), json!(event));
    match fields {
        Value::Object(fields) => {
            for (key, value) in fields {
                line.entry(key).or_insert(value);
            }
        }
        Value::Null => {}
        other => {
            line.insert("data".into(), other);
        }
    }
    let mut bytes = Value::Object(line).to_string().into_bytes();
    bytes.push(b'\n');
    let logger = logger();
    let length = bytes.len();
    if logger.state.stop.load(Ordering::Acquire)
        || logger
            .state
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |used| {
                used.checked_add(length).filter(|v| *v <= QUEUE_BYTES)
            })
            .is_err()
    {
        logger.state.dropped.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if logger.tx.try_send(bytes).is_err() {
        logger.state.bytes.fetch_sub(length, Ordering::Relaxed);
        logger.state.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn info(event: &str, fields: Value) {
    emit("info", event, fields);
}

pub fn warn(event: &str, fields: Value) {
    emit("warn", event, fields);
}

pub fn error(event: &str, fields: Value) {
    emit("error", event, fields);
}

/// The error `code` of an [`crate::error::ApiError`] answer, for the request log.
#[derive(Clone, Copy)]
pub struct ErrorCode(pub &'static str);

/// Identifier headers the page already sends; at most 64 printable bytes.
pub(crate) fn id_header(headers: &HeaderMap, name: &str) -> Option<String> {
    let value = headers.get(name)?.to_str().ok()?;
    (!value.is_empty() && value.len() <= 64 && value.bytes().all(|b| b.is_ascii_graphic()))
        .then(|| value.to_owned())
}

/// Request layer for the `/api` routers.
pub async fn requests(request: Request, next: Next) -> Response {
    let mode = request_mode();
    if mode == RequestLog::Off || !request.uri().path().starts_with("/api/") {
        return next.run(request).await;
    }
    let method = request.method().to_string();
    let path = request.uri().path().to_owned();
    let trace = id_header(request.headers(), "x-sessiondock-trace");
    let page = id_header(request.headers(), "x-sessiondock-page");
    let started = Instant::now();
    let response = next.run(request).await;
    let ms = started.elapsed().as_millis();
    let status = response.status();
    let failed = status.is_server_error();
    if mode == RequestLog::Errors && !failed && ms < SLOW_MS {
        return response;
    }
    let mut fields = json!({"method": method, "path": path, "status": status.as_u16(), "ms": ms});
    if let Some(code) = response.extensions().get::<ErrorCode>() {
        fields["code"] = json!(code.0);
    }
    if let Some(trace) = trace {
        fields["trace"] = json!(trace);
    }
    if let Some(page) = page {
        fields["page"] = json!(page);
    }
    let level = if failed {
        "error"
    } else if ms >= SLOW_MS || status.is_client_error() {
        "warn"
    } else {
        "info"
    };
    emit(level, "http.request", fields);
    response
}
