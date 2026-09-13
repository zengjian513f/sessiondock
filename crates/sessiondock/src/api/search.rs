//! JSON/NDJSON search transport: bounded search admission (two at once, a
//! bounded wait, then 503 `search_busy`) and eight queued packets. Search
//! never takes a read-pool permit: bodies come from the search-text cache and
//! its own parse budget (`search::service`). Dropping a response cancels its
//! workers and unblocks sends.

use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    body::{Body, Bytes},
    extract::{Query, State, rejection::QueryRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use crate::{
    error::ApiError,
    search::{self, SearchError, SearchQuery},
    state::{AppState, JsonBytes},
};

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// How long a third concurrent search waits for one of the two slots.
const SEARCH_ADMISSION_WAIT: Duration = Duration::from_secs(10);

/// One search over the frozen pool: every worker thread owns one reusable
/// chunk buffer; bodies are never retained beyond their match.
fn run(
    service: &crate::search::service::SearchService,
    pool: &crate::sessions::SearchPool,
    query: &search::PreparedSearch,
    cancelled: &AtomicBool,
    emit: impl FnMut(Value) -> Result<(), SearchError>,
) -> Result<Value, SearchError> {
    let produced = service.produced();
    let result = search::execute(
        &pool.rows,
        |uid, cancelled, buffer| service.scan(pool, uid, query, cancelled, buffer),
        query,
        cancelled,
        emit,
        service.workers,
    );
    if service.produced() != produced {
        crate::search::service::release_memory();
    }
    result
}

fn api_error(error: SearchError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        error.code,
        error.message,
    )
}

fn packet(value: Value) -> Bytes {
    let mut data = serde_json::to_vec(&value).expect("JSON value serializes");
    data.push(b'\n');
    data.into()
}

fn ndjson(body: Body) -> Response {
    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/x-ndjson; charset=utf-8".parse().unwrap(),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    response
}

fn events(
    mut rx: tokio::sync::mpsc::Receiver<Bytes>,
    guard: CancelOnDrop,
    permit: Arc<tokio::sync::OwnedSemaphorePermit>,
    shutdown: tokio_util::sync::CancellationToken,
) -> Body {
    let stream = async_stream::stream! {
        let _guard = guard;
        let _permit = permit;
        let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                next = rx.recv() => match next {
                    Some(data) => yield Ok::<Bytes, Infallible>(data),
                    None => break,
                },
                _ = heartbeat.tick() => yield Ok(packet(json!({"type": "heartbeat"}))),
            }
        }
    };
    Body::from_stream(stream)
}

pub async fn get(
    State(state): State<AppState>,
    query: Result<Query<SearchQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_search_query",
            error.body_text(),
        )
    })?;
    let query = query.prepare().map_err(api_error)?;
    if query.is_empty() {
        let data = search::empty_result();
        return Ok(if query.progress {
            ndjson(Body::from(packet(json!({"type": "result", "data": data}))))
        } else {
            JsonBytes::new(&data).into_response()
        });
    }
    let busy = || {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "search_busy",
            "同时搜索过多，请稍后重试",
        )
    };
    let search_permit = Arc::new(
        match tokio::time::timeout(
            SEARCH_ADMISSION_WAIT,
            state.searches.clone().acquire_owned(),
        )
        .await
        {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) | Err(_) => return Err(busy()),
        },
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    let guard = CancelOnDrop(cancelled.clone());
    let service = state.search.clone();
    let job_permit = search_permit.clone();
    if !query.progress {
        let job = tokio::task::spawn_blocking(move || {
            let _permit = job_permit;
            let pool = service
                .store
                .search_pool_view(&query.debug_run)
                .map_err(SearchError::from)?;
            let result = run(&service, &pool, &query, &cancelled, |_| Ok(()))?;
            Ok::<_, SearchError>(JsonBytes::new(&result))
        });
        let result = tokio::select! {
            _ = state.shutdown.cancelled() => return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "search_cancelled", "服务正在退出，搜索已取消")),
            result = job => result,
        }.map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "search_failed", "搜索任务失败"))?.map_err(api_error)?;
        drop(guard);
        return Ok(result.into_response());
    }
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(8);
    tokio::task::spawn_blocking(move || {
        let _permit = job_permit;
        let send = |value| {
            tx.blocking_send(packet(value))
                .map_err(|_| SearchError::cancelled())
        };
        let result = service
            .store
            .search_pool_view(&query.debug_run)
            .map_err(SearchError::from)
            .and_then(|pool| run(&service, &pool, &query, &cancelled, send));
        if !cancelled.load(Ordering::Relaxed) {
            let event = match result {
                Ok(data) => json!({"type": "result", "data": data}),
                Err(error) => {
                    json!({"type": "error", "error": error.message, "status": error.status, "code": error.code})
                }
            };
            let _ = send(event);
        }
    });
    Ok(ndjson(events(rx, guard, search_permit, state.shutdown)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn idle_search_heartbeats_and_shutdown_cancels_the_body() {
        let (_sender, receiver) = tokio::sync::mpsc::channel(8);
        let cancelled = Arc::new(AtomicBool::new(false));
        let admission = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = Arc::new(admission.clone().acquire_owned().await.unwrap());
        let shutdown = tokio_util::sync::CancellationToken::new();
        let mut body = events(
            receiver,
            CancelOnDrop(cancelled.clone()),
            permit,
            shutdown.clone(),
        );
        let heartbeat = tokio::time::timeout(Duration::from_millis(1500), body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&heartbeat).unwrap(),
            json!({"type":"heartbeat"})
        );
        shutdown.cancel();
        assert!(body.frame().await.is_none());
        assert!(cancelled.load(Ordering::Relaxed));
        assert_eq!(admission.available_permits(), 1);
    }
}
