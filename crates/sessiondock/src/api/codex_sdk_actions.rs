//! Transport for native Codex conversation actions.
use std::path::Path;

use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::codex_sdk::{rpc, service, thread_id};
use crate::{codex_sdk::RpcError, error::ApiError, state::AppState};

#[derive(Deserialize)]
pub struct Target {
    uid: String,
    #[serde(default)]
    agent: String,
}

#[derive(Deserialize)]
pub struct Fork {
    uid: String,
    #[serde(default)]
    agent: String,
    last_turn_id: Option<String>,
    before_turn_id: Option<String>,
}

pub async fn fork(
    State(state): State<AppState>,
    Json(body): Json<Fork>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let id = thread_id(&state, &body.uid, &body.agent).await?;
    let result = sdk
        .fork_thread(&id, body.last_turn_id.as_deref())
        .await
        .map_err(rpc)?;
    if let Some(before) = body.before_turn_id.as_deref() {
        let child = result["thread"]["id"]
            .as_str()
            .ok_or_else(|| rpc(RpcError::Disconnected))?;
        sdk.revert_thread(child, before).await.map_err(rpc)?;
    }
    let path = result["thread"]["path"]
        .as_str()
        .ok_or_else(|| rpc(RpcError::Disconnected))?;
    let uid = crate::sessions::uid_for("codex", Path::new(path));
    state.reader.store.invalidate_inventory();
    let mut session = sdk.pending().into_iter().find(|row| row["uid"] == uid);
    if session.is_none() {
        let lookup = uid.clone();
        session = state
            .reader
            .run(move |store| {
                let document = store.list_view(false)?;
                Ok(document["sessions"]
                    .as_array()
                    .and_then(|rows| rows.iter().find(|row| row["uid"] == lookup))
                    .cloned())
            })
            .await?;
    }
    Ok(Json(
        json!({"ok":true,"uid":uid,"session":session,"thread_id":result["thread"]["id"],"raw":result}),
    ))
}

#[derive(Deserialize)]
pub struct Turns {
    uid: String,
    #[serde(default)]
    agent: String,
    cursor: Option<String>,
}

pub async fn turns(
    State(state): State<AppState>,
    Json(body): Json<Turns>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let id = thread_id(&state, &body.uid, &body.agent).await?;
    let raw = sdk
        .list_turns(&id, body.cursor.as_deref())
        .await
        .map_err(rpc)?;
    Ok(Json(json!({"ok":true,"raw":raw})))
}

#[derive(Deserialize)]
pub struct Revert {
    uid: String,
    #[serde(default)]
    agent: String,
    before_turn_id: String,
}

pub async fn revert(
    State(state): State<AppState>,
    Json(body): Json<Revert>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let id = thread_id(&state, &body.uid, &body.agent).await?;
    let raw = sdk
        .revert_thread(&id, &body.before_turn_id)
        .await
        .map_err(rpc)?;
    state.reader.store.invalidate_inventory();
    Ok(Json(json!({"ok":true,"raw":raw})))
}

pub async fn compact(
    State(state): State<AppState>,
    Json(body): Json<Target>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let id = thread_id(&state, &body.uid, &body.agent).await?;
    let raw = sdk.compact_thread(&id).await.map_err(rpc)?;
    Ok(Json(json!({"ok":true,"raw":raw})))
}

#[derive(Deserialize)]
pub struct Review {
    uid: String,
    #[serde(default)]
    agent: String,
    target: Value,
}

pub async fn review(
    State(state): State<AppState>,
    Json(body): Json<Review>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let id = thread_id(&state, &body.uid, &body.agent).await?;
    let raw = sdk.review_thread(&id, body.target).await.map_err(rpc)?;
    Ok(Json(json!({"ok":true,"raw":raw})))
}
