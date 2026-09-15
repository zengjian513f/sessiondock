//! Explicit creation receipts; native identities and reliable send stay separate.
use crate::{
    error::ApiError,
    lifecycle::{
        launcher::{DEFAULT_COMPLETIONS, Entry},
        model::{Launch, LaunchSpec, Record, Source, State as LaunchState},
        service::{Error as ServiceError, LifecycleService},
        store::Error as StoreError,
    },
    state::AppState,
};
use axum::{
    Json,
    extract::{
        Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

pub(super) fn enabled(state: &AppState) -> Result<&std::sync::Arc<LifecycleService>, ApiError> {
    state
        .lifecycle
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("受控会话创建与 pending 生命周期"))
}

pub(super) fn failure(error: ServiceError) -> ApiError {
    let (status, code, message) = match error {
        ServiceError::InvalidBinding => (
            StatusCode::BAD_REQUEST,
            "invalid_native_binding",
            "需要明确确认同一来源的完整原生主会话；不能用显示名称或子代理替代",
        ),
        ServiceError::BindingUnsupported => (
            StatusCode::NOT_IMPLEMENTED,
            "native_binding_unsupported",
            "该宿主或数据源尚不支持一次性原生绑定",
        ),
        ServiceError::BindingConflict => (
            StatusCode::CONFLICT,
            "native_binding_conflict",
            "此启动实例已有不同关联意图或宿主绑定；不会覆盖或自动选择其他会话",
        ),
        ServiceError::Busy => (
            StatusCode::TOO_MANY_REQUESTS,
            "lifecycle_busy",
            "创建服务繁忙，请稍后查询同一请求，不要生成新的请求 ID",
        ),
        ServiceError::NotReady => (
            StatusCode::CONFLICT,
            "launch_not_ready",
            "该创建实例尚未通过就绪验证或已取消，不能连接控制台",
        ),
        ServiceError::IdentityConflict => (
            StatusCode::CONFLICT,
            "launch_identity",
            "创建实例身份不匹配，不会操作替换进程",
        ),
        ServiceError::Store(StoreError::Missing) => {
            (StatusCode::NOT_FOUND, "launch_missing", "没有这个创建回执")
        }
        ServiceError::Store(
            StoreError::Conflict | StoreError::WrongState | StoreError::StaleAuthority,
        ) => (
            StatusCode::CONFLICT,
            "launch_conflict",
            "请求或创建状态冲突；不会重新启动不确定实例",
        ),
        ServiceError::Store(StoreError::InvalidRequest | StoreError::InvalidSpec)
        | ServiceError::Launcher(_) => (
            StatusCode::BAD_REQUEST,
            "invalid_launch",
            "启动配置、适配器或工作目录无效",
        ),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            "lifecycle_unavailable",
            "创建服务或持久状态不可用；操作结果可能不确定，请保留请求 ID 查询，不能盲目重试",
        ),
    };
    ApiError::new(status, code, message)
}

fn invalid() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "invalid_launch_request",
        "创建请求格式或身份字段无效",
    )
}
fn parse_body<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body.map(|Json(value)| value).map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "创建请求体过大",
            )
        } else {
            invalid()
        }
    })
}
fn diagnostics(_values: [&str; 3]) -> Result<(), ApiError> {
    Ok(())
}
pub(super) fn check_instance(record: &Record, instance: &str) -> Result<(), ApiError> {
    if record.instance_id() != instance {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "launch_identity",
            "创建回执与进程实例不匹配",
        ));
    }
    Ok(())
}

