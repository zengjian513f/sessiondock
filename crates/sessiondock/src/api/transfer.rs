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
fn service(state: &AppState) -> Result<Arc<TransferService>, Box<Response>> {
    state.transfer.clone().ok_or_else(|| {
        Box::new(failure(TransferError::new(
            "move_group_unsupported",
            "此节点未启用会话整组复制",
        )))
    })
}
async fn stopped(state: &AppState, op: &Operation) -> Result<(), Box<Response>> {
    let (_, live) = super::trash::frozen_liveness(state)
        .await
        .map_err(|error| Box::new(error.into_response()))?;
    for member in &op.group().members {
        let uid = if op.incoming_digest.is_some() {
            match (&state.transfer, &op.staged) {
                (Some(service), Some(staged)) => service
                    .member_target_uid(op, member, staged)
                    .map_err(|error| Box::new(failure(error)))?,
                _ => member.uid.clone(),
            }
        } else {
            member.uid.clone()
        };
        if matches!(live.state(&uid), RunState::Running(_)) {
            return Err(Box::new(failure(TransferError::new(
                "move_session_running",
                format!("会话仍在运行：{}", member.title),
            ))));
        }
    }
    Ok(())
}
#[derive(Deserialize)]
pub struct PlanRequest {
    uid: String,
    #[serde(default)]
    new_ids: Option<bool>,
    #[serde(default)]
    mode: Option<String>,
}
#[derive(Deserialize)]
pub struct ExecuteRequest {
    uid: String,
    operation_id: String,
}
pub async fn plan(State(state): State<AppState>, Json(body): Json<PlanRequest>) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let _guard = service.session_guard(&body.uid).await;
    let copy = service.clone();
    let selected = body.uid;
    let moving = match body.mode.as_deref().unwrap_or("clone") {
        "clone" => false,
        "move" => true,
        _ => return failure(TransferError::new("move_format", "未知的操作类型")),
    };
    if moving && state.trash.is_none() {
        return failure(TransferError::new(
            "move_group_unsupported",
            "源机器未配置回收站",
        ));
    }
    let new_ids = body.new_ids.unwrap_or(!moving);
    let op = match tokio::task::spawn_blocking(move || {
        if moving {
            copy.plan_move(&selected, new_ids)
        } else {
            copy.plan_copy(&selected, new_ids)
        }
    })
    .await
    {
        Ok(Ok(op)) => op,
        Ok(Err(e)) => return failure(e),
        Err(e) => return failure(TransferError::new("move_io", e.to_string())),
    };
    if let Err(e) = stopped(&state, &op).await {
        return *e;
    }
    Json(TransferService::public(&op)).into_response()
}

pub async fn clone_progress(
    State(state): State<AppState>,
    Json(body): Json<ExecuteRequest>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    match service.load(&body.operation_id) {
        Ok(op) if op.uid == body.uid && op.incoming_digest.is_none() => {
            service.touch(&op.id);
            Json(TransferService::public(&op)).into_response()
        }
        Ok(_) => failure(TransferError::new(
            "move_plan_stale",
            "复制清单与所选会话不一致",
        )),
        Err(e) => failure(e),
    }
}

/// Browser cancellation for a same-node copy or an unconfirmed preview.
pub async fn cancel_clone(
    State(state): State<AppState>,
    Json(body): Json<ExecuteRequest>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let op = match service.load(&body.operation_id) {
        Ok(op) if op.uid == body.uid && op.incoming_digest.is_none() => op,
        Ok(_) => {
            return failure(TransferError::new(
                "move_plan_stale",
                "复制清单与所选会话不一致",
            ));
        }
        Err(e) => return failure(e),
    };
    service.interrupts.cancel(&op.id);
    let guard = match service.operation_guard(&op.id).await {
        Ok(v) => v,
        Err(e) => return failure(e),
    };
    let result = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        if op.moving {
            service.abort_source(&op.id, false)?;
            service.abort_source(&op.id, true)
        } else {
            service.abort_local(&op.id)
        }
    })
    .await;
    match result {
        Ok(Ok(op)) => Json(TransferService::public(&op)).into_response(),
        Ok(Err(e)) => failure(e),
        Err(e) => failure(TransferError::new("move_io", e.to_string())),
    }
}

