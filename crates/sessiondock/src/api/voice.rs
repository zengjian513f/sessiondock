use axum::{Json, extract::State, http::StatusCode, response::Response};

use crate::{error::ApiError, state::AppState, voice::TranscribeRequest};

pub const VOICE_BODY_LIMIT: usize = 8 * 1024 * 1024;

pub async fn transcribe(
    State(state): State<AppState>,
    Json(body): Json<TranscribeRequest>,
) -> Result<Response, ApiError> {
    let Some(voice) = &state.voice else {
        return Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "voice_input_unconfigured",
            "未配置语音输入 API Key（OPENROUTER_API_KEY）",
        ));
    };
    voice.transcribe(body, state.shutdown.clone()).await
}