/// Deliberate browser projection. No argv/env/host token/endpoint/native fake UID.
pub(super) fn project(record: &Record) -> Value {
    let running = record.state() == LaunchState::Running && !record.cancel_requested();
    let reason = if record.cancel_requested() && record.state() != LaunchState::Exited {
        "取消意图已保存，不再授权新的输入连接；进程是否退出仍须独立确认"
    } else {
        match record.state() {
            LaunchState::Running => "",
            LaunchState::Prepared => "创建意图已保存，尚未启动；不会重复授权启动",
            LaunchState::Starting => "正在启动并校验宿主实例",
            LaunchState::Exited => "终端进程已退出；创建回执仍保留",
            LaunchState::Failed => "创建已失败或启动前被取消；创建回执仍保留",
            _ => "启动或取消结果尚不确定；不会自动重试或按名称操作其它进程",
        }
    };
    // `method` is who asserted the association: the operator dialog, or the
    // server's own process evidence; `evidence` / `bound_at` are the
    // persisted note and confirmation time of the latter.
    let binding = record.binding().map(|binding| {
        json!({"source":binding.spec().source(),
        "sid":binding.spec().sid(),"uid":binding.spec().uid(),"state":binding.state(),
        "method":binding.method(),"evidence":binding.evidence(),"bound_at":binding.bound_at()})
    });
    // A declared SID/UID is the exact command-line identity (Claude
    // `--session-id`, or `--resume`/`resume` of an inventory session). It is
    // reported as a declaration, never as confirmation or reliable delivery.
    json!({"ok":true,"record_id":record.record_id(),"request_id":record.request_id(),
        "name":record.host_name(),"source":record.spec().source(),"cwd":record.spec().cwd(),
        "launch_id":record.launch_id(),"instance_id":record.instance_id(),"state":record.state(),
        "pending":true,"waiting":true,"running":running,"stale":!running,
        "launch_kind":record.spec().launch().kind(),
        "declared_sid":record.declared_sid(),"declared_uid":record.declared_uid(),
        "started":record.created_at(),"finished_at":record.finished_at(),"discarded":record.discarded(),
        "discardable":record.discardable(),
        "unavailable_reason":reason,"native_binding":binding.as_ref().map(|v|v["state"].clone()).unwrap_or(json!("unbound")),"binding":binding})
}

#[derive(Deserialize)]
pub struct BindRequest {
    record_id: String,
    instance_id: String,
    uid: String,
    operator_confirmed: bool,
    #[serde(default)]
    _build: String,
    #[serde(default)]
    _trace_id: String,
    #[serde(default)]
    _page_id: String,
}
pub async fn bind(
    State(state): State<AppState>,
    body: Result<Json<BindRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let permit = admit(&state).await?;
    let body = parse_body(body)?;
    diagnostics([&body._build, &body._trace_id, &body._page_id])?;
    if body.record_id.len() != 32 || body.instance_id.len() != 32 || !body.operator_confirmed {
        return Err(invalid());
    }
    let record = service.get(body.record_id).await.map_err(failure)?;
    check_instance(&record, &body.instance_id)?;
    if record.declared_sid().is_some() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "launch_identity_declared",
            "该实例启动时已在命令行声明完整原生会话 ID；由运行时目录关联，不接受另行的操作者绑定",
        ));
    }
    let uid = body.uid;
    let scope = state
        .reader
        .run_wait(&state.shutdown, move |store| {
            store.native_catalog()?.verified_scope(&uid).map_err(|_| {
                crate::sessions::SessionError {
                    status: 409,
                    message: "原生会话身份缺失、冲突或不支持；不会按显示 SID、文件名或目录猜测关联"
                        .into(),
                }
            })
        })
        .await?;
    let authority = crate::lifecycle::service::VerifiedNativeBinding::from_scope(
        &scope,
        &record,
        body.operator_confirmed,
    )
    .map_err(failure)?;
    let result = service.bind(authority).await.map_err(failure)?;
    response(project(&result), permit).await
}
/// The receipt list for the display polls (`/api/live` exit receipts,
/// `/api/term/list.pending`): one `LifecycleService::list` — every live
/// receipt probed through its host — serves both for
/// [`crate::polls::RECEIPTS_TTL`] under one lifecycle generation; concurrent
/// misses wait for one refresh and `force` refreshes regardless. Mutation
/// paths keep their own fresh lists.
pub(super) async fn shared_records(
    state: &AppState,
    service: &LifecycleService,
    force: bool,
) -> Result<std::sync::Arc<Vec<Record>>, ApiError> {
    let generation = service.generation();
    if !force && let Some(records) = state.polls.receipts(generation) {
        return Ok(records);
    }
    let _flight = state.polls.receipts_flight.lock().await;
    let generation = service.generation();
    if !force && let Some(records) = state.polls.receipts(generation) {
        return Ok(records);
    }
    let records = std::sync::Arc::new(service.list(0, usize::MAX).await.map_err(failure)?);
    state.polls.store_receipts(generation, records.clone());
    Ok(records)
}
pub(super) async fn admit(state: &AppState) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    crate::state::admit(&state.lifecycle_http, "lifecycle_response_busy").await
}
pub(super) async fn response(
    value: Value,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<Response, ApiError> {
    let (bytes, permit) = serialize(value, permit).await?;
    Ok(response_bytes(bytes, permit))
}
/// Encode a lifecycle answer off the reactor; the response permit travels
/// with the bytes.
pub(super) async fn serialize(
    value: Value,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<(axum::body::Bytes, tokio::sync::OwnedSemaphorePermit), ApiError> {
    let (bytes, permit) = tokio::task::spawn_blocking(move || (serde_json::to_vec(&value), permit))
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "lifecycle_response",
                "创建状态序列化失败",
            )
        })?;
    let bytes = bytes.map_err(|_| invalid())?;
    Ok((axum::body::Bytes::from(bytes), permit))
}
/// The lifecycle response shape over already encoded bytes: the permit is
/// held until the body has been sent or dropped.
pub(super) fn response_bytes(
    mut bytes: axum::body::Bytes,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Response {
    let length = bytes.len();
    let body = axum::body::Body::from_stream(async_stream::stream! {
        let _permit=permit;
        while !bytes.is_empty(){yield Ok::<_,std::convert::Infallible>(bytes.split_to(bytes.len().min(32*1024)));}
    });
    (
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_TYPE, "application/json"),
        ],
        [(header::CONTENT_LENGTH, length.to_string())],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod response_tests {
    use super::*;

    #[tokio::test]
    async fn large_status_response_streams_complete_body_and_releases_permit() {
        let admission = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let permit = admission.clone().acquire_owned().await.unwrap();
        let value = json!({"value": "x".repeat(2 * 1024 * 1024 + 1)});
        let response = response(value.clone(), permit).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(admission.available_permits(), 0);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), value);
        assert_eq!(admission.available_permits(), 1);
    }
}

