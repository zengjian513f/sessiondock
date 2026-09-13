//! Explicit-directory development transport only; legacy CLI actions stay gated.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::{
    Extension, Json,
    extract::{
        ConnectInfo, Query, RawQuery, State,
        rejection::{JsonRejection, QueryRejection},
        ws::{WebSocketUpgrade, rejection::WebSocketUpgradeRejection},
    },
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    error::ApiError,
    state::AppState,
    terminal::{ExpectedTarget, InputPayload, TerminalError, TerminalService, input},
};

impl From<TerminalError> for ApiError {
    fn from(error: TerminalError) -> Self {
        Self::new(
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
            error.code,
            error.message,
        )
    }
}

fn enabled(state: &AppState) -> Result<&std::sync::Arc<TerminalService>, ApiError> {
    state.terminal.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "terminal_disabled",
            "终端传输未启用：必须显式配置隔离的 ptyhost 目录",
        )
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimRequest {
    name: String,
    page: String,
    #[serde(default)]
    uid: Option<String>,
    #[serde(default)]
    instance_id: Option<String>,
    #[serde(default)]
    record_id: Option<String>,
    #[serde(default)]
    launch_id: Option<String>,
    #[serde(default)]
    force: bool,
    // Legacy post() adds these diagnostics. They confer no authority and are
    // not logged, stored, or forwarded to the host.
    #[serde(default)]
    _build: String,
    #[serde(default)]
    _trace_id: String,
    #[serde(default)]
    _page_id: String,
}

