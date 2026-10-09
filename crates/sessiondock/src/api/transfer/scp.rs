//! Private node routes, never reachable through the browser proxy. The Hub
//! supplies an administrator-selected SSH endpoint and a source-issued archive.
use super::*;
use crate::transfer::transport::{Archive, Endpoint};

pub(super) fn lock_key(id: &str) -> String {
    format!("transport:{id}")
}

pub async fn prepare(State(state): State<AppState>, Json(body): Json<TransferId>) -> Response {
    // Keep the operation gate alive until preparation finishes even if the Hub
    // disconnects. Abort waits for the existing operation gate before cleanup.
    match tokio::spawn(async move { super::prepare_archive(&state, body.operation_id).await }).await
    {
        Ok(Ok(archive)) => Json(archive).into_response(),
        Ok(Err(error)) => *error,
        Err(error) => failure(TransferError::new("move_io", error.to_string())),
    }
}

#[derive(Deserialize)]
pub struct Pull {
    source: Endpoint,
    archive: Archive,
}

pub async fn receive(State(state): State<AppState>, Json(body): Json<Pull>) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    if let Err(error) = body.source.validate().and_then(|_| body.archive.validate()) {
        return failure(error);
    }
    // Cancellation of the HTTP waiter never leaves an untracked scp running.
    // Interrupt sets the flag and waits on this same gate before acknowledging.
    let shutdown = state.shutdown.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        let _guard = service
            .locks
            .blocking(vec![lock_key(&body.archive.operation_id)]);
        let _scope = crate::transfer::coordination::Scope::enter(
            service.interrupts.flag(&body.archive.operation_id),
        );
        crate::transfer::coordination::check()?;
        let _work = service.progress.enter(&body.archive.operation_id);
        if service
            .directory
            .join(&body.archive.operation_id)
            .join("operation.json")
            .exists()
        {
            let op = service.load(&body.archive.operation_id)?;
            if op.incoming_digest.is_some() && op.phase != "aborted" && op.phase != "aborting" {
                return Ok(op);
            }
            return Err(TransferError::new(
                "move_conflict",
                "目标已有不同的迁移操作",
            ));
        }
        let directory = service
            .directory
            .join(format!("incoming-{}", crate::transfer::codex::uuid()?));
        std::fs::create_dir(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let path = directory.join("archive.tar");
        let result = (|| {
            crate::transfer::transport::pull(&body.source, &body.archive, &path, &shutdown)?;
            crate::transfer::coordination::check()?;
            service.receive_bundle_for(
                std::fs::File::open(&path)?,
                Some(&body.archive.operation_id),
            )
        })();
        // A reported transport failure guarantees the subprocess is gone and
        // the partial archive removed, so the Hub can safely choose its relay.
        std::fs::remove_dir_all(&directory)?;
        result
    });
    // JSON permits leading whitespace. Keep the private control response alive
    // while a large archive is progressing; the final JSON carries the result
    // or error. There is no total transfer-duration limit.
    let stream = async_stream::stream! {
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            tokio::select! {
                result = &mut worker => {
                    let value = match result {
                        Ok(Ok(op)) => TransferService::public(&op),
                        Ok(Err(error)) => json!({"code":error.code,"error":error.message}),
                        Err(error) => json!({"code":"move_io","error":error.to_string()}),
                    };
                    yield Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(value.to_string()));
                    break;
                }
                _ = heartbeat.tick() => yield Ok(axum::body::Bytes::from_static(b"\n")),
            }
        }
    };
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

/// Wait out an uncertain SCP request before reading the durable receive result.
pub async fn settle(State(state): State<AppState>, Json(body): Json<TransferId>) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let _guard = service
        .locks
        .acquire(vec![lock_key(&body.operation_id)])
        .await;
    super::transfer_status(State(state), Json(body)).await
}

pub async fn release(State(state): State<AppState>, Json(archive): Json<Archive>) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    if let Err(error) = archive.validate() {
        return failure(error);
    }
    let _guard = match service.operation_guard(&archive.operation_id).await {
        Ok(g) => g,
        Err(e) => return failure(e),
    };
    // Never remove a caller-selected path outside this node's operation folder.
    let expected = service
        .directory
        .join(&archive.operation_id)
        .join(archive.path.file_name().unwrap());
    if std::path::absolute(&expected).ok().as_ref() != Some(&archive.path) {
        return failure(TransferError::new("move_format", "迁移包不属于此节点"));
    }
    match std::fs::remove_file(expected) {
        Ok(()) => Json(json!({"released":true})).into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Json(json!({"released":true})).into_response()
        }
        Err(error) => failure(error.into()),
    }
}
