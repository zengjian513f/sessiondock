//! Structured server log: one JSON object per line on stderr (journald keeps
//! it under the service unit). See `docs/logging.md`.
//!
//! Lines carry `ts`, `level`, `event` and the caller's fields. Callers pass
//! metadata only: never credentials, conversation text, native records or
//! request bodies. The request layer logs method, path without the query
//! string, status, duration and the page's trace/page ids.

use std::sync::OnceLock;
use std::time::{Instant, SystemTime};
#[path = "../../runtime_log.rs"]
mod runtime;

use axum::{extract::Request, http::HeaderMap, middleware::Next, response::Response};
use serde_json::{Value, json};

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

pub fn snapshot() -> Value {
    runtime::snapshot()
}
pub async fn shutdown() -> bool {
    emit("info", "runtime.stopping", snapshot());
    tokio::task::spawn_blocking(runtime::shutdown)
        .await
        .unwrap_or(false)
}

pub fn emit(level: &str, event: &str, mut fields: Value) {
    let directory = match std::env::var_os("SESSIONDOCK_RUNTIME_LOG_DIR") {
        Some(value) if value == "-" => None,
        Some(value) => Some(std::path::PathBuf::from(value)),
        None => std::env::var_os("SESSIONDOCK_STATE_DIR")
            .map(|p| std::path::PathBuf::from(p).join("runtime-logs")),
    };
    runtime::init(directory, "server", json!({}));
    if !fields.is_object() {
        fields = json!({"data": fields});
    }
    fields["ts"] = json!(crate::audit::query::rfc3339(SystemTime::now()));
    runtime::emit(level, event, fields);
}

pub fn info(event: &str, fields: Value) {
    emit("info", event, fields);
}

/// Preserve each low-frequency state transition instead of folding distinct
/// requests/states into a five-second example. Never use for token/byte streams.
pub fn transition(level: &str, event: &str, mut fields: Value) {
    fields["aggregation"] = json!("none");
    emit(level, event, fields);
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
