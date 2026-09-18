//! Explicit-directory development transport only; legacy CLI actions stay gated.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::{
    Extension, Json,
    extract::{
        ConnectInfo, Query, RawQuery, State,
        rejection::{JsonRejection, QueryRejection},
        ws::{WebSocketUpgrade, rejection::WebSocketUpgradeRejection},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    error::ApiError,
    hub::proxy::display_ip,
    state::AppState,
    terminal::{
        Claimant, ExpectedTarget, InputPayload, TerminalError, TerminalService, UnleasedTarget,
        device::device_label, input,
    },
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
pub struct ClaimRequest {
    #[serde(deserialize_with = "python_string")]
    name: String,
    #[serde(deserialize_with = "python_string")]
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
    force: Value,
    // Legacy post() adds these diagnostics. They confer no authority and are
    // not logged, stored, or forwarded to the host.
    #[serde(default)]
    _build: Value,
    #[serde(default)]
    _trace_id: Value,
    #[serde(default)]
    _page_id: Value,
}

fn python_string<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let raw = <Box<serde_json::value::RawValue>>::deserialize(deserializer)?;
    super::delivery::python_id_value(raw.get(), false).map_err(serde::de::Error::custom)
}

fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::Number(number) => number.as_f64() != Some(0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
        Value::Bool(true) => true,
    }
}

/// The claimant's ownership label. Only the
/// authenticated node listener trusts the hub's forwarded `X-Real-IP` /
/// `X-Forwarded-For`; on the browser listener the TCP peer is the label, so a
/// page cannot pick its own. Display only — it never enters lease identity.
fn claimant_ip(hub: bool, headers: &HeaderMap, peer: Option<IpAddr>) -> IpAddr {
    let untrusted = HeaderMap::new();
    display_ip(if hub { headers } else { &untrusted }, peer)
        .parse()
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

pub async fn claim(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    headers: HeaderMap,
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
    let ip = claimant_ip(
        hub.is_some(),
        &headers,
        peer.map(|Extension(ConnectInfo(address))| address.ip()),
    );
    let claimant = Claimant {
        ip,
        label: device_label(
            headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default(),
        ),
    };
    let result = tokio::select! {
        biased;
        _ = state.shutdown.cancelled() => return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "shutdown", "服务正在关闭")),
        result = async {
            match (&body.uid,&body.instance_id,&body.record_id,&body.launch_id) {
                (None,None,None,None) => service.claim(&body.name,&body.page,claimant,python_truthy(&body.force)).await.map_err(ApiError::from),
                (None,Some(instance),Some(record),Some(launch)) => {
                    let target=launch_target(&state,&body.name,record,launch,instance).await?;
                    service.claim_launch(target,&body.page,claimant,python_truthy(&body.force)).await.map_err(ApiError::from)
                }
                (Some(uid),Some(instance),None,None) => {
                    let observed=super::runtime::observe(&state).await?.ok_or_else(binding_unavailable)?;
                    let target=observed.hosts.iter().filter_map(|host|host.bound_target()).find(|target|
                        target.name()==body.name && target.uid()==uid && target.instance_id()==instance
                    ).ok_or_else(binding_unavailable)?;
                    authorize_native(&state,target).await?;
                    service.claim_bound(target,&body.page,claimant,python_truthy(&body.force)).await.map_err(ApiError::from)
                }
                _ => Err(binding_unavailable()),
            }
        } => result?,
    };
    let status = StatusCode::from_u16(result.status()).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
    // A holder at the claimant's own display address is "another page", not a
    // place: the page then drops the address from its takeover prompt.
    let same_address = status == StatusCode::CONFLICT && result.owner().ip == ip;
    let mut body = result.into_api_json();
    if let (StatusCode::CONFLICT, Value::Object(fields)) = (status, &mut body) {
        fields.insert("same_address".into(), Value::Bool(same_address));
    }
    Ok((status, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response())
}

#[derive(Deserialize)]
#[serde(default)]
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
    /// URL by the frontend; accepted and ignored here.
    #[allow(dead_code)]
    debug_run: String,
    /// `grid` streams the server-side grid protocol instead of raw bytes.
    mode: String,
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
            mode: String::new(),
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
        (Some(uid), Some(instance), None, None) => {
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
    // WebSocket frames up to 8 MiB are accepted. HTTP send/paste keeps the
    // ptyhost guarded-operation 1 MiB ceiling.
    let max_input = service.limits().max_host_frame_bytes;
    let mode = if query.mode == "grid" {
        crate::terminal::AttachMode::Grid
    } else {
        crate::terminal::AttachMode::Bytes
    };
    Ok(ws
        .read_buffer_size(16 * 1024)
        .write_buffer_size(0)
        .max_write_buffer_size(128 * 1024)
        .max_message_size(usize::MAX)
        .max_frame_size(max_input)
        // Upgrade failures drop the callback's PreparedAttachment guard; do not
        // log the rejection, request URI, token, or peer's arbitrary text.
        .on_failed_upgrade(|_| {})
        .on_upgrade(move |socket| prepared.with_mode(mode).run(socket, size, state.shutdown)))
}

