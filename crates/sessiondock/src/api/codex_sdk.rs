//! HTTP transport for the Codex SDK mode (docs/codex-sdk.md). Handlers only
//! resolve the page's opaque session UID to a native Codex thread id and call
//! `codex_sdk::CodexSdk`; the Hub forwards every route by that `uid`.
use std::{convert::Infallible, path::Path, sync::Arc, time::Duration};

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::{
    codex_sdk::{CodexSdk, RpcError},
    error::ApiError,
    state::AppState,
};

pub(super) fn service(state: &AppState) -> Result<Arc<CodexSdk>, ApiError> {
    state.codex_sdk.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "codex_sdk_disabled",
            "此机器未启用 Codex SDK 模式",
        )
    })
}

pub(super) fn rpc(error: RpcError) -> ApiError {
    let message = error.to_string();
    match error {
        RpcError::Unavailable(_) => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "codex_sdk_unavailable",
            message,
        ),
        RpcError::Storage(_) => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "codex_sdk_storage",
            message,
        ),
        RpcError::Disconnected => {
            ApiError::new(StatusCode::BAD_GATEWAY, "codex_sdk_disconnected", message)
        }
        // The rollout's single-writer lock: another runtime (a terminal that
        // did not join the shared daemon) owns this thread.
        RpcError::Remote {
            message: ref remote,
            ..
        } if remote.contains("active writer") => ApiError::new(
            StatusCode::CONFLICT,
            "codex_sdk_thread_busy",
            "该会话正被另一个独立运行的 Codex 进程占用（不是通过共享 app-server 启动的终端）；请先退出它，或在终端里重新接管该会话。",
        ),
        RpcError::Remote { .. } => {
            ApiError::new(StatusCode::BAD_GATEWAY, "codex_sdk_rejected", message)
        }
        RpcError::NoActiveTurn => ApiError::new(StatusCode::CONFLICT, "codex_sdk_idle", message),
        RpcError::UnknownRequest => {
            ApiError::new(StatusCode::CONFLICT, "codex_sdk_request_gone", message)
        }
    }
}

/// Resolve the selected Codex view through inventory ownership and native identity.
/// A child uses its own thread; only an unpersisted main view uses a start receipt.
pub(super) async fn thread_id(
    state: &AppState,
    uid: &str,
    agent: &str,
) -> Result<String, ApiError> {
    if agent.is_empty()
        && let Some(thread) = state
            .codex_sdk
            .as_ref()
            .and_then(|sdk| sdk.pending_thread(uid))
    {
        return Ok(thread);
    }
    let uid = uid.to_string();
    let agent = agent.to_string();
    let scope = state
        .reader
        .run(move |sessions| sessions.native_scope(&uid, &agent))
        .await?;
    if scope.source != "codex" || scope.session_id.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "codex_sdk_unsupported",
            "SDK 模式只支持 Codex 会话",
        ));
    }
    Ok(scope.session_id)
}

#[derive(Deserialize)]
pub struct Target {
    uid: String,
    #[serde(default)]
    agent: String,
}

#[derive(Deserialize)]
pub struct Create {
    request_id: String,
    cwd: String,
    #[serde(default)]
    create_cwd: bool,
    model: Option<String>,
    effort: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<Create>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let cwd = match super::lifecycle::prepare_cwd(body.cwd, body.create_cwd).await? {
        super::lifecycle::PreparedCwd::Ready(cwd) => cwd,
        super::lifecycle::PreparedCwd::Missing(cwd) => {
            return Ok(Json(
                json!({"needs_create":true,"cwd":cwd,"error":"启动目录不存在"}),
            ));
        }
    };
    let mut params = json!({"cwd":cwd,"ephemeral":false});
    if let Some(model) = body.model.filter(|s| !s.is_empty()) {
        params["model"] = json!(model);
    }
    if let Some(effort) = body.effort.filter(|s| !s.is_empty()) {
        params["config"] = json!({"model_reasoning_effort":effort});
    }
    let result = sdk.create(&body.request_id, params).await.map_err(rpc)?;
    let path = result["thread"]["path"]
        .as_str()
        .ok_or_else(|| rpc(RpcError::Disconnected))?;
    let uid = crate::sessions::uid_for("codex", Path::new(path));
    state.reader.store.invalidate_inventory();
    let session = sdk.pending().into_iter().find(|row| row["uid"] == uid);
    Ok(Json(
        json!({"ok":true,"uid":uid,"session":session,"cwd":cwd,"thread_id":result["thread"]["id"],"raw":result}),
    ))
}

pub async fn discard(
    State(state): State<AppState>,
    Json(body): Json<Target>,
) -> Result<Json<Value>, ApiError> {
    service(&state)?.discard(&body.uid).await.map_err(rpc)?;
    Ok(Json(json!({"ok":true})))
}

pub async fn open(
    State(state): State<AppState>,
    Json(body): Json<Target>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let thread = thread_id(&state, &body.uid, &body.agent).await?;
    let summary = sdk.open(&thread).await.map_err(rpc)?;
    Ok(Json(
        json!({"ok": true, "thread_id": thread, "raw": summary}),
    ))
}

