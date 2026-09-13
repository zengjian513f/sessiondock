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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::SessionRoots;
    use std::time::Duration;

    #[tokio::test]
    async fn cancelled_media_workers_keep_both_budgets_and_leave_history_capacity() {
        let reader = Reader {
            store: Arc::new(SessionStore::new(SessionRoots::default())),
            workers: Arc::new(Semaphore::new(4)),
        };
        let jobs = Arc::new(Semaphore::new(2));
        let responses = Arc::new(Semaphore::new(8));
        let mut tasks = Vec::new();
        let mut releases = Vec::new();
        for _ in 0..2 {
            let (started, entered) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel();
            let reader = reader.clone();
            let jobs = jobs.clone();
            let permit = Arc::new(responses.clone().try_acquire_owned().unwrap());
            tasks.push(tokio::spawn(async move {
                read_media(&reader, jobs, permit, move |_| {
                    started.send(()).unwrap();
                    // Deterministic worker barrier, never a wall-clock sleep.
                    wait.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(())
                })
                .await
            }));
            tokio::time::timeout(Duration::from_secs(5), entered)
                .await
                .unwrap()
                .unwrap();
            releases.push(release);
        }
        assert_eq!(jobs.available_permits(), 0);
        assert_eq!(responses.available_permits(), 6);
        assert_eq!(reader.workers.available_permits(), 2);
        let waiting_reader = reader.clone();
        let waiting_jobs = jobs.clone();
        let waiting_response = Arc::new(responses.clone().try_acquire_owned().unwrap());
        let (waiting, entered) = tokio::sync::oneshot::channel();
        let pending = tokio::spawn(async move {
            waiting.send(()).unwrap();
            read_media(
                &waiting_reader,
                waiting_jobs,
                waiting_response,
                |_| -> Result<(), SessionError> {
                    panic!("queued work must not enter a saturated worker pool")
                },
            )
            .await
        });
        entered.await.unwrap();
        assert!(!pending.is_finished());
        assert_eq!(reader.workers.available_permits(), 2);
        assert_eq!(responses.available_permits(), 5);
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        assert_eq!(responses.available_permits(), 6);
        for task in tasks {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        }
        assert_eq!(jobs.available_permits(), 0);
        assert_eq!(responses.available_permits(), 6);
        assert_eq!(reader.run(|_| Ok(73)).await.unwrap(), 73);
        for release in releases {
            release.send(()).unwrap();
        }
        let returned =
            tokio::time::timeout(Duration::from_secs(5), jobs.clone().acquire_many_owned(2))
                .await
                .unwrap()
                .unwrap();
        drop(returned);
        assert_eq!(responses.available_permits(), 8);
        let workers = tokio::time::timeout(
            Duration::from_secs(5),
            reader.workers.clone().acquire_many_owned(4),
        )
        .await
        .unwrap()
        .unwrap();
        drop(workers);
        assert_eq!(reader.workers.available_permits(), 4);
    }
}
