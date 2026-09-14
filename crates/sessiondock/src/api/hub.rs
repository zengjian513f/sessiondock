//! The hub's HTTP surface (`hub.py` `HubHandler.dispatch`, 518–584), served
//! by the `sessiondock-hub` binary: the legacy page in hub mode, `/api/meta`,
//! `/api/nodes` with the settings-page display/order writes, the five
//! aggregated reads and three split writes (`hub::aggregate`), browser audit
//! fan-out, and everything else resolved to one machine and proxied
//! (`hub::proxy`). One dispatcher: an unmatched path is not a
//! 404 but a request that must name exactly one machine.
//!
//! Gate (before any handler): loopback `Host`, no hub protocol headers, no
//! cross-site or foreign-origin `/api/` request, bounded URI. JSON bodies are
//! read up to `hub::BODY_LIMIT`; uploads stream up to
//! `proxy::ATTACHMENT_MAX_BYTES`.

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Extension, Router,
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, Method, StatusCode, header, uri::Authority},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::any,
};
use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::{
    assets::{self, Assets, HUB_HOSTNAME, HUB_STORAGE_NAMESPACE, Mode},
    hub::{
        BODY_LIMIT, Client, Monitor, PROTOCOL, Registry, RegistryError,
        aggregate::{self, Params},
        namespace,
        proxy::{
            self, ATTACHMENT_PATHS, BUILD_CHECKED_PATHS, Forward, PageNodes, ProxyError, Upload,
            json_response,
        },
    },
    hub_config::HubConfig,
};

/// Routes the hub answers itself (method, path); everything else is
/// resolved to one machine. `tests/route_ledger.py` reads this table.
pub const HUB_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/meta"),
    ("GET", "/api/nodes"),
    ("POST", "/api/nodes/order"),
    ("POST", "/api/nodes/{nid}/display"),
    ("ANY", "/api/nodes/{nid}/api/{*path}"),
    ("GET", "/api/sessions"),
    ("GET", "/api/search"),
    ("GET", "/api/live"),
    ("GET", "/api/term/list"),
    ("GET", "/api/trash"),
    ("POST", "/api/sessions/delete"),
    ("POST", "/api/sessions/fork-visibility"),
    ("POST", "/api/trash/purge"),
    ("POST", "/api/audit/browser"),
    ("GET", "/api/session/file"),
];
/// Reads merged across the selected machines.
const AGGREGATED: [&str; 5] = [
    "/api/sessions",
    "/api/search",
    "/api/live",
    "/api/term/list",
    "/api/trash",
];
/// Browser audit forwarding waits this long per machine (`timeout=2`).
const AUDIT_TIMEOUT: Duration = Duration::from_secs(2);
const NOT_REGISTERED: &str = "机器未注册或已移除";

/// What the hub page declares. Per-machine features (terminal, outbox, files,
/// trash, live, audit) stay undeclared so the page degrades per request like
/// Python's hub page, which declares nothing; the read-model pages every Rust
/// node serves are declared. `media_lazy` is left out: the page blanks a
/// `/api/nodes/<nid>/api/media/…` source before its hub check when set.
pub fn hub_capabilities() -> Value {
    json!({
        "backend": "rust", "hub": true, "storage_namespace": HUB_STORAGE_NAMESPACE,
        "history_pages": true, "media_continuation": true,
        "history_semantics": "limited_native"
    })
}

/// Server-side records of settings-page changes (`server.audit.record` in
/// Python): one JSONL line per event under the hub's audit directory.
pub struct HubAudit {
    directory: PathBuf,
}

impl HubAudit {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// Best effort and off the request path; a full disk never fails the
    /// change it records.
    pub fn record(&self, event: &'static str, category: &'static str, data: Value) {
        let now: chrono::DateTime<chrono::Utc> = std::time::SystemTime::now().into();
        let line = json!({
            "ts": now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "event": event, "category": category, "data": data,
        });
        let path = self
            .directory
            .join(format!("hub-{}.jsonl", now.format("%Y-%m-%d")));
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            if let Ok(mut file) = options.open(path) {
                let mut bytes = serde_json::to_vec(&line).expect("JSON serializes");
                bytes.push(b'\n');
                let _ = file.write_all(&bytes);
            }
        });
    }
}