pub async fn claim(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    body: Result<Json<ClaimRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let Json(body) = body.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            return ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "终端预约请求体过大",
            );
        }
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_claim",
            "终端预约请求格式无效",
        )
    })?;
    if [&body._build, &body._trace_id, &body._page_id]
        .iter()
        .any(|field| field.len() > 128)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_claim",
            "终端预约诊断字段过长",
        ));
    }
    let ip = peer
        .map(|Extension(ConnectInfo(address))| address.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let result = tokio::select! {
        biased;
        _ = state.shutdown.cancelled() => return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "shutdown", "服务正在关闭")),
        result = async {
            match (&body.uid,&body.instance_id,&body.record_id,&body.launch_id) {
                (None,None,None,None) => service.claim(&body.name,&body.page,ip,body.force).await.map_err(ApiError::from),
                (None,Some(instance),Some(record),Some(launch)) => {
                    let target=launch_target(&state,&body.name,record,launch,instance).await?;
                    service.claim_launch(target,&body.page,ip,body.force).await.map_err(ApiError::from)
                }
                (Some(uid),Some(instance),None,None) => {
                    validate_binding(uid,instance)?;
                    let observed=super::runtime::observe(&state).await?.ok_or_else(binding_unavailable)?;
                    let target=observed.hosts.iter().filter_map(|host|host.bound_target()).find(|target|
                        target.name()==body.name && target.uid()==uid && target.instance_id()==instance
                    ).ok_or_else(binding_unavailable)?;
                    authorize_native(&state,target).await?;
                    service.claim_bound(target,&body.page,ip,body.force).await.map_err(ApiError::from)
                }
                _ => Err(binding_unavailable()),
            }
        } => result?,
    };
    let status = StatusCode::from_u16(result.status()).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
    Ok((
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(result.into_api_json()),
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AttachQuery {
    name: String,
    page: String,
    token: String,
    /// Client display/audit identifier, never an ownership connection ID.
    connection: String,
    cols: u16,
    rows: u16,
    uid: Option<String>,
    instance_id: Option<String>,
    record_id: Option<String>,
    launch_id: Option<String>,
    /// `debug_run`: the page's view selector, appended to every `/api/`
    /// URL by the frontend; accepted and ignored here like Python.
    #[allow(dead_code)]
    debug_run: String,
}

impl Default for AttachQuery {
    fn default() -> Self {
        Self {
            name: String::new(),
            page: String::new(),
            token: String::new(),
            connection: String::new(),
            cols: 120,
            rows: 32,
            uid: None,
            instance_id: None,
            record_id: None,
            launch_id: None,
            debug_run: String::new(),
        }
    }
}

pub async fn attach(
    State(state): State<AppState>,
    query: Result<Query<AttachQuery>, QueryRejection>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let Query(query) = query.map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_attach",
            "终端连接参数无效",
        )
    })?;
    let binding_validation = match (
        &query.uid,
        &query.instance_id,
        &query.record_id,
        &query.launch_id,
    ) {
        (None, None, None, None) => Ok(()),
        (Some(uid), Some(instance), None, None) => match validate_binding(uid, instance) {
            Err(error) => Err(error),
            Ok(()) => {
                async {
                    let observed = super::runtime::observe(&state)
                        .await?
                        .ok_or_else(binding_unavailable)?;
                    let target = observed
                        .hosts
                        .iter()
                        .filter_map(|host| host.bound_target())
                        .find(|target| {
                            target.name() == query.name
                                && target.uid() == uid
                                && target.instance_id() == instance
                        })
                        .ok_or_else(binding_unavailable)?;
                    authorize_native(&state, target).await
                }
                .await
            }
        },
        (None, Some(instance), Some(record), Some(launch)) => {
            launch_target(&state, &query.name, record, launch, instance)
                .await
                .map(|_| ())
        }
        _ => Err(binding_unavailable()),
    };
    if let Err(error) = binding_validation {
        service.cancel_reservation(&query.name, &query.page, &query.token);
        return Err(error);
    }
    if query.connection.len() > 128
        || !query
            .connection
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
        service.cancel_reservation(&query.name, &query.page, &query.token);
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_attach",
            "终端连接标识无效",
        ));
    }
    let size = match crate::terminal::terminal_size(query.cols, query.rows) {
        Ok(size) => size,
        Err(error) => {
            service.cancel_reservation(&query.name, &query.page, &query.token);
            return Err(error.into());
        }
    };
    let ws = match ws {
        Ok(ws) => ws,
        Err(_) => {
            service.cancel_reservation(&query.name, &query.page, &query.token);
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "websocket_required",
                "需要有效的 WebSocket 升级请求",
            ));
        }
    };
    if state.shutdown.is_cancelled() {
        service.cancel_reservation(&query.name, &query.page, &query.token);
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "shutdown",
            "服务正在关闭",
        ));
    }
    let prepared = match match (
        &query.uid,
        &query.instance_id,
        &query.record_id,
        &query.launch_id,
    ) {
        (None, None, None, None) => service.prepare(&query.name, &query.page, &query.token),
        (None, Some(instance), Some(_), Some(launch)) => {
            service.prepare_launch(&query.name, &query.page, &query.token, launch, instance)
        }
        (Some(uid), Some(instance), None, None) => {
            service.prepare_bound(&query.name, &query.page, &query.token, uid, instance)
        }
        _ => return Err(binding_unavailable()),
    } {
        Ok(prepared) => prepared,
        Err(error) => {
            service.cancel_reservation(&query.name, &query.page, &query.token);
            return Err(error.into());
        }
    };
    let max_input = service.limits().max_input_bytes;
    Ok(ws
        .read_buffer_size(16 * 1024)
        .write_buffer_size(0)
        .max_write_buffer_size(128 * 1024)
        .max_message_size(max_input)
        .max_frame_size(max_input)
        // Upgrade failures drop the callback's PreparedAttachment guard; do not
        // log the rejection, request URI, token, or peer's arbitrary text.
        .on_failed_upgrade(|_| {})
        .on_upgrade(move |socket| prepared.run(socket, size, state.shutdown)))
}

fn binding_unavailable() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "terminal_binding_unavailable",
        "无法确认会话与终端实例的唯一关联；请刷新，不会降级按名称连接。",
    )
}

/// Raw HTTP input. Same lease/page/token and binding tuple as the WebSocket
/// attach query; exactly one of `data` (UTF-8 text, sent as-is) or `keys`
/// (named keys). Legacy `text`/`enter` submit semantics are deliberately not
/// accepted: this is not the reliable-send composer.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendRequest {
    name: String,
    page: String,
    token: String,
    #[serde(default)]
    uid: Option<String>,
    #[serde(default)]
    instance_id: Option<String>,
    #[serde(default)]
    record_id: Option<String>,
    #[serde(default)]
    launch_id: Option<String>,
    /// Legacy page context only; the lease decides the target, not this field.
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    keys: Option<Vec<String>>,
    #[serde(default)]
    _build: String,
    #[serde(default)]
    _trace_id: String,
    #[serde(default)]
    _page_id: String,
}

