use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{
    assets::Assets,
    error::ApiError,
    sessions::{SessionError, SessionStore},
};

/// Per-source availability only; ID selection goes through the lifecycle
/// service's entry catalog.
#[derive(Clone)]
pub struct LaunchAdapter {
    pub source: crate::lifecycle::model::Source,
}

#[derive(Clone)]
pub struct AppState {
    pub assets: Arc<Assets>,
    pub capabilities: Arc<Value>,
    pub terminal: Option<Arc<crate::terminal::TerminalService>>,
    pub metadata: Option<Arc<crate::metadata::MetadataStore>>,
    pub delivery: Option<Arc<crate::delivery::service::DeliveryService>>,
    /// Claude reliable send over managed instances; `None` keeps the four
    /// send routes `501` (needs both the ledger and the terminal transport).
    pub executor: Option<Arc<crate::delivery::executor::DeliveryExecutor>>,
    pub lifecycle: Option<Arc<crate::lifecycle::service::LifecycleService>>,
    pub launch_adapters: Arc<Vec<LaunchAdapter>>,
    pub lifecycle_http: Arc<Semaphore>,
    pub media: Arc<crate::media::MediaStore>,
    pub history_pages: Arc<crate::sessions::PageStore>,
    pub history_page_http: Arc<Semaphore>,
    pub media_http: Arc<Semaphore>,
    pub media_jobs: Arc<Semaphore>,
    pub files: Option<Arc<crate::files::FileService>>,
    pub file_jobs: Arc<Semaphore>,
    /// Explicit write roots; `None` keeps upload/action/attachment routes `501`.
    pub files_write: Option<Arc<crate::files::WriteService>>,
    pub file_write_http: Arc<Semaphore>,
    pub runtime: Option<Arc<crate::runtime::ManagedRuntime>>,
    pub runtime_probes: Arc<Semaphore>,
    /// Bounded admission wait shared by every response/probe pool (batch 44
    /// WP-A, `Pools::wait`); the reader carries its own copy.
    pub admission_wait: Duration,
    /// Explicit read-only `/proc` scan for external CLIs (`SESSIONDOCK_PROC_SCAN=1`);
    /// `None` keeps `/api/live` to managed observations and `live:false`.
    pub proc_scan: Option<Arc<crate::runtime::procscan::ProcScanner>>,
    /// Spawner recording (needs the scan and the metadata store).
    pub spawn_watch: Option<Arc<crate::runtime::spawn::SpawnWatcher>>,
    pub reader: Reader,
    pub watchers: Arc<Semaphore>,
    pub observations: Arc<crate::observe::WatchHub>,
    pub searches: Arc<Semaphore>,
    /// Search-text cache, parse budget and warm-up (WP-B); search never
    /// takes a read-pool permit.
    pub search: Arc<crate::search::service::SearchService>,
    pub hostname: Arc<str>,
    /// `Config::public_hosts`, lower-cased; the loopback gate's extra authorities.
    pub public_hosts: Arc<[String]>,
    pub shutdown: tokio_util::sync::CancellationToken,
    pub audit: Option<Arc<crate::audit::AuditService>>,
    /// Session recycle bin; `None` keeps delete/trash routes `501`.
    pub trash: Option<Arc<crate::trash::TrashService>>,
    /// Node identity behind the second listener (batch 38 H1); `None` keeps
    /// `/api/meta` at `protocol: 0, node_id: null` and no hub can register us.
    pub node: Option<Arc<crate::api::node_auth::NodeIdentity>>,
    /// Bug-report bundles and workers (batch 41); `None` keeps
    /// `POST /api/bug-report` 501 and `capabilities.bug_report` false.
    pub bug_report: Option<Arc<crate::bug_report::worker::WorkerContext>>,
    /// Live `prompt` sources for `/api/messages` and `/api/watch` (WP-G):
    /// Claude question-card files under the state dir, Codex approvals off
    /// the managed instance screen; JSON `null` when neither applies.
    pub prompts: Arc<crate::bridge::LivePrompts>,
}

#[derive(Clone)]
pub struct Reader {
    pub store: Arc<SessionStore>,
    pub workers: Arc<Semaphore>,
    /// How long `run`/`acquire` queue for a worker before 503 `reader_busy`
    /// (batch 44 WP-A). Zero restores immediate rejection.
    pub wait: Duration,
}

/// Queue for one permit of `pool` for at most `wait`, then 503 `code`.
/// Dropping the future while queued (a cancelled request) leaves the queue;
/// once returned, the permit is the caller's to hold through its work.
pub async fn admit(
    pool: &Arc<Semaphore>,
    wait: Duration,
    code: &'static str,
    busy: &'static str,
) -> Result<OwnedSemaphorePermit, ApiError> {
    if wait.is_zero() {
        return pool
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, code, busy));
    }
    match tokio::time::timeout(wait, pool.clone().acquire_owned()).await {
        Ok(Ok(permit)) => Ok(permit),
        Ok(Err(_closed)) => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            code,
            "服务正在关闭",
        )),
        Err(_elapsed) => Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, code, busy)),
    }
}

impl Reader {
    /// Queue for a blocking worker within the bounded wait; the caller owns
    /// the permit and must keep it until its blocking work has really ended.
    pub async fn acquire(&self) -> Result<OwnedSemaphorePermit, ApiError> {
        admit(
            &self.workers,
            self.wait,
            "reader_busy",
            "只读工作池繁忙，请稍后重试",
        )
        .await
    }