#[derive(Deserialize)]
pub struct CreateRequest {
    source: Source,
    cwd: String,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    #[serde(rename = "adapter_id")]
    _adapter_id: Option<String>,
    /// Full local UID of an inventory main session to resume. Its SID is
    /// resolved server-side from the frozen native catalog, never accepted
    /// from the client or derived from a path.
    #[serde(default)]
    resume_uid: Option<String>,
    #[serde(default)]
    create_cwd: bool,
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
    #[serde(default)]
    _build: String,
    #[serde(default)]
    _trace_id: String,
    #[serde(default)]
    _page_id: String,
}

/// Exactly one interactive entry must match the source; a resume additionally
/// requires resume support. Legacy `adapter_id` input is ignored because the
/// endpoint selects commands solely from its fixed source table.
fn select_entry(
    service: &LifecycleService,
    source: Source,
    resume: bool,
) -> Result<&Entry, ApiError> {
    match service.entry_for(source, resume) {
        Some(entry) => Ok(entry),
        None if resume => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "launch_adapter",
            "来源没有唯一的可续接 CLI 配置",
        )),
        None => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "launch_adapter",
            "来源没有唯一的已配置 CLI",
        )),
    }
}
fn launch_for(entry: &Entry, resume: Option<(String, String)>) -> Launch {
    match resume {
        Some((sid, uid)) => Launch::Resume { sid, uid },
        None if !entry.profile || entry.source == Source::Shell => Launch::Fixed,
        None if entry.source != Source::Codex => Launch::NewAssigned,
        None => Launch::NewPending,
    }
}
fn request_id(value: Option<String>, strict: bool) -> Result<String, ApiError> {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        let valid = (8..=128).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte));
        if valid {
            return Ok(value);
        }
        if strict {
            return Err(invalid());
        }
    }
    crate::lifecycle::store::fresh_request_id()
        .map_err(|_| failure(ServiceError::Store(StoreError::RandomUnavailable)))
}

fn normalize_missing(path: &Path) -> Result<PathBuf, ApiError> {
    let absolute = std::path::absolute(path).map_err(|_| invalid())?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    let mut ancestor = normalized.as_path();
    let mut suffix = Vec::new();
    loop {
        match ancestor.canonicalize() {
            Ok(base) => {
                let mut target = base;
                for name in suffix.iter().rev() {
                    target.push(name);
                }
                return Ok(target);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = ancestor.file_name() else {
                    return Err(invalid());
                };
                suffix.push(name.to_owned());
                ancestor = ancestor.parent().ok_or_else(invalid)?;
            }
            Err(_) => return Err(invalid()),
        }
    }
}

enum PreparedCwd {
    Ready(String),
    Missing(String),
}

