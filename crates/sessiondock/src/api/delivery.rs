//! Read-only projection of an explicitly opened SessionDock-owned ledger, plus
//! the four send routes driven by the reliable-send executor
//! (Claude and Codex main sessions).
//! Native scope is resolved before querying; acknowledgment input never comes
//! from HTTP.
use std::convert::Infallible;

use axum::{
    Extension, Json,
    body::{Body, Bytes},
    extract::{Query, State, rejection::JsonRejection, rejection::QueryRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    delivery::{
        claude::Scope,
        driver::PageLease,
        executor::{DeliveryExecutor, Reply, SendRequest},
        service,
    },
    error::ApiError,
    state::AppState,
};

impl From<service::Error> for ApiError {
    fn from(error: service::Error) -> Self {
        let (status, code, message) = match error {
            service::Error::Closed => (
                StatusCode::SERVICE_UNAVAILABLE,
                "delivery_closed",
                "发送账本读取服务已关闭",
            ),
            service::Error::Engine(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "delivery_unavailable",
                "发送账本无法确认已提交状态；请检查独立账本，不会返回空队列代替错误",
            ),
            _ => (
                StatusCode::SERVICE_UNAVAILABLE,
                "delivery_read_failed",
                "发送账本读取失败",
            ),
        };
        Self::new(status, code, message)
    }
}

pub async fn outbox(
    State(state): State<AppState>,
    query: Result<Query<Vec<(String, String)>>, QueryRejection>,
) -> Result<Response, ApiError> {
    let service = state.delivery.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "delivery_disabled",
            "发送账本读取未启用：需要显式初始化并配置独立开发目录",
        )
    })?;
    let Query(query) = query.map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_outbox_query",
            "需要有效的会话 UID 和完整子代理 ID",
        )
    })?;
    if state.shutdown.is_cancelled() {
        return Err(service::Error::Closed.into());
    }
    // First `uid` query value (`""` if absent); ignores every other
    // query field, including the legacy agent/debug selectors.
    let uid = query
        .into_iter()
        .find_map(|(key, value)| (key == "uid").then_some(value))
        .unwrap_or_default();
    let native = match state
        .reader
        .run({
            let uid = uid.clone();
            move |store| store.native_scope(&uid, "")
        })
        .await
    {
        Ok(native) => native,
        Err(error)
            if matches!(
                error.status,
                StatusCode::BAD_REQUEST
                    | StatusCode::NOT_FOUND
                    | StatusCode::CONFLICT
                    | StatusCode::NOT_IMPLEMENTED
            ) =>
        {
            return Ok(empty_outbox());
        }
        Err(error) => return Err(error),
    };
    let encoded = match native.source.as_str() {
        "codex" => service.codex_outbox(native.uid, native.agent_id).await?,
        "claude" => {
            service
                .claude_outbox(Scope {
                    uid: native.uid,
                    session_id: native.session_id,
                    agent_id: native.agent_id,
                })
                .await?
        }
        _ => {
            return Ok(empty_outbox());
        }
    };
    Ok(body(encoded))
}

fn empty_outbox() -> Response {
    Json(json!({
        "outbox": [],
        "outbox_version": {"epoch": "none", "revision": 0},
    }))
    .into_response()
}

fn body(encoded: service::EncodedJson) -> Response {
    let bytes = encoded.into_bytes();
    let length = bytes.len();
    let stream = async_stream::stream! {
        let mut bytes = Bytes::from(bytes);
        while !bytes.is_empty() {
            let size = bytes.len().min(32 * 1024);
            yield Ok::<_, Infallible>(bytes.split_to(size));
        }
    };
    let mut response = Body::from_stream(stream).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/json; charset=utf-8".parse().unwrap(),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, length.into());
    response
}

// ---- send routes ------------------------------------------

fn executor(state: &AppState) -> Result<&std::sync::Arc<DeliveryExecutor>, ApiError> {
    state.executor.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "delivery_send_disabled",
            "可靠发送未启用：需要已初始化的发送账本目录和显式终端传输目录",
        )
    })
}

/// The page's own instance lease, sent by the legacy composer under the Rust
/// `outbox` capability. Absent or stale, the executor claims for itself.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct LeaseBody {
    page: String,
    token: String,
    instance_id: String,
    /// Present when the page's console holds a launch (pending) lease.
    launch_id: String,
}

impl LeaseBody {
    fn page_lease(self) -> Option<PageLease> {
        if self.token.is_empty() || self.page.is_empty() || self.instance_id.is_empty() {
            return None;
        }
        if self.token.len() != 64 {
            return None;
        }
        Some(PageLease {
            page: self.page.chars().take(128).collect(),
            token: self.token,
            instance_id: self.instance_id,
            launch_id: (!self.launch_id.is_empty()).then_some(self.launch_id),
        })
    }
}

