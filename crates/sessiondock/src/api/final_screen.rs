//! `GET /api/term/final?id=…`: the final screen an exited session left, as one
//! grid snapshot (see `terminal::final_screen`). Needs the explicit ptyhost
//! directory; no ownership lease is involved.

use axum::{
    Json,
    extract::{Query, State, rejection::QueryRejection},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{error::ApiError, state::AppState, terminal::final_screen};

#[derive(Deserialize)]
pub struct FinalQuery {
    id: String,
}

pub async fn get(
    State(state): State<AppState>,
    query: Result<Query<FinalQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let root = state
        .terminal
        .as_ref()
        .map(|service| service.directory().to_path_buf())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_IMPLEMENTED,
                "terminal_disabled",
                "终端传输未启用：必须显式配置隔离的 ptyhost 目录",
            )
        })?;
    let Query(query) = query.map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_final_screen",
            "最终画面参数无效",
        )
    })?;
    tokio::task::spawn_blocking(move || final_screen::render(&root, &query.id))
        .await
        .ok()
        .flatten()
        .map(Json)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "final_screen_not_found",
                "没有这份最终画面",
            )
        })
}
