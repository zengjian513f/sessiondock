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
    #[serde(default = "default_new_ids")]
    new_ids: bool,
}
fn default_new_ids() -> bool {
    true
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
    let op =
        match tokio::task::spawn_blocking(move || copy.plan_copy(&selected, body.new_ids)).await {
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

#[derive(Deserialize)]
pub struct TransferId {
    operation_id: String,
    #[serde(default)]
    completed: bool,
}

/// These routes are mounted only on the authenticated node listener.
pub async fn export_bundle(
    State(state): State<AppState>,
    Json(body): Json<TransferId>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let guard = service.gate.clone().lock_owned().await;
    let copy = service.clone();
    let op =
        match tokio::task::spawn_blocking(move || copy.reserve_export(&body.operation_id)).await {
            Ok(Ok(op)) => op,
            Ok(Err(e)) => return failure(e),
            Err(e) => return failure(TransferError::new("move_io", e.to_string())),
        };
    if let Err(error) = stopped(&state, &op).await {
        let _ = service.release_export(&op.id, false);
        return error;
    }
    let name = match crate::transfer::codex::uuid() {
        Ok(v) => v,
        Err(e) => return failure(e),
    };
    let path = service
        .directory
        .join(&op.id)
        .join(format!("export-{name}.tar"));
    let copy = service.clone();
    let copy_op = op.clone();
    let copy_path = path.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        copy.export_bundle(&copy_op, &copy_path)
    })
    .await;
    match result {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            let _ = service.release_export(&op.id, false);
            return failure(e);
        }
        Err(e) => {
            let _ = service.release_export(&op.id, false);
            return failure(TransferError::new("move_io", e.to_string()));
        }
    }
    if let Err(error) = stopped(&state, &op).await {
        let _ = service.release_export(&op.id, false);
        let _ = std::fs::remove_file(&path);
        return error;
    }
    let file = match std::fs::File::open(&path) {
        Ok(v) => v,
        Err(e) => return failure(e.into()),
    };
    let length = match file.metadata() {
        Ok(v) => v.len(),
        Err(e) => return failure(e.into()),
    };
    let (sender, mut receiver) =
        tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(4);
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut file = file;
        loop {
            let mut bytes = vec![0; 65536];
            match file.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    bytes.truncate(n);
                    if sender.blocking_send(Ok(bytes.into())).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = sender.blocking_send(Err(e));
                    break;
                }
            }
        }
        let _ = std::fs::remove_file(path);
    });
    let stream = async_stream::stream! {while let Some(chunk)=receiver.recv().await {yield chunk;}};
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/x-tar".to_owned(),
            ),
            (axum::http::header::CONTENT_LENGTH, length.to_string()),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

struct ChannelReader {
    receiver: tokio::sync::mpsc::Receiver<Result<axum::body::Bytes, String>>,
    current: std::io::Cursor<axum::body::Bytes>,
}
impl std::io::Read for ChannelReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            let n = std::io::Read::read(&mut self.current, output)?;
            if n != 0 {
                return Ok(n);
            }
            match self.receiver.blocking_recv() {
                Some(Ok(bytes)) => self.current = std::io::Cursor::new(bytes),
                Some(Err(e)) => return Err(std::io::Error::other(e)),
                None => return Ok(0),
            }
        }
    }
}
pub async fn receive_bundle(State(state): State<AppState>, body: axum::body::Body) -> Response {
    use futures_util::StreamExt;
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let guard = service.gate.clone().lock_owned().await;
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    let worker = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        service.receive_bundle(ChannelReader {
            receiver,
            current: std::io::Cursor::new(axum::body::Bytes::new()),
        })
    });
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        if sender.send(chunk.map_err(|e| e.to_string())).await.is_err() {
            break;
        }
    }
    drop(sender);
    match worker.await {
        Ok(Ok(op)) => Json(TransferService::public(&op)).into_response(),
        Ok(Err(e)) => failure(e),
        Err(e) => failure(TransferError::new("move_io", e.to_string())),
    }
}
pub async fn release_export(
    State(state): State<AppState>,
    Json(body): Json<TransferId>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let guard = service.gate.clone().lock_owned().await;
    let result = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        service.release_export(&body.operation_id, body.completed)
    })
    .await;
    match result {
        Ok(Ok(op)) => Json(TransferService::public(&op)).into_response(),
        Ok(Err(e)) => failure(e),
        Err(e) => failure(TransferError::new("move_io", e.to_string())),
    }
}

pub async fn reserve_export(
    State(state): State<AppState>,
    Json(body): Json<TransferId>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let _guard = service.gate.clone().lock_owned().await;
    let copy = service.clone();
    let result = tokio::task::spawn_blocking(move || copy.reserve_export(&body.operation_id)).await;
    match result {
        Ok(Ok(op)) => {
            if let Err(error) = stopped(&state, &op).await {
                let _ = service.release_export(&op.id, false);
                return error;
            }
            Json(TransferService::public(&op)).into_response()
        }
        Ok(Err(e)) => failure(e),
        Err(e) => failure(TransferError::new("move_io", e.to_string())),
    }
}
pub async fn transfer_status(
    State(state): State<AppState>,
    Json(body): Json<TransferId>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    match service.load(&body.operation_id) {
        Ok(op) => {
            let mut public = TransferService::public(&op);
            public["incoming"] = op.incoming_digest.is_some().into();
            Json(public).into_response()
        }
        Err(e) if e.code == "move_io" => (
            StatusCode::NOT_FOUND,
            Json(json!({"code":"move_plan_stale","error":"迁移操作不存在"})),
        )
            .into_response(),
        Err(e) => failure(e),
    }
}

pub async fn bundle_manifest(
    State(state): State<AppState>,
    Json(body): Json<TransferId>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let guard = service.gate.clone().lock_owned().await;
    let result = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let op = service.load(&body.operation_id)?;
        service.bundle_manifest(&op)
    })
    .await;
    match result {
        Ok(Ok(manifest)) => Json(manifest).into_response(),
        Ok(Err(e)) => failure(e),
        Err(e) => failure(TransferError::new("move_io", e.to_string())),
    }
}
pub async fn check_bundle(
    State(state): State<AppState>,
    Json(manifest): Json<crate::transfer::bundle::Manifest>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let guard = service.gate.clone().lock_owned().await;
    let result = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        service.validate_bundle(&manifest)?;
        for snapshot in &manifest.environment {
            let dependencies = snapshot.dependencies.keys().cloned().collect::<Vec<_>>();
            snapshot.compare(&crate::transfer::environment::Snapshot::capture(
                &snapshot.cwd,
                &dependencies,
            )?)?;
        }
        crate::transfer::native::preflight_copy(
            manifest.operation.rewritten.as_ref().unwrap(),
            !manifest.operation.new_ids(),
        )?;
        Ok::<_, TransferError>(())
    })
    .await;
    match result {
        Ok(Ok(())) => Json(json!({"ready":true})).into_response(),
        Ok(Err(e)) => failure(e),
        Err(e) => failure(TransferError::new("move_io", e.to_string())),
    }
}
