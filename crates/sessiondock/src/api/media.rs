//! Opaque media transport. File tokens require current native-scope authorization.
use std::sync::Arc;

use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{
    error::ApiError,
    media::MediaBlob,
    sessions::{SessionError, SessionStore},
    state::{AppState, Reader},
};

// Bytes owns this holder, so even a consumer retaining a yielded frame cannot
// release the response/decoded-byte budgets while its data remain alive.
struct MediaBody {
    blob: Arc<MediaBlob>,
    _permit: Arc<OwnedSemaphorePermit>,
}

impl AsRef<[u8]> for MediaBody {
    fn as_ref(&self) -> &[u8] {
        self.blob.bytes()
    }
}

// Reserve at most two shared blocking readers for media work. A request that
// is cancelled after spawning still holds both its work and response permits
// until that worker really finishes; history retains independent reader capacity.
async fn read_media<T, F>(
    reader: &Reader,
    jobs: Arc<Semaphore>,
    response: Arc<OwnedSemaphorePermit>,
    work: F,
) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&SessionStore) -> Result<T, SessionError> + Send + 'static,
{
    // The caller already owns one of the HTTP response permits: admitted
    // requests wait here behind two media workers. Waiting never owns a Reader.
    let job = jobs.acquire_owned().await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "media_busy",
            "图片处理繁忙，请稍后重试",
        )
    })?;
    reader
        .run(move |store| {
            let _job = job;
            let _response = response;
            work(store)
        })
        .await
}

pub async fn get(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Response, ApiError> {
    if token.len() != 32
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "media_not_found",
            "图片不存在或已过期，请重新加载会话",
        ));
    }
    if state.shutdown.is_cancelled() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "shutdown",
            "服务正在关闭",
        ));
    }
    let permit = Arc::new(crate::state::admit(&state.media_http, "media_busy").await?);
    let media = state.media.clone();
    let files = state.files.clone();
    let blob = read_media(
        &state.reader,
        state.media_jobs.clone(),
        permit.clone(),
        move |store| {
            let ticket = media.ticket(&token).ok_or_else(|| SessionError {
                status: 404,
                message: "图片不存在或已过期，请重新加载会话".to_owned(),
            })?;
            let result = if let Some((uid, agent, span)) = ticket.native_grant() {
                let mut reader = store.native_media_reader(uid, agent, span)?;
                let result = media.materialize_native(ticket, &mut reader);
                // No HTTP response is published before both decoded content and
                // the retained checked native handle pass final verification.
                if result.is_ok() {
                    reader.finish()?;
                }
                result
            } else if let Some((uid, agent)) = ticket.scope() {
                let files = files.as_deref().ok_or_else(|| SessionError {
                    status: 501,
                    message: "未配置图片读取目录".into(),
                })?;
                let snapshot = store.snapshot(uid, agent)?;
                let scope = snapshot.media_scope(files).map_err(|error| SessionError {
                    status: error.status,
                    message: error.message,
                })?;
                media.materialize(ticket, Some(&scope))
            } else {
                media.materialize(ticket, None)
            };
            result.map_err(|error| SessionError {
                status: error.status,
                message: error.message,
            })
        },
    )
    .await?;
    let mime = blob.mime().to_owned();
    let length = blob.bytes().len();
    let extension = blob.extension().to_owned();
    let body = Bytes::from_owner(MediaBody {
        blob,
        _permit: permit,
    });
    let mut response = Body::from(body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        mime.parse().expect("validated media MIME"),
    );
    headers.insert(header::CONTENT_LENGTH, length.into());
    headers.insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("inline; filename=\"image.{extension}\"")
            .parse()
            .unwrap(),
    );
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    Ok(response)
}