#[derive(Clone)]
pub struct HubState {
    pub registry: Arc<Registry>,
    pub client: Arc<Client>,
    pub assets: Arc<Assets>,
    pub page_nodes: Arc<PageNodes>,
    pub audit: Option<Arc<HubAudit>>,
    pub shutdown: CancellationToken,
    /// `SESSIONDOCK_PUBLIC_HOSTS`: accepted besides loopback (same rule as the node).
    pub public_hosts: Arc<Vec<String>>,
}

/// The hub router: one gate, one dispatcher.
pub fn hub_router(state: HubState) -> Router {
    Router::new()
        .fallback(any(dispatch))
        .layer(middleware::from_fn_with_state(state.clone(), hub_gate))
        .with_state(state)
}

/// The registry as the binary's subcommands see it (no monitor, no page).
pub fn open_registry(config: &HubConfig) -> std::io::Result<Registry> {
    config.validate()?;
    Ok(Registry::open(
        &config.nodes_file,
        config.networks.clone(),
        &config.cache_dir,
    )?
    .with_public_payload(namespace::public_payload))
}

/// Everything the `sessiondock-hub` binary serves and drives.
pub struct HubApp {
    pub router: Router,
    pub registry: Arc<Registry>,
    pub client: Arc<Client>,
    /// The health monitor; `stop()` on shutdown.
    pub monitor: Monitor,
}

/// Build the hub from its configuration inside a Tokio runtime: the registry
/// with the wire namespace installed, its monitor, the client with Python's
/// timeouts and the page snapshot in hub mode.
pub fn hub_app(config: &HubConfig, shutdown: CancellationToken) -> std::io::Result<HubApp> {
    let registry = Arc::new(open_registry(config)?);
    let client = Arc::new(Client::default());
    let assets = Arc::new(Assets::load_mode(
        &config.web_dir,
        HUB_HOSTNAME,
        &hub_capabilities(),
        Mode::Hub,
    )?);
    let audit = config
        .audit_dir
        .clone()
        .map(|directory| Arc::new(HubAudit::new(directory)));
    let monitor = Monitor::spawn(registry.clone(), client.clone(), shutdown.clone());
    let state = HubState {
        registry: registry.clone(),
        client: client.clone(),
        assets,
        page_nodes: PageNodes::new(),
        audit,
        shutdown,
        public_hosts: Arc::new(config.public_hosts.clone()),
    };
    Ok(HubApp {
        router: hub_router(state),
        registry,
        client,
        monitor,
    })
}

fn closing(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("close"));
    response
}

fn error_json(status: StatusCode, message: &str, code: &str) -> Response {
    closing(json_response(
        status,
        &json!({"error": message, "code": code}),
    ))
}

/// `Host` header or URI authority as sent.
fn authority(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| request.uri().authority().map(|a| a.as_str()))
}

/// Same trust boundary as the node's loopback listener (`security::local_only`)
/// plus Python's cross-origin write rejection, without the blanket body
/// limit: uploads are bounded per path by the dispatcher.
async fn hub_gate(State(state): State<HubState>, request: Request, next: Next) -> Response {
    let allowed = authority(&request)
        .and_then(|h| h.parse::<Authority>().ok())
        .is_some_and(|a| crate::security::host_allowed(&a, &state.public_hosts));
    if !allowed {
        return error_json(
            StatusCode::FORBIDDEN,
            "仅允许本地 loopback Host",
            "local_only",
        );
    }
    if request.headers().contains_key("x-sessiondock-protocol")
        || request.headers().contains_key("x-sessiondock-node-token")
    {
        return error_json(
            StatusCode::FORBIDDEN,
            "尚不支持旧 Hub 节点协议",
            "hub_unsupported",
        );
    }
    let api_request = request.uri().path().starts_with("/api/");
    if api_request {
        if request
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|h| h == "cross-site")
        {
            return error_json(StatusCode::FORBIDDEN, "拒绝跨站 API 请求", "cross_site");
        }
        if let Some(origin) = request.headers().get(header::ORIGIN) {
            let authority = authority(&request).unwrap_or_default();
            let matches = origin.to_str().ok().is_some_and(|origin| {
                ["http", "https"]
                    .iter()
                    .any(|scheme| origin == format!("{scheme}://{authority}"))
            });
            if !matches {
                return error_json(
                    StatusCode::FORBIDDEN,
                    "cross-origin write rejected",
                    "cross_origin",
                );
            }
        }
    }
    if request.method().as_str().len() + request.uri().to_string().len() + 12 > 64 * 1024 {
        return error_json(StatusCode::URI_TOO_LONG, "请求 URI 过长", "uri_too_long");
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if !headers.contains_key(header::REFERRER_POLICY) {
        headers.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        );
    }
    if api_request && !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