async fn prepare_cwd(raw: String, create: bool) -> Result<PreparedCwd, ApiError> {
    tokio::task::spawn_blocking(move || {
        let text = raw.trim();
        if text.is_empty() || text.contains('\0') {
            return Err(invalid());
        }
        let path = crate::lifecycle::model::expand_user(Path::new(text)).ok_or_else(invalid)?;
        if !path.is_absolute() {
            return Err(invalid());
        }
        match path.canonicalize() {
            Ok(path) if path.is_dir() => {
                Ok(PreparedCwd::Ready(path.to_string_lossy().into_owned()))
            }
            Ok(_) => Err(invalid()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let target = normalize_missing(&path)?;
                if !create {
                    return Ok(PreparedCwd::Missing(target.to_string_lossy().into_owned()));
                }
                std::fs::create_dir_all(&target).map_err(|error| {
                    ApiError::new(
                        StatusCode::BAD_REQUEST,
                        "create_cwd_failed",
                        format!("创建启动目录失败：{error}"),
                    )
                })?;
                let resolved = target.canonicalize().map_err(|_| invalid())?;
                if !resolved.is_dir() {
                    return Err(invalid());
                }
                Ok(PreparedCwd::Ready(resolved.to_string_lossy().into_owned()))
            }
            Err(_) => Err(invalid()),
        }
    })
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "cwd_check_failed",
            "启动目录检查失败",
        )
    })?
}
/// Resolve a main-session native scope and its inventory cwd from one frozen
/// snapshot. Missing sessions are 404; conflicting/unsupported/subagent
/// identities are 409. Nothing here trusts a client-provided path or SID.
async fn resolve_resume(
    state: &AppState,
    uid: String,
) -> Result<(crate::sessions::NativeScope, Option<String>), ApiError> {
    state
        .reader
        .run_wait(&state.shutdown, move |store| {
            let snapshot = store.search_snapshot()?;
            let scope = snapshot
                .native_catalog()
                .verified_scope(&uid)
                .map_err(|reason| crate::sessions::SessionError {
                    status: if reason == crate::runtime::AssociationReason::NativeMissing {
                        404
                    } else {
                        409
                    },
                    message: "原生会话身份缺失、冲突或不支持续接；不会按显示 SID、文件名或目录猜测"
                        .into(),
                })?;
            let cwd = snapshot.list["sessions"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|row| row["uid"] == scope.uid)
                .and_then(|row| row["cwd"].as_str())
                .filter(|cwd| !cwd.is_empty())
                .map(str::to_owned);
            Ok((scope, cwd))
        })
        .await
}

struct ExternalProcesses {
    scanner: std::sync::Arc<crate::runtime::procscan::ProcScanner>,
    pids: Vec<i64>,
    raw: bool,
    hosted: bool,
    tmux: bool,
    /// A running lifecycle host whose Codex process moved from its declared
    /// parent session into this exact fork. The durable host identity stays
    /// unchanged; takeover can reuse its launch lease instead of starting a
    /// competing `codex resume`.
    managed_fork: Option<(String, String)>,
}

