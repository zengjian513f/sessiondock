//! Preview and confirm the same connected component used by whole-group copy.
use crate::{
    error::ApiError,
    state::AppState,
    transfer::service::TransferService,
    trash::{RunState, TrashService, tree},
};
use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Deserialize)]
pub struct Request {
    uid: String,
    #[serde(default)]
    request_id: String,
    #[serde(default)]
    uids: BTreeSet<String>,
}
fn services(state: &AppState) -> Result<(Arc<TrashService>, Arc<TransferService>), ApiError> {
    match (&state.trash, &state.transfer) {
        (Some(trash), Some(transfer)) => {
            transfer.store().seed_inventory(&state.reader.store);
            Ok((trash.clone(), transfer.clone()))
        }
        _ => Err(ApiError::unavailable("删除会话树")),
    }
}
fn error(message: impl ToString) -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "tree_delete_refused",
        message.to_string(),
    )
}

pub async fn plan(
    State(state): State<AppState>,
    Json(request): Json<Request>,
) -> Result<Json<Value>, ApiError> {
    let (trash, transfer) = services(&state)?;
    let result = tokio::task::spawn_blocking(move || {
        let snapshot = transfer.inventory_snapshot().map_err(error)?;
        let group = transfer.group_in(&request.uid, &snapshot).map_err(error)?;
        Ok::<_, ApiError>(tree::preview(&trash.plan_tree(&group)?))
    })
    .await
    .map_err(error)??;
    Ok(Json(result))
}
pub async fn progress(
    State(state): State<AppState>,
    Json(request): Json<Request>,
) -> Result<Json<Value>, ApiError> {
    let (trash, _) = services(&state)?;
    Ok(Json(trash.tree_status(&request.request_id, &request.uid)?))
}
pub async fn delete(
    State(state): State<AppState>,
    Json(request): Json<Request>,
) -> Result<Json<Value>, ApiError> {
    let (trash, transfer) = services(&state)?;
    if request.request_id.is_empty() || request.uids.is_empty() {
        return Err(error("请先预览并确认整棵会话树"));
    }
    if trash.start_tree(&request.request_id, &request.uid, &request.uids)? {
        let job_request = request.clone();
        let job_trash = trash.clone();
        // The job owns its lifetime; a closed tab or proxy timeout cannot interrupt it.
        tokio::spawn(async move {
            let result = execute(state, job_trash.clone(), transfer, &job_request).await;
            if let Err(failure) =
                job_trash.finish_tree(&job_request.request_id, result.map_err(|e| e.message))
            {
                crate::log::warn(
                    "trash.tree_delete_receipt_failed",
                    serde_json::json!({"error": failure.to_string()}),
                );
            }
        });
    }
    Ok(Json(trash.tree_status(&request.request_id, &request.uid)?))
}
async fn execute(
    state: AppState,
    trash: Arc<TrashService>,
    transfer: Arc<TransferService>,
    request: &Request,
) -> Result<Vec<crate::trash::Deleted>, ApiError> {
    let work = trash.tree_progress.start(&request.request_id);
    let progress = work.task("删除会话树", "步", Some(100));
    let _guards = transfer
        .locks
        .acquire(
            request
                .uids
                .iter()
                .map(|uid| format!("session:{uid}"))
                .collect(),
        )
        .await;
    let uid = request.uid.clone();
    let expected = request.uids.clone();
    let id = request.request_id.clone();
    let tr = trash.clone();
    let service = transfer.clone();
    let (plan, snapshot) = tokio::task::spawn_blocking(move || {
        let _scope = tr.tree_progress.enter(&id);
        let snapshot = service.inventory_snapshot().map_err(error)?;
        let group = service.group_in(&uid, &snapshot).map_err(error)?;
        if tree::members(&group) != expected {
            return Err(error("会话树成员已变化，请重新预览后确认"));
        }
        if service.any_locked(&expected).map_err(error)? {
            return Err(error("组内会话正在迁移，请先完成迁移"));
        }
        Ok::<_, ApiError>((tr.plan_tree(&group)?, snapshot))
    })
    .await
    .map_err(error)??;
    progress.set(15);
    // Include physical generations and agents hidden from the normal sidebar.
    let mut rows = snapshot.list["sessions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for member in &plan.sessions {
        if !rows.iter().any(|row| row["uid"] == member["uid"]) {
            rows.push(member.clone());
        }
    }
    let liveness =
        super::trash::observe_liveness(&state, &rows, &snapshot.native_catalog()).await?;
    for member in &plan.sessions {
        if matches!(
            liveness.state(member["uid"].as_str().unwrap_or_default()),
            RunState::Running(_)
        ) {
            return Err(error(format!(
                "{} 仍在运行，请先停止；整棵会话树尚未删除。",
                member["title"].as_str().unwrap_or("组内会话")
            )));
        }
    }
    progress.set(25);
    let tr = trash.clone();
    let id = request.request_id.clone();
    let deleted = state
        .reader
        .run_wait(&state.shutdown, move |store| {
            let _scope = tr.tree_progress.enter(&id);
            let deleted = tr.move_tree(&id, &plan);
            let _ = store.list(true);
            Ok(deleted)
        })
        .await?;
    let deleted = deleted?;
    progress.set(95);
    super::trash::retire_deleted_launches(&state, &deleted, &rows).await;
    progress.set(100);
    Ok(deleted)
}