fn binding_unavailable() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "terminal_binding_unavailable",
        "无法确认会话与终端实例的唯一关联；请刷新，不会降级按名称连接。",
    )
}

/// HTTP input under the same lease as WebSocket attach, or — with an empty
/// `token` and the pinned identity — from a page holding no lease, refused
/// while any lease exists. Exactly one of `data`, `paste` or `keys` is
/// accepted. `paste` uses the host's bracketed-paste path; composers on the
/// raw path send Enter separately after its acknowledgement.
#[derive(Deserialize)]
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
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    paste: Option<String>,
    #[serde(default)]
    keys: Option<Vec<String>>,
    #[serde(default)]
    #[serde(deserialize_with = "python_string")]
    _build: String,
    #[serde(default)]
    _trace_id: Value,
    #[serde(default)]
    _page_id: Value,
}

fn invalid_input(message: &'static str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid_terminal_input", message)
}

pub async fn send(
    State(state): State<AppState>,
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
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
    let payload = match (body.data, body.paste, body.keys) {
        (Some(data), None, None) => {
            // The legacy page gates text writes on the served asset build so a
            // tab that outlived a deployment cannot keep typing blindly.
            if hub.is_none() && body._build != state.assets.build {
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
                    "单次终端输入不能超过 1 MiB",
                ));
            }
            InputPayload::Text(data)
        }
        (None, Some(paste), None) => {
            if hub.is_none() && body._build != state.assets.build {
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
            if paste.is_empty() {
                return Err(invalid_input("终端粘贴内容为空"));
            }
            if paste.len() > crate::terminal::MAX_PASTE_BYTES {
                return Err(ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "terminal_input_too_large",
                    "单次终端粘贴不能超过 1 MiB",
                ));
            }
            InputPayload::Paste(paste)
        }
        (None, None, Some(keys)) => {
            let (names, _) = input::map_keys(&keys).map_err(|error| match error {
                input::KeyError::Empty => invalid_input("keys 不能为空"),
                input::KeyError::TooMany => invalid_input("单次最多 256 个按键"),
                input::KeyError::TooLarge => ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "terminal_input_too_large",
                    "单次终端输入不能超过 1 MiB",
                ),
                input::KeyError::Unknown(_) => invalid_input("按键名不能为空且不能超过 256 字节"),
            })?;
            InputPayload::Keys(names)
        }
        _ => return Err(invalid_input("必须且只能提供 data、paste 或 keys 之一")),
    };
    let expected = match (
        &body.uid,
        &body.instance_id,
        &body.record_id,
        &body.launch_id,
    ) {
        (None, None, None, None) => ExpectedTarget::Raw,
        (Some(uid), Some(instance), None, None) => ExpectedTarget::Native { uid, instance },
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
        result = async {
            if !body.token.is_empty() {
                return service
                    .send_input(&body.name, &body.page, &body.token, expected, payload)
                    .await
                    .map_err(ApiError::from);
            }
            // An empty token is a page that holds no console lease here (the
            // conversation view's Esc, question cards and Grok text). The
            // pinned instance is resolved exactly like a claim; the service
            // then applies the ordinary-claimant rule under the per-name gate.
            match (&body.uid, &body.instance_id, &body.record_id, &body.launch_id) {
                (Some(uid), Some(instance), None, None) => {
                    let observed = super::runtime::observe(&state).await?.ok_or_else(binding_unavailable)?;
                    let target = observed.hosts.iter().filter_map(|host| host.bound_target()).find(|target|
                        target.name() == body.name && target.uid() == uid && target.instance_id() == instance
                    ).ok_or_else(binding_unavailable)?;
                    authorize_native(&state, target).await?;
                    service.send_input_unleased(UnleasedTarget::Native(target), payload).await.map_err(ApiError::from)
                }
                (None, Some(instance), Some(record), Some(launch)) => {
                    let target = launch_target(&state, &body.name, record, launch, instance).await?;
                    service.send_input_unleased(UnleasedTarget::Launch(&target), payload).await.map_err(ApiError::from)
                }
                // Without a lease there is no name-only write to a host.
                _ => Err(binding_unavailable()),
            }
        } => result?,
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
/// scroll position: `scroll` returns 0 and lets the browser xterm scroll its
/// own buffer, and `leave_copy_mode` is a no-op.
/// The host's `capture` operation is a text snapshot, not a view state, so no
/// PTY write, key translation, or host I/O happens here at all.
#[derive(Deserialize)]
pub struct ScrollRequest {
    name: String,
    #[serde(default)]
    up: Value,
    #[serde(default = "default_scroll_lines")]
    lines: Value,
    #[serde(default)]
    cancel: Value,
    #[serde(default)]
    _build: Value,
    #[serde(default)]
    _trace_id: Value,
    #[serde(default)]
    _page_id: Value,
}

fn default_scroll_lines() -> Value {
    json!(3)
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
    if !service.has_host(&body.name).await? {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "terminal_missing",
            "指定目录中没有这个终端 host",
        ));
    }
    // Direction and cancel are accepted for the legacy request shape; there is
    // no server-side position to move, so both are no-ops here.
    let _ = (body.up, body.lines, body.cancel);
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
/// A resolved row is kept for 600 s; a finished
/// Rust receipt (Exited/Failed) leaves `term/list.pending` this long after
/// its terminal state was recorded. It stays queryable by record id.
pub const PENDING_ARCHIVE_AFTER: u64 = 600;

