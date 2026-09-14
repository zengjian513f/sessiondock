//! Session recycle-bin HTTP routes, using delete protection.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State, rejection::JsonRejection, rejection::QueryRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    error::ApiError,
    lifecycle::model::{BindingState, State as LaunchState},
    runtime::{ExitReceipt, RuntimeSnapshot},
    state::AppState,
    trash::{DeleteOutcome, LIST_DEFAULT, Liveness, TrashError, TrashService},
};

impl From<TrashError> for ApiError {
    fn from(error: TrashError) -> Self {
        Self::new(
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            error.code,
            error.message,
        )
    }
}

fn configured(state: &AppState, route: &str) -> Result<Arc<TrashService>, ApiError> {
    state
        .trash
        .clone()
        .ok_or_else(|| ApiError::unavailable(route))
}

fn invalid(rejection: JsonRejection) -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "invalid_trash_request",
        format!("请求体无效: {}", rejection.body_text()),
    )
}

fn valid_uid(uid: &str) -> bool {
    !uid.trim().is_empty()
}

/// Freeze one native snapshot (rows + verified catalog), then observe the
/// managed runtime against exactly that catalog. Without a host directory the
/// liveness is unconfigured: every session is unknown.
async fn frozen_liveness(state: &AppState) -> Result<(Vec<Value>, Liveness), ApiError> {
    let (rows, catalog) = state
        .reader
        .run_wait(&state.shutdown, |store| {
            let snapshot = store.search_snapshot()?;
            let catalog = snapshot.native_catalog();
            let rows = snapshot.list["sessions"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            Ok((rows, catalog))
        })
        .await?;
    let document = json!({"sessions": &rows});
    let external = super::runtime::observe_external_all(state, &document).await?;
    let mut liveness = if let Some(runtime) = &state.runtime {
        let _permit = state
            .runtime_probes
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| {
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "runtime_closed",
                    "进程观察服务已关闭",
                )
            })?;
        let receipts = exit_receipts(state).await?;
        let snapshot: RuntimeSnapshot =
            runtime
                .observe_with(&catalog, &receipts)
                .await
                .map_err(|_| {
                    ApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "runtime_unavailable",
                        "无法读取受管进程状态",
                    )
                })?;
        Liveness::from_runtime(Some(&snapshot))
    } else {
        Liveness::default()
    };
    liveness.configured = true;
    for (uid, running) in external {
        if running {
            liveness.states.insert(
                uid,
                crate::trash::RunState::Running("native_process".into()),
            );
        }
    }
    Ok((rows, liveness))
}

/// Same receipt rule as `/api/live`: a confirmed lifecycle exit for one exact
/// instance. Never outranks a newer reachable instance.
async fn exit_receipts(state: &AppState) -> Result<Vec<ExitReceipt>, ApiError> {
    let Some(service) = &state.lifecycle else {
        return Ok(Vec::new());
    };
    let records = service
        .list(0, usize::MAX)
        .await
        .map_err(super::lifecycle::failure)?;
    Ok(records
        .iter()
        .filter(|record| record.state() == LaunchState::Exited)
        .filter_map(|record| {
            let binding = record.binding()?;
            (binding.state() == BindingState::Confirmed).then(|| ExitReceipt {
                uid: binding.spec().uid().to_owned(),
                host: record.host_name().to_owned(),
                instance_id: record.instance_id().to_owned(),
            })
        })
        .collect())
}

async fn run_delete(
    state: &AppState,
    trash: Arc<TrashService>,
    uids: Vec<String>,
    force: bool,
) -> Result<DeleteOutcome, ApiError> {
    let (rows, liveness) = frozen_liveness(state).await?;
    state
        .reader
        .run_wait(&state.shutdown, move |store| {
            let outcome = trash.delete(&rows, &liveness, &uids, force);
            if !outcome.deleted.is_empty() {
                // Files moved: the published inventory must not keep listing
                // them. A transient refresh failure is retried by the next list.
                let _ = store.list(true);
            }
            Ok(outcome)
        })
        .await
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct ForceQuery {
    force: String,
}

pub async fn delete_session(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    query: Result<Query<ForceQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let trash = configured(&state, "/api/session/{uid} DELETE")?;
    let Query(query) = query
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    if !matches!(query.force.as_str(), "" | "0" | "1" | "true" | "false") {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "force 必须为 0/1",
        ));
    }
    if !valid_uid(&uid) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_uid",
            "会话 uid 无效",
        ));
    }
    let force = matches!(query.force.as_str(), "1" | "true");
    let mut outcome = run_delete(&state, trash, vec![uid], force).await?;
    if let Some(deleted) = outcome.deleted.pop() {
        let mut body = serde_json::to_value(&deleted).map_err(|_| encoding())?;
        body["ok"] = json!(true);
        return Ok((StatusCode::OK, Json(body)).into_response());
    }
    let refused = outcome
        .skipped
        .pop()
        .or_else(|| outcome.failed.pop())
        .ok_or_else(encoding)?;
    let status = match refused.code {
        "not_found" => StatusCode::NOT_FOUND,
        "fork_parent_protected"
        | "session_running"
        | "changed_since_inventory"
        | "destination_exists"
        | "root_unconfigured"
        | "path_missing"
        | "unknown_source" => StatusCode::CONFLICT,
        "not_regular_file" => StatusCode::FORBIDDEN,
        "stat_failed" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut body = serde_json::to_value(&refused).map_err(|_| encoding())?;
    body["ok"] = json!(false);
    Ok((status, Json(body)).into_response())
}

