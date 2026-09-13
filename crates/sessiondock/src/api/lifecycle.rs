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
        ServiceError::StopRequestConflict => (
            StatusCode::CONFLICT,
            "stop_request_conflict",
            "同一 request_id 已用于停止另一个会话；请使用新的请求 ID",
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
        ServiceError::Store(StoreError::Limit) => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "lifecycle_limit",
            "创建回执或请求达到容量限制；已有记录不会被淘汰",
        ),
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
            "启动配置、适配器或工作目录不符合显式授权范围",
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
fn diagnostics(values: [&str; 3]) -> Result<(), ApiError> {
    if values.iter().any(|v| v.len() > 128) {
        Err(invalid())
    } else {
        Ok(())
    }
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
    // server's own process evidence (WP-E); `evidence` / `bound_at` are the
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
#[serde(deny_unknown_fields)]
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
    let permit = admit(&state)?;
    let body = parse_body(body)?;
    diagnostics([&body._build, &body._trace_id, &body._page_id])?;
    if body.record_id.len() != 32
        || body.instance_id.len() != 32
        || body.uid.len() > 256
        || !body.operator_confirmed
    {
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
pub(super) fn admit(state: &AppState) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    state
        .lifecycle_http
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "lifecycle_response_busy",
                "创建状态响应达到并发限制，请释放旧响应后重试",
            )
        })
}
pub(super) async fn response(
    value: Value,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<Response, ApiError> {
    let (bytes, permit) = tokio::task::spawn_blocking(move || {
        let mut writer = LimitedJson {
            bytes: Vec::new(),
            exceeded: false,
        };
        let result = serde_json::to_writer(&mut writer, &value);
        (
            result.map(|()| writer.bytes).map_err(|_| writer.exceeded),
            permit,
        )
    })
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "lifecycle_response",
            "创建状态序列化失败",
        )
    })?;
    let bytes = bytes.map_err(|exceeded| {
        if exceeded {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "lifecycle_response_limit",
                "创建状态响应超过大小限制",
            )
        } else {
            invalid()
        }
    })?;
    let length = bytes.len();
    let mut bytes = axum::body::Bytes::from(bytes);
    let body = axum::body::Body::from_stream(async_stream::stream! {
        let _permit=permit;
        while !bytes.is_empty(){yield Ok::<_,std::convert::Infallible>(bytes.split_to(bytes.len().min(32*1024)));}
    });
    Ok((
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_TYPE, "application/json"),
        ],
        [(header::CONTENT_LENGTH, length.to_string())],
        body,
    )
        .into_response())
}