async fn external_processes(state: &AppState, uid: &str) -> Result<ExternalProcesses, ApiError> {
    let document = state
        .reader
        .run_wait(&state.shutdown, |store| store.list_recent())
        .await?;
    let sessions = crate::runtime::procscan::SessionRow::from_list(&document);
    let target = sessions
        .iter()
        .find(|session| session.uid == uid)
        .cloned()
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "session_missing", "会话不存在"))?;
    let scanner = state.proc_scan.clone().unwrap_or_else(|| {
        std::sync::Arc::new(crate::runtime::procscan::ProcScanner::new(
            "/proc".into(),
            None,
            crate::runtime::procscan::SessionRoots::default(),
        ))
    });
    let observed = if state.runtime.is_some() {
        super::runtime::observe(state).await?
    } else {
        None
    };
    let hosted_by_runtime = observed.as_ref().is_some_and(|snapshot| {
        snapshot
            .running_uids()
            .into_iter()
            .any(|running| running == uid)
    });
    let host_roots = observed
        .as_ref()
        .map(|snapshot| snapshot.hosts.iter().map(|host| host.summary.pid).collect())
        .unwrap_or_default();
    let scan = match scanner.snapshot(true).await {
        Ok(scan) => Some(scan),
        // Off `/proc`, an unavailable provider is treated as
        // an empty observation. With no exact PID evidence this path can still
        // reuse or start a managed host, but can never signal a process.
        Err(crate::runtime::procscan::ScanError::UnsupportedPlatform) => None,
        Err(error) => {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "process_scan_unavailable",
                format!("无法核对会话进程：{error}"),
            ));
        }
    };
    let Some(scan) = scan else {
        return Ok(ExternalProcesses {
            scanner,
            pids: Vec::new(),
            raw: false,
            hosted: hosted_by_runtime,
            tmux: false,
            managed_fork: None,
        });
    };
    let raw = !scan.scan.pids_of(&target).is_empty();
    let active = scan.scan.active_processes(&sessions);
    let pids = active.owned.get(uid).cloned().unwrap_or_default();
    let hosted = hosted_by_runtime || scan.scan.tree.hosted(&pids, &host_roots);
    let tmux = scan.scan.tree.in_tmux(&pids);
    let managed_fork = observed
        .as_ref()
        .and_then(|snapshot| snapshot.fork_host(&scan.scan, &sessions, uid))
        .and_then(|host| {
            let bound = host.bound_target()?;
            Some((host.summary.name.clone(), bound.instance_id().to_owned()))
        });
    Ok(ExternalProcesses {
        scanner,
        pids,
        raw,
        hosted,
        tmux,
        managed_fork,
    })
}
fn source_of(scope: &crate::sessions::NativeScope) -> Option<Source> {
    match scope.source.as_str() {
        "claude" => Some(Source::Claude),
        "codex" => Some(Source::Codex),
        "grok" => Some(Source::Grok),
        _ => None,
    }
}
fn build_spec(
    entry: &Entry,
    cwd: &str,
    resume: Option<(String, String)>,
) -> Result<LaunchSpec, ApiError> {
    // The coordinator/store validates new cwd off-reactor, after idempotent
    // replay lookup; an old receipt remains readable when its cwd is gone.
    // Directory completion returns `dir/`; one trailing separator is display
    // form, not a different directory.
    let cwd = cwd
        .strip_suffix('/')
        .filter(|stripped| !stripped.is_empty())
        .unwrap_or(cwd);
    let launch = launch_for(entry, resume);
    serde_json::from_value(
        json!({"source":entry.source,"adapter_id":entry.id,"cwd":cwd,"launch":launch}),
    )
    .map_err(|_| invalid())
}
pub async fn create(
    State(state): State<AppState>,
    body: Result<Json<CreateRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let permit = admit(&state).await?;
    let body = parse_body(body)?;
    diagnostics([&body._build, &body._trace_id, &body._page_id])?;
    let request_id = request_id(body.request_id, true)?;
    crate::terminal::terminal_size(body.cols.unwrap_or(120), body.rows.unwrap_or(32))?;
    let entry = select_entry(service, body.source, body.resume_uid.is_some())?.clone();
    let resume = match body.resume_uid {
        Some(uid) => {
            let (scope, _) = resolve_resume(&state, uid).await?;
            if source_of(&scope) != Some(body.source) {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "launch_source",
                    "续接会话的数据源与请求来源不一致",
                ));
            }
            Some((scope.session_id, scope.uid))
        }
        None => None,
    };
    let cwd = match prepare_cwd(body.cwd, body.create_cwd).await? {
        PreparedCwd::Ready(cwd) => cwd,
        PreparedCwd::Missing(cwd) => {
            let mut reply = response(
                json!({"error":"启动目录不存在","needs_create":true,"cwd":cwd}),
                permit,
            )
            .await?;
            *reply.status_mut() = StatusCode::CONFLICT;
            return Ok(reply);
        }
    };
    let spec = build_spec(&entry, &cwd, resume)?;
    let record = service.create(request_id, spec).await.map_err(failure)?;
    response(project(&record), permit).await
}

