//! SessionDock-owned preferences only. No native session writes or CLI actions.
use std::{collections::BTreeSet, sync::Arc};

use axum::{
    Extension, Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    error::ApiError,
    metadata::{MetadataError, MetadataSnapshot, MetadataStore, SpawnedBy, fork_parent_uids},
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
    fn validate(&self, state: &AppState, hub: bool) -> Result<(), ApiError> {
        if !hub && !self._build.is_empty() && self._build != state.assets.build {
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

/// `parent_uid` attaches this session under another listed session;
/// `independent` shows it as a root and ignores `spawned_by`.
#[derive(Deserialize)]
pub struct NestRequest {
    uid: String,
    #[serde(default)]
    parent_uid: Option<String>,
    #[serde(default)]
    independent: bool,
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
            "偏好保存未配置状态目录",
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
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    body: Result<Json<StarRequest>, JsonRejection>,
) -> Result<JsonBytes, ApiError> {
    let metadata = configured(&state)?;
    let Json(body) = body.map_err(invalid)?;
    body.diagnostics.validate(&state, hub.is_some())?;
    if body.uid.is_empty() {
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
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    body: Result<Json<VisibilityRequest>, JsonRejection>,
) -> Result<JsonBytes, ApiError> {
    let metadata = configured(&state)?;
    let Json(body) = body.map_err(invalid)?;
    body.diagnostics.validate(&state, hub.is_some())?;
    if body.uids.is_empty() || body.uids.iter().any(|uid| uid.is_empty()) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_metadata_batch",
            "需要有效会话 uid",
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

fn listed_row<'a>(rows: &'a [Value], uid: &str) -> Option<&'a Value> {
    rows.iter().find(|row| row["uid"] == uid)
}

fn row_node(row: &Value) -> &str {
    row["node_id"].as_str().unwrap_or("")
}

fn lookup_nest_parent(rows: &[Value], child_uid: &str, source: &str, sid: &str) -> Option<String> {
    let child_node = listed_row(rows, child_uid).map(row_node).unwrap_or("");
    rows.iter().find_map(|row| {
        let uid = row["uid"].as_str()?;
        if uid == child_uid {
            return None;
        }
        (row["source"] == source && row["sid"] == sid && row_node(row) == child_node)
            .then(|| uid.to_owned())
    })
}

fn display_parent_uid(uid: &str, rows: &[Value], snapshot: &MetadataSnapshot) -> Option<String> {
    if snapshot.nest_independent(uid) {
        return None;
    }
    if let Some(parent) = snapshot.nest_parent(uid) {
        return lookup_nest_parent(rows, uid, &parent.source, &parent.sid);
    }
    let spawned = snapshot.spawned_by(uid)?;
    lookup_nest_parent(rows, uid, &spawned.source, &spawned.sid)
}

fn nest_would_cycle(
    uid: &str,
    parent_uid: &str,
    rows: &[Value],
    snapshot: &MetadataSnapshot,
) -> bool {
    if uid == parent_uid {
        return true;
    }
    let mut seen = BTreeSet::from([uid.to_owned()]);
    let mut current = Some(parent_uid.to_owned());
    while let Some(next) = current {
        if !seen.insert(next.clone()) {
            return true;
        }
        current = display_parent_uid(&next, rows, snapshot);
    }
    false
}

pub async fn nest(
    State(state): State<AppState>,
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    body: Result<Json<NestRequest>, JsonRejection>,
) -> Result<JsonBytes, ApiError> {
    let metadata = configured(&state)?;
    let Json(body) = body.map_err(invalid)?;
    body.diagnostics.validate(&state, hub.is_some())?;
    if body.uid.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_metadata_uid",
            "需要有效的会话 uid",
        ));
    }
    let parent_uid = body
        .parent_uid
        .as_deref()
        .map(str::trim)
        .filter(|uid| !uid.is_empty())
        .map(str::to_owned);
    if body.independent && parent_uid.is_some() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "nest_conflict",
            "独立显示时不能同时指定父会话",
        ));
    }
    write(state, move |store| {
        let list = store.list(true)?;
        let rows = list["sessions"].as_array().expect("session snapshot rows");
        if listed_row(rows, &body.uid).is_none() {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "session_missing",
                "会话不存在",
            ));
        }
        let parent = if let Some(parent_uid) = parent_uid {
            if parent_uid == body.uid {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "nest_parent_self",
                    "不能附属到自己下面",
                ));
            }
            let parent_row = listed_row(rows, &parent_uid).ok_or_else(|| {
                ApiError::new(
                    StatusCode::NOT_FOUND,
                    "nest_parent_missing",
                    "目标会话不存在",
                )
            })?;
            let child_node = listed_row(rows, &body.uid).map(row_node).unwrap_or("");
            let parent_node = row_node(parent_row);
            if !child_node.is_empty() && !parent_node.is_empty() && child_node != parent_node {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "nest_parent_node",
                    "只能附属到同一台机器上的会话",
                ));
            }
            let source = parent_row["source"].as_str().unwrap_or("").trim();
            let sid = parent_row["sid"].as_str().unwrap_or("").trim();
            if source.is_empty() || sid.is_empty() {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "nest_parent_missing",
                    "目标会话缺少来源或会话 id",
                ));
            }
            let snapshot = metadata.snapshot()?;
            if nest_would_cycle(&body.uid, &parent_uid, rows, &snapshot) {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "nest_parent_cycle",
                    "不能附属到自己的子会话下面",
                ));
            }
            Some(SpawnedBy {
                source: source.to_owned(),
                sid: sid.to_owned(),
            })
        } else {
            None
        };
        let snapshot = metadata.set_nest_display(&body.uid, parent, body.independent)?;
        let nest_parent = snapshot
            .nest_parent(&body.uid)
            .map(|parent| json!({"source": parent.source, "sid": parent.sid}));
        Ok(json!({
            "ok": true,
            "uid": body.uid,
            "nest_parent": nest_parent,
            "nest_independent": snapshot.nest_independent(&body.uid),
            "metadata_revision": snapshot.revision(),
        }))
    })
    .await
}

/// Persist a Claude display pin ("rewind" of the read model only). The target
/// is validated against the frozen native inventory; native files are never
/// written and the CLI is never signalled, so the response says
/// `native_rewind:false`. Later native records retire the pin explicitly.
pub async fn rewind(
    State(state): State<AppState>,
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    body: Result<Json<RewindRequest>, JsonRejection>,
) -> Result<JsonBytes, ApiError> {
    let metadata = configured(&state)?;
    let Json(body) = body.map_err(invalid)?;
    body.diagnostics.validate(&state, hub.is_some())?;
    if body.uid.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_metadata_uid",
            "需要有效的会话 uid",
        ));
    }
    if body.target.as_ref().is_some_and(|target| target.is_empty()) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_rewind_target",
            "target 必须是 Claude 记录节点 ID，或 null 表示取消固定",
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
