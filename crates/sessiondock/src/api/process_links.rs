use crate::{error::ApiError, state::AppState};
use axum::{Extension, Json, extract::State, http::StatusCode};

pub async fn get(State(state): State<AppState>) -> Result<Json<process_links::Report>, ApiError> {
    Ok(Json(crate::runtime::process_links::report(&state).await?))
}

pub async fn post(
    State(state): State<AppState>,
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    Json(body): Json<process_links::Published>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if hub.is_none() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "node_auth_required",
            "进程关联仅接受经过认证的 Hub",
        ));
    }
    let count = crate::runtime::process_links::publish(&state, body).await?;
    Ok(Json(serde_json::json!({"linked":count})))
}
