//! HTTP transport for explicit SDK settings, goals and native shell commands.
use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::codex_sdk::{rpc, service, thread_id};
use crate::{error::ApiError, state::AppState};

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalAction {
    Read,
    Set,
    Clear,
}

#[derive(Deserialize)]
pub struct Goal {
    uid: String,
    #[serde(default)]
    agent: String,
    action: GoalAction,
    objective: Option<String>,
    status: Option<String>,
    token_budget: Option<i64>,
}

pub async fn goal(
    State(state): State<AppState>,
    Json(body): Json<Goal>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let thread = thread_id(&state, &body.uid, &body.agent).await?;
    let action = match body.action {
        GoalAction::Read => "read",
        GoalAction::Set => "set",
        GoalAction::Clear => "clear",
    };
    let raw = sdk
        .controls_goal(
            &thread,
            action,
            body.objective.as_deref(),
            body.status.as_deref(),
            body.token_budget,
        )
        .await
        .map_err(rpc)?;
    Ok(Json(json!({"ok":true,"raw":raw})))
}

#[derive(Deserialize)]
pub struct Settings {
    uid: String,
    #[serde(default)]
    agent: String,
    mode: Option<String>,
    approval_policy: Option<Value>,
    sandbox_policy: Option<Value>,
}

pub async fn settings(
    State(state): State<AppState>,
    Json(body): Json<Settings>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let thread = thread_id(&state, &body.uid, &body.agent).await?;
    let raw = sdk
        .controls_settings(
            &thread,
            body.mode.as_deref(),
            body.approval_policy,
            body.sandbox_policy,
        )
        .await
        .map_err(rpc)?;
    Ok(Json(json!({"ok":true,"raw":raw})))
}

#[derive(Deserialize)]
pub struct Shell {
    uid: String,
    #[serde(default)]
    agent: String,
    command: String,
}

pub async fn shell(
    State(state): State<AppState>,
    Json(body): Json<Shell>,
) -> Result<Json<Value>, ApiError> {
    let sdk = service(&state)?;
    let thread = thread_id(&state, &body.uid, &body.agent).await?;
    let raw = sdk
        .controls_shell(&thread, &body.command)
        .await
        .map_err(rpc)?;
    Ok(Json(json!({"ok":true,"raw":raw})))
}