/// Whether a receipt still belongs in the sidebar's pending list: not
/// discarded by the operator and not finished for longer than
/// [`PENDING_ARCHIVE_AFTER`]. A finished receipt with no recorded time (an
/// older ledger) is archived at once.
fn pending_listed(record: &crate::lifecycle::model::Record, now: u64) -> bool {
    use crate::lifecycle::model::State;
    if record.discarded() {
        return false;
    }
    if matches!(record.state(), State::Exited | State::Failed) {
        if record.spec().source() == crate::lifecycle::model::Source::Shell {
            return false;
        }
        return record
            .finished_at()
            .is_some_and(|finished| now.saturating_sub(finished) < PENDING_ARCHIVE_AFTER);
    }
    true
}

/// `GET /api/term/list`: the managed panes with their verified session
/// identity, the pending receipts and the source table. The assembled body
/// is served for [`crate::polls::TERM_LIST_TTL`] per debug-run view
/// (`polls::PollCache`) and dropped at once by any lifecycle mutation
/// (create, kill, takeover, bind, stop, discard: the service generation)
/// or a changed debug-run registry; `?force=1` bypasses it and the shared
/// managed observation. The list authorizes nothing, so it reads the
/// shared observation (`runtime::shared`, 2 s) that `/api/live` reads;
/// claim, attach, stop and process-evidence binding keep their fresh probes.
pub async fn list(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let permit = super::lifecycle::admit(&state).await?;
    let force = query
        .as_deref()
        .is_some_and(|query| query.split('&').any(|pair| pair == "force=1"));
    let debug_run = crate::sessions::debug_run_of(query.as_deref());
    let runs = state.reader.store.debug_runs();
    let generation = super::runtime::lifecycle_generation(&state);
    if !force && let Some(bytes) = state.polls.term_list(&debug_run, generation, &runs) {
        return Ok(super::lifecycle::response_bytes(bytes, permit));
    }
    // The frontend abbreviates cwd with it.
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut response = json!({"enabled":false, "transport_enabled":state.terminal.is_some(),
        "unavailable_reason":"没有通过完整会话 UID 和实例校验的运行中终端；创建和 CLI 接管尚未启用。",
        "sources":{},"home":home,"backend":"ptyhost","backends":[],"sessions":[],"pending":[],"hosts":[]});
    if let Some(service) = &state.terminal {
        response["hosts"] = json!(service.hosts().await?);
        if let Some(runtime) = &state.runtime {
            let observed = super::runtime::shared(&state, runtime, force).await?;
            // Display the thread currently open in a Codex TUI separately from
            // its immutable protocol binding. Never rewrite the guard identity.
            let mut current = std::collections::BTreeMap::<String, Vec<String>>::new();
            if let Some(scanner) = &state.proc_scan
                && let Ok(scan) = scanner.snapshot(force).await
            {
                let document = state
                    .reader
                    .run_wait(&state.shutdown, |store| store.list_recent())
                    .await?;
                let rows = crate::runtime::procscan::SessionRow::from_list(&document);
                for row in &rows {
                    if let Some(host) = observed
                        .snapshot
                        .codex_process_host(&scan.scan, &rows, &row.uid)
                    {
                        current
                            .entry(host.summary.name.clone())
                            .or_default()
                            .push(row.uid.clone());
                    }
                }
            }
            let sessions: Vec<Value> = observed
                .snapshot
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
                    if let Some(uids) = current.get(&host.summary.name)
                        && let [uid] = uids.as_slice()
                    {
                        row["current_uid"] = json!(uid);
                    }
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
        // The first 128 receipts in ledger order, as before, out of the
        // shared list (`force=1` refreshes it like everything else).
        let shared = super::lifecycle::shared_records(&state, service, force).await?;
        let records = &shared[..shared.len().min(128)];
        // A bug-report worker's row carries the pending record
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
            crate::lifecycle::model::Source::Shell,
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
            // Resume/takeover needs exactly one resume-capable CLI profile.
            if source != crate::lifecycle::model::Source::Shell {
                response["resume_sources"][key] = json!(service.entry_for(source, true).is_some());
            }
        }
        response["backends"] = super::lifecycle::backends();
        response["enabled"] = json!(true);
        response["unavailable_reason"] = json!("");
    } else if let Some(sessions) = response["sessions"].as_array_mut() {
        sessions.retain(|row| row["origin_launch_id"].is_null());
        response["enabled"] = json!(!sessions.is_empty());
    }
    // The pane list and the pending receipts are filtered through the
    // debug-run registry exactly like the session list (`filter_rows`).
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
    let (bytes, permit) = super::lifecycle::serialize(response, permit).await?;
    state
        .polls
        .store_term_list(&debug_run, generation, runs, bytes.clone());
    Ok(super::lifecycle::response_bytes(bytes, permit))
}
