//! File transport. Every request resolves the selected session; opening a
//! referenced directory grants authenticated operator navigation. Persisted
//! grants survive rename/delete, while write targets keep OS and private-data
//! guards. Request cwd alone never establishes a browser grant.
use std::{
    io::{self, Read},
    sync::Arc,
};

use crate::{
    error::ApiError,
    files::{
        FileBody, FileError, FileResponse, FileScope, FileService, ReadOptions, STREAM_CHUNK_BYTES,
        WriteService,
    },
    sessions::{MessageQuery, SessionStore},
    state::{AppState, JsonBytes},
};
use axum::{
    Json,
    body::{Body, Bytes, to_bytes},
    extract::{
        Query, Request, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::OwnedSemaphorePermit;

impl From<FileError> for ApiError {
    fn from(error: FileError) -> Self {
        Self::new(
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
            error.code,
            error.message,
        )
    }
}
fn invalid() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "invalid_file_request",
        "需要有效的会话、分支和文件参数",
    )
}
fn configured(state: &AppState) -> Result<Arc<FileService>, ApiError> {
    state.files.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "files_disabled",
            "文件读取服务不可用",
        )
    })
}
async fn admission(state: &AppState) -> Result<Arc<OwnedSemaphorePermit>, ApiError> {
    if state.shutdown.is_cancelled() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "shutdown",
            "服务正在关闭",
        ));
    }
    crate::state::admit(&state.file_jobs, "files_busy")
        .await
        .map(Arc::new)
}
fn validate_scope(uid: &str, _agent: &str) -> Result<(), ApiError> {
    if uid.is_empty() {
        return Err(invalid());
    }
    Ok(())
}

async fn work<T: Send + 'static>(
    state: &AppState,
    lease: Arc<OwnedSemaphorePermit>,
    job: impl FnOnce(&SessionStore) -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let worker = state.reader.acquire().await?;
    let store = state.reader.store.clone();
    tokio::task::spawn_blocking(move || {
        let (_worker, _lease) = (worker, lease);
        job(&store)
    })
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "file_worker_failed",
            "文件读取任务异常退出",
        )
    })?
}
fn scope<'a>(view: &'a Value, uid: &'a str, agent: &'a str) -> Result<FileScope<'a>, ApiError> {
    // An unsupported history is rejected by SessionStore before this point.
    Ok(FileScope {
        uid,
        agent: (!agent.is_empty()).then_some(agent),
        cwd: view["meta"]["cwd"].as_str().unwrap_or(""),
        messages: view["messages"].as_array().ok_or_else(invalid)?,
    })
}
#[derive(Deserialize)]
pub struct ResolveRequest {
    uid: String,
    #[serde(default)]
    agent: String,
    refs: Vec<String>,
}