    /// Only bounded background coordinators use a waiting admission. Their
    /// cancellation must not release a permit once blocking work has started.
    pub async fn run_wait<T, F>(
        &self,
        cancel: &tokio_util::sync::CancellationToken,
        work: F,
    ) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&SessionStore) -> Result<T, SessionError> + Send + 'static,
    {
        let permit = tokio::select! {
            _ = cancel.cancelled() => return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "cancelled", "观察已取消")),
            permit = self.workers.clone().acquire_owned() => permit.map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "reader_busy", "读取服务已关闭"))?,
        };
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work(&store)
        })
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "reader_failed",
                "只读任务失败",
            )
        })?
        .map_err(ApiError::from)
    }

    /// Queue (bounded) then spawn: a request cancelled while queued never
    /// holds a permit; one cancelled after spawning retains it until the
    /// blocking work ends.
    pub async fn run<T, F>(&self, work: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&SessionStore) -> Result<T, SessionError> + Send + 'static,
    {
        let permit = self.acquire().await?;
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work(&store)
        })
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "reader_failed",
                "只读任务失败",
            )
        })?
        .map_err(ApiError::from)
    }
}

/// Serialize large read results inside the bounded worker, not on the async reactor.
pub struct JsonBytes(pub Vec<u8>);

impl JsonBytes {
    pub fn new(value: &Value) -> Self {
        Self(serde_json::to_vec(value).expect("serde_json::Value serializes"))
    }
}

impl IntoResponse for JsonBytes {
    fn into_response(self) -> Response {
        let length = self.0.len();
        let mut response = Body::from(self.0).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        response
            .headers_mut()
            .insert("x-agenthub-decoded-length", length.into());
        response
    }
}

pub fn capabilities() -> Value {
    json!({
        "backend": "rust", "stage": "replacement", "read_only": false,
        "storage_namespace": "sessiondock.",
        "sessions": true, "watch": true, "search": true, "live": false,
        "terminal": false, "outbox": false, "audit": false, "files": false,
        "mutations": false, "hub": false, "trash": false, "timeline_pin": false,
        "bug_report": false,
        "media": true, "media_remote": false, "media_lazy": true, "history_pages": true,
        "media_continuation": true,
        "history_semantics": "limited_native"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::SessionRoots;

    fn make_reader(workers: usize, wait: Duration) -> Reader {
        Reader {
            store: Arc::new(SessionStore::new(SessionRoots::default())),
            workers: Arc::new(Semaphore::new(workers)),
            wait,
        }
    }

    #[tokio::test]
    async fn a_queued_request_is_admitted_when_a_worker_frees_within_the_wait() {
        let reader = make_reader(1, Duration::from_secs(5));
        let held = reader.workers.clone().acquire_owned().await.unwrap();
        let queued = reader.clone();
        let task = tokio::spawn(async move { queued.run(|_| Ok(7)).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!task.is_finished(), "must queue, not fail immediately");
        drop(held);
        assert_eq!(task.await.unwrap().unwrap(), 7);
        assert_eq!(reader.workers.available_permits(), 1);
    }

    #[tokio::test]
    async fn an_exhausted_wait_is_503_reader_busy_and_zero_wait_rejects_at_once() {
        let reader = make_reader(1, Duration::from_millis(60));
        let held = reader.workers.clone().acquire_owned().await.unwrap();
        let started = std::time::Instant::now();
        let error = reader.run(|_| Ok(())).await.unwrap_err();
        assert!(started.elapsed() >= Duration::from_millis(60));
        assert_eq!(
            (error.status, error.code),
            (StatusCode::SERVICE_UNAVAILABLE, "reader_busy")
        );
        let immediate = make_reader(1, Duration::ZERO);
        let _held2 = immediate.workers.clone().acquire_owned().await.unwrap();
        let started = std::time::Instant::now();
        let error = immediate.run(|_| Ok(())).await.unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(50));
        assert_eq!(error.code, "reader_busy");
        drop(held);
        assert_eq!(reader.run(|_| Ok(1)).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn a_request_cancelled_while_queued_never_takes_a_permit() {
        let reader = make_reader(1, Duration::from_secs(5));
        let held = reader.workers.clone().acquire_owned().await.unwrap();
        let queued = reader.clone();
        let task = tokio::spawn(async move {
            queued
                .run(|_| -> Result<(), SessionError> { panic!("cancelled work must not run") })
                .await
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        drop(held);
        // The freed permit goes to the next real caller, not to the aborted one.
        assert_eq!(reader.workers.available_permits(), 1);
        assert_eq!(reader.run(|_| Ok(3)).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn the_shared_admission_helper_uses_the_pool_specific_code() {
        let pool = Arc::new(Semaphore::new(1));
        let held = admit(&pool, Duration::from_secs(1), "x_busy", "x")
            .await
            .unwrap();
        let error = admit(&pool, Duration::from_millis(20), "x_busy", "busy text")
            .await
            .unwrap_err();
        assert_eq!(
            (error.status, error.code, error.message.as_str()),
            (StatusCode::SERVICE_UNAVAILABLE, "x_busy", "busy text")
        );
        drop(held);
        let permit = admit(&pool, Duration::from_millis(20), "x_busy", "busy text")
            .await
            .unwrap();
        drop(permit);
        pool.close();
        let error = admit(&pool, Duration::from_millis(20), "x_busy", "busy text")
            .await
            .unwrap_err();
        assert_eq!(error.message, "服务正在关闭");
    }
}