/// How `handle` ends early: a finished answer, Python's `ValueError` 400 or
/// the 502 of a lost upstream connection. The response is boxed so the error
/// path stays small (most of these functions return `Result<Response, Reply>`).
enum Reply {
    Response(Box<Response>),
    Invalid(String),
    Upstream,
}

impl From<ProxyError> for Reply {
    fn from(error: ProxyError) -> Self {
        match error {
            ProxyError::Invalid(message) => Self::Invalid(message),
            ProxyError::Upstream => Self::Upstream,
        }
    }
}

impl From<aggregate::AggregateError> for Reply {
    fn from(error: aggregate::AggregateError) -> Self {
        Self::Invalid(error.message().to_string())
    }
}

fn ok(value: &Value) -> Result<Response, Reply> {
    Ok(json_response(StatusCode::OK, value))
}

fn status(status: StatusCode, value: Value) -> Result<Response, Reply> {
    Ok(json_response(status, &value))
}

async fn dispatch(
    State(state): State<HubState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    request: Request,
) -> Response {
    let peer = peer.map(|Extension(ConnectInfo(address))| address);
    match handle(state, peer, request).await {
        Ok(response) => response,
        Err(Reply::Response(response)) => *response,
        Err(Reply::Invalid(message)) => closing(json_response(
            StatusCode::BAD_REQUEST,
            &json!({"error": message}),
        )),
        Err(Reply::Upstream) => closing(json_response(
            StatusCode::BAD_GATEWAY,
            &json!({"error": proxy::UPSTREAM_FAILED}),
        )),
    }
}

/// `HubHandler.read_body`: at most `BODY_LIMIT` bytes of one JSON object;
/// chunked bodies are refused; an empty body is `{}`.
async fn read_body(request: Request) -> Result<Map<String, Value>, Reply> {
    if request.headers().contains_key(header::TRANSFER_ENCODING) {
        return Err(Reply::Invalid("chunked requests are not supported".into()));
    }
    let size = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().parse::<u64>())
        .transpose()
        .map_err(|_| Reply::Invalid("request too large".into()))?
        .unwrap_or(0);
    if size > BODY_LIMIT as u64 {
        return Err(Reply::Invalid("request too large".into()));
    }
    let bytes = axum::body::to_bytes(request.into_body(), BODY_LIMIT)
        .await
        .map_err(|_| Reply::Invalid("request too large".into()))?;
    if bytes.is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(Reply::Invalid("expected JSON object".into())),
        Err(error) => Err(Reply::Invalid(error.to_string())),
    }
}

/// Python truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

