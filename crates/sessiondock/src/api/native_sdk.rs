//! HTTP/SSE transport shared by native SDK providers.
use crate::{error::ApiError, state::AppState};
use axum::{
    Extension, Json,
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
    let (status, code) = if message.contains("kiro_sdk_session_busy") {
        (StatusCode::CONFLICT, "kiro_sdk_session_busy")
    } else if message.contains("session_busy") || message.contains("独立 OpenCode") {
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
        return Ok(state.native_sdks.canonical_target(
            &json!({"uid":uid,"session_id":row["sid"],"cwd":row["cwd"],"source":row["source"],"agent":"","sdk_fork":row["sdk_fork"]})
        ));
    }
    let uid = uid.to_owned();
    let agent = agent.to_owned();
    state.reader.run(move |store| {
        let scope = store.native_scope(&uid, &agent)?;
        let snapshot = store.snapshot(&uid, &agent)?;
        Ok(json!({"uid":uid,"session_id":scope.session_id,"cwd":snapshot.metadata()["cwd"],"source":scope.source,"agent":agent,
            "observed_model":snapshot.metadata()["model"]}))
    }).await.map(|target| state.native_sdks.canonical_target(&target))
}

pub async fn call(
    State(state): State<AppState>,
    Path(method): Path<String>,
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    tokio::spawn(async move {
        let continuation = if method == "create" {
            super::relations::sdk_continuation(
                &state,
                body["continuation_uid"].as_str().map(str::to_owned),
                serde_json::from_value(body["remote_continuation"].clone())
                    .map_err(|e| failure(e.to_string()))?,
                hub.is_some(),
            )
            .await?
        } else {
            None
        };
        let result = execute(state.clone(), method, body).await?;
        if let Some(uid) = result.0["uid"].as_str() {
            super::relations::record_sdk_continuation(&state, uid, continuation)?;
        }
        Ok(result)
    })
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
        "open"
            | "create"
            | "turn"
            | "steer"
            | "interrupt"
            | "stop"
            | "respond"
            | "models"
            | "rename"
            | "discard"
            | "usage"
            | "billing"
            | "settings"
            | "compact"
            | "fork"
            | "rewind_points"
            | "rewind"
            | "shell"
            | "queue_cancel"
            | "queue_retry"
            | "queue_update"
            | "revert"
            | "unrevert"
            | "revert_commit"
            | "agents"
            | "commands"
            | "command"
    ) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "没有这个 SDK 操作",
        ));
    }
    body.as_object_mut().unwrap().remove("images");
    body.as_object_mut().unwrap().remove("files");
    let target = if method == "models" && body["uid"].as_str().is_none() {
        // New-session model picker: always fetch fresh catalog, ignoring cache TTL.
        // Users opening the dialog after subscription changes expect to see new models.
        body["refresh"] = json!(true);
        let cwd = body["cwd"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or(std::env::current_dir().map_err(|e| failure(e.to_string()))?);
        json!({"source":body["source"],"cwd":cwd})
    } else if method == "create" {
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
        let uid = body["uid"].as_str().unwrap_or_default();
        let agent = body["agent"].as_str().unwrap_or_default();
        match target(&state, uid, agent).await {
            Ok(target) => target,
            Err(_error) if matches!(method.as_str(), "stop" | "discard") && !uid.is_empty() => {
                let source = body["source"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .or_else(|| uid.split_once(':').map(|(s, _)| s))
                    .unwrap_or_default();
                let sid = body["session_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .or_else(|| uid.split_once(':').map(|(_, id)| id))
                    .unwrap_or(uid);
                json!({"uid":uid,"session_id":sid,"cwd":body["cwd"],"source":source,"agent":agent})
            }
            Err(error) => return Err(error),
        }
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
    if method == "usage" && matches!(source, "agy" | "claude" | "kiro") {
        let quotas = if body["refresh"] == true {
            state.quotas.collect_source(source).await
        } else if body["refresh"] == "auto" {
            state.quotas.collect_cached(Some(source)).await
        } else {
            state.quotas.read().await
        };
        let mut result = state
            .native_sdks
            .call(source, &method, target.clone(), body.clone())
            .await
            .map_err(failure)?;
        result["quota"] = quotas["agents"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["source"] == source))
            .cloned()
            .unwrap_or(Value::Null);
        return Ok(Json(response(result)));
    }
    if matches!(method.as_str(), "queue_cancel" | "queue_retry")
        && (body["origin"] == "bridge"
            || body["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("bridge:")))
    {
        let action = if method == "queue_cancel" {
            "cancel"
        } else {
            "retry"
        };
        let (result, draft) = state
            .native_sdks
            .input_action(
                &target,
                body["id"].as_str().unwrap_or_default(),
                action,
                body["confirmed"] == true,
                body["request_id"].as_str().unwrap_or_default(),
            )
            .await
            .map_err(failure)?;
        if let Some(draft) = draft {
            return Box::pin(execute(state, "turn".into(), draft)).await;
        }
        return Ok(Json(response(result)));
    }
    if matches!(method.as_str(), "turn" | "steer") {
        let id = body["request_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .map(Ok)
            .unwrap_or_else(crate::conversation::random_id)
            .map_err(super::conversation::error)?;
        body["request_id"] = json!(id);
        let _submission = state
            .native_sdks
            .submission(&target, &format!("{method}:{id}"))
            .lock_owned()
            .await;
        let entry = state
            .native_sdks
            .begin_input(&target, &method, body)
            .map_err(failure)?;
        if entry.state != "preparing" {
            return Ok(Json(response(
                crate::native_sdk::NativeSdks::input_receipt(&entry),
            )));
        }
        let draft = entry.draft.clone();
        let mut body = entry.draft.clone();
        let preparation = async {
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
            Ok::<Value, ApiError>(body)
        }
        .await;
        let prepared = match preparation {
            Ok(input) => input,
            Err(error) => {
                state
                    .native_sdks
                    .input_preparation_failed(&target, &entry.id, format!("{error:?}"))
                    .map_err(failure)?;
                return Err(error);
            }
        };
        let mut result = state
            .native_sdks
            .prepare_input(&target, &entry.id, prepared)
            .map_err(failure)?;
        if method == "steer" && result["accepted"] == true {
            result = state
                .native_sdks
                .dispatch_input_control(&target, &entry.id)
                .await
                .map_err(failure)?;
        }
        if result["accepted"] == true
            && target["agent"].as_str().unwrap_or_default().is_empty()
            && let Some(conversations) = &state.conversations
        {
            conversations
                .sdk_accepted(
                    target["uid"].as_str().unwrap_or_default(),
                    &id,
                    &draft,
                    result.clone(),
                )
                .await
                .map_err(super::conversation::error)?;
        }
        return Ok(Json(response(result)));
    }
    if method == "stop" {
        let stopped = state.native_sdks.stop(&target).await.map_err(failure)?;
        return Ok(Json(json!({"ok":true,"stopped":stopped})));
    }
    if method == "interrupt" {
        state
            .native_sdks
            .input_stop_requested(&target, true)
            .map_err(failure)?;
    }
    let native_result = state
        .native_sdks
        .call(source, &method, target.clone(), body)
        .await;
    if method == "interrupt" && native_result.is_err() {
        state
            .native_sdks
            .input_stop_requested(&target, false)
            .map_err(failure)?;
    }
    let mut result = native_result.map_err(failure)?;
    if matches!(method.as_str(), "create" | "fork") {
        state.reader.store.invalidate_inventory();
        result["ok"] = json!(true);
        return Ok(Json(result));
    }
    if method == "open" {
        // The native-SDK open HTTP response is consumed only for
        // `capabilities`/config by the shared frontend (`NativeSdkView.start`
        // reads `data.capabilities`); the transcript is rendered from the SSE
        // snapshot and the paginated `/api/messages`. Dropping the (potentially
        // multi-MB) `items`/`subtasks` here avoids sending the whole history a
        // second time over the public link on every session switch. opencode
        // already omits `raw`, so the removes are harmless no-ops there.
        if let Some(raw) = result.get_mut("raw").and_then(Value::as_object_mut) {
            raw.remove("items");
            raw.remove("subtasks");
        }
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
    let uid = target["uid"].as_str().unwrap_or(&query.uid);
    let key = if query.agent.is_empty() {
        uid.to_owned()
    } else {
        format!("{}:{}", uid, query.agent)
    };
    let native = if source == "kiro" {
        // An SSE subscription is observation, including after process exit or
        // browser reconnection. Never turn it into a replacement native launch.
        match state.native_sdks.subscribe(source, &key) {
            Ok(native) => Ok(native),
            Err(_) => match state
                .native_sdks
                .call(source, "reattach", target.clone(), json!({}))
                .await
            {
                Ok(_) => state.native_sdks.subscribe(source, &key),
                Err(error) => Err(error),
            },
        }
    } else {
        state
            .native_sdks
            .call(source, "open", target.clone(), json!({}))
            .await
            .map_err(failure)?;
        state.native_sdks.subscribe(source, &key)
    };
    let (snapshot, mut events) = match native {
        Ok((snapshot, events)) => (snapshot, Some(events)),
        Err(error) => (state.native_sdks.input_fallback(&target, &error), None),
    };
    let (input_snapshot, mut queue_events) = state.native_sdks.input_events(&target);
    let mut snapshot = snapshot;
    snapshot["raw"]["input_queue"] = input_snapshot;
    let queue_scope = crate::native_sdk::NativeSdks::input_scope(&target);
    let sdks = state.native_sdks.clone();
    let source = source.to_owned();
    let shutdown = state.shutdown.clone();
    let stream = async_stream::stream! {
        yield Ok::<_,Infallible>(Event::default().event("sdk").data(snapshot.to_string()));
        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = heartbeat.tick() => {
                    if events.is_none() && let Ok((snapshot,receiver))=sdks.subscribe(&source,&key) {
                        events=Some(receiver);
                        yield Ok(Event::default().event("sdk").data(sdks.with_inputs(&target,snapshot).to_string()));
                    }
                    yield Ok(Event::default().event("heartbeat").data("{}"));
                },
                queue_event=queue_events.recv()=>match queue_event {
                    Ok(event)=>{
                        if let Ok(mut value)=serde_json::from_str::<Value>(&event) && value["scope"]==queue_scope {
                            value.as_object_mut().unwrap().remove("scope");
                            yield Ok(Event::default().event("sdk").data(value.to_string()));
                        }
                    },
                    Err(broadcast::error::RecvError::Lagged(_))=>{
                        yield Ok(Event::default().event("sdk").data(json!({"type":"input_queue","raw":{"input_queue":sdks.input_snapshot(&target)}}).to_string()));
                    },
                    Err(broadcast::error::RecvError::Closed)=>break,
                },
                event = async { match events.as_mut() {Some(events)=>events.recv().await,None=>std::future::pending().await} } => match event {
                    Ok(event) => {
                        match serde_json::from_str::<Value>(&event) {
                            Ok(snapshot)=>yield Ok(Event::default().event("sdk").data(sdks.with_inputs(&target,snapshot).to_string())),
                            Err(_)=>yield Ok(Event::default().event("sdk").data(&*event)),
                        }
                    },
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        yield Ok(Event::default().event("sdk").data(json!({"method":"bridge/reset","raw":{}}).to_string()));
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => events=None,
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
