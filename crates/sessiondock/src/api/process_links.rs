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

pub async fn resources(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(crate::runtime::process_links::resources(&state).await)
}

#[derive(serde::Deserialize)]
pub struct SessionResourcesQuery {
    uid: String,
    scope: Option<String>,
}

pub async fn session_resources(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<SessionResourcesQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let inclusive = crate::hub::resources::inclusive(query.scope.as_deref())
        .map_err(|message| ApiError::new(StatusCode::BAD_REQUEST, "invalid_scope", message))?;
    let node_id = state
        .node
        .as_ref()
        .map(|node| node.node_id.clone())
        .unwrap_or_default();
    let document = state
        .reader
        .run_wait(&state.shutdown, |store| store.list_recent())
        .await?;
    let session = crate::hub::resources::session_from_list(&document, &query.uid, &node_id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "session_not_found", "会话不存在"))?;
    let value = crate::runtime::process_links::resources(&state).await;
    let row =
        crate::hub::resources::node_row(&node_id, &state.hostname, value, &session, inclusive);
    Ok(Json(crate::hub::resources::response(vec![row])))
}

#[derive(serde::Deserialize)]
pub struct ProbeBody {
    pub enabled: bool,
}
#[derive(serde::Deserialize)]
pub struct SessionProbeBody {
    pub uid: String,
    pub scope: Option<String>,
    pub enabled: bool,
}

pub async fn node_probe(
    State(state): State<AppState>,
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    Json(body): Json<ProbeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if hub.is_none() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "node_auth_required",
            "机器探测仅接受经过认证的 Hub",
        ));
    }
    Ok(Json(
        crate::runtime::process_links::probe(&state, body.enabled).await?,
    ))
}

pub async fn session_probe(
    State(state): State<AppState>,
    Json(body): Json<SessionProbeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::hub::resources::inclusive(body.scope.as_deref())
        .map_err(|message| ApiError::new(StatusCode::BAD_REQUEST, "invalid_scope", message))?;
    let node_id = state
        .node
        .as_ref()
        .map(|n| n.node_id.as_str())
        .unwrap_or_default();
    let document = state
        .reader
        .run_wait(&state.shutdown, |store| store.list_recent())
        .await?;
    crate::hub::resources::session_from_list(&document, &body.uid, node_id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "session_not_found", "会话不存在"))?;
    Ok(Json(
        crate::runtime::process_links::probe(&state, body.enabled).await?,
    ))
}
