//! HTTP/SSE transport shared by native SDK providers.
use crate::{error::ApiError, state::AppState};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{convert::Infallible, time::Duration};
use tokio::sync::broadcast;

fn failure(message: String) -> ApiError {
    let (status, code) = if message.contains("session_busy") || message.contains("独立 OpenCode")
    {
        (StatusCode::CONFLICT, "native_sdk_session_busy")
    } else {
        (StatusCode::BAD_GATEWAY, "native_sdk_error")
    };
    ApiError::new(status, code, message)
}

async fn target(state: &AppState, uid: &str, agent: &str) -> Result<Value, ApiError> {
    if agent.is_empty()
        && let Some(row) = state.native_sdks.receipt(uid)
    {
        return Ok(
            json!({"uid":uid,"session_id":row["sid"],"cwd":row["cwd"],"source":row["source"],"agent":""}),
        );
    }
    let uid = uid.to_owned();
    let agent = agent.to_owned();
    state.reader.run(move |store| {
        let scope = store.native_scope(&uid, &agent)?;
        let snapshot = store.snapshot(&uid, &agent)?;
        Ok(json!({"uid":uid,"session_id":scope.session_id,"cwd":snapshot.metadata()["cwd"],"source":scope.source,"agent":agent}))
    }).await
}

pub async fn call(
    State(state): State<AppState>,
    Path(method): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    tokio::spawn(execute(state, method, body))
        .await
        .map_err(|_| failure("SDK 请求执行失败".into()))?
}

async fn execute(
    state: AppState,
    method: String,
    mut body: Value,
) -> Result<Json<Value>, ApiError> {
    if !body.is_object() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "SDK 请求需要 JSON 对象",
        ));
    }
    if !matches!(
        method.as_str(),
        "open" | "create" | "turn" | "interrupt" | "respond" | "models" | "discard"
    ) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "没有这个 SDK 操作",
        ));
    }
    let target = if method == "create" {
        let cwd = body["cwd"].as_str().unwrap_or_default().to_owned();
        let cwd = match super::lifecycle::prepare_cwd(cwd, body["create_cwd"] == true).await? {
            super::lifecycle::PreparedCwd::Ready(cwd) => cwd,
            super::lifecycle::PreparedCwd::Missing(cwd) => {
                return Ok(Json(
                    json!({"needs_create":true,"cwd":cwd,"error":"启动目录不存在"}),
                ));
            }
        };
        json!({"source":body["source"],"cwd":cwd})
    } else {
        target(
            &state,
            body["uid"].as_str().unwrap_or_default(),
            body["agent"].as_str().unwrap_or_default(),
        )
        .await?
    };
    let source = target["source"].as_str().unwrap_or_default();
    if !state.native_sdks.capabilities()[source]
        .as_bool()
        .unwrap_or_default()
    {
        return Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "native_sdk_disabled",
            state.native_sdks.unavailable()[source]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("此机器未配置 {source} SDK 模式")),
        ));
    }
    let mut submission = if method == "turn" {
        let id = body["request_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .map(Ok)
            .unwrap_or_else(crate::conversation::random_id)
            .map_err(super::conversation::error)?;
        body["request_id"] = json!(id);
        Some(
            state
                .native_sdks
                .submission(&target, &id)
                .lock_owned()
                .await,
        )
    } else {
        None
    };
    if let Some(result) = submission
        .as_ref()
        .and_then(|submission| submission.result.clone())
    {
        return Ok(Json(response(result)));
    }
    if let Some(input) = submission
        .as_ref()
        .and_then(|submission| submission.input.clone())
    {
        body = input;
    } else if method == "turn" {
        if let Some(submission) = submission.as_mut() {
            submission.draft = Some(body.clone());
        }
        let attachments = body["attachments"].as_array().cloned().unwrap_or_default();
        let quotes = body["quotes"].as_array().cloned().unwrap_or_default();
        let published = if attachments.is_empty() {
            Vec::new()
        } else {
            let conversations = state
                .conversations
                .as_ref()
                .ok_or_else(|| ApiError::unavailable("附件暂存"))?;
            let identity = conversations
                .identity(target["uid"].as_str().unwrap_or_default())
                .await
                .map_err(super::conversation::error)?;
            conversations
                .publish(&identity, &attachments)
                .await
                .map_err(super::conversation::error)?
        };
        body["text"] = json!(crate::conversation::build_prompt(
            body["text"].as_str().unwrap_or_default(),
            &published,
            &quotes
        ));
        body["attachments"] = json!(published);
        if let Some(submission) = submission.as_mut() {
            submission.input = Some(body.clone());
        }
    }
    let result = state
        .native_sdks
        .call(source, &method, target.clone(), body)
        .await
        .map_err(failure)?;
    if let Some(submission) = submission.as_mut() {
        if target["agent"].as_str().unwrap_or_default().is_empty()
            && let Some(conversations) = &state.conversations
            && let Some(draft) = &submission.draft
        {
            conversations
                .sdk_accepted(
                    target["uid"].as_str().unwrap_or_default(),
                    draft["request_id"].as_str().unwrap_or_default(),
                    draft,
                    result.clone(),
                )
                .await
                .map_err(super::conversation::error)?;
        }
        submission.result = Some(result.clone());
    }
    if method == "create" {
        state.reader.store.invalidate_inventory();
        let mut result = result;
        result["ok"] = json!(true);
        return Ok(Json(result));
    }
    Ok(Json(response(result)))
}

fn response(result: Value) -> Value {
    let mut response = result.clone();
    if !response.is_object() {
        response = json!({});
    }
    response["ok"] = json!(true);
    response["raw"] = result.get("raw").cloned().unwrap_or(result);
    response
}

#[derive(Deserialize)]
pub struct Selected {
    uid: String,
    #[serde(default)]
    agent: String,
}

pub async fn events(
    State(state): State<AppState>,
    Query(query): Query<Selected>,
) -> Result<Response, ApiError> {
    let target = target(&state, &query.uid, &query.agent).await?;
    let source = target["source"].as_str().unwrap_or_default();
    state
        .native_sdks
        .call(source, "open", target.clone(), json!({}))
        .await
        .map_err(failure)?;
    let key = if query.agent.is_empty() {
        query.uid
    } else {
        format!("{}:{}", query.uid, query.agent)
    };
    let (snapshot, mut events) = state.native_sdks.subscribe(source, &key).map_err(failure)?;
    let shutdown = state.shutdown.clone();
    let stream = async_stream::stream! {
        yield Ok::<_,Infallible>(Event::default().event("sdk").data(snapshot.to_string()));
        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = heartbeat.tick() => yield Ok(Event::default().event("heartbeat").data("{}")),
                event = events.recv() => match event {
                    Ok(event) => yield Ok(Event::default().event("sdk").data(&*event)),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        yield Ok(Event::default().event("sdk").data(json!({"method":"bridge/reset","raw":{}}).to_string()));
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
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
