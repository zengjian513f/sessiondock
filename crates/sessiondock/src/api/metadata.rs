//! AgentHub-owned preferences only. No native session writes or CLI actions.
use std::{collections::BTreeSet, sync::Arc};

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    error::ApiError,
    metadata::{MetadataError, MetadataStore, fork_parent_uids},
    sessions::SessionStore,
    state::{AppState, JsonBytes},
};

impl From<MetadataError> for ApiError {
    fn from(error: MetadataError) -> Self {
        Self::new(
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
            error.code,
            error.message,
        )
    }
}

#[derive(Default, Deserialize)]
pub struct Diagnostics {
    #[serde(default)]
    _build: String,
    #[serde(default)]
    _trace_id: String,
    #[serde(default)]
    _page_id: String,
}

impl Diagnostics {
    fn validate(&self, state: &AppState) -> Result<(), ApiError> {
        if [&self._build, &self._trace_id, &self._page_id]
            .iter()
            .any(|field| field.len() > 128)
        {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_metadata_request",
                "偏好请求诊断字段过长",
            ));
        }
        if !self._build.is_empty() && self._build != state.assets.build {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "stale_build",
                "页面版本已过期，请刷新后再保存偏好",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct StarRequest {
    uid: String,
    starred: bool,
    #[serde(flatten)]
    diagnostics: Diagnostics,
}

#[derive(Deserialize)]
pub struct VisibilityRequest {
    uids: Vec<String>,
    visible: bool,
    #[serde(flatten)]
    diagnostics: Diagnostics,
}

/// `target` is the Claude node (a message `turn_id`) to rewind the display to
/// before; `null` clears the pin. This never rewinds the CLI.
#[derive(Deserialize)]
pub struct RewindRequest {
    uid: String,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(flatten)]
    diagnostics: Diagnostics,
}

fn configured(state: &AppState) -> Result<Arc<MetadataStore>, ApiError> {
    state.metadata.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "metadata_disabled",
            "偏好保存未启用：必须显式配置独立的开发状态目录",
        )
    })
}

fn invalid(error: JsonRejection) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "偏好请求体超过大小限制",
        );
    }
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "invalid_metadata_request",
        "需要有效的偏好 JSON 请求和布尔状态",
    )
}

async fn write(
    state: AppState,
    work: impl FnOnce(&SessionStore) -> Result<Value, ApiError> + Send + 'static,
) -> Result<JsonBytes, ApiError> {
    if state.shutdown.is_cancelled() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "shutdown",
            "服务正在关闭",
        ));
    }
    let permit = state.reader.acquire().await?;
    let store = state.reader.store.clone();
    // Once admitted, an HTTP cancellation cannot roll back an atomic write or
    // release capacity before the blocking job has actually finished.
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work(&store).map(|value| JsonBytes::new(&value))
    })
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata_worker_failed",
            "偏好工作异常退出，请重新读取状态确认结果",
        )
    })?
}

pub async fn star(
    State(state): State<AppState>,
    body: Result<Json<StarRequest>, JsonRejection>,
) -> Result<JsonBytes, ApiError> {
    let metadata = configured(&state)?;
    let Json(body) = body.map_err(invalid)?;
    body.diagnostics.validate(&state)?;
    if body.uid.is_empty() || body.uid.len() > 256 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_metadata_uid",
            "需要有效的会话 uid",
        ));
    }
    write(state, move |store| {
        let list = store.list(true)?;
        if !list["sessions"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["uid"] == body.uid))
        {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "session_missing",
                "会话不存在",
            ));
        }
        let snapshot = metadata.set_starred(&body.uid, body.starred)?;
        let row = snapshot.row(&body.uid);
        Ok(json!({"ok":true,"uid":body.uid,"starred":body.starred,
            "starred_at":row["starred_at"],"metadata_revision":snapshot.revision()}))
    })
    .await
}