fn invalid_input(message: &'static str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid_terminal_input", message)
}

pub async fn send(
    State(state): State<AppState>,
    body: Result<Json<SendRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let Json(body) = body.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            return ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "terminal_input_too_large",
                "终端输入请求体过大",
            );
        }
        invalid_input("终端输入请求格式无效")
    })?;
    if [&body._build, &body._trace_id, &body._page_id]
        .iter()
        .any(|field| field.len() > 128)
        || body.agent.as_ref().is_some_and(|agent| agent.len() > 256)
    {
        return Err(invalid_input("终端输入诊断字段过长"));
    }
    let payload = match (body.data, body.keys) {
        (Some(data), None) => {
            // The legacy page gates text writes on the served asset build so a
            // tab that outlived a deployment cannot keep typing blindly.
            if body._build != state.assets.build {
                return Ok((
                    StatusCode::CONFLICT,
                    [(header::CACHE_CONTROL, "no-store")],
                    Json(json!({
                        "error": "页面版本已过期，请重新加载整个网页后再输入",
                        "code": "stale_build",
                        "reload": true,
                        "build": state.assets.build,
                    })),
                )
                    .into_response());
            }
            if data.is_empty() {
                return Err(invalid_input("终端输入为空"));
            }
            if data.len() > input::MAX_INPUT_BYTES {
                return Err(ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "terminal_input_too_large",
                    "单次终端输入不能超过 16 KiB",
                ));
            }
            InputPayload::Text(data)
        }
        (None, Some(keys)) => {
            let (names, _) = input::map_keys(&keys).map_err(|error| match error {
                input::KeyError::Empty => invalid_input("keys 不能为空"),
                input::KeyError::TooMany => invalid_input("单次最多 256 个按键"),
                input::KeyError::TooLarge => ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "terminal_input_too_large",
                    "单次终端输入不能超过 16 KiB",
                ),
                input::KeyError::Unknown(_) => invalid_input(
                    "不支持的按键名；可用：enter escape tab backspace delete insert space home end pageup pagedown up down left right f1-f12 ctrl-<字母>",
                ),
            })?;
            InputPayload::Keys(names)
        }
        _ => return Err(invalid_input("必须且只能提供 data 或 keys 之一")),
    };
    let expected = match (
        &body.uid,
        &body.instance_id,
        &body.record_id,
        &body.launch_id,
    ) {
        (None, None, None, None) => ExpectedTarget::Raw,
        (Some(uid), Some(instance), None, None) => {
            validate_binding(uid, instance)?;
            ExpectedTarget::Native { uid, instance }
        }
        (None, Some(instance), Some(record), Some(launch)) => {
            if record.len() != 32 || launch.len() != 32 || instance.len() != 32 {
                return Err(binding_unavailable());
            }
            ExpectedTarget::Launch { launch, instance }
        }
        _ => return Err(binding_unavailable()),
    };
    let receipt = tokio::select! {
        biased;
        _ = state.shutdown.cancelled() => return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "shutdown", "服务正在关闭")),
        result = service.send_input(&body.name, &body.page, &body.token, expected, payload) => result?,
    };
    Ok((
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        // `acknowledged` is the host's PTY write acknowledgement. Whether the
        // CLI processed the bytes is unknown here and is not claimed.
        Json(json!({"ok":true,"bytes":receipt.bytes,"acknowledged":true,"processed":"unknown"})),
    )
        .into_response())
}

/// Legacy scroll protocol. The ptyhost backend has no copy-mode or server-side
/// scroll position: the original Python `term_host.scroll` returns 0 and lets
/// the browser xterm scroll its own buffer, and `leave_copy_mode` is a no-op.
/// The host's `capture` operation is a text snapshot, not a view state, so no
/// PTY write, key translation, or host I/O happens here at all.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScrollRequest {
    name: String,
    #[serde(default)]
    up: bool,
    #[serde(default = "default_scroll_lines")]
    lines: u32,
    #[serde(default)]
    cancel: bool,
    #[serde(default)]
    _build: String,
    #[serde(default)]
    _trace_id: String,
    #[serde(default)]
    _page_id: String,
}