/// Queue-message body. Unknown fields (`activity`, `cursor`,
/// `page_id`, diagnostics) are accepted and ignored; `media` is
/// accepted and retained as opaque outbox preview metadata. Uploaded paths are
/// already embedded in `text` by the composer.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct SendBody {
    #[serde(deserialize_with = "python_request_id")]
    uid: String,
    #[serde(deserialize_with = "python_request_id")]
    agent: String,
    #[serde(deserialize_with = "python_request_id")]
    name: String,
    #[serde(deserialize_with = "python_request_id")]
    text: String,
    media: Value,
    #[serde(deserialize_with = "python_request_id")]
    request_id: String,
    #[serde(deserialize_with = "python_request_id")]
    overwrite_draft: String,
    lease: LeaseBody,
    #[serde(deserialize_with = "python_request_id")]
    _build: String,
}

/// Send/retry/discard use `str(value or "")`. Keep JSON raw here so
/// integer precision and object insertion order survive only this conversion;
/// other native JSON parsers keep their existing number/ordering behavior.
fn python_request_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let raw = <Box<serde_json::value::RawValue>>::deserialize(deserializer)?;
    python_id_value(raw.get(), false).map_err(serde::de::Error::custom)
}

pub(crate) fn python_id_value(raw: &str, nested: bool) -> serde_json::Result<String> {
    let raw = raw.trim();
    Ok(match raw.as_bytes()[0] {
        b'n' => if nested { "None" } else { "" }.into(),
        b'f' => if nested { "False" } else { "" }.into(),
        b't' => "True".into(),
        b'"' => {
            let value: String = serde_json::from_str(raw)?;
            if nested {
                python_string_repr(&value)
            } else {
                value
            }
        }
        b'[' => {
            let items: Vec<Box<serde_json::value::RawValue>> = serde_json::from_str(raw)?;
            if items.is_empty() && !nested {
                String::new()
            } else {
                let items = items
                    .iter()
                    .map(|item| python_id_value(item.get(), true))
                    .collect::<serde_json::Result<Vec<_>>>()?;
                format!("[{}]", items.join(", "))
            }
        }
        b'{' => {
            struct Object;
            impl<'de> serde::de::Visitor<'de> for Object {
                type Value = Vec<(String, String)>;
                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("a JSON object")
                }
                fn visit_map<M: serde::de::MapAccess<'de>>(
                    self,
                    mut map: M,
                ) -> Result<Self::Value, M::Error> {
                    let mut entries = Vec::new();
                    let mut positions = std::collections::HashMap::<String, usize>::new();
                    while let Some((key, value)) =
                        map.next_entry::<String, Box<serde_json::value::RawValue>>()?
                    {
                        let value =
                            python_id_value(value.get(), true).map_err(serde::de::Error::custom)?;
                        if let Some(index) = positions.get(&key) {
                            entries[*index] = (key, value);
                        } else {
                            positions.insert(key.clone(), entries.len());
                            entries.push((key, value));
                        }
                    }
                    Ok(entries)
                }
            }
            let entries = serde::Deserializer::deserialize_map(
                &mut serde_json::Deserializer::from_str(raw),
                Object,
            )?;
            if entries.is_empty() && !nested {
                String::new()
            } else {
                format!(
                    "{{{}}}",
                    entries
                        .iter()
                        .map(|(key, value)| format!("{}: {value}", python_string_repr(key)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        _ => {
            // JSON's integer spelling is already the decimal representation,
            // except -0. Floating point uses the fixed/scientific threshold.
            if !raw.contains(['.', 'e', 'E']) {
                if raw == "0" || raw == "-0" {
                    if nested { "0" } else { "" }.into()
                } else {
                    raw.into()
                }
            } else {
                let number: f64 = raw
                    .parse()
                    .map_err(<serde_json::Error as serde::de::Error>::custom)?;
                if number == 0.0 && !nested {
                    String::new()
                } else if !number.is_finite() {
                    number.to_string()
                } else {
                    let scientific = format!("{number:e}");
                    let (mantissa, exponent) =
                        scientific.split_once('e').expect("finite float exponent");
                    let exponent: i32 = exponent.parse().expect("formatted exponent");
                    if (-4..16).contains(&exponent) {
                        let mut text = number.to_string();
                        if !text.contains('.') {
                            text.push_str(".0");
                        }
                        text
                    } else {
                        format!("{mantissa}e{exponent:+03}")
                    }
                }
            }
        }
    })
}

fn python_string_repr(value: &str) -> String {
    use std::{fmt::Write, sync::OnceLock};
    static NON_PRINTABLE: OnceLock<regex::Regex> = OnceLock::new();
    let non_printable = NON_PRINTABLE.get_or_init(|| regex::Regex::new(r"[\p{C}\p{Z}]").unwrap());
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut result = String::new();
    result.push(quote);
    for ch in value.chars() {
        match ch {
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            ch if ch == quote => {
                result.push('\\');
                result.push(ch);
            }
            ch if ch != ' ' && non_printable.is_match(ch.encode_utf8(&mut [0; 4])) => {
                let number = ch as u32;
                if number <= 0xff {
                    write!(result, "\\x{number:02x}").unwrap();
                } else if number <= 0xffff {
                    write!(result, "\\u{number:04x}").unwrap();
                } else {
                    write!(result, "\\U{number:08x}").unwrap();
                }
            }
            ch => result.push(ch),
        }
    }
    result.push(quote);
    result
}

fn bad_body(error: JsonRejection, what: &str) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            format!("{what}请求体过大"),
        );
    }
    ApiError::new(StatusCode::BAD_REQUEST, "bad_body", "bad body")
}

