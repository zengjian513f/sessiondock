//! HTTP JSON error envelope `{error, code}` shared by Axum handlers.
//! `unavailable` is 501 `not_implemented` for capabilities not yet migrated.
//! `SessionError` maps 501 to `unsupported_history`; other session failures
//! stay `session_error`. This type does not log, redact, or persist.
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::sessions::SessionError;

#[derive(Clone, Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn unavailable(capability: &str) -> Self {
        Self::new(
            StatusCode::NOT_IMPLEMENTED,
            "not_implemented",
            format!("Rust 后端尚未迁移此能力：{capability}。当前是只读开发阶段。"),
        )
    }
}

impl From<SessionError> for ApiError {
    fn from(error: SessionError) -> Self {
        let status =
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        Self::new(
            status,
            if status == StatusCode::NOT_IMPLEMENTED {
                "unsupported_history"
            } else {
                "session_error"
            },
            error.message,
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error": self.message, "code": self.code})),
        )
            .into_response()
    }
}