#[derive(Deserialize)]
pub struct TakeoverRequest {
    uid: String,
    #[serde(default)]
    #[serde(rename = "request_id")]
    _request_id: Option<String>,
    #[serde(default)]
    #[serde(rename = "adapter_id")]
    _adapter_id: Option<String>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
    #[serde(default)]
    _build: String,
    #[serde(default)]
    _trace_id: String,
    #[serde(default)]
    _page_id: String,
}
/// Takeover: reuse a managed console, ask before replacing an external
/// CLI, then resume the exact catalog identity in its recorded cwd.
pub async fn takeover(
    State(state): State<AppState>,
    body: Result<Json<TakeoverRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let permit = admit(&state).await?;
    let body = parse_body(body)?;
    diagnostics([&body._build, &body._trace_id, &body._page_id])?;
    let request_id = crate::lifecycle::store::fresh_request_id()
        .map_err(|_| failure(ServiceError::Store(StoreError::RandomUnavailable)))?;
    crate::terminal::terminal_size(body.cols.unwrap_or(120), body.rows.unwrap_or(32))?;
    let (scope, cwd) = resolve_resume(&state, body.uid).await?;
    let source = source_of(&scope).ok_or_else(invalid)?;
    let processes = external_processes(&state, &scope.uid).await?;
    if let Some((host, instance)) = &processes.managed_fork {
        let records = service.list(0, usize::MAX).await.map_err(failure)?;
        let mut matches = records.iter().filter(|record| {
            record.host_name() == host
                && record.instance_id() == instance
                && record.spec().source() == source
                && record.state() == crate::lifecycle::model::State::Running
                && !record.cancel_requested()
        });
        if let Some(record) = matches.next()
            && matches.next().is_none()
        {
            let mut value = project(record);
            value["action"] = json!("reused");
            value["followed_fork"] = json!(true);
            return response(value, permit).await;
        }
    }
    if processes.raw && processes.pids.is_empty() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "takeover_superseded",
            "该回滚分支的运行实例已转移到更新的子会话，请先处理当前子会话",
        ));
    }
    let external: Vec<i64> = processes
        .pids
        .iter()
        .copied()
        .filter(|pid| *pid > 0)
        .collect();
    if !external.is_empty() && !processes.hosted && !body.force {
        return response(
            json!({"needs_confirm":true,"pids":external,
                "reason": if processes.tmux {"会话正在 tmux 中运行；当前宿主不能直接附加该 pane"}
                    else {"会话正在运行, 且不在受管终端里"}}),
            permit,
        )
        .await;
    }
    let killed = if !external.is_empty() && !processes.hosted && body.force {
        let outcome = processes
            .scanner
            .kill_pids(&external)
            .await
            .map_err(|error| {
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "process_control_unavailable",
                    format!("无法结束外部会话进程：{error:?}"),
                )
            })?;
        !outcome.killed.is_empty()
    } else {
        false
    };
    let entry = select_entry(service, source, true)?.clone();
    let cwd = cwd.ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "launch_cwd_unknown",
            "该会话没有记录可用的工作目录；请通过创建接口明确指定目录续接",
        )
    })?;
    let spec = build_spec(&entry, &cwd, Some((scope.session_id, scope.uid)))?;
    let record = service
        .create(request_id.clone(), spec)
        .await
        .map_err(failure)?;
    let mut value = project(&record);
    value["action"] = json!(if killed {
        "killed"
    } else if processes.hosted || record.request_id() != request_id {
        "reused"
    } else {
        "started"
    });
    response(value, permit).await
}

#[derive(Deserialize)]
pub struct CompleteDirQuery {
    #[serde(default)]
    path: String,
    #[serde(default)]
    limit: Option<usize>,
    /// `debug_run`: the page's view selector, appended to every `/api/`
    /// URL by the frontend; accepted and ignored here.
    #[serde(default)]
    #[allow(dead_code)]
    debug_run: String,
}
/// Absolute-directory completion, with the typed spelling
/// preserved.
pub async fn complete_dir(
    State(state): State<AppState>,
    query: Result<Query<CompleteDirQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let permit = admit(&state).await?;
    let Query(query) = query.map_err(|_| invalid())?;
    let directories = service
        .complete_directories(query.path, query.limit.unwrap_or(DEFAULT_COMPLETIONS))
        .await
        .map_err(|error| match error {
            ServiceError::Launcher(_) => {
                ApiError::new(StatusCode::BAD_REQUEST, "invalid_path", "启动目录路径无效")
            }
            other => failure(other),
        })?;
    response(json!({"directories":directories}), permit).await
}

