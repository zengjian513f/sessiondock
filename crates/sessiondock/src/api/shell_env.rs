//! `GET /api/shell-env` and `POST /api/shell-env/restart` (`crate::shell_env`).

use std::time::Duration;

use axum::{Json, extract::State, http::StatusCode};
use serde_json::{Value, json};

use crate::{error::ApiError, state::AppState};

/// Drift of the login-shell environment since this service started.
pub async fn status(State(state): State<AppState>) -> Json<Value> {
    match &state.shell_env {
        Some(service) => Json(service.check().await),
        None => Json(json!({"configured": false, "stale": false, "changed": []})),
    }
}

/// Answer first, then shut down gracefully and exit for the supervisor to
/// restart with a freshly loaded environment.
pub async fn restart(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    if state.shell_env.is_none() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "shell_env_unconfigured",
            "本机没有配置登录环境检查，不能从页面重启后端",
        ));
    }
    crate::shell_env::request_restart();
    eprintln!("SessionDock: restart requested from the page to reload the login-shell environment");
    let shutdown = state.shutdown.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        shutdown.cancel();
    });
    Ok(Json(json!({"ok": true, "restarting": true})))
}