fn reply(reply: Reply) -> Response {
    (
        StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        [(header::CACHE_CONTROL, "no-store")],
        Json(reply.body),
    )
        .into_response()
}

fn stale_build(state: &AppState, build: &str, hub: bool) -> Option<Response> {
    // Text writes from a tab that outlived a deployment are rejected before
    // touching the terminal; old clients surface the error and keep their text.
    (!hub && build != state.assets.build).then(|| {
        (
            StatusCode::CONFLICT,
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({
                "error": "页面版本已过期，请重新加载整个网页后再发送",
                "code": "stale_build",
                "reload": true,
                "build": state.assets.build,
            })),
        )
            .into_response()
    })
}

fn shutting_down(state: &AppState) -> Result<(), ApiError> {
    if state.shutdown.is_cancelled() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "shutdown",
            "服务正在关闭",
        ));
    }
    Ok(())
}

fn python_media_list(value: Value) -> Result<Vec<Value>, ApiError> {
    match value {
        Value::Null | Value::Bool(false) => Ok(Vec::new()),
        Value::Array(items) => Ok(items),
        Value::String(text) if text.is_empty() => Ok(Vec::new()),
        Value::String(text) => Ok(text.chars().map(|ch| Value::String(ch.into())).collect()),
        Value::Object(entries) if entries.is_empty() => Ok(Vec::new()),
        Value::Object(entries) => Ok(entries
            .into_iter()
            .map(|(key, _)| Value::String(key))
            .collect()),
        Value::Number(number) if number.as_f64() == Some(0.0) => Ok(Vec::new()),
        _ => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "bad_body",
            "bad body",
        )),
    }
}

pub async fn send(
    State(state): State<AppState>,
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    body: Result<Json<SendBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let executor = executor(&state)?;
    let Json(body) = body.map_err(|error| bad_body(error, "发送"))?;
    if let Some(response) = stale_build(&state, &body._build, hub.is_some()) {
        return Ok(response);
    }
    let media = python_media_list(body.media)?;
    if body.text.len() > 1024 * 1024 {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "delivery_text_too_large",
            "消息正文超过 1 MiB",
        ));
    }
    shutting_down(&state)?;
    Ok(reply(
        executor
            .send(SendRequest {
                uid: body.uid,
                agent: body.agent,
                name: body.name,
                text: body.text,
                media,
                request_id: body.request_id,
                overwrite_draft: body.overwrite_draft,
                page_lease: body.lease.page_lease(),
            })
            .await,
    ))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct DraftStatusBody {
    #[serde(deserialize_with = "python_request_id")]
    uid: String,
    #[serde(deserialize_with = "python_request_id")]
    name: String,
    lease: LeaseBody,
}

pub async fn draft_status(
    State(state): State<AppState>,
    body: Result<Json<DraftStatusBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let executor = executor(&state)?;
    let Json(body) = body.map_err(|error| bad_body(error, "草稿检测"))?;
    shutting_down(&state)?;
    Ok(reply(
        executor
            .draft_status(&body.uid, &body.name, body.lease.page_lease())
            .await,
    ))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct RetryBody {
    #[serde(deserialize_with = "python_request_id")]
    uid: String,
    #[serde(deserialize_with = "python_request_id")]
    id: String,
    #[serde(deserialize_with = "python_request_id")]
    overwrite_draft: String,
    lease: LeaseBody,
    #[serde(deserialize_with = "python_request_id")]
    _build: String,
}

pub async fn retry(
    State(state): State<AppState>,
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    body: Result<Json<RetryBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let executor = executor(&state)?;
    let Json(body) = body.map_err(|error| bad_body(error, "重试"))?;
    if let Some(response) = stale_build(&state, &body._build, hub.is_some()) {
        return Ok(response);
    }
    shutting_down(&state)?;
    Ok(reply(
        executor
            .retry(
                &body.uid,
                &body.id,
                &body.overwrite_draft,
                body.lease.page_lease(),
            )
            .await,
    ))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct DiscardBody {
    #[serde(deserialize_with = "python_request_id")]
    uid: String,
    #[serde(deserialize_with = "python_request_id")]
    id: String,
}

pub async fn discard(
    State(state): State<AppState>,
    body: Result<Json<DiscardBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let executor = executor(&state)?;
    let Json(body) = body.map_err(|error| bad_body(error, "移除"))?;
    shutting_down(&state)?;
    Ok(reply(executor.discard(&body.uid, &body.id).await))
}