pub async fn resolve(
    State(state): State<AppState>,
    body: Result<Json<ResolveRequest>, JsonRejection>,
) -> Result<JsonBytes, ApiError> {
    let files = configured(&state)?;
    let Json(body) = body.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "文件引用请求体过大",
            )
        } else {
            invalid()
        }
    })?;
    validate_scope(&body.uid, &body.agent)?;
    let lease = admission(&state).await?;
    work(&state, lease, move |store| {
        let view = store.messages(
            &body.uid,
            &MessageQuery {
                agent: body.agent.clone(),
                ..Default::default()
            },
        )?;
        Ok(JsonBytes::new(&files.resolve_many(
            &scope(&view, &body.uid, &body.agent)?,
            &body.refs,
        )?))
    })
    .await
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct FileQuery {
    uid: String,
    agent: String,
    r#ref: String,
    path: Option<String>,
    mode: String,
    download: String,
}
impl FileQuery {
    fn validate(&mut self) -> Result<(), ApiError> {
        validate_scope(&self.uid, &self.agent)?;
        crate::files::clean_ref(&self.r#ref)?;
        if self.download == "1" || !matches!(self.mode.as_str(), "" | "info" | "preview") {
            // Unknown modes are ordinary file reads.
            self.mode.clear();
        }
        // The direct-reference route ignores any browser navigation field.
        self.path = None;
        Ok(())
    }
}

enum Prepared {
    Json(JsonBytes),
    File(FileResponse),
}

pub async fn file(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
    query: Result<Query<FileQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    get(state, method, headers, query).await
}
async fn get(
    state: AppState,
    method: Method,
    headers: HeaderMap,
    query: Result<Query<FileQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let files = configured(&state)?;
    let Query(mut query) = query.map_err(|_| invalid())?;
    query.validate()?;
    // This service deliberately emits no stable validator. An If-Range request
    // cannot prove continuity across filesystem versions, so send the complete
    // representation instead of helping a client splice different versions.
    let range = match headers
        .get(axum::http::header::RANGE)
        .filter(|_| !headers.contains_key(axum::http::header::IF_RANGE))
    {
        Some(value) => Some(value.to_str().map_err(|_| invalid())?.to_owned()),
        None => None,
    };
    let head = method == Method::HEAD;
    let lease = admission(&state).await?;
    let prepared = work(&state, lease.clone(), move |store| {
        let view = store.messages(
            &query.uid,
            &MessageQuery {
                agent: query.agent.clone(),
                ..Default::default()
            },
        )?;
        let scope = scope(&view, &query.uid, &query.agent)?;
        let target = files.target(&scope, &query.r#ref, query.path.as_deref())?;
        if query.mode == "info" {
            return Ok(Prepared::Json(JsonBytes::new(&files.describe(&target)?)));
        }
        Ok(Prepared::File(files.read(
            target,
            &ReadOptions {
                download: query.download == "1",
                preview: query.mode == "preview",
                head,
                range,
            },
        )?))
    })
    .await?;
    match prepared {
        Prepared::Json(value) => Ok(value.into_response()),
        Prepared::File(response) => response_body(response, lease, state.shutdown),
    }
}

fn response_body(
    response: FileResponse,
    lease: Arc<OwnedSemaphorePermit>,
    shutdown: tokio_util::sync::CancellationToken,
) -> Result<Response, ApiError> {
    // Pull-driven: an unpolled response starts no I/O; a slow consumer has at
    // most one 64 KiB chunk in flight. Both the response and any unfinished
    // blocking read retain capacity, even if the HTTP request disappears.
    let body = match response.body {
        FileBody::Empty => Body::empty(),
        FileBody::Bytes(bytes) => {
            let stream = async_stream::try_stream! {
                let _lease = lease;
                if shutdown.is_cancelled() { Err(io::Error::other("file transfer cancelled"))?; }
                yield Bytes::from(bytes);
            };
            file_stream(stream)
        }
        FileBody::Reader(mut reader) => {
            let stream = async_stream::try_stream! {
                let lease = lease;
                while reader.remaining() > 0 {
                    if shutdown.is_cancelled() { Err(io::Error::other("file transfer cancelled"))?; }
                    let capacity = lease.clone();
                    let job = tokio::task::spawn_blocking(move || -> io::Result<_> {
                        let _capacity = capacity;
                        let mut chunk = vec![0;STREAM_CHUNK_BYTES];
                        let count = reader.read(&mut chunk)?;
                        chunk.truncate(count);
                        Ok((reader,chunk))
                    });
                    let result = tokio::select! {
                        _ = shutdown.cancelled() => Err(io::Error::other("file transfer cancelled")),
                        result = job => result.map_err(|_| io::Error::other("file read task failed")).and_then(|result| result),
                    }?;
                    reader = result.0;
                    yield Bytes::from(result.1);
                }
            };
            file_stream(stream)
        }
    };
    let mut builder = Response::builder().status(response.status);
    for (name, value) in response.headers {
        builder = builder.header(name, value);
    }
    builder.body(body).map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "file_headers_invalid",
            "文件响应头无效",
        )
    })
}
fn file_stream(
    stream: impl futures_util::Stream<Item = io::Result<Bytes>> + Send + 'static,
) -> Body {
    Body::from_stream(stream)
}

// ---- write side ----------------------------------------------------------

fn write_configured(state: &AppState) -> Result<(Arc<FileService>, Arc<WriteService>), ApiError> {
    let writer = state.files_write.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "files_jobs_disabled",
            "文件写入未启用：必须显式配置 SESSIONDOCK_FILE_WRITE_ROOTS（只读目录不会隐式变为可写）",
        )
    })?;
    Ok((configured(state)?, writer))
}
async fn write_admission(state: &AppState) -> Result<Arc<OwnedSemaphorePermit>, ApiError> {
    if state.shutdown.is_cancelled() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "shutdown",
            "服务正在关闭",
        ));
    }
    crate::state::admit(&state.file_write_http, "files_busy")
        .await
        .map(Arc::new)
}
/// Errors carry their explicit limits/expectations at the top level.
fn write_error(error: FileError) -> Response {
    let mut body = json!({"error": error.message, "code": error.code});
    if let Some(details) = error.details.as_object() {
        for (key, value) in details {
            body[key] = value.clone();
        }
    }
    (
        StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
        Json(body),
    )
        .into_response()
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct AttachmentQuery {
    uid: String,
    agent: String,
    name: String,
    id: Option<String>,
    record_id: String,
    instance_id: String,
}

fn pending_attachment_cwd(
    query: &AttachmentQuery,
    record: &crate::lifecycle::model::Record,
) -> Result<String, ApiError> {
    let receipt_identity = !query.record_id.is_empty() || !query.instance_id.is_empty();
    if !query.agent.is_empty() || query.uid.strip_prefix("tmux:") != Some(record.host_name()) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "launch_identity",
            "创建回执与附件目标实例不匹配",
        ));
    }
    if receipt_identity
        && (query.record_id != record.record_id() || query.instance_id != record.instance_id())
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "launch_identity",
            "创建回执与附件目标实例不匹配",
        ));
    }
    // The pending store remains usable while its receipt exists, including
    // the short resolved/exited retention window. Rust keeps historical ledger
    // rows indefinitely, so only the explicit discard tombstone ends this grant.
    if record.discarded() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "launch_not_ready",
            "创建回执已被丢弃，不能上传附件",
        ));
    }
    Ok(record.spec().cwd().to_string_lossy().into_owned())
}

