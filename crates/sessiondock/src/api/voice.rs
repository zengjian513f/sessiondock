use std::collections::HashMap;

use axum::{
    Json,
    extract::{
        Query, State,
        ws::{WebSocketUpgrade, rejection::WebSocketUpgradeRejection},
    },
    http::StatusCode,
    response::Response,
};
use serde_json::Value;

use crate::{error::ApiError, state::AppState, voice::TranscribeRequest};

pub const VOICE_BODY_LIMIT: usize = 8 * 1024 * 1024;

pub async fn models(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let Some(voice) = &state.voice else {
        return Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "voice_input_unconfigured",
            "未配置语音输入 API Key",
        ));
    };
    let refresh = params
        .get("refresh")
        .map(|v| v.trim())
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let payload = voice.list_models(refresh, state.shutdown.clone()).await?;
    Ok(Json(payload))
}

pub async fn transcribe(
    State(state): State<AppState>,
    Json(body): Json<TranscribeRequest>,
) -> Result<Response, ApiError> {
    let Some(voice) = &state.voice else {
        return Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "voice_input_unconfigured",
            "未配置语音输入 API Key",
        ));
    };
    voice.transcribe(body, state.shutdown.clone()).await
}

pub async fn realtime(
    State(state): State<AppState>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, ApiError> {
    let Some(voice) = state.voice.clone() else {
        return Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "voice_input_unconfigured",
            "未配置语音输入 API Key",
        ));
    };
    let upgrade = ws.map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_voice_ws",
            "实时语音连接需使用 WebSocket 升级",
        )
    })?;
    let shutdown = state.shutdown.clone();
    Ok(upgrade.on_upgrade(move |socket| voice.handle_realtime_ws(socket, shutdown)))
}
