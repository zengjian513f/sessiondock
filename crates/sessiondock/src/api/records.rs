//! `/api/term/records`: list session recordings; `/api/term/records/attach`:
//! read-only WebSocket replay of one recording (see `terminal::records`).
//! Both need the explicit ptyhost directory; no ownership lease is involved.

use axum::{
    Json,
    extract::{
        Query, State,
        rejection::QueryRejection,
        ws::{WebSocketUpgrade, rejection::WebSocketUpgradeRejection},
    },
    http::StatusCode,
    response::Response,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{error::ApiError, state::AppState, terminal::records};

fn root(state: &AppState) -> Result<std::path::PathBuf, ApiError> {
    state
        .terminal
        .as_ref()
        .map(|service| service.directory().to_path_buf())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_IMPLEMENTED,
                "terminal_disabled",
                "终端传输未启用：必须显式配置隔离的 ptyhost 目录",
            )
        })
}

fn unreadable() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "records_unreadable",
        "无法读取录制目录",
    )
}

/// `GET /api/term/records` → `{"records":[…]}` newest first.
pub async fn list(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let root = root(&state)?;
    let records = tokio::task::spawn_blocking(move || records::list(&root))
        .await
        .map_err(|_| unreadable())?
        .map_err(|_| unreadable())?;
    Ok(Json(json!({"records": records})))
}

#[derive(Deserialize)]
pub struct AttachQuery {
    id: String,
}

/// `GET /api/term/records/attach?id=…` (WebSocket).
pub async fn attach(
    State(state): State<AppState>,
    query: Result<Query<AttachQuery>, QueryRejection>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, ApiError> {
    let root = root(&state)?;
    let Query(query) = query
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "invalid_record", "录制参数无效"))?;
    let id = query.id;
    let found = {
        let root = root.clone();
        let id = id.clone();
        tokio::task::spawn_blocking(move || records::entry(&root, &id))
            .await
            .map_err(|_| unreadable())?
    };
    let Some(entry) = found else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "record_not_found",
            "没有这个录制",
        ));
    };
    let Some(dir) = records::record_dir(&root, &id) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "record_not_found",
            "没有这个录制",
        ));
    };
    let ws = ws.map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "websocket_required",
            "需要有效的 WebSocket 升级请求",
        )
    })?;
    if state.shutdown.is_cancelled() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "shutdown",
            "服务正在关闭",
        ));
    }
    let shutdown = state.shutdown.clone();
    Ok(ws
        .read_buffer_size(4 * 1024)
        .write_buffer_size(0)
        .max_write_buffer_size(256 * 1024)
        .on_failed_upgrade(|_| {})
        .on_upgrade(move |socket| records::stream(dir, entry, socket, shutdown)))
}