pub async fn events(
    State(state): State<AppState>,
    Query(query): Query<Target>,
) -> Result<Response, ApiError> {
    let sdk = service(&state)?;
    let thread = thread_id(&state, &query.uid, &query.agent).await?;
    let attached = sdk.attach(&thread).await.map_err(rpc)?;
    let shutdown = state.shutdown.clone();
    let stream = async_stream::stream! {
        let guard = attached.guard;
        let mut events = attached.events;
        yield Ok::<_, Infallible>(Event::default().event("sdk").data(attached.snapshot.to_string()));
        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = heartbeat.tick() => {
                    yield Ok(Event::default().event("heartbeat").data("{}"));
                }
                received = events.recv() => match received {
                    Ok(text) => {
                        let reset = text.contains("\"method\":\"bridge/reset\"");
                        yield Ok(Event::default().event("sdk").data(&*text));
                        if reset { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // A page that fell behind reloads instead of guessing.
                        yield Ok(Event::default().event("sdk").data(
                            json!({"seq": 0, "method": "bridge/reset", "raw": {"reason": "lagged"}}).to_string()));
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
        drop(guard);
    };
    let mut response = Sse::new(stream).into_response();
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    response
        .headers_mut()
        .insert("cache-control", "no-cache".parse().unwrap());
    Ok(response)
}

#[derive(Deserialize)]
pub struct Turn {
    request_id: Option<String>,
    uid: String,
    #[serde(default)]
    agent: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    attachments: Vec<Value>,
    #[serde(default)]
    quotes: Vec<Value>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
}

pub async fn turn(
    State(state): State<AppState>,
    Json(body): Json<Turn>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    if body.text.trim().is_empty() && body.attachments.is_empty() && body.quotes.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "codex_sdk_empty",
            "消息内容为空",
        ));
    }
    let thread = thread_id(&state, &body.uid, &body.agent).await?;
    let attachments = if body.attachments.is_empty() {
        Vec::new()
    } else {
        let conversations = state.conversations.as_ref().ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_IMPLEMENTED,
                "conversation_disabled",
                "附件暂存未配置",
            )
        })?;
        let identity = conversations
            .identity(&body.uid)
            .await
            .map_err(super::conversation::error)?;
        conversations
            .publish(&identity, &body.attachments)
            .await
            .map_err(super::conversation::error)?
    };
    let text = crate::conversation::build_prompt(&body.text, &attachments, &body.quotes);
    let mut input = Vec::new();
    if !text.is_empty() {
        input.push(json!({"type":"text","text":text,"text_elements":[]}));
    }
    for attachment in &attachments {
        if attachment["kind"] == "image" {
            input.push(json!({"type":"localImage","path":attachment["path"]}));
        }
    }
    let request_id = match body.request_id {
        Some(id) => id,
        None => crate::conversation::random_id().map_err(super::conversation::error)?,
    };
    let result = sdk
        .send(&thread, request_id, text, input, body.model, body.effort)
        .await
        .map_err(rpc)?;
    Ok(Json(json!({"ok": true, "raw": result})))
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueueAction {
    Cancel,
    Steer,
    Retry,
}

#[derive(Deserialize)]
pub struct QueueInput {
    uid: String,
    #[serde(default)]
    agent: String,
    id: String,
    action: QueueAction,
}

pub async fn queue_action(
    State(state): State<AppState>,
    Json(body): Json<QueueInput>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let thread = thread_id(&state, &body.uid, &body.agent).await?;
    let action = match body.action {
        QueueAction::Cancel => "cancel",
        QueueAction::Steer => "steer",
        QueueAction::Retry => "retry",
    };
    let raw = tokio::spawn(async move { sdk.queue_action(&thread, &body.id, action).await })
        .await
        .map_err(|_| rpc(RpcError::Disconnected))?
        .map_err(rpc)?;
    Ok(Json(json!({"ok":true,"raw":raw})))
}

pub async fn interrupt(
    State(state): State<AppState>,
    Json(body): Json<Target>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let thread = thread_id(&state, &body.uid, &body.agent).await?;
    let result = sdk.interrupt(&thread).await.map_err(rpc)?;
    Ok(Json(json!({"ok": true, "raw": result})))
}

#[derive(Deserialize)]
pub struct Answer {
    uid: String,
    #[serde(default)]
    agent: String,
    key: String,
    result: Value,
}

pub async fn respond(
    State(state): State<AppState>,
    Json(body): Json<Answer>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let thread = thread_id(&state, &body.uid, &body.agent).await?;
    sdk.respond(&thread, &body.key, body.result)
        .await
        .map_err(rpc)?;
    Ok(Json(json!({"ok": true})))
}

pub async fn limits(
    State(state): State<AppState>,
    Query(query): Query<Target>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let _ = &query.uid;
    let value = sdk.rate_limits().await.map_err(rpc)?;
    Ok(Json(json!({"ok": true, "raw": value})))
}

#[derive(Deserialize)]
pub struct Redeem {
    #[allow(dead_code)]
    uid: String,
    #[serde(default)]
    credit_id: Option<String>,
    idempotency_key: String,
}

pub async fn reset_credit(
    State(state): State<AppState>,
    Json(body): Json<Redeem>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    if body.idempotency_key.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "codex_sdk_empty",
            "缺少兑换幂等键",
        ));
    }
    let outcome = sdk
        .consume_reset(body.credit_id.as_deref(), &body.idempotency_key)
        .await
        .map_err(rpc)?;
    // The daemon says to read the windows again instead of inferring them.
    let limits = sdk.rate_limits().await.map_err(rpc)?;
    Ok(Json(
        json!({"ok": true, "raw": {"outcome": outcome, "limits": limits}}),
    ))
}

pub async fn models(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    // Models are per daemon. The Hub accepts either uid or an explicit node.
    let value = sdk.models().await.map_err(rpc)?;
    Ok(Json(json!({"ok": true, "raw": value})))
}
