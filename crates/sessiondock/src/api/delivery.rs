//! Read-only projection of an explicitly opened AgentHub-owned ledger, plus
//! (batch 31) the four Python send routes driven by the reliable-send executor
//! (Claude main sessions; Codex main sessions since batch 32).
//! Native scope is resolved before querying; acknowledgment input never comes
//! from HTTP.
use std::convert::Infallible;

use axum::{
    Json,
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
        engine,
        executor::{DeliveryExecutor, Reply, SendRequest},
        service,
    },
    error::ApiError,
    state::AppState,
};

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct OutboxQuery {
    uid: String,
    agent: String,
    /// `debug_run`: the page's view selector, appended to every `/api/`
    /// URL by the frontend; accepted and ignored here like Python.
    #[allow(dead_code)]
    debug_run: String,
}

impl From<service::Error> for ApiError {
    fn from(error: service::Error) -> Self {
        let (status, code, message) = match error {
            service::Error::Busy => (
                StatusCode::SERVICE_UNAVAILABLE,
                "delivery_busy",
                "发送账本读取繁忙，请稍后重试",
            ),
            service::Error::Closed => (
                StatusCode::SERVICE_UNAVAILABLE,
                "delivery_closed",
                "发送账本读取服务已关闭",
            ),
            service::Error::ResponseLimit => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "delivery_response_limit",
                "发送账本响应超过读取预算",
            ),
            service::Error::Engine(engine::Error::UnsupportedAgent) => (
                StatusCode::NOT_IMPLEMENTED,
                "delivery_agent_unsupported",
                "此来源尚不支持子代理发送账本",
            ),
            service::Error::Engine(engine::Error::UnsupportedMedia) => (
                StatusCode::NOT_IMPLEMENTED,
                "delivery_media_unsupported",
                "发送账本包含尚未迁移的媒体引用",
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
    query: Result<Query<OutboxQuery>, QueryRejection>,
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
    if query.uid.is_empty() || query.uid.len() > 256 || query.agent.len() > 256 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_outbox_query",
            "发送账本查询标识无效",
        ));
    }
    if state.shutdown.is_cancelled() {
        return Err(service::Error::Closed.into());
    }
    let uid = query.uid;
    let agent = query.agent;
    let native = state
        .reader
        .run({
            let (uid, agent) = (uid.clone(), agent.clone());
            move |store| store.native_scope(&uid, &agent)
        })
        .await?;
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
            return Err(ApiError::new(
                StatusCode::NOT_IMPLEMENTED,
                "delivery_source_unsupported",
                "此来源尚未实现发送账本",
            ));
        }
    };
    Ok(body(encoded))
}

fn body(encoded: service::EncodedJson) -> Response {
    let (bytes, guard) = encoded.into_parts();
    let length = bytes.len();
    let stream = async_stream::stream! {
        let _guard = guard;
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

// ---- batch 31: Python send routes ------------------------------------------

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
        if self.token.len() != 64
            || self.page.len() > 128
            || self.instance_id.len() > 128
            || self.launch_id.len() > 128
        {
            return None;
        }
        Some(PageLease {
            page: self.page,
            token: self.token,
            instance_id: self.instance_id,
            launch_id: (!self.launch_id.is_empty()).then_some(self.launch_id),
        })
    }
}

/// Python `_queue_message` body. Unknown fields (`activity`, `cursor`,
/// `page_id`, diagnostics) are accepted and ignored as in Python; `media` is
/// accepted but rejected explicitly because uploaded attachments are not
/// resolvable in this backend yet.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct SendBody {
    uid: String,
    name: String,
    text: String,
    media: Value,
    request_id: String,
    overwrite_draft: String,
    lease: LeaseBody,
    _build: String,
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

fn stale_build(state: &AppState, build: &str) -> Option<Response> {
    // Python rejects text writes from a tab that outlived a deployment before
    // touching the terminal; old clients surface the error and keep their text.
    (build != state.assets.build).then(|| {
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

fn bounded(uid: &str, name: &str, id: &str) -> Result<(), ApiError> {
    if uid.is_empty() || uid.len() > 256 || name.len() > 128 || id.len() > 128 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_send",
            "会话 UID、终端名或消息 ID 无效",
        ));
    }
    Ok(())
}

pub async fn send(
    State(state): State<AppState>,
    body: Result<Json<SendBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let executor = executor(&state)?;
    let Json(body) = body.map_err(|error| bad_body(error, "发送"))?;
    if let Some(response) = stale_build(&state, &body._build) {
        return Ok(response);
    }
    bounded(&body.uid, &body.name, &body.request_id)?;
    if body.media.as_array().is_some_and(|media| !media.is_empty())
        || (!body.media.is_null() && !body.media.is_array())
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "delivery_media_unsupported",
            "此后端尚不能解析上传附件；请去掉附件后再发送",
        ));
    }
    if body.text.len() > 256 * 1024 {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "delivery_text_too_large",
            "消息正文超过 256 KiB",
        ));
    }
    shutting_down(&state)?;
    Ok(reply(
        executor
            .send(SendRequest {
                uid: body.uid,
                name: body.name,
                text: body.text,
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
    uid: String,
    name: String,
    lease: LeaseBody,
}

pub async fn draft_status(
    State(state): State<AppState>,
    body: Result<Json<DraftStatusBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let executor = executor(&state)?;
    let Json(body) = body.map_err(|error| bad_body(error, "草稿检测"))?;
    bounded(&body.uid, &body.name, "")?;
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
    uid: String,
    id: String,
    overwrite_draft: String,
    lease: LeaseBody,
    _build: String,
}

pub async fn retry(
    State(state): State<AppState>,
    body: Result<Json<RetryBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let executor = executor(&state)?;
    let Json(body) = body.map_err(|error| bad_body(error, "重试"))?;
    if let Some(response) = stale_build(&state, &body._build) {
        return Ok(response);
    }
    bounded(&body.uid, "", &body.id)?;
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
    uid: String,
    id: String,
}

pub async fn discard(
    State(state): State<AppState>,
    body: Result<Json<DiscardBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let executor = executor(&state)?;
    let Json(body) = body.map_err(|error| bad_body(error, "移除"))?;
    bounded(&body.uid, "", &body.id)?;
    shutting_down(&state)?;
    Ok(reply(executor.discard(&body.uid, &body.id).await))
}