struct LimitedJson {
    bytes: Vec<u8>,
    exceeded: bool,
}
impl std::io::Write for LimitedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > (2 * 1024 * 1024usize).saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::other("bounded JSON response exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod response_tests {
    use super::*;

    #[tokio::test]
    async fn oversized_serialization_stops_at_limit_and_releases_response_permit() {
        let admission = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let permit = admission.clone().acquire_owned().await.unwrap();
        let response = response(json!({"value": "x".repeat(2 * 1024 * 1024)}), permit)
            .await
            .unwrap_err()
            .into_response();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(admission.available_permits(), 1);
        let mut writer = LimitedJson {
            bytes: Vec::new(),
            exceeded: false,
        };
        assert!(serde_json::to_writer(&mut writer, &"x".repeat(2 * 1024 * 1024)).is_err());
        assert!(writer.exceeded);
        assert!(writer.bytes.len() <= 2 * 1024 * 1024);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRequest {
    source: Source,
    cwd: String,
    request_id: String,
    #[serde(default)]
    adapter_id: Option<String>,
    /// Full local UID of an inventory main session to resume. Its SID is
    /// resolved server-side from the frozen native catalog, never accepted
    /// from the client or derived from a path.
    #[serde(default)]
    resume_uid: Option<String>,
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

/// Exactly one allowlisted entry must match the source (and explicit ID); a
/// resume additionally requires a resume-capable CLI profile. Without an
/// explicit ID only interactive entries count: the bug-report worker profile
/// of a source is never the browser's implicit choice.
fn select_entry<'a>(
    service: &'a LifecycleService,
    source: Source,
    adapter_id: Option<&str>,
    resume: bool,
) -> Result<&'a Entry, ApiError> {
    let candidates: Vec<&Entry> = service
        .entries()
        .iter()
        .filter(|entry| {
            entry.source == source
                && adapter_id.map_or(entry.interactive(), |id| id == entry.id)
                && (!resume || entry.resume)
        })
        .collect();
    match candidates.as_slice() {
        [entry] => Ok(entry),
        _ if resume => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "launch_adapter",
            "来源没有唯一的可续接 CLI 配置；请明确选择已授权的版本化 adapter_id",
        )),
        _ => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "launch_adapter",
            "来源没有唯一的已配置适配器；请明确选择已授权的版本化 adapter_id",
        )),
    }
}
fn launch_for(entry: &Entry, resume: Option<(String, String)>) -> Launch {
    match resume {
        Some((sid, uid)) => Launch::Resume { sid, uid },
        None if !entry.profile => Launch::Fixed,
        None if entry.source == Source::Claude => Launch::NewAssigned,
        None => Launch::NewPending,
    }
}
fn valid_uid(uid: &str) -> bool {
    !uid.is_empty()
        && uid.len() <= 256
        && uid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
}
/// Resolve a main-session native scope and its inventory cwd from one frozen
/// snapshot. Missing sessions are 404; conflicting/unsupported/subagent
/// identities are 409. Nothing here trusts a client-provided path or SID.
async fn resolve_resume(
    state: &AppState,
    uid: String,
) -> Result<(crate::sessions::NativeScope, Option<String>), ApiError> {
    if !valid_uid(&uid) {
        return Err(invalid());
    }
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
    let permit = admit(&state)?;
    let body = parse_body(body)?;
    diagnostics([&body._build, &body._trace_id, &body._page_id])?;
    if body.cwd.len() > 4096 || body.request_id.len() > 128 {
        return Err(invalid());
    }
    crate::terminal::terminal_size(body.cols.unwrap_or(120), body.rows.unwrap_or(32))?;
    let entry = select_entry(
        service,
        body.source,
        body.adapter_id.as_deref(),
        body.resume_uid.is_some(),
    )?
    .clone();
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
    let spec = build_spec(&entry, &body.cwd, resume)?;
    let record = service
        .create(body.request_id, spec)
        .await
        .map_err(failure)?;
    response(project(&record), permit).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeoverRequest {
    uid: String,
    request_id: String,
    #[serde(default)]
    adapter_id: Option<String>,
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
/// Legacy takeover = resume of an inventory session in its recorded cwd. There
/// is no external-process discovery, so `force` (kill an unmanaged instance)
/// stays an explicit 501 instead of guessing PIDs by cwd, name or time.
pub async fn takeover(
    State(state): State<AppState>,
    body: Result<Json<TakeoverRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let permit = admit(&state)?;
    let body = parse_body(body)?;
    diagnostics([&body._build, &body._trace_id, &body._page_id])?;
    if body.request_id.len() > 128 || !valid_uid(&body.uid) {
        return Err(invalid());
    }
    if body.force {
        return Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "takeover_force_unsupported",
            "不支持结束未受管的外部 CLI 实例：没有跨平台的进程归属证据，不会按目录或时间猜测 PID",
        ));
    }
    crate::terminal::terminal_size(body.cols.unwrap_or(120), body.rows.unwrap_or(32))?;
    let (scope, cwd) = resolve_resume(&state, body.uid).await?;
    let source = source_of(&scope).ok_or_else(invalid)?;
    let entry = select_entry(service, source, body.adapter_id.as_deref(), true)?.clone();
    let cwd = cwd.ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "launch_cwd_unknown",
            "该会话没有记录可用的工作目录；请通过创建接口明确指定白名单内的目录续接",
        )
    })?;
    let spec = build_spec(&entry, &cwd, Some((scope.session_id, scope.uid)))?;
    let record = service
        .create(body.request_id.clone(), spec)
        .await
        .map_err(failure)?;
    let mut value = project(&record);
    value["action"] = json!(if record.request_id() == body.request_id {
        "started"
    } else {
        "reused"
    });
    response(value, permit).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteDirQuery {
    #[serde(default)]
    path: String,
    #[serde(default)]
    limit: Option<usize>,
    /// `debug_run`: the page's view selector, appended to every `/api/`
    /// URL by the frontend; accepted and ignored here like Python.
    #[serde(default)]
    #[allow(dead_code)]
    debug_run: String,
}
/// Bounded completion strictly inside the launcher's cwd roots. The UI's
/// `~` and relative forms are not expanded: only absolute paths complete.
pub async fn complete_dir(
    State(state): State<AppState>,
    query: Result<Query<CompleteDirQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let permit = admit(&state)?;
    let Query(query) = query.map_err(|_| invalid())?;
    if query.path.len() > 4096 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_path",
            "启动目录路径过长",
        ));
    }
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
#[serde(deny_unknown_fields)]
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
    let permit = admit(&state)?;
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
#[serde(deny_unknown_fields)]
pub struct StatusQuery {
    record_id: String,
    instance_id: String,
    /// `debug_run`: the page's view selector, appended to every `/api/`
    /// URL by the frontend; accepted and ignored here like Python.
    #[serde(default)]
    #[allow(dead_code)]
    debug_run: String,
}
pub async fn status(
    State(state): State<AppState>,
    query: Result<Query<StatusQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let permit = admit(&state)?;
    let Query(query) = query.map_err(|_| invalid())?;
    if query.record_id.len() != 32 || query.instance_id.len() != 32 {
        return Err(invalid());
    }
    let record = service.get(query.record_id).await.map_err(failure)?;
    check_instance(&record, &query.instance_id)?;
    response(project(&record), permit).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
    let permit = admit(&state)?;
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

/// Python `pending_store.discard` for the Rust receipt ledger: a finished
/// (Exited/Failed) or durably cancelled receipt leaves the sidebar's pending
/// list. The receipt itself stays queryable through `term/new-status`; a
/// receipt whose instance may still run is 409 and must be stopped first.
pub async fn discard(
    State(state): State<AppState>,
    body: Result<Json<CancelRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let permit = admit(&state)?;
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
#[serde(deny_unknown_fields)]
pub struct StopRequest {
    uid: String,
    /// Optional: the same ID replays the remembered outcome for this UID.
    /// The Python page sends `{uid}` only and needs no server-side flag.
    #[serde(default)]
    request_id: Option<String>,
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
const UNMANAGED: &str = "该会话没有由本服务托管的运行实例，无法停止：Rust 后端不探测未受管的外部 CLI 进程，也不会按目录、时间或 PID 猜测。请在启动该 CLI 的终端里退出它。";
/// Python `_stop_session` for managed instances only. The UID is resolved
/// through the index's native catalog and one fresh guarded runtime observation
/// (never the `/api/live` cache, never a name/cwd/time/PID guess); the
/// service then escalates Ctrl-D → host-performed guarded stop and reports
/// which stage ended the instance. An unmanaged session is a typed 501, an
/// unreachable/duplicate managed record a typed 409 — never a silent success.
pub async fn stop(
    State(state): State<AppState>,
    body: Result<Json<StopRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    if state.terminal.is_none() || state.runtime.is_none() {
        return Err(ApiError::unavailable("受管实例停止"));
    }
    let permit = admit(&state)?;
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
    if !valid_uid(&body.uid)
        || body
            .request_id
            .as_deref()
            .is_some_and(|id| id.is_empty() || id.len() > 128)
    {
        return Err(invalid_stop());
    }
    let uid = body.uid;
    // Index catalog: an unknown UID is 404 like Python's "会话不存在".
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
    let observed = super::runtime::observe(&state)
        .await?
        .ok_or_else(|| ApiError::unavailable("受管实例停止"))?;
    let targets: Vec<&ptyhost_client::BoundTarget> = observed
        .hosts
        .iter()
        .filter_map(|host| host.bound_target())
        .filter(|target| target.uid() == uid)
        .collect();
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
        .stop_session(uid, candidate, body.request_id)
        .await
        .map_err(failure)?;
    use crate::lifecycle::service::StopStage;
    match outcome.stage {
        StopStage::NoInstance => Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "session_stop_unmanaged",
            UNMANAGED,
        )),
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
            value["external_detection"] = json!("not_implemented");
            value["explanation"] = json!(explanation);
            response(value, permit).await
        }
    }
}