pub async fn abort_move(State(state): State<AppState>, Json(body): Json<AbortRequest>) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let guard = match service.operation_guard(&body.operation_id).await {
        Ok(guard) => guard,
        Err(e) => return failure(e),
    };
    if body.step == "target" {
        let op = match service.load(&body.operation_id) {
            Ok(op) => op,
            Err(e) => return failure(e),
        };
        if op.phase != "complete"
            && let Err(e) = stopped(&state, &op).await
        {
            return *e;
        }
    }
    // Compensation and its gate survive a disconnected Hub request.
    let result = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        match body.step.as_str() {
            "source" => service.abort_source(&body.operation_id, false),
            "target" => service.abort_target(&body.operation_id),
            "local" => service.abort_local(&body.operation_id),
            "finish" => service.abort_source(&body.operation_id, true),
            _ => Err(TransferError::new("move_format", "未知的撤回步骤")),
        }
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
pub struct AbortRequest {
    operation_id: String,
    step: String,
}

pub async fn switch_source(
    State(state): State<AppState>,
    Json(body): Json<TransferId>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let guard = match service.operation_guard(&body.operation_id).await {
        Ok(guard) => guard,
        Err(e) => return failure(e),
    };
    let op = match service.load(&body.operation_id) {
        Ok(op) => op,
        Err(e) => return failure(e),
    };
    if let Err(e) = stopped(&state, &op).await {
        return *e;
    }
    match tokio::task::spawn_blocking(move || {
        let _guard = guard;
        service.switch_source(&body.operation_id)
    })
    .await
    {
        Ok(Ok(op)) => Json(TransferService::public(&op)).into_response(),
        Ok(Err(e)) => failure(e),
        Err(e) => failure(TransferError::new("move_io", e.to_string())),
    }
}
pub async fn activate_target(
    State(state): State<AppState>,
    Json(body): Json<TransferId>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let guard = match service.operation_guard(&body.operation_id).await {
        Ok(guard) => guard,
        Err(e) => return failure(e),
    };
    match tokio::task::spawn_blocking(move || {
        let _guard = guard;
        service.activate_target(&body.operation_id)
    })
    .await
    {
        Ok(Ok(op)) => Json(TransferService::public(&op)).into_response(),
        Ok(Err(e)) => failure(e),
        Err(e) => failure(TransferError::new("move_io", e.to_string())),
    }
}
pub async fn retire_source(
    State(state): State<AppState>,
    Json(body): Json<TransferId>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let Some(trash) = state.trash.clone() else {
        return failure(TransferError::new(
            "move_group_unsupported",
            "源机器未配置回收站",
        ));
    };
    let guard = match service.operation_guard(&body.operation_id).await {
        Ok(guard) => guard,
        Err(e) => return failure(e),
    };
    let op = match service.load(&body.operation_id) {
        Ok(op) => op,
        Err(e) => return failure(e),
    };
    // A late retry must not clean receipts or drafts created after this group
    // returned to the source under a newer operation.
    if op.phase == "retired" && op.moving && op.incoming_digest.is_none() {
        return Json(TransferService::public(&op)).into_response();
    }
    if let Err(e) = stopped(&state, &op).await {
        return *e;
    }
    // Keep both the journal gate and asynchronous receipt cleanup alive if
    // the Hub disconnects while source retirement is in flight.
    let task = tokio::spawn(async move {
        let _guard = guard;
        let copy = service.clone();
        let mut op = tokio::task::spawn_blocking(move || {
            copy.retire_source(&body.operation_id, trash.directory())
        })
        .await
        .map_err(|e| TransferError::new("move_io", e.to_string()))??;
        let mut retired_launches = std::collections::HashSet::new();
        if let Some(lifecycle) = &state.lifecycle {
            let aliases = if let Some(conversations) = &state.conversations {
                let store = conversations.store.clone();
                let uids: Vec<_> = op.group().members.iter().map(|m| m.uid.clone()).collect();
                tokio::task::spawn_blocking(move || {
                    uids.iter()
                        .flat_map(|uid| store.launch_records_for(uid))
                        .collect::<std::collections::HashSet<_>>()
                })
                .await
                .map_err(|e| TransferError::new("move_cleanup", e.to_string()))?
            } else {
                Default::default()
            };
            let records = lifecycle
                .list(0, usize::MAX)
                .await
                .map_err(|e| TransferError::new("move_cleanup", e.to_string()))?;
            for record in records {
                use crate::lifecycle::model::{BindingState, Source};
                let belongs = op.group().members.iter().any(|m| {
                    record.declared_uid() == Some(m.uid.as_str())
                        || (matches!(
                            (record.spec().source(), m.source.as_str()),
                            (Source::Codex, "codex")
                                | (Source::Claude, "claude")
                                | (Source::Grok, "grok")
                        ) && (record.declared_sid() == Some(m.sid.as_str())
                            || record.session_id() == Some(m.sid.as_str())))
                        || record.binding().is_some_and(|binding| {
                            binding.state() == BindingState::Confirmed
                                && binding.spec().uid() == m.uid
                        })
                }) || aliases.contains(record.record_id());
                if belongs && record.discardable() {
                    if !record.discarded() {
                        lifecycle
                            .discard(
                                record.record_id().to_owned(),
                                record.instance_id().to_owned(),
                            )
                            .await
                            .map_err(|e| TransferError::new("move_cleanup", e.to_string()))?;
                    }
                    // Discard can have succeeded before draft cleanup failed.
                    // Retry that cleanup even for an already discarded receipt.
                    retired_launches.insert(record.record_id().to_owned());
                }
            }
        }
        if let Some(conversations) = &state.conversations {
            let store = conversations.store.clone();
            let uids = op.group().members.iter().map(|m| m.uid.clone()).collect();
            tokio::task::spawn_blocking(move || {
                store.retire_drafts_and_launches(&uids, &retired_launches)
            })
            .await
            .map_err(|e| TransferError::new("move_io", e.to_string()))?
            .map_err(|e| TransferError::new("move_cleanup", e.message))?;
        } else if let Some(metadata) = &state.metadata {
            // Drafts may outlive a temporarily disabled terminal subsystem.
            let directory = metadata.directory().join("conversations");
            let uids = op.group().members.iter().map(|m| m.uid.clone()).collect();
            tokio::task::spawn_blocking(move || {
                if directory.is_dir() {
                    crate::conversation::store::Store::open(&directory)?.retire_drafts(&uids)?;
                }
                Ok::<_, crate::delivery::target::Failure>(())
            })
            .await
            .map_err(|e| TransferError::new("move_io", e.to_string()))?
            .map_err(|e| TransferError::new("move_cleanup", e.message))?;
        }
        op = service.finish_retirement(op)?;
        let _ = state.reader.run(|store| store.list(true)).await;
        Ok::<_, TransferError>(op)
    });
    match task.await {
        Ok(Ok(op)) => Json(TransferService::public(&op)).into_response(),
        Ok(Err(e)) => failure(e),
        Err(e) => failure(TransferError::new("move_io", e.to_string())),
    }
}
pub async fn execute(State(state): State<AppState>, Json(body): Json<ExecuteRequest>) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let guard = match service.operation_guard(&body.operation_id).await {
        Ok(guard) => guard,
        Err(e) => return failure(e),
    };
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
    if op.phase != "complete"
        && let Err(e) = stopped(&state, &op).await
    {
        return *e;
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
    #[serde(default)]
    reset: bool,
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
        Err(e) => return *e,
    };
    let guard = match service.operation_guard(&body.operation_id).await {
        Ok(guard) => guard,
        Err(e) => return failure(e),
    };
    let copy = service.clone();
    let op =
        match tokio::task::spawn_blocking(move || copy.reserve_export(&body.operation_id)).await {
            Ok(Ok(op)) => op,
            Ok(Err(e)) => return failure(e),
            Err(e) => return failure(TransferError::new("move_io", e.to_string())),
        };
    if let Err(error) = stopped(&state, &op).await {
        let _ = service.release_export(&op.id, false);
        return *error;
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
        let _scope = crate::transfer::coordination::Scope::enter(copy.interrupts.flag(&copy_op.id));
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
        return *error;
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
            crate::transfer::coordination::check().map_err(std::io::Error::other)?;
            match self.receiver.try_recv() {
                Ok(Ok(bytes)) => self.current = std::io::Cursor::new(bytes),
                Ok(Err(e)) => return Err(std::io::Error::other(e)),
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return Ok(0),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
            }
        }
    }
}
pub async fn receive_bundle(State(state): State<AppState>, body: axum::body::Body) -> Response {
    use futures_util::StreamExt;
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    let mut worker = tokio::task::spawn_blocking(move || {
        service.receive_bundle(ChannelReader {
            receiver,
            current: std::io::Cursor::new(axum::body::Bytes::new()),
        })
    });
    let mut stream = body.into_data_stream();
    let finished = loop {
        tokio::select! {
            result = &mut worker => break Some(result),
            chunk = stream.next() => match chunk {
                Some(chunk) => if sender.send(chunk.map_err(|e| e.to_string())).await.is_err() { break None; },
                None => break None,
            }
        }
    };
    drop(sender);
    let result = match finished {
        Some(result) => result,
        None => worker.await,
    };
    match result {
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
        Err(e) => return *e,
    };
    let guard = match service.operation_guard(&body.operation_id).await {
        Ok(guard) => guard,
        Err(e) => return failure(e),
    };
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
        Err(e) => return *e,
    };
    let _guard = match service.operation_guard(&body.operation_id).await {
        Ok(guard) => guard,
        Err(e) => return failure(e),
    };
    let copy = service.clone();
    let result = tokio::task::spawn_blocking(move || copy.reserve_export(&body.operation_id)).await;
    match result {
        Ok(Ok(op)) => {
            if let Err(error) = stopped(&state, &op).await {
                let _ = service.release_export(&op.id, false);
                return *error;
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
        Err(e) => return *e,
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
        Err(e) => return *e,
    };
    let guard = match service.operation_guard(&body.operation_id).await {
        Ok(guard) => guard,
        Err(e) => return failure(e),
    };
    let result = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let _scope = crate::transfer::coordination::Scope::enter(
            service.interrupts.flag(&body.operation_id),
        );
        crate::transfer::coordination::check()?;
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
        Err(e) => return *e,
    };
    let guard = service
        .locks
        .acquire(service.operation_keys(&manifest.operation))
        .await;
    let result = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let _scope = crate::transfer::coordination::Scope::enter(
            service.interrupts.flag(&manifest.operation.id),
        );
        crate::transfer::coordination::check()?;
        service.validate_bundle(&manifest)?;
        service.verify_environment(&manifest)?;
        // Preserved identities require the incoming history bytes to prove
        // a prefix; execute performs that proof before publishing any file.
        if manifest.operation.new_ids() {
            crate::transfer::native::preflight_copy(
                manifest.operation.rewritten.as_ref().unwrap(),
                false,
            )?;
        }
        Ok::<_, TransferError>(())
    })
    .await;
    match result {
        Ok(Ok(())) => Json(json!({"ready":true,"move_handoff":true})).into_response(),
        Ok(Err(e)) => failure(e),
        Err(e) => failure(TransferError::new("move_io", e.to_string())),
    }
}

/// Signal a preparatory worker without waiting for its session locks.
pub async fn interrupt(State(state): State<AppState>, Json(body): Json<TransferId>) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    if body.reset {
        service.interrupts.reset(&body.operation_id);
    } else {
        service.interrupts.cancel(&body.operation_id);
    }
    Json(json!({"cancel_requested":true})).into_response()
}
