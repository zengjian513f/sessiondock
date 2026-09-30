//! Browser-facing clone orchestration. Paths and identity maps never come from
//! the browser; the operation journal owns both between plan and confirmation.
use crate::{
    state::AppState,
    transfer::{
        TransferError,
        service::{Operation, TransferService},
    },
    trash::RunState,
};
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

fn failure(error: TransferError) -> Response {
    let status = match error.code.as_str() {
        "move_io" | "move_native_database" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::CONFLICT,
    };
    (
        status,
        Json(json!({"code":error.code,"error":error.message})),
    )
        .into_response()
}
fn service(state: &AppState) -> Result<Arc<TransferService>, Response> {
    state.transfer.clone().ok_or_else(|| {
        failure(TransferError::new(
            "move_group_unsupported",
            "此节点未启用会话整组复制",
        ))
    })
}
async fn stopped(state: &AppState, op: &Operation) -> Result<(), Response> {
    let (_, live) = super::trash::frozen_liveness(state)
        .await
        .map_err(IntoResponse::into_response)?;
    for member in &op.group().members {
        if matches!(live.state(&member.uid), RunState::Running(_)) {
            return Err(failure(TransferError::new(
                "move_session_running",
                format!("会话仍在运行：{}", member.title),
            )));
        }
    }
    Ok(())
}
#[derive(Deserialize)]
pub struct PlanRequest {
    uid: String,
}
#[derive(Deserialize)]
pub struct ExecuteRequest {
    uid: String,
    operation_id: String,
}
pub async fn plan(State(state): State<AppState>, Json(body): Json<PlanRequest>) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let _guard = service.gate.clone().lock_owned().await;
    let copy = service.clone();
    let selected = body.uid;
    let op = match tokio::task::spawn_blocking(move || copy.plan(&selected)).await {
        Ok(Ok(op)) => op,
        Ok(Err(e)) => return failure(e),
        Err(e) => return failure(TransferError::new("move_io", e.to_string())),
    };
    if let Err(e) = stopped(&state, &op).await {
        return e;
    }
    Json(TransferService::public(&op)).into_response()
}
pub async fn execute(State(state): State<AppState>, Json(body): Json<ExecuteRequest>) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let guard = service.gate.clone().lock_owned().await;
    let op = match service.load(&body.operation_id) {
        Ok(op) if op.uid == body.uid => op,
        Ok(_) => {
            return failure(TransferError::new(
                "move_plan_stale",
                "复制清单与所选会话不一致",
            ));
        }
        Err(e) => return failure(e),
    };
    if op.phase != "complete" {
        if let Err(e) = stopped(&state, &op).await {
            return e;
        }
    }
    // The transaction and gate outlive a cancelled HTTP request.
    let result = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        service.execute(op)
    })
    .await;
    match result {
        Ok(Ok(op)) => {
            let _ = state.reader.run(|store| store.list(true)).await;
            Json(TransferService::public(&op)).into_response()
        }
        Ok(Err(e)) => failure(e),
        Err(e) => failure(TransferError::new("move_io", e.to_string())),
    }
}