async fn handle(
    state: HubState,
    peer: Option<SocketAddr>,
    mut request: Request,
) -> Result<Response, Reply> {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let mut path = uri.path().to_string();
    let query = proxy::parse_qs(uri.query().unwrap_or(""));
    let pairs: Vec<(String, String)> = proxy::query_pairs(&query);
    let headers = request.headers().clone();
    // The browser connection's pending upgrade, taken before the body is
    // consumed; only `/api/term/attach` ever awaits it.
    let upgrade = request
        .extensions_mut()
        .remove::<hyper::upgrade::OnUpgrade>();
    let registry = &state.registry;
    let client = &state.client;
    if (method == Method::GET || method == Method::HEAD) && !path.starts_with("/api/") {
        return Ok(assets::serve_asset(&state.assets, &uri, &method, &headers));
    }
    if method == Method::GET && path == "/api/meta" {
        return ok(
            &json!({"mode": "hub", "protocol": PROTOCOL, "build": state.assets.build,
            "hostname": HUB_HOSTNAME, "capabilities": hub_capabilities()}),
        );
    }
    if method == Method::GET && path == "/api/nodes" {
        return ok(
            &json!({"mode": "hub", "nodes": registry.public(), "machines": registry.machines()}),
        );
    }
    if method == Method::POST && path == "/api/nodes/order" {
        let body = read_body(request).await?;
        return set_order(&state, &body);
    }
    if method == Method::POST
        && let Some(nid) = proxy::display_route(&path).map(str::to_string)
    {
        let body = read_body(request).await?;
        return set_display(&state, &nid, &body);
    }
    let mut explicit: Option<String> = None;
    if let Some((nid, rest)) = proxy::explicit_node(&path) {
        explicit = Some(nid.to_string());
        path = rest.to_string();
    }
    if method == Method::GET && path == "/api/session/file" {
        let accept = headers
            .get(header::ACCEPT)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if let Some(location) = proxy::file_navigation(accept, &query, explicit.as_deref())? {
            return Ok(proxy::file_redirect(&location));
        }
    }
    if method == Method::GET && explicit.is_none() && AGGREGATED.contains(&path.as_str()) {
        return aggregated(&state, &path, &pairs).await;
    }
    let attachment = ATTACHMENT_PATHS.contains(&path.as_str());
    let (body, upload) = if method == Method::POST && !attachment {
        (Some(read_body(request).await?), None)
    } else if method == Method::POST {
        (
            None,
            Some(Upload {
                body: request.into_body(),
            }),
        )
    } else {
        (None, None)
    };
    if explicit.is_none() {
        if path == "/api/sessions/delete" {
            let value = Value::Object(body.unwrap_or_default());
            return ok(&aggregate::delete(registry, client, &value).await?);
        }
        if path == "/api/sessions/fork-visibility" {
            let value = Value::Object(body.unwrap_or_default());
            return ok(&aggregate::fork_visibility(registry, client, &value).await?);
        }
        if path == "/api/trash/purge"
            && body
                .as_ref()
                .and_then(|map| map.get("all"))
                .is_some_and(truthy)
        {
            return ok(&aggregate::purge_all(registry, client, &pairs).await?);
        }
        if path == "/api/audit/browser" {
            return browser_audit(&state, &body.unwrap_or_default()).await;
        }
    }
    let resolved = proxy::resolve(explicit.as_deref(), method.as_str(), &path, query, body)?;
    let Some(node) = registry.get(&resolved.nid) else {
        return status(StatusCode::NOT_FOUND, json!({"error": NOT_REGISTERED}));
    };
    if registry.offline(&node.id) && !registry.recheck(client, &node).await {
        let health = registry.state(&node.id).unwrap_or_default();
        let response = json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({
                "error": format!("{} 离线：{}", node.name,
                    health.error.as_deref().unwrap_or("中央站未能连接该机器")),
                "node_offline": true, "node_id": node.id, "offline_since": health.offline_since,
            }),
        );
        return Err(Reply::Response(Box::new(closing(response))));
    }
    if method == Method::POST
        && BUILD_CHECKED_PATHS.contains(&resolved.path.as_str())
        && resolved
            .body
            .as_ref()
            .and_then(|map| map.get("_build"))
            .and_then(Value::as_str)
            != Some(state.assets.build.as_str())
    {
        return status(
            StatusCode::CONFLICT,
            json!({"error": "页面版本已过期，请重新加载", "reload": true,
                "build": state.assets.build}),
        );
    }
    let target = registry
        .target(&node)
        .map_err(|_| Reply::Invalid(NOT_REGISTERED.into()))?;
    let display_ip = proxy::display_ip(&headers, peer.map(|address| address.ip()));
    let response = proxy::proxy(
        client,
        &target,
        &node,
        state.shutdown.clone(),
        Forward {
            method: method.as_str(),
            path: &resolved.path,
            query: &resolved.query,
            body: resolved.body.as_ref(),
            upload,
            browser: &headers,
            display_ip: &display_ip,
            upgrade,
        },
    )
    .await?;
    Ok(response)
}

/// `HubHandler.aggregate`: the merged read, or the NDJSON search stream.
async fn aggregated(state: &HubState, path: &str, query: &Params) -> Result<Response, Reply> {
    let registry = &state.registry;
    let client = &state.client;
    let value = match path {
        "/api/sessions" => aggregate::sessions(registry, client, query).await?,
        "/api/search" if aggregate::progress_requested(query) => {
            let stream = aggregate::search_stream(registry.clone(), client.clone(), query)?;
            let body = Body::from_stream(
                stream
                    .take_until(state.shutdown.clone().cancelled_owned())
                    .map(|line| Ok::<_, std::io::Error>(axum::body::Bytes::from(line))),
            );
            let mut response = (StatusCode::OK, body).into_response();
            let headers = response.headers_mut();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-ndjson; charset=utf-8"),
            );
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
            headers.insert(header::CONNECTION, HeaderValue::from_static("close"));
            return Ok(response);
        }
        "/api/search" => aggregate::search(registry, client, query).await?,
        "/api/live" => aggregate::live(registry, client, query).await?,
        "/api/term/list" => aggregate::term_list(registry, client, query).await?,
        _ => aggregate::trash(registry, client, query).await?,
    };
    ok(&value)
}

