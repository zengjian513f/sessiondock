use crate::state::AppState;
use axum::{Json, extract::State};
use serde_json::Value;
pub async fn get(State(state): State<AppState>) -> Json<Value> {
    Json(state.quotas.read().await)
}
pub async fn refresh(State(state): State<AppState>) -> Json<Value> {
    Json(state.quotas.collect().await)
}
