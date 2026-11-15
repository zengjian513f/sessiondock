use crate::{error::ApiError, state::AppState};
use axum::{Json, extract::State, http::StatusCode};
use serde_json::Value;

pub async fn get(State(state): State<AppState>) -> Json<Value> {
    Json(state.agent_usage.read().await)
}
pub async fn refresh(State(state): State<AppState>) -> Json<Value> {
    state.agent_usage.start(true);
    Json(state.agent_usage.read().await)
}
pub async fn configure(
    State(state): State<AppState>,
    Json(value): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    state
        .agent_usage
        .configure(value)
        .await
        .map_err(|message| ApiError::new(StatusCode::BAD_REQUEST, "usage_settings", message))?;
    state.agent_usage.start(false);
    Ok(Json(state.agent_usage.read().await))
}