#[derive(Deserialize)]
pub struct BackendRequest {
    backend: String,
    #[serde(default)]
    _build: String,
    #[serde(default)]
    _trace_id: String,
    #[serde(default)]
    _page_id: String,
}
pub fn backends() -> Value {
    json!([
        {"name":"ptyhost","label":"默认宿主","available":true,"current":true,"unavailable_reason":""},
        {"name":"tmux","label":"tmux","available":false,"current":false,
         "unavailable_reason":"Rust 后端不支持 tmux 会话托管；新建会话只能由 ptyhost 托管。"}
    ])
}
/// Only ptyhost exists. Selecting it is idempotent and persists nothing; tmux
/// is an explicit error rather than a silently ignored preference.
pub async fn backend(
    State(state): State<AppState>,
    body: Result<Json<BackendRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    if state.terminal.is_none() {
        return Err(ApiError::unavailable("终端后端选择"));
    }
    let permit = admit(&state).await?;
    let body = parse_body(body)?;
    diagnostics([&body._build, &body._trace_id, &body._page_id])?;
    match body.backend.trim().to_ascii_lowercase().as_str() {
        "ptyhost" | "host" => {
            response(
                json!({"ok":true,"backend":"ptyhost","backends":backends()}),
                permit,
            )
            .await
        }
        "tmux" => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "backend_unsupported",
            "Rust 后端不支持 tmux；新建会话只能由 ptyhost 托管",
        )),
        _ => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "backend_unknown",
            "未知终端后端",
        )),
    }
}

#[derive(Deserialize)]
pub struct StatusQuery {
    record_id: String,
    instance_id: String,
    /// `debug_run`: the page's view selector, appended to every `/api/`
    /// URL by the frontend; accepted and ignored here.
    #[serde(default)]
    #[allow(dead_code)]
    debug_run: String,
}
pub async fn status(
    State(state): State<AppState>,
    query: Result<Query<StatusQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let permit = admit(&state).await?;
    let Query(query) = query.map_err(|_| invalid())?;
    if query.record_id.len() != 32 || query.instance_id.len() != 32 {
        return Err(invalid());
    }
    let record = service.get(query.record_id).await.map_err(failure)?;
    check_instance(&record, &query.instance_id)?;
    response(project(&record), permit).await
}

#[derive(Deserialize)]
pub struct CancelRequest {
    record_id: String,
    instance_id: String,
    #[serde(default)]
    _build: String,
    #[serde(default)]
    _trace_id: String,
    #[serde(default)]
    _page_id: String,
}
pub async fn cancel(
    State(state): State<AppState>,
    body: Result<Json<CancelRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let permit = admit(&state).await?;
    let body = parse_body(body)?;
    diagnostics([&body._build, &body._trace_id, &body._page_id])?;
    if body.record_id.len() != 32 || body.instance_id.len() != 32 {
        return Err(invalid());
    }
    let record = service
        .cancel(body.record_id, body.instance_id)
        .await
        .map_err(failure)?;
    response(project(&record), permit).await
}

/// Discard for the Rust receipt ledger: a finished
/// (Exited/Failed) or durably cancelled receipt leaves the sidebar's pending
/// list. The receipt itself stays queryable through `term/new-status`; a
/// receipt whose instance may still run is 409 and must be stopped first.
pub async fn discard(
    State(state): State<AppState>,
    body: Result<Json<CancelRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let permit = admit(&state).await?;
    let body = parse_body(body)?;
    diagnostics([&body._build, &body._trace_id, &body._page_id])?;
    if body.record_id.len() != 32 || body.instance_id.len() != 32 {
        return Err(invalid());
    }
    let record = service
        .discard(body.record_id, body.instance_id)
        .await
        .map_err(|error| match error {
            ServiceError::NotReady => ApiError::new(
                StatusCode::CONFLICT,
                "launch_not_finished",
                "该创建实例尚未退出或取消，不能丢弃；请先停止它",
            ),
            other => failure(other),
        })?;
    response(project(&record), permit).await
}