pub async fn visibility(
    State(state): State<AppState>,
    body: Result<Json<VisibilityRequest>, JsonRejection>,
) -> Result<JsonBytes, ApiError> {
    let metadata = configured(&state)?;
    let Json(body) = body.map_err(invalid)?;
    body.diagnostics.validate(&state)?;
    if body.uids.is_empty()
        || body.uids.len() > 1000
        || body
            .uids
            .iter()
            .any(|uid| uid.is_empty() || uid.len() > 256)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_metadata_batch",
            "需要 1 至 1000 个有效会话 uid",
        ));
    }
    write(state, move |store| {
        let list = store.list(true)?;
        let rows = list["sessions"].as_array().expect("session snapshot rows");
        let known: BTreeSet<_> = rows.iter().filter_map(|row| row["uid"].as_str()).collect();
        let parents = fork_parent_uids(rows);
        let mut seen = BTreeSet::new();
        let mut valid = Vec::new();
        let mut errors = Vec::new();
        for uid in body.uids {
            if !seen.insert(uid.clone()) { continue }
            let error = if !known.contains(uid.as_str()) { Some("会话不存在") }
                else if !parents.contains(&uid) { Some("会话不是父会话") } else { None };
            if let Some(error) = error { errors.push(json!({"uid":uid,"error":error})) }
            else { valid.push(uid) }
        }
        let snapshot = metadata.set_fork_visibility(&valid, body.visible)?;
        let updated: Vec<_> = valid.into_iter().map(|uid| json!({"uid":uid,"fork_parent_visible":body.visible})).collect();
        Ok(json!({"ok":true,"updated":updated,"errors":errors,"metadata_revision":snapshot.revision()}))
    }).await
}

/// Persist a Claude display pin ("rewind" of the read model only). The target
/// is validated against the frozen native inventory; native files are never
/// written and the CLI is never signalled, so the response says
/// `native_rewind:false`. Later native records retire the pin explicitly.
pub async fn rewind(
    State(state): State<AppState>,
    body: Result<Json<RewindRequest>, JsonRejection>,
) -> Result<JsonBytes, ApiError> {
    let metadata = configured(&state)?;
    let Json(body) = body.map_err(invalid)?;
    body.diagnostics.validate(&state)?;
    if body.uid.is_empty() || body.uid.len() > 256 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_metadata_uid",
            "需要有效的会话 uid",
        ));
    }
    if body.target.as_ref().is_some_and(|target| {
        target.is_empty() || target.len() > 256 || target.chars().any(char::is_control)
    }) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_rewind_target",
            "target 必须是 1 至 256 字节的 Claude 记录节点 ID，或 null 表示取消固定",
        ));
    }
    if body.request_id.as_ref().is_some_and(|id| id.len() > 128) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_metadata_request",
            "request_id 过长",
        ));
    }
    write(state, move |store| {
        let uid = body.uid;
        let snapshot = match &body.target {
            Some(target) => {
                let resolved = store.claude_rewind_target(&uid, target)?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| {
                        ApiError::new(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "metadata_clock_invalid",
                            "系统时钟无效，不能记录固定时间",
                        )
                    })?
                    .as_secs_f64();
                metadata.set_timeline_pin(
                    &uid,
                    crate::metadata::TimelinePin {
                        tip: resolved.tip,
                        stale_end: resolved.stale_end,
                        target: Some(target.clone()),
                        pinned_at: Some(now),
                    },
                )?
            }
            None => {
                let list = store.list(true)?;
                if !list["sessions"]
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|row| row["uid"] == uid))
                {
                    return Err(ApiError::new(
                        StatusCode::NOT_FOUND,
                        "session_missing",
                        "会话不存在",
                    ));
                }
                metadata.clear_timeline_pin(&uid)?
            }
        };
        // Report the row exactly as `/api/sessions` now publishes it, including
        // an immediate retirement if native records already moved past the pin.
        let list = store.list(true)?;
        let row = list["sessions"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["uid"] == uid))
            .cloned()
            .unwrap_or(Value::Null);
        let pin = row["timeline_pin"].clone();
        Ok(json!({
            "ok": true, "uid": uid, "pinned": pin.is_object(),
            "target": pin["target"], "tip": pin["tip"], "stale_end": pin["stale_end"],
            "timeline_pin": pin, "native_rewind": false,
            "request_id": body.request_id, "metadata_revision": snapshot.revision(),
            "message": if pin.is_object() {
                if pin["retired"] == true { "固定显示已失效；CLI 未回滚" } else { "已固定显示；CLI 未回滚" }
            } else { "已取消固定显示；CLI 未回滚" },
        }))
    })
    .await
}
