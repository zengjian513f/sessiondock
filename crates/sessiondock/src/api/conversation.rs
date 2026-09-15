//! Thin HTTP transport for server-owned conversation drafts, staging and one-shot SEND.
use crate::{
    conversation::{Conversations, SendInput, page_lease, store::Upload},
    delivery::executor::Failure,
    error::ApiError,
    state::AppState,
};
use axum::{
    Extension, Json,
    extract::{Query, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
fn error(e: Failure) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(e.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
        e.code,
        e.message,
    )
}
fn enabled(s: &AppState) -> Result<Arc<Conversations>, ApiError> {
    s.conversations.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "conversation_disabled",
            "会话保存和发送未配置",
        )
    })
}
#[derive(Deserialize)]
pub struct Read {
    uid: String,
    #[serde(default)]
    legacy: bool,
    #[serde(default)]
    report_request_id: String,
    #[serde(default)]
    request_id: String,
}
pub async fn get(State(s): State<AppState>, Query(q): Query<Read>) -> Result<Response, ApiError> {
    let service = enabled(&s)?;
    let identity = service.identity(&q.uid).await.map_err(error)?;
    let value = if !q.report_request_id.is_empty() {
        let row = service
            .store
            .report(&identity.key, &q.report_request_id)
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::NOT_FOUND,
                    "submission_missing",
                    "报告提交不存在",
                )
            })?;
        if !row["result"].is_object() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "report_result_unknown",
                "诊断已开始保存，输入保留，不会重复创建",
            ));
        }
        return Ok((
            [(header::CACHE_CONTROL, "no-store")],
            Json(row["result"].clone()),
        )
            .into_response());
    } else if q.legacy {
        json!({"ok":true,"legacy":service.store.legacy(&identity.key)})
    } else if q.request_id.is_empty() {
        json!({"ok":true,"draft":service.store.draft(&identity.key)})
    } else {
        let request = service
            .store
            .request(&identity.key, &q.request_id)
            .ok_or_else(|| {
                ApiError::new(StatusCode::NOT_FOUND, "submission_missing", "提交不存在")
            })?;
        let mut value = crate::conversation::submission_result(&request).map_err(error)?;
        value["draft"] = json!(service.store.draft(&identity.key));
        value
    };
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response())
}
fn normalize_draft(value: &mut Value, identity: &crate::conversation::Identity) {
    if !value.is_object() {
        return;
    }
    for (field, default) in [
        ("text", json!("")),
        ("attachments", json!([])),
        ("quotes", json!([])),
    ] {
        if value[field].is_null() {
            value[field] = default;
        }
    }
    if !value["session"].is_object() {
        value["session"] = json!({});
    }
    value["session"]["uid"] = json!(identity.uid);
    value["session"]["source"] = json!(identity.source);
    value["session"]["cwd"] = json!(identity.cwd);
    if let Some(record) = &identity.record {
        for (key, text) in [
            ("name", record.host_name()),
            ("record_id", record.record_id()),
            ("launch_id", record.launch_id()),
            ("instance_id", record.instance_id()),
        ] {
            value["session"][key] = json!(text);
        }
    }
    if let Some(session) = value["session"].as_object_mut() {
        session.remove("node_id");
        session.remove("node_name");
    }
    if let Some(items) = value["attachments"].as_array_mut() {
        for item in items {
            if item["uploaded"].is_object() {
                item["uploaded"]["uid"] = json!(identity.uid);
                if let Some(uploaded) = item["uploaded"].as_object_mut() {
                    uploaded.remove("node");
                }
            }
        }
    }
}
#[derive(Deserialize)]
pub struct Save {
    uid: String,
    revision: u64,
    value: Value,
}
pub async fn save(
    State(s): State<AppState>,
    Json(mut q): Json<Save>,
) -> Result<Response, ApiError> {
    let service = enabled(&s)?;
    let identity = service.identity(&q.uid).await.map_err(error)?;
    normalize_draft(&mut q.value, &identity);
    let store = service.store.clone();
    let draft = tokio::task::spawn_blocking(move || store.save(&identity.key, q.revision, q.value))
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "conversation_storage",
                "草稿保存任务失败",
            )
        })?
        .map_err(error)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"ok":true,"draft":draft})),
    )
        .into_response())
}
pub async fn send(
    State(s): State<AppState>,
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    Json(q): Json<SendInput>,
) -> Result<Response, ApiError> {
    if let Some(response) = super::delivery::stale_build(&s, &q._build, hub.is_some()) {
        return Ok(response);
    }
    let service = enabled(&s)?;
    // Detached operation ownership: a disconnected HTTP caller cannot cancel a
    // started one-shot SEND or release its serialization lock early.
    let result = tokio::spawn(async move { service.send(q).await })
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "conversation_send",
                "发送任务异常退出",
            )
        })?
        .map_err(error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(result)).into_response())
}
pub async fn check(
    State(s): State<AppState>,
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    Json(q): Json<SendInput>,
) -> Result<Response, ApiError> {
    if let Some(response) = super::delivery::stale_build(&s, &q._build, hub.is_some()) {
        return Ok(response);
    }
    let service = enabled(&s)?;
    let page = page_lease(q.lease.as_ref());
    service.check(&q.uid, page.as_ref()).await.map_err(error)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"ok":true})),
    )
        .into_response())
}
pub async fn restart(
    State(s): State<AppState>,
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    Json(q): Json<SendInput>,
) -> Result<Response, ApiError> {
    if let Some(response) = super::delivery::stale_build(&s, &q._build, hub.is_some()) {
        return Ok(response);
    }
    let service = enabled(&s)?;
    let response = tokio::spawn(async move { service.restart(&q.uid, &q.request_id).await })
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "conversation_start",
                "启动任务异常退出，输入保留",
            )
        })?
        .map_err(error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(response)).into_response())
}
#[derive(Deserialize)]
pub struct UploadQuery {
    uid: String,
    id: String,
    name: String,
}
pub async fn upload(State(s): State<AppState>, request: Request) -> Result<Response, ApiError> {
    let service = enabled(&s)?;
    let Query(q) = Query::<UploadQuery>::try_from_uri(request.uri())
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "attachment_query", "附件请求无效"))?;
    let identity = service.identity(&q.uid).await.map_err(error)?;
    let limit = crate::files::WriteService::BUG_REPORT_ATTACHMENT_MAX_BYTES as u64;
    let too_large = || {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "file_upload_too_large",
            "单个附件不能超过 512 MiB",
        )
    };
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|s| s.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .is_some_and(|n| n > limit)
    {
        return Err(too_large());
    }
    let lock = service.upload_lock(&identity.key, &q.id);
    let _guard = lock.lock().await;
    let mime = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|s| s.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let path = service.upload_path(&identity.key, &q.id);
    let temporary = path.with_extension(format!(
        "{}.upload",
        crate::conversation::random_id().map_err(error)?
    ));
    let dir = path.parent().unwrap().to_owned();
    tokio::fs::create_dir_all(&dir).await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "attachment_storage",
            "附件暂存目录创建失败",
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|_| {
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "attachment_storage",
                    "附件目录权限设置失败",
                )
            })?;
    }
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let cleanup = Cleanup(temporary.clone());
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary).await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "attachment_storage",
            "附件暂存失败",
        )
    })?;
    let mut stream = request.into_body().into_data_stream();
    let mut size = 0u64;
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "attachment_interrupted",
                "附件上传中断",
            )
        })?;
        size += chunk.len() as u64;
        if size > limit {
            return Err(too_large());
        }
        hash.update(&chunk);
        file.write_all(&chunk).await.map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "attachment_storage",
                "附件写入失败",
            )
        })?;
    }
    if size == 0 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "file_upload_empty",
            "附件为空",
        ));
    }
    file.sync_all().await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "attachment_storage",
            "附件同步失败",
        )
    })?;
    drop(file);
    let sha256 = format!("{:x}", hash.finalize());
    if let Ok(old) = service.store.upload(&identity.key, &q.id) {
        if old.sha256 != sha256 || old.name != crate::files::WriteService::attachment_name(&q.name)
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "attachment_conflict",
                "相同附件上传 ID 对应了不同文件",
            ));
        }
        return Ok((
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({"ok":true,"upload_id":old.id,"name":old.name,"size":old.size})),
        )
            .into_response());
    }
    tokio::fs::rename(&temporary, &path).await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "attachment_storage",
            "附件发布到暂存区失败",
        )
    })?;
    drop(cleanup);
    #[cfg(unix)]
    {
        tokio::task::spawn_blocking(move || std::fs::File::open(dir)?.sync_all())
            .await
            .map_err(|_| {
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "attachment_storage",
                    "附件目录同步任务失败",
                )
            })?
            .map_err(|_| {
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "attachment_storage",
                    "附件目录同步失败",
                )
            })?;
    }
    let upload = Upload {
        key: identity.key,
        id: q.id.clone(),
        name: crate::files::WriteService::attachment_name(&q.name),
        mime,
        size,
        published: None,
        sha256,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    service.store.note_upload(upload.clone()).map_err(error)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"ok":true,"upload_id":q.id,"name":upload.name,"size":size})),
    )
        .into_response())
}
#[derive(Deserialize)]
pub struct Import {
    uid: String,
    value: Value,
}
pub async fn import(
    State(s): State<AppState>,
    Json(q): Json<Import>,
) -> Result<Response, ApiError> {
    let service = enabled(&s)?;
    let identity = service.identity(&q.uid).await.map_err(error)?;
    service
        .store
        .import(&identity.key, q.value)
        .map_err(error)?;
    let draft = service.store.draft(&identity.key);
    let mut value = draft.value.clone();
    normalize_draft(&mut value, &identity);
    if value != draft.value {
        service
            .store
            .save(&identity.key, draft.revision, value)
            .map_err(error)?;
    }
    Ok(Json(json!({"ok":true})).into_response())
}
pub async fn drafts(State(s): State<AppState>) -> Result<Response, ApiError> {
    let service = enabled(&s)?;
    let records = service
        .store
        .drafts()
        .into_iter()
        .map(|(_, d)| json!({"uid":d.value["session"]["uid"],"draft":d}))
        .collect::<Vec<_>>();
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"drafts":records})),
    )
        .into_response())
}