fn default_scroll_lines() -> u32 {
    3
}

pub async fn scroll(
    State(state): State<AppState>,
    body: Result<Json<ScrollRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let service = enabled(&state)?;
    let Json(body) = body.map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_scroll",
            "终端滚动请求格式无效",
        )
    })?;
    if [&body._build, &body._trace_id, &body._page_id]
        .iter()
        .any(|field| field.len() > 128)
        || !(1..=100).contains(&body.lines)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_scroll",
            "终端滚动参数无效：lines 须在 1..100",
        ));
    }
    if !service.has_host(&body.name).await? {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "terminal_missing",
            "指定目录中没有这个终端 host",
        ));
    }
    // Direction and cancel are accepted for the legacy request shape; there is
    // no server-side position to move, so both are no-ops here.
    let _ = (body.up, body.cancel);
    Ok((
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"pos":0,"scrollback":"browser"})),
    )
        .into_response())
}
async fn authorize_native(
    state: &AppState,
    target: &ptyhost_client::BoundTarget,
) -> Result<(), ApiError> {
    if target.origin_launch_id().is_some() {
        super::lifecycle::enabled(state)?
            .authorize_native(target)
            .await
            .map_err(super::lifecycle::failure)?;
    }
    Ok(())
}
async fn launch_target(
    state: &AppState,
    name: &str,
    record: &str,
    launch: &str,
    instance: &str,
) -> Result<std::sync::Arc<ptyhost_client::LaunchTarget>, ApiError> {
    if record.len() != 32 || launch.len() != 32 || instance.len() != 32 {
        return Err(binding_unavailable());
    }
    let target = super::lifecycle::enabled(state)?
        .target(record.into())
        .await
        .map_err(super::lifecycle::failure)?;
    if target.name() != name || target.launch_id() != launch || target.instance_id() != instance {
        return Err(binding_unavailable());
    }
    Ok(target)
}
fn validate_binding(uid: &str, instance: &str) -> Result<(), ApiError> {
    if uid.is_empty()
        || uid.len() > 256
        || !(16..=128).contains(&instance.len())
        || uid
            .bytes()
            .chain(instance.bytes())
            .any(|byte| !byte.is_ascii_alphanumeric() && !b"_.:-".contains(&byte))
    {
        return Err(binding_unavailable());
    }
    Ok(())
}

/// Python `pending_store.active` keeps a resolved row for 600 s; a finished
/// Rust receipt (Exited/Failed) leaves `term/list.pending` this long after
/// its terminal state was recorded. It stays queryable by record id.
pub const PENDING_ARCHIVE_AFTER: u64 = 600;

/// Whether a receipt still belongs in the sidebar's pending list: not
/// discarded by the operator and not finished for longer than
/// [`PENDING_ARCHIVE_AFTER`]. A finished receipt with no recorded time (an
/// older ledger) is archived at once, like Python's vanished pane.
fn pending_listed(record: &crate::lifecycle::model::Record, now: u64) -> bool {
    use crate::lifecycle::model::State;
    if record.discarded() {
        return false;
    }
    if matches!(record.state(), State::Exited | State::Failed) {
        return record
            .finished_at()
            .is_some_and(|finished| now.saturating_sub(finished) < PENDING_ARCHIVE_AFTER);
    }
    true
}