// ------------------------------------------------------------------ session stop
#[derive(Deserialize)]
pub struct StopRequest {
    #[serde(default)]
    uid: String,
    #[serde(default)]
    _build: String,
    #[serde(default)]
    _trace_id: String,
    #[serde(default)]
    _page_id: String,
}
fn invalid_stop() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "invalid_stop_request",
        "停止请求格式或会话 UID 无效",
    )
}
/// Use guarded host control for a managed instance and
/// the same native SID/file/process-tree evidence for an external CLI.
pub async fn stop(
    State(state): State<AppState>,
    body: Result<Json<StopRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    if state.terminal.is_none() || state.runtime.is_none() {
        return Err(ApiError::unavailable("受管实例停止"));
    }
    let permit = admit(&state).await?;
    let body = body.map(|Json(value)| value).map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "停止请求体过大",
            )
        } else {
            invalid_stop()
        }
    })?;
    diagnostics([&body._build, &body._trace_id, &body._page_id]).map_err(|_| invalid_stop())?;
    let uid = body.uid;
    // Index catalog: an unknown UID is 404 ("会话不存在").
    // Subagent/unsupported/ambiguous rows proceed to the runtime lookup, which
    // can only ever match a verified main-session scope.
    let known = {
        let uid = uid.clone();
        state
            .reader
            .run_wait(&state.shutdown, move |store| {
                Ok(!matches!(
                    store.native_catalog()?.verified_scope(&uid),
                    Err(crate::runtime::AssociationReason::NativeMissing)
                ))
            })
            .await?
    };
    if !known {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "session_missing",
            "会话不存在",
        ));
    }
    let external = external_processes(&state, &uid).await?;
    if external.raw && external.pids.is_empty() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "stop_superseded",
            "该回滚分支已不是当前运行分支，未停止共享的子会话",
        ));
    }
    let observed = super::runtime::observe(&state)
        .await?
        .ok_or_else(|| ApiError::unavailable("受管实例停止"))?;
    let targets: Vec<&ptyhost_client::BoundTarget> = observed
        .hosts
        .iter()
        .filter_map(|host| host.bound_target())
        .filter(|target| {
            external.managed_fork.as_ref().map_or_else(
                || target.uid() == uid,
                |(name, instance)| target.name() == name && target.instance_id() == instance,
            )
        })
        .collect();
    if targets.is_empty() {
        let pids: Vec<i64> = external.pids.into_iter().filter(|pid| *pid > 0).collect();
        if pids.is_empty() {
            return response(
                json!({"ok":true,"stopped":false,"tmux":external.tmux,
                    "external_detection":"proc_scan"}),
                permit,
            )
            .await;
        }
        let killed = external.scanner.kill_pids(&pids).await.map_err(|error| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "process_control_unavailable",
                format!("无法结束外部会话进程：{error:?}"),
            )
        })?;
        return response(
            json!({"ok":true,"stopped":!killed.killed.is_empty(),"tmux":external.tmux,
                "external_detection":"proc_scan"}),
            permit,
        )
        .await;
    }
    let session = observed.sessions.get(&uid);
    let candidate = match targets.as_slice() {
        [target] => crate::lifecycle::service::StopCandidate::Instance(Box::new((*target).clone())),
        [_, _, ..] => crate::lifecycle::service::StopCandidate::Unknown,
        [] => match session.map(|session| session.state) {
            Some(crate::runtime::RunState::Exited) => {
                crate::lifecycle::service::StopCandidate::Exited
            }
            Some(crate::runtime::RunState::Unknown | crate::runtime::RunState::Running) => {
                crate::lifecycle::service::StopCandidate::Unknown
            }
            None => crate::lifecycle::service::StopCandidate::NoInstance,
        },
    };
    let outcome = service
        .stop_session(uid, candidate)
        .await
        .map_err(failure)?;
    use crate::lifecycle::service::StopStage;
    match outcome.stage {
        StopStage::NoInstance => {
            response(
                json!({"ok":true,"stopped":false,"tmux":false,"external_detection":"proc_scan"}),
                permit,
            )
            .await
        }
        StopStage::Unknown => {
            let reason = session
                .and_then(|session| session.reason)
                .and_then(|reason| serde_json::to_value(reason).ok())
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "duplicate_host".into());
            Err(ApiError::new(
                StatusCode::CONFLICT,
                "run_state_unknown",
                format!(
                    "该会话的受管实例运行状态未知（{reason}），未发送任何停止指令；未知不等于已退出，请稍后重试或检查宿主"
                ),
            ))
        }
        stage => {
            let explanation = match stage {
                StopStage::Graceful => "CLI 已在收到 Ctrl-D 后自行退出",
                StopStage::Stopped => "CLI 未响应 Ctrl-D，已由宿主执行受保护停止并确认退出",
                StopStage::AlreadyExited => "该受管实例此前已退出；未发送任何停止指令",
                _ => {
                    "已发送 Ctrl-D 与宿主受保护停止，但在限时内未观察到退出；结果不确定，不会自动重试，也不会按 PID 强杀"
                }
            };
            let mut value = serde_json::to_value(&outcome).map_err(|_| invalid_stop())?;
            value["ok"] = json!(true);
            value["tmux"] = json!(false);
            value["external_detection"] = json!("proc_scan");
            value["explanation"] = json!(explanation);
            response(value, permit).await
        }
    }
}