async fn pending_attachment_scope(
    state: &AppState,
    query: &AttachmentQuery,
) -> Result<Option<String>, ApiError> {
    if !query.uid.starts_with("tmux:") {
        return Ok(None);
    }
    if query.record_id.is_empty() != query.instance_id.is_empty() {
        return Err(invalid());
    }
    let lifecycle = super::lifecycle::enabled(state)?;
    let record = if query.record_id.is_empty() {
        // Pages (and an older Hub shell cached in a browser) send only
        // the server-generated pending host UID. Host names are unique ledger
        // identities; resolve them on the server instead of treating the raw
        // UID as a client-supplied cwd grant.
        let name = query.uid.strip_prefix("tmux:").unwrap_or_default();
        lifecycle
            .list(0, usize::MAX)
            .await
            .map_err(super::lifecycle::failure)?
            .into_iter()
            .find(|record| record.host_name() == name && !record.discarded())
            .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "session_error", "会话不存在"))?
    } else {
        lifecycle
            .get(query.record_id.clone())
            .await
            .map_err(super::lifecycle::failure)?
    };
    pending_attachment_cwd(query, &record).map(Some)
}

/// Legacy composer: raw file body, with the native UID and filename in the
/// query. Resolve the destination from the selected history on the server.
pub async fn upload_attachment(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    let (_, writer) = write_configured(&state)?;
    let Query(query) =
        Query::<AttachmentQuery>::try_from_uri(request.uri()).map_err(|_| invalid())?;
    validate_scope(&query.uid, &query.agent)?;
    // A newly launched CLI has no native history UID yet. Its exact lifecycle
    // receipt still carries the server-validated cwd; require both immutable
    // receipt identities before granting the same attachment subdirectory.
    let pending_cwd = pending_attachment_scope(&state, &query).await?;
    let limit = WriteService::BUG_REPORT_ATTACHMENT_MAX_BYTES;
    let too_large = || {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "file_upload_too_large",
            format!("单个附件不能超过 {} MB", limit / (1024 * 1024)),
        )
    };
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|length| length > limit)
    {
        return Err(too_large());
    }
    let mime = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let lease = write_admission(&state).await?;
    let bytes = to_bytes(request.into_body(), limit)
        .await
        .map_err(|_| too_large())?;
    let metadata = state.metadata.clone();
    work(&state, lease, move |store| {
        let view;
        let empty_messages = [];
        let (scope, record_metadata) = match pending_cwd.as_deref() {
            Some(cwd) => (
                FileScope {
                    uid: &query.uid,
                    agent: None,
                    cwd,
                    messages: &empty_messages,
                },
                false,
            ),
            None => {
                view = store.messages(
                    &query.uid,
                    &MessageQuery {
                        agent: query.agent.clone(),
                        ..Default::default()
                    },
                )?;
                (scope(&view, &query.uid, &query.agent)?, true)
            }
        };
        let mut upload = match writer.session_attachment_upload(
            &scope,
            query
                .id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty()),
            &query.name,
            &mime,
            &bytes,
        ) {
            Ok(upload) => upload,
            Err(error) => return Ok(write_error(error)),
        };
        if record_metadata && let Some(metadata) = &metadata {
            use sha2::{Digest, Sha256};
            metadata
                .record_attachment(
                    &query.uid,
                    crate::metadata::Attachment {
                        path: upload["path"].as_str().unwrap_or("").to_owned(),
                        name: upload["name"].as_str().unwrap_or("").to_owned(),
                        size: bytes.len() as u64,
                        sha256: Some(format!("{:x}", Sha256::digest(&bytes))),
                        agent: scope.agent.map(str::to_owned),
                        at: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs_f64())
                            .unwrap_or(0.0),
                    },
                )
                .map_err(|error| {
                    ApiError::new(
                        StatusCode::from_u16(error.status)
                            .unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                        error.code,
                        error.message,
                    )
                })?;
        }
        upload["recorded"] = json!(record_metadata && metadata.is_some());
        Ok(([(header::CACHE_CONTROL, "no-store")], Json(upload)).into_response())
    })
    .await
}