fn encoding() -> ApiError {
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "trash_encoding",
        "回收站结果无法编码",
    )
}

#[derive(Deserialize)]
pub struct DeleteBody {
    uids: Vec<String>,
    #[serde(default)]
    force: bool,
}

pub async fn delete_batch(
    State(state): State<AppState>,
    body: Result<Json<DeleteBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let trash = configured(&state, "/api/sessions/delete")?;
    let Json(body) = body.map_err(invalid)?;
    let mut uids = Vec::new();
    for uid in body.uids {
        let uid = uid.trim().to_owned();
        if uid.is_empty() {
            continue;
        }
        if !valid_uid(&uid) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_uid",
                "会话 uid 无效",
            ));
        }
        if !uids.contains(&uid) {
            uids.push(uid);
        }
    }
    if uids.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "no_sessions",
            "没有选中任何会话",
        ));
    }
    let outcome = run_delete(&state, trash, uids, body.force).await?;
    let mut value = serde_json::to_value(&outcome).map_err(|_| encoding())?;
    // Legacy reads `deleted` and `errors`; the typed lists are additive.
    let errors: Vec<Value> = outcome
        .skipped
        .iter()
        .chain(outcome.failed.iter())
        .map(|refused| serde_json::to_value(refused).unwrap_or(Value::Null))
        .collect();
    value["errors"] = Value::Array(errors);
    value["ok"] = json!(true);
    value["force"] = json!(body.force);
    Ok(Json(value))
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct ListQuery {
    limit: String,
    cursor: String,
}

pub async fn list(
    State(state): State<AppState>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let trash = configured(&state, "/api/trash")?;
    let Query(query) = query
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let limit = query
        .limit
        .parse::<usize>()
        .ok()
        .filter(|limit| *limit > 0)
        .unwrap_or(LIST_DEFAULT);
    let cursor = query.cursor;
    let listing = tokio::task::spawn_blocking(move || {
        trash.list(limit, (!cursor.is_empty()).then_some(cursor.as_str()))
    })
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "trash_failed",
            "回收站任务失败",
        )
    })??;
    let mut value = serde_json::to_value(&listing).map_err(|_| encoding())?;
    value["ok"] = json!(true);
    Ok(Json(value))
}

#[derive(Deserialize)]
pub struct RestoreBody {
    id: String,
}

pub async fn restore(
    State(state): State<AppState>,
    body: Result<Json<RestoreBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let trash = configured(&state, "/api/trash/restore")?;
    let Json(body) = body.map_err(invalid)?;
    let id = body.id.trim().to_owned();
    let restored = state
        .reader
        .run_wait(&state.shutdown, move |store| {
            let restored = trash.restore(&id);
            if restored.is_ok() {
                let _ = store.list(true);
            }
            Ok(restored)
        })
        .await??;
    let mut value = serde_json::to_value(&restored).map_err(|_| encoding())?;
    value["ok"] = json!(true);
    Ok(Json(value))
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct PurgeBody {
    id: Option<String>,
    ids: Option<Vec<String>>,
    all: bool,
    days: Option<u64>,
}

pub async fn purge(
    State(state): State<AppState>,
    body: Result<Json<PurgeBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let trash = configured(&state, "/api/trash/purge")?;
    let Json(body) = body.map_err(invalid)?;
    let single = body
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let explicit_list = body.ids.is_some();
    let mut ids: Vec<String> = body.ids.clone().unwrap_or_default();
    if let Some(id) = single {
        ids.push(id.to_owned());
    }
    let days = if body.all { Some(0) } else { body.days };
    if ids.is_empty() == days.is_none() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_purge",
            "需要 id/ids，或 all:true / days:N（二者不能同时给出）",
        ));
    }
    let outcome = tokio::task::spawn_blocking(move || match days {
        Some(days) => trash.purge_older_than(days),
        None => trash.purge_ids(&ids),
    })
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "trash_failed",
            "回收站任务失败",
        )
    })??;
    if single.is_some() && !explicit_list && outcome.removed == 0 {
        // Python answered a single missing id with 404; keep that for legacy.
        if let Some(first) = outcome.failed.first()
            && first["code"] == "entry_not_found"
        {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "entry_not_found",
                "回收站条目不存在",
            ));
        }
    }
    let mut value = serde_json::to_value(&outcome).map_err(|_| encoding())?;
    value["ok"] = json!(true);
    Ok(Json(value))
}
