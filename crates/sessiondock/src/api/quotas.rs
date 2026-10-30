use crate::state::AppState;
use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use serde_json::Value;
pub async fn get(State(state): State<AppState>) -> Json<Value> {
    Json(state.quotas.read().await)
}
#[derive(Default, Deserialize)]
pub struct Refresh {
    #[serde(default)]
    cached: bool,
    source: Option<String>,
}
pub async fn refresh(State(state): State<AppState>, Query(options): Query<Refresh>) -> Json<Value> {
    Json(if options.cached {
        state.quotas.collect_cached(options.source.as_deref()).await
    } else if let Some(source) = options.source.as_deref() {
        state.quotas.collect_source(source).await
    } else {
        state.quotas.collect().await
    })
}