/// 网页只改机器的名称、配色和是否启用；接机器、下机器、地址和凭据仍是服务器端操作。
fn set_display(state: &HubState, nid: &str, body: &Map<String, Value>) -> Result<Response, Reply> {
    let registry = &state.registry;
    let Some(node) = registry.find(nid) else {
        return status(StatusCode::NOT_FOUND, json!({"error": NOT_REGISTERED}));
    };
    let before = json!({"name": node.name, "color": node.color(), "enabled": node.enabled()});
    let flag = match body.get("enabled") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(flag)) => Some(*flag),
        Some(_) => {
            return status(
                StatusCode::BAD_REQUEST,
                json!({"error": "enabled 只能是 true 或 false"}),
            );
        }
    };
    let text = |key: &str| -> Result<Option<String>, Reply> {
        match body.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => Ok(Some(value.clone())),
            Some(_) => Err(Reply::Invalid(format!("{key} 必须是字符串"))),
        }
    };
    let name = text("name")?;
    let color = text("color")?;
    let row = match registry.update_display(nid, name.as_deref(), color.as_deref(), flag) {
        Ok(row) => row,
        Err(RegistryError::NotFound(_)) => {
            return status(StatusCode::NOT_FOUND, json!({"error": NOT_REGISTERED}));
        }
        Err(RegistryError::Invalid(message)) => {
            return status(StatusCode::BAD_REQUEST, json!({"error": message}));
        }
        Err(error) => return Err(Reply::Invalid(error.to_string())),
    };
    let after = json!({"name": row.name, "color": row.color, "enabled": row.enabled});
    if after != before
        && let Some(audit) = &state.audit
    {
        audit.record(
            "hub.node.display.changed",
            "terminal",
            json!({"node_id": nid, "from": before, "to": after}),
        );
    }
    ok(
        &json!({"ok": true, "node": {"id": row.id, "name": row.name, "color": row.color,
        "enabled": row.enabled}}),
    )
}

/// 设置页拖动后的机器顺序：机器筛选、新建会话下拉和设置页都按它排。
fn set_order(state: &HubState, body: &Map<String, Value>) -> Result<Response, Reply> {
    let registry = &state.registry;
    let before: Vec<String> = registry
        .machines()
        .iter()
        .filter_map(|row| row["id"].as_str().map(str::to_string))
        .collect();
    let after = match registry.reorder_json(body.get("ids").unwrap_or(&Value::Null)) {
        Ok(after) => after,
        Err(RegistryError::Invalid(message)) => {
            return status(StatusCode::BAD_REQUEST, json!({"error": message}));
        }
        Err(error) => return Err(Reply::Invalid(error.to_string())),
    };
    if after != before
        && let Some(audit) = &state.audit
    {
        audit.record(
            "hub.node.order.changed",
            "terminal",
            json!({"from": before, "to": after}),
        );
    }
    ok(&json!({"ok": true, "machines": registry.machines()}))
}

/// `HubHandler.browser_audit`: split the batch by machine and forward each
/// share with a short timeout; failures are ignored, the page never waits
/// on diagnostics.
async fn browser_audit(state: &HubState, body: &Map<String, Value>) -> Result<Response, Reply> {
    let groups = proxy::group_browser_audit(body, &state.page_nodes);
    let registry = &state.registry;
    let client = &state.client;
    let forwards = groups.iter().filter_map(|group| {
        registry.get(&group.nid).map(|node| async move {
            let _ = registry
                .request(
                    client,
                    &node,
                    "/api/audit/browser",
                    "POST",
                    Some(&group.body),
                    AUDIT_TIMEOUT,
                )
                .await;
        })
    });
    futures_util::future::join_all(forwards).await;
    ok(&json!({"ok": true}))
}
