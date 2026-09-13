//! File transport. Read authority comes from explicit roots and a complete
//! semantic branch, never a client-supplied cwd or native filesystem path.
//! Write routes additionally require explicit write roots and re-resolve the
//! session anchor on every request (including every upload chunk).
use std::{
    io::{self, Read},
    sync::Arc,
};

use crate::{
    error::ApiError,
    files::{
        ActionRequest, FileBody, FileError, FileResponse, FileScope, FileService, ListOptions,
        Outcome, ReadOptions, STREAM_CHUNK_BYTES, WriteService,
    },
    sessions::{MessageQuery, SessionStore},
    state::{AppState, JsonBytes},
};
use axum::{
    Json,
    body::{Body, Bytes, to_bytes},
    extract::{
        Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::OwnedSemaphorePermit;

/// Route body cap for one upload chunk: the explicit 4 MiB chunk limit plus
/// one byte so an oversized chunk reaches the handler's explicit 413.
pub const UPLOAD_BODY_LIMIT: usize = 4 * 1024 * 1024 + 1;

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
            "文件读取未启用：必须显式配置独立的开发文件目录",
        )
    })
}
fn admission(state: &AppState) -> Result<Arc<OwnedSemaphorePermit>, ApiError> {
    if state.shutdown.is_cancelled() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "shutdown",
            "服务正在关闭",
        ));
    }
    state
        .file_jobs
        .clone()
        .try_acquire_owned()
        .map(Arc::new)
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "files_busy",
                "文件工作池繁忙，请稍后重试",
            )
        })
}
fn validate_scope(uid: &str, agent: &str) -> Result<(), ApiError> {
    if uid.is_empty()
        || uid.len() > 256
        || agent.len() > 256
        || uid.chars().chain(agent.chars()).any(char::is_control)
    {
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
#[serde(deny_unknown_fields)]
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
    let lease = admission(&state)?;
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
#[serde(default, deny_unknown_fields)]
pub struct FileQuery {
    uid: String,
    agent: String,
    r#ref: String,
    path: Option<String>,
    mode: String,
    download: String,
    raw: String,
    offset: usize,
    limit: Option<usize>,
    sort: String,
    order: String,
    hidden: String,
    /// `debug_run`: the page's view selector, appended to every `/api/`
    /// URL by the frontend; accepted and ignored here like Python.
    #[allow(dead_code)]
    debug_run: String,
}
impl FileQuery {
    fn validate(&self, directory: bool) -> Result<(), ApiError> {
        validate_scope(&self.uid, &self.agent)?;
        crate::files::clean_ref(&self.r#ref)?;
        if (!directory && self.path.is_some())
            || self
                .path
                .as_ref()
                .is_some_and(|s| s.len() > 4096 || s.chars().any(char::is_control))
            || [&self.download, &self.raw, &self.hidden]
                .iter()
                .any(|s| !["", "0", "1"].contains(&s.as_str()))
        {
            return Err(invalid());
        }
        if !["", "info", "preview", "jobs"].contains(&self.mode.as_str()) {
            return Err(FileError::unsupported(&self.mode).into());
        }
        if self.mode == "jobs" && (!directory || self.path.is_some()) {
            return Err(invalid());
        }
        if self.mode == "info" && (self.download == "1" || self.raw == "1") {
            return Err(invalid());
        }
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
    get(state, method, headers, query, false).await
}
pub async fn directory(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
    query: Result<Query<FileQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    get(state, method, headers, query, true).await
}
async fn get(
    state: AppState,
    method: Method,
    headers: HeaderMap,
    query: Result<Query<FileQuery>, QueryRejection>,
    directory: bool,
) -> Result<Response, ApiError> {
    let files = configured(&state)?;
    let Query(query) = query.map_err(|_| invalid())?;
    query.validate(directory)?;
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
    // Jobs listing stays 501 (same code as before) without write roots.
    if query.mode == "jobs" && state.files_write.is_none() {
        return Err(FileError::unsupported("jobs").into());
    }
    let lease = admission(&state)?;
    let hostname = state.hostname.clone();
    let writer = state.files_write.clone();
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
        if query.mode == "jobs" {
            let writer = writer.ok_or_else(|| FileError::unsupported("jobs"))?;
            if target.kind() != "directory" {
                return Err(invalid());
            }
            return Ok(Prepared::Json(JsonBytes::new(&writer.jobs(&scope))));
        }
        if query.mode == "info" {
            return Ok(Prepared::Json(JsonBytes::new(&files.describe(&target)?)));
        }
        if directory && query.mode.is_empty() && query.download != "1" && query.raw != "1" {
            let mut listing = files.list(
                &target,
                &ListOptions {
                    offset: query.offset,
                    limit: query.limit.unwrap_or(500),
                    sort: if query.sort.is_empty() {
                        "name".into()
                    } else {
                        query.sort
                    },
                    order: if query.order.is_empty() {
                        "asc".into()
                    } else {
                        query.order
                    },
                    hidden: query.hidden != "0",
                },
            )?;
            listing["hostname"] = serde_json::json!(&*hostname);
            listing["node_id"] = Value::Null;
            // Display hint only; each mutation re-authorizes independently.
            listing["writable"] =
                serde_json::json!(writer.is_some_and(|writer| writer.writable(target.path())));
            return Ok(Prepared::Json(JsonBytes::new(&listing)));
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
    crate::state::admit(
        &state.file_write_http,
        state.admission_wait,
        "files_busy",
        "文件写入工作池繁忙，请稍后重试",
    )
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
fn outcome(outcome: Outcome) -> Response {
    (
        StatusCode::from_u16(outcome.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
        Json(outcome.body),
    )
        .into_response()
}
fn anchor<'a>(
    files: &FileService,
    view: &'a Value,
    uid: &'a str,
    agent: &'a str,
    reference: &str,
) -> Result<(FileScope<'a>, crate::files::ResolvedTarget), ApiError> {
    let scope = scope(view, uid, agent)?;
    let target = files.target(&scope, reference, None)?;
    if target.kind() != "directory" {
        return Err(FileError::new(
            400,
            "file_directory_anchor_required",
            "文件操作入口必须是会话提及的目录",
        )
        .into());
    }
    Ok((scope, target))
}

pub async fn action(
    State(state): State<AppState>,
    body: Result<Json<ActionRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let (files, writer) = write_configured(&state)?;
    let Json(request) = body.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "文件操作请求体最多 512 KiB",
            )
        } else {
            invalid()
        }
    })?;
    validate_scope(&request.uid, &request.agent)?;
    crate::files::clean_ref(&request.r#ref)?;
    let lease = write_admission(&state).await?;
    work(&state, lease, move |store| {
        let view = store.messages(
            &request.uid,
            &MessageQuery {
                agent: request.agent.clone(),
                ..Default::default()
            },
        )?;
        let (uid, agent, reference) = (
            request.uid.clone(),
            request.agent.clone(),
            request.r#ref.clone(),
        );
        let (scope, anchor) = anchor(&files, &view, &uid, &agent, &reference)?;
        Ok(match writer.action(&anchor, &scope, request) {
            Ok(result) => outcome(result),
            Err(error) => write_error(error),
        })
    })
    .await
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UploadQuery {
    uid: String,
    agent: String,
    r#ref: String,
    job: String,
    offset: String,
    /// `debug_run`: the page's view selector, appended to every `/api/`
    /// URL by the frontend; accepted and ignored here like Python.
    #[allow(dead_code)]
    debug_run: String,
}

pub async fn upload(
    State(state): State<AppState>,
    query: Result<Query<UploadQuery>, QueryRejection>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let (files, writer) = write_configured(&state)?;
    let Query(query) = query.map_err(|_| invalid())?;
    validate_scope(&query.uid, &query.agent)?;
    crate::files::clean_ref(&query.r#ref)?;
    let offset: u64 = query.offset.parse().map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "file_upload_offset_invalid",
            "offset 必须是非负整数",
        )
    })?;
    if headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or("").trim())
        != Some("application/octet-stream")
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "file_upload_content_type",
            "上传分块必须使用 application/octet-stream",
        ));
    }
    let limit = writer.limits().max_chunk_bytes;
    // Admit before buffering; the chunk is bounded by the explicit limit.
    let lease = write_admission(&state).await?;
    let chunk: Bytes = match to_bytes(body, limit).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return Ok(write_error(
                FileError::new(
                    413,
                    "file_upload_chunk_too_large",
                    format!("单个分块最多 {limit} 字节"),
                )
                .with_details(json!({"limit": limit})),
            ));
        }
    };
    work(&state, lease, move |store| {
        let view = store.messages(
            &query.uid,
            &MessageQuery {
                agent: query.agent.clone(),
                ..Default::default()
            },
        )?;
        let (scope, anchor) = anchor(&files, &view, &query.uid, &query.agent, &query.r#ref)?;
        Ok(
            match writer.upload(&anchor, &scope, &query.job, offset, &chunk) {
                Ok(result) => outcome(result),
                Err(error) => write_error(error),
            },
        )
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentRequest {
    uid: String,
    #[serde(default)]
    agent: String,
    r#ref: String,
    job: String,
}

/// Records a completed upload's final path for the session. Without a
/// metadata store the path is returned with `recorded: false`.
pub async fn attachment(
    State(state): State<AppState>,
    body: Result<Json<AttachmentRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let (files, writer) = write_configured(&state)?;
    let Json(request) = body.map_err(|_| invalid())?;
    validate_scope(&request.uid, &request.agent)?;
    crate::files::clean_ref(&request.r#ref)?;
    let lease = write_admission(&state).await?;
    let metadata = state.metadata.clone();
    work(&state, lease, move |store| {
        let view = store.messages(
            &request.uid,
            &MessageQuery {
                agent: request.agent.clone(),
                ..Default::default()
            },
        )?;
        let (scope, _anchor) = anchor(&files, &view, &request.uid, &request.agent, &request.r#ref)?;
        let mut upload = match writer.completed_upload(&scope, &request.job) {
            Ok(value) => value,
            Err(error) => return Ok(write_error(error)),
        };
        let path = upload["path"].as_str().unwrap_or("").to_owned();
        let name = upload["name"].as_str().unwrap_or("").to_owned();
        let mime = mime_guess::from_path(&name)
            .first_raw()
            .unwrap_or("application/octet-stream");
        let kind = match mime.split('/').next().unwrap_or("") {
            kind @ ("image" | "video" | "audio") => kind,
            _ => "file",
        };
        let recorded = match &metadata {
            Some(store) => {
                store
                    .record_attachment(
                        &request.uid,
                        crate::metadata::Attachment {
                            path: path.clone(),
                            name: name.clone(),
                            size: upload["size"].as_u64().unwrap_or(0),
                            sha256: upload["sha256"].as_str().map(str::to_owned),
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
                true
            }
            None => false,
        };
        upload["ok"] = json!(true);
        upload["original_name"] = json!(name);
        upload["attachment_id"] = json!(request.job);
        upload["mime"] = json!(mime);
        upload["kind"] = json!(kind);
        upload["recorded"] = json!(recorded);
        upload["media"] = Value::Null;
        Ok(Json(upload).into_response())
    })
    .await
}