pub async fn list(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let permit = super::lifecycle::admit(&state)?;
    // Python `str(Path.home())`: the frontend abbreviates cwd with it.
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut response = json!({"enabled":false, "transport_enabled":state.terminal.is_some(),
        "unavailable_reason":"没有通过完整会话 UID 和实例校验的运行中终端；创建和 CLI 接管尚未启用。",
        "sources":{},"home":home,"backend":"ptyhost","backends":[],"sessions":[],"pending":[],"hosts":[]});
    if let Some(service) = &state.terminal {
        response["hosts"] = json!(service.hosts().await?);
        if let Some(observed) = super::runtime::observe(&state).await? {
            let sessions: Vec<Value> = observed
                .hosts
                .iter()
                .filter_map(|host| {
                    let target = host.bound_target()?;
                    let mut row = json!(host.summary);
                    row["uid"] = json!(target.uid());
                    row["sid"] = json!(target.sid());
                    row["source"] = json!(target.source().as_str());
                    row["instance_id"] = json!(target.instance_id());
                    row["origin_launch_id"] = json!(target.origin_launch_id());
                    Some(row)
                })
                .collect();
            response["enabled"] = json!(!sessions.is_empty());
            response["sessions"] = json!(sessions);
        }
    } else {
        response["unavailable_reason"] =
            json!("Rust 后端当前为只读开发阶段，尚未接入控制台或 CLI 进程。");
    }
    if let Some(service) = &state.lifecycle {
        let records = service
            .list(0, 128)
            .await
            .map_err(super::lifecycle::failure)?;
        // Batch 41: a bug-report worker's row carries Python's pending record
        // fields (`kind`, `title` "处理 <id>", `report_id`) so the sidebar
        // names the report instead of "新建 … 会话".
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        response["pending"] = json!(
            records
                .iter()
                .filter(|record| pending_listed(record, now))
                .map(|record| {
                    let mut row = super::lifecycle::project(record);
                    if let Some(extra) = state
                        .bug_report
                        .as_ref()
                        .and_then(|bug| bug.service.pending_decoration(record.record_id()))
                        && let (Some(target), Some(fields)) =
                            (row.as_object_mut(), extra.as_object())
                    {
                        for (key, value) in fields {
                            target.insert(key.clone(), value.clone());
                        }
                    }
                    row
                })
                .collect::<Vec<_>>()
        );
        if let Some(sessions) = response["sessions"].as_array_mut() {
            sessions.retain(|row| {
                row["origin_launch_id"].is_null()
                    || records.iter().any(|record| {
                        record.launch_id() == row["origin_launch_id"]
                            && record.instance_id() == row["instance_id"]
                            && record.host_name() == row["name"]
                            && record.state() == crate::lifecycle::model::State::Running
                            && !record.cancel_requested()
                            && record.binding().is_some_and(|binding| {
                                binding.state() == crate::lifecycle::model::BindingState::Confirmed
                                    && binding.spec().uid() == row["uid"]
                                    && binding.spec().sid() == row["sid"]
                            })
                    })
            });
        }
        let sources = [
            crate::lifecycle::model::Source::Claude,
            crate::lifecycle::model::Source::Codex,
            crate::lifecycle::model::Source::Grok,
        ];
        for source in sources {
            let key = serde_json::to_value(source).expect("source enum");
            let key = key.as_str().expect("source string");
            response["sources"][key] = json!(
                state
                    .launch_adapters
                    .iter()
                    .filter(|adapter| adapter.source == source)
                    .count()
                    == 1
            );
            // Resume/takeover needs exactly one resume-capable interactive CLI profile.
            response["resume_sources"][key] = json!(
                service
                    .entries()
                    .iter()
                    .filter(|entry| entry.source == source && entry.resume && entry.interactive())
                    .count()
                    == 1
            );
        }
        response["backends"] = super::lifecycle::backends();
        response["enabled"] = json!(true);
        response["unavailable_reason"] = json!("");
    } else if let Some(sessions) = response["sessions"].as_array_mut() {
        sessions.retain(|row| row["origin_launch_id"].is_null());
        response["enabled"] = json!(!sessions.is_empty());
    }
    // Python filters the pane list and the pending receipts through the
    // debug-run registry exactly like the session list (`filter_rows`).
    let debug_run = crate::sessions::debug_run_of(query.as_deref());
    let runs = state.reader.store.debug_runs();
    if !debug_run.is_empty() || !runs.is_empty() {
        if let Some(sessions) = response["sessions"].as_array_mut() {
            sessions.retain(|row| runs.keeps(row, &debug_run));
        }
        if let Some(pending) = response["pending"].as_array_mut() {
            pending.retain(|row| {
                runs.keeps(
                    &json!({"uid": row["declared_uid"], "sid": row["declared_sid"],
                            "name": row["name"], "cwd": row["cwd"]}),
                    &debug_run,
                )
            });
        }
    }
    super::lifecycle::response(response, permit).await
}
