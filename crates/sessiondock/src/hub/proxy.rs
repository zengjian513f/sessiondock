//! The hub's pass-through to one node (`hub.py` `HubHandler.resolve`
//! 760–809, `_remember_page_node`/`browser_audit` 917–949 and `proxy`
//! 951–1076). `resolve` finds the single machine a browser request addresses
//! (path, query and body references are unscoped on the way in), `proxy`
//! forwards it with the node headers and rewrites what comes back: JSON
//! through the wire namespace, `text/event-stream` line by line, a WebSocket
//! upgrade as a raw bidirectional copy after the 101, everything else (media,
//! files) streamed with its framing headers preserved. Attachment uploads are
//! streamed upstream in chunks up to `ATTACHMENT_MAX_BYTES`.
//!
//! Nothing here binds a listener: `api::hub` decides what is aggregated, what
//! is answered locally and what reaches `resolve`/`proxy`.

#[cfg(test)]
mod tests;

use std::{
    fmt,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use indexmap::IndexMap;
use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::{
    client::{Client, ClientError, JSON_LIMIT, PROXY_TIMEOUT, Request, Target, WATCH_TIMEOUT},
    identity::is_node_id,
    namespace::{self, public_payload, qualify, split, truthy},
    registry::{Node, encode_query},
};

/// `server.py ATTACHMENT_MAX_BYTES`: the largest upload the hub forwards.
pub const ATTACHMENT_MAX_BYTES: u64 = 512 * 1024 * 1024;
/// `bug_report.BUG_REPORT_UPLOAD_UID`: an upload with no session yet names
/// its machine with `?node=` instead of a scoped uid.
pub const BUG_REPORT_UPLOAD_UID: &str = "bug-report";
/// Raw-body uploads the hub streams instead of parsing as JSON.
pub const ATTACHMENT_PATHS: [&str; 3] = [
    "/api/session/attachment",
    "/api/session/files/upload",
    "/api/session/conversation/attachment",
];
/// Writes that must carry the page's build (`_build`) or answer 409.
pub const BUILD_CHECKED_PATHS: [&str; 7] = [
    "/api/session/conversation/send",
    "/api/session/conversation/check",
    "/api/session/conversation/restart",
    "/api/session/send",
    "/api/session/outbox/retry",
    "/api/term/send",
    "/api/term/create",
];
/// Browser headers passed through to the node; `X-Real-IP` is added.
/// `User-Agent` only feeds the terminal ownership device label.
pub const FORWARDED_HEADERS: [&str; 6] = [
    "Content-Type",
    "X-SessionDock-Page",
    "X-SessionDock-Trace",
    "X-SessionDock-Build",
    "Range",
    "User-Agent",
];
/// Node headers a streamed (non-JSON, non-SSE) answer keeps.
pub const STREAMED_HEADERS: [&str; 7] = [
    "Content-Length",
    "Content-Disposition",
    "X-Content-Type-Options",
    "Cache-Control",
    "Content-Security-Policy",
    "Content-Range",
    "Accept-Ranges",
];
/// `_PAGE_NODES_MAX`: page ids remembered for audit routing.
pub const PAGE_NODES_MAX: usize = 512;
/// An SSE line longer than this is not a native event stream.
const SSE_LINE_LIMIT: usize = JSON_LIMIT;
/// SSE reads wait this long (`conn.sock.settimeout(45)`), whatever the path.
const SSE_IDLE: Duration = Duration::from_secs(45);
const UPLOAD_CHUNK: usize = 64 * 1024;

pub const ONE_MACHINE: &str = "操作必须明确指定同一台机器";
pub const FOREIGN_ATTACHMENT: &str = "附件来自另一台机器";
pub const UPSTREAM_FAILED: &str = "机器连接中断；写操作可能已执行，请核对目标机器状态";
pub const NODE_RESPONSE_TOO_LARGE: &str = "节点响应过大";

/// JSON APIs never return HTML; a node's reverse-proxy error page is 502.
fn reject_html_upstream(content_type: &str) -> Result<(), ProxyError> {
    if content_type.contains("text/html") {
        Err(ProxyError::Upstream)
    } else {
        Ok(())
    }
}

/// Query in `parse_qs` shape: values grouped per key in first-appearance
/// order, blank values kept.
pub type Query = IndexMap<String, Vec<String>>;

/// `parse_qs(raw, keep_blank_values=True)`.
pub fn parse_qs(raw: &str) -> Query {
    let mut query = Query::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        query
            .entry(unquote_plus(name))
            .or_default()
            .push(unquote_plus(value));
    }
    query
}

/// The wire pairs of a `Query` (`doseq=True` order).
pub fn query_pairs(query: &Query) -> Vec<(String, String)> {
    query
        .iter()
        .flat_map(|(key, values)| values.iter().map(move |value| (key.clone(), value.clone())))
        .collect()
}

/// A `Query` from wire pairs (the aggregate layer's `Params`).
pub fn query_from_pairs(pairs: &[(String, String)]) -> Query {
    let mut query = Query::new();
    for (key, value) in pairs {
        query.entry(key.clone()).or_default().push(value.clone());
    }
    query
}

/// `urlencode(query, doseq=True)`.
pub fn encode_qs(query: &Query) -> String {
    encode_query(&query_pairs(query))
}

fn first<'a>(query: &'a Query, key: &str) -> Option<&'a str> {
    query
        .get(key)
        .and_then(|values| values.first())
        .map(String::as_str)
}

/// `urllib.parse.unquote`: percent-decoding, invalid UTF-8 replaced.
pub fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).expect("ASCII hex");
            out.push(u8::from_str_radix(hex, 16).expect("ASCII hex"));
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `parse_qs` value decoding: `+` is a space, then percent-decoding.
fn unquote_plus(value: &str) -> String {
    unquote(&value.replace('+', " "))
}

/// `urllib.parse.quote(value, safe)`: unreserved bytes and `safe` kept.
pub fn quote(value: &str, safe: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'.' | b'-' | b'~')
            || safe.as_bytes().contains(&byte);
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Why a request could not be forwarded. `Invalid` is
/// `ValueError`/`TypeError`/`KeyError` (400 with the text); `Upstream` is an
/// `OSError`/`HTTPException` before the answer started (502).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProxyError {
    Invalid(String),
    Upstream,
}

impl ProxyError {
    fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

impl fmt::Display for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::Upstream => f.write_str(UPSTREAM_FAILED),
        }
    }
}

impl std::error::Error for ProxyError {}

impl From<namespace::NamespaceError> for ProxyError {
    fn from(error: namespace::NamespaceError) -> Self {
        Self::Invalid(error.message().to_string())
    }
}

/// Transport failures are 502; a node body that is
/// too large is the `ValueError("节点响应过大")` → 400.
impl From<ClientError> for ProxyError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Invalid(NODE_RESPONSE_TOO_LARGE) => {
                Self::Invalid(NODE_RESPONSE_TOO_LARGE.to_string())
            }
            _ => Self::Upstream,
        }
    }
}

/// What `resolve` produced: the machine plus the request as the node sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub nid: String,
    pub path: String,
    pub query: Query,
    pub body: Option<Map<String, Value>>,
}

/// The explicit-node route: `/api/nodes/<32 hex>(/api/...)`.
pub fn explicit_node(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/api/nodes/")?;
    let (nid, tail) = (rest.get(..32)?, rest.get(32..)?);
    if !is_node_id(nid) || !tail.starts_with("/api/") {
        return None;
    }
    Some((nid, tail))
}

/// `/api/nodes/<32 hex>/display`.
pub fn display_route(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/api/nodes/")?;
    let nid = rest.strip_suffix("/display")?;
    is_node_id(nid).then_some(nid)
}

/// `HubHandler.resolve`: exactly one machine from the explicit route, the
/// path (`/api/messages/<global>`, `DELETE /api/session/<global>`), the
/// query (`uid`, `name` on terminal routes, `node`) and the body (`uid`,
/// `name`, `_node`, `terminal_name`, `id`, `media[].src`); every scoped
/// reference is replaced by its local form. Several machines, or none, is
/// `操作必须明确指定同一台机器`.
pub fn resolve(
    explicit: Option<&str>,
    method: &str,
    path: &str,
    mut query: Query,
    body: Option<Map<String, Value>>,
) -> Result<Resolved, ProxyError> {
    let mut candidates: IndexMap<String, ()> = IndexMap::new();
    if let Some(nid) = explicit {
        candidates.insert(nid.to_string(), ());
    }
    let mut path = path.to_string();
    let mut body = body;
    let mut decode = |value: &str, uid: bool| -> Result<String, ProxyError> {
        let (nid, local) = split(value, uid)?;
        candidates.insert(nid, ());
        Ok(local)
    };
    // Explicit routes carry local references; aggregate routes carry scoped ones.
    if explicit.is_none() {
        for prefix in ["/api/messages/", "/api/session/"] {
            if let Some(rest) = path.strip_prefix(prefix)
                && (prefix == "/api/messages/" || method == "DELETE")
            {
                // The Rust node has `/page` and `/media-page` under a
                // message uid; a uid never contains `/`.
                let (head, tail) = match rest.split_once('/') {
                    Some((head, tail)) => (head.to_string(), format!("/{tail}")),
                    None => (rest.to_string(), String::new()),
                };
                let local = decode(&unquote(&head), true)?;
                path = format!("{prefix}{}{tail}", quote(&local, ":"));
            }
        }
        for key in ["uid", "name"] {
            // Bug-report uploads have no session yet; the browser names the
            // machine with ?node= instead of a scoped uid.
            if key == "uid"
                && path == "/api/session/attachment"
                && query.get("uid").map(Vec::as_slice) == Some(&[BUG_REPORT_UPLOAD_UID.to_string()])
            {
                continue;
            }
            let present = query.get(key).is_some_and(|values| !values.is_empty());
            if present && (key == "uid" || path.starts_with("/api/term/")) {
                let value = query[key][0].clone();
                let local = decode(&value, key == "uid")?;
                query[key] = vec![local];
            }
        }
        if let Some(map) = body.as_mut().filter(|map| !map.is_empty()) {
            for key in ["uid", "name", "draft_uid"] {
                let scoped = map
                    .get(key)
                    .filter(|value| truthy(value))
                    .map(reference_text);
                if let Some(value) = scoped
                    && (key == "uid"
                        || key == "draft_uid"
                        || path.starts_with("/api/term/")
                        || matches!(
                            path.as_str(),
                            "/api/session/draft-status"
                                | "/api/session/rewind"
                                | "/api/session/send"
                                | "/api/session/conversation/send"
                                | "/api/session/conversation/check"
                        ))
                {
                    let local = decode(&value, key == "uid" || key == "draft_uid")?;
                    map.insert(key.to_string(), Value::String(local));
                }
            }
            if matches!(path.as_str(), "/api/bug-report" | "/api/bug-report/capture")
                && let Some(value) = map
                    .get("terminal_name")
                    .filter(|value| truthy(value))
                    .map(reference_text)
            {
                let local = decode(&value, false)?;
                map.insert("terminal_name".to_string(), Value::String(local));
            }
            if path.starts_with("/api/trash/")
                && let Some(value) = map
                    .get("id")
                    .filter(|value| truthy(value))
                    .map(reference_text)
            {
                let local = decode(&value, false)?;
                map.insert("id".to_string(), Value::String(local));
            }
        }
    }
    // `(body or {}).pop("_node", None) or query.pop("node", [None])[0]`:
    // the query key is only consumed when the body names no machine.
    let from_body = body
        .as_mut()
        .filter(|map| !map.is_empty())
        .and_then(|map| map.shift_remove("_node"))
        .filter(truthy)
        .map(|value| reference_text(&value));
    let nid = match from_body {
        Some(nid) => Some(nid),
        None => query
            .shift_remove("node")
            .and_then(|values| values.into_iter().next())
            .filter(|value| !value.is_empty()),
    };
    if let Some(nid) = nid {
        candidates.insert(nid, ());
    }
    if candidates.len() != 1 {
        return Err(ProxyError::invalid(ONE_MACHINE));
    }
    let nid = candidates.keys().next().cloned().expect("one candidate");
    if let Some(map) = body.as_mut().filter(|map| !map.is_empty())
        && let Some(Value::Array(items)) = map.get("media").cloned()
    {
        let mut clean = Vec::with_capacity(items.len());
        for item in items {
            let Value::Object(mut item) = item else {
                return Err(ProxyError::invalid("附件必须是对象"));
            };
            let src = match item.get("src") {
                None => String::new(),
                Some(Value::String(src)) => src.clone(),
                Some(_) => return Err(ProxyError::invalid("附件 src 必须是字符串")),
            };
            if let Some((owner, local)) = scoped_media(&src) {
                if owner != nid {
                    return Err(ProxyError::invalid(FOREIGN_ATTACHMENT));
                }
                item.insert("src".to_string(), Value::String(local.to_string()));
            }
            clean.push(Value::Object(item));
        }
        map.insert("media".to_string(), Value::Array(clean));
    }
    query.shift_remove("nodes");
    Ok(Resolved {
        nid,
        path,
        query,
        body,
    })
}

/// `/api/nodes/<nid>/api/media/<token>` → `(nid, /api/media/<token>)`.
fn scoped_media(src: &str) -> Option<(&str, &str)> {
    let rest = src.strip_prefix("/api/nodes/")?;
    let (nid, local) = (rest.get(..32)?, rest.get(32..)?);
    let token = local.strip_prefix("/api/media/")?;
    (is_node_id(nid) && is_node_id(token)).then_some((nid, local))
}

/// `str(value)` for a reference field; real clients only send strings.
fn reference_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `_file_navigation`: a browser navigating to `/api/session/file` is sent
/// to the reader page instead of raw bytes. Returns the `Location`.
pub fn file_navigation(
    accept: &str,
    query: &Query,
    node: Option<&str>,
) -> Result<Option<String>, ProxyError> {
    if !accept.contains("text/html")
        || first(query, "raw") == Some("1")
        || first(query, "download") == Some("1")
        || first(query, "mode").is_some_and(|mode| !mode.is_empty())
    {
        return Ok(None);
    }
    let mut query = query.clone();
    if let Some(node) = node
        && let Some(uid) = first(&query, "uid").map(str::to_string)
        && !uid.is_empty()
    {
        query.insert("uid".to_string(), vec![qualify(node, &uid, true)?]);
    }
    let up = "../".repeat(if node.is_some() { 5 } else { 2 });
    Ok(Some(format!("{up}file.html?{}", encode_qs(&query))))
}

/// The 303 for `file_navigation`.
pub fn file_redirect(location: &str) -> Response {
    let mut response = (StatusCode::SEE_OTHER, Body::empty()).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(location).unwrap_or(HeaderValue::from_static("file.html")),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::VARY, HeaderValue::from_static("Accept"));
    response
}

/// page_id → 最近一次成功从 uid 解析出的节点；满了淘汰最老的条目。
#[derive(Default)]
pub struct PageNodes {
    inner: Mutex<IndexMap<String, String>>,
}

impl PageNodes {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn remember(&self, page_id: &str, nid: &str) {
        if page_id.is_empty() {
            return;
        }
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.shift_remove(page_id);
        inner.insert(page_id.to_string(), nid.to_string());
        while inner.len() > PAGE_NODES_MAX {
            inner.shift_remove_index(0);
        }
    }

    pub fn get(&self, page_id: &str) -> Option<String> {
        if page_id.is_empty() {
            return None;
        }
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(page_id)
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One node's share of a browser audit batch.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditGroup {
    pub nid: String,
    pub body: Value,
}

/// `browser_audit` grouping: every event goes to the machine of its scoped
/// uid (the batch uid when the event has none); an event with a pending or
/// unscoped uid follows the page's last resolved machine, with an empty uid.
pub fn group_browser_audit(body: &Map<String, Value>, page_nodes: &PageNodes) -> Vec<AuditGroup> {
    let page_id = body
        .get("page_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let batch_uid = body.get("uid").and_then(Value::as_str).unwrap_or_default();
    let mut groups: IndexMap<String, Vec<Value>> = IndexMap::new();
    for event in body
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Value::Object(event) = event else {
            continue;
        };
        let scoped = event
            .get("uid")
            .and_then(Value::as_str)
            .filter(|uid| !uid.is_empty())
            .unwrap_or(batch_uid);
        let (nid, uid) = match split(scoped, true) {
            Ok((nid, uid)) => {
                page_nodes.remember(page_id, &nid);
                (nid, uid)
            }
            Err(_) => match page_nodes.get(page_id) {
                Some(nid) => (nid, String::new()),
                None => continue,
            },
        };
        let mut event = event.clone();
        event.insert("uid".to_string(), Value::String(uid));
        groups.entry(nid).or_default().push(Value::Object(event));
    }
    groups
        .into_iter()
        .map(|(nid, events)| {
            let mut forwarded = body.clone();
            forwarded.insert("uid".to_string(), events[0]["uid"].clone());
            forwarded.insert("events".to_string(), Value::Array(events));
            AuditGroup {
                nid,
                body: Value::Object(forwarded),
            }
        })
        .collect()
}

/// `_display_ip`: `X-Real-IP`, else the first `X-Forwarded-For` hop, when it
/// parses as an address; else the TCP peer. Ownership labels only.
pub fn display_ip(headers: &HeaderMap, peer: Option<IpAddr>) -> String {
    let forwarded = headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            headers
                .get("x-forwarded-for")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        });
    if let Some(candidate) = forwarded
        && let Ok(address) = candidate.parse::<IpAddr>()
    {
        return address.to_string();
    }
    match peer {
        Some(IpAddr::V6(v6)) if v6.to_ipv4_mapped().is_some() => {
            v6.to_ipv4_mapped().expect("checked").to_string()
        }
        Some(address) => address.to_string(),
        None => "127.0.0.1".to_string(),
    }
}

/// The request headers a proxied call carries besides the node headers.
pub fn upstream_headers(
    browser: &HeaderMap,
    display_ip: &str,
    websocket: bool,
) -> Vec<(String, String)> {
    let present = |name: &str| {
        browser
            .get(name)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(|value| (name.to_string(), value.to_string()))
    };
    let mut headers: Vec<(String, String)> = FORWARDED_HEADERS
        .iter()
        .filter_map(|name| present(name))
        .collect();
    headers.push(("X-Real-IP".to_string(), display_ip.to_string()));
    if websocket {
        headers.extend(
            [
                "Upgrade",
                "Connection",
                "Sec-WebSocket-Key",
                "Sec-WebSocket-Version",
            ]
            .iter()
            .filter_map(|name| present(name)),
        );
    }
    headers
}

/// One SSE line as the browser sees it: `data:` payloads pass through the
/// wire namespace, everything else (comments, blank lines, ids) is verbatim.
pub fn rewrite_sse_line(line: Vec<u8>, node: &Node, path: &str) -> Result<Vec<u8>, ProxyError> {
    let Some(payload) = line.strip_prefix(b"data: ") else {
        return Ok(line);
    };
    let value: Value =
        serde_json::from_slice(payload).map_err(|error| ProxyError::invalid(error.to_string()))?;
    let mut out = b"data: ".to_vec();
    out.extend(serde_json::to_vec(&public_payload(value, node, path)).expect("JSON serializes"));
    out.push(b'\n');
    Ok(out)
}

/// A JSON answer with the decoded length the page's progress bar reads.
pub fn json_response(status: StatusCode, value: &Value) -> Response {
    let bytes = serde_json::to_vec(value).expect("JSON serializes");
    let length = bytes.len();
    let mut response = (status, Body::from(bytes)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    headers.insert("x-sessiondock-decoded-length", HeaderValue::from(length));
    response
}

/// A raw-body upload the hub streams to the node.
pub struct Upload {
    pub body: Body,
}

/// Everything `proxy` needs about the browser request.
pub struct Forward<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: &'a Query,
    pub body: Option<&'a Map<String, Value>>,
    pub upload: Option<Upload>,
    pub browser: &'a HeaderMap,
    pub display_ip: &'a str,
    /// The browser connection's pending upgrade (`hyper::upgrade::on`), for
    /// `/api/term/attach`; `None` outside a hyper connection.
    pub upgrade: Option<hyper::upgrade::OnUpgrade>,
}

/// `HubHandler.proxy`: forward one request to `target` and translate the
/// answer. Errors before the response starts are the caller's 400/502; once
/// bytes flow, a failure ends the stream.
pub async fn proxy(
    client: &Client,
    target: &Target,
    node: &Node,
    shutdown: CancellationToken,
    forward: Forward<'_>,
) -> Result<Response, ProxyError> {
    let websocket = forward.path == "/api/term/attach";
    let idle = if forward.path == "/api/watch" {
        WATCH_TIMEOUT
    } else {
        PROXY_TIMEOUT
    };
    let mut headers = upstream_headers(forward.browser, forward.display_ip, websocket);
    let encoded = encode_qs(forward.query);
    let request_target = if encoded.is_empty() {
        forward.path.to_string()
    } else {
        format!("{}?{encoded}", forward.path)
    };
    let response = match forward.upload {
        Some(upload) => {
            let size = forward
                .browser
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok());
            let size = match size {
                Some(size)
                    if !forward.browser.contains_key(header::TRANSFER_ENCODING)
                        && size <= ATTACHMENT_MAX_BYTES =>
                {
                    size
                }
                _ => return Err(ProxyError::invalid("invalid attachment size")),
            };
            headers.push(("Content-Length".to_string(), size.to_string()));
            let borrowed: Vec<(&str, &str)> = headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            let request = Request {
                method: forward.method,
                target: &request_target,
                headers: &borrowed,
                body: None,
                connect: idle,
                idle,
            };
            let mut pending = client.connect(target, &request).await?;
            let mut remaining = size;
            let mut stream = upload.body.into_data_stream();
            while remaining > 0 {
                let chunk = match stream.next().await {
                    Some(Ok(chunk)) => chunk,
                    // Interrupted upload → 502.
                    Some(Err(_)) | None => return Err(ProxyError::Upstream),
                };
                if chunk.is_empty() {
                    continue;
                }
                let take = (chunk.len() as u64).min(remaining) as usize;
                for piece in chunk[..take].chunks(UPLOAD_CHUNK) {
                    pending.send(piece).await?;
                }
                remaining -= take as u64;
            }
            pending.response().await?
        }
        None => {
            let payload = forward
                .body
                .map(|body| serde_json::to_vec(body).expect("JSON serializes"));
            let borrowed: Vec<(&str, &str)> = headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            client
                .open(
                    target,
                    Request {
                        method: forward.method,
                        target: &request_target,
                        headers: &borrowed,
                        body: payload.as_deref(),
                        connect: idle,
                        idle,
                    },
                )
                .await?
        }
    };
    let status = StatusCode::from_u16(response.status).map_err(|_| ProxyError::Upstream)?;
    let content_type = response
        .header("Content-Type")
        .unwrap_or("application/octet-stream")
        .to_string();
    if response.status == 101 && websocket {
        let mut reply = (StatusCode::SWITCHING_PROTOCOLS, Body::empty()).into_response();
        for name in [
            "Upgrade",
            "Connection",
            "Sec-WebSocket-Accept",
            "Sec-WebSocket-Protocol",
        ] {
            if let Some(value) = response.header(name)
                && let Ok(value) = HeaderValue::from_str(value)
            {
                reply.headers_mut().insert(
                    HeaderName::from_bytes(name.as_bytes()).expect("static header name"),
                    value,
                );
            }
        }
        let (mut upstream, prefetched) = response.into_body().into_raw();
        if let Some(upgrade) = forward.upgrade {
            // After the 101 no frame has been consumed: hand both sockets to a
            // raw copy, prefetched bytes first, until either side closes.
            tokio::spawn(async move {
                let Ok(upgraded) = upgrade.await else {
                    return;
                };
                let mut browser = hyper_util::rt::TokioIo::new(upgraded);
                if !prefetched.is_empty() && browser.write_all(&prefetched).await.is_err() {
                    return;
                }
                tokio::select! {
                    _ = shutdown.cancelled() => {}
                    _ = tokio::io::copy_bidirectional(&mut browser, &mut upstream) => {}
                }
                let _ = browser.shutdown().await;
                let _ = upstream.shutdown().await;
            });
        }
        return Ok(reply);
    }
    if content_type.contains("application/json") {
        let raw = response.into_body().read_to_end(JSON_LIMIT).await?;
        let value: Value =
            serde_json::from_slice(&raw).map_err(|error| ProxyError::invalid(error.to_string()))?;
        return Ok(json_response(
            status,
            &public_payload(value, node, forward.path),
        ));
    }
    // A node bouncing behind nginx answers HTML 502. Streaming that to the
    // browser made `response.json()` throw `Unexpected token '<'`.
    reject_html_upstream(&content_type)?;
    let mut reply_headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&content_type) {
        reply_headers.insert(header::CONTENT_TYPE, value);
    }
    if content_type.contains("text/event-stream") {
        // Preserve stream ordering and native cursors; only rewrite wire references.
        reply_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        reply_headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
        reply_headers.insert(header::CONNECTION, HeaderValue::from_static("close"));
        let mut body = response.into_body();
        body.set_idle(SSE_IDLE);
        let node = node.clone();
        let path = forward.path.to_string();
        let stream = async_stream::stream! {
            // A read error or an unrewritable data line ends the stream; the
            // loop condition handles Ok(None)/Err, the body handles the rewrite.
            loop {
                let line = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    line = body.read_line(SSE_LINE_LIMIT) => line,
                };
                let Ok(Some(line)) = line else { break };
                match rewrite_sse_line(line, &node, &path) {
                    Ok(line) => yield Ok::<Bytes, std::io::Error>(Bytes::from(line)),
                    Err(_) => break,
                }
            }
        };
        let mut reply = (status, Body::from_stream(stream)).into_response();
        reply.headers_mut().extend(reply_headers);
        return Ok(reply);
    }
    for name in STREAMED_HEADERS {
        if let Some(value) = response.header(name)
            && let Ok(value) = HeaderValue::from_str(value)
        {
            reply_headers.insert(
                HeaderName::from_bytes(name.as_bytes()).expect("static header name"),
                value,
            );
        }
    }
    reply_headers.insert(header::CONNECTION, HeaderValue::from_static("close"));
    let mut body = response.into_body();
    let stream = async_stream::stream! {
        loop {
            let chunk = tokio::select! {
                _ = shutdown.cancelled() => break,
                chunk = body.read() => chunk,
            };
            let Ok(Some(chunk)) = chunk else { break };
            yield Ok::<Bytes, std::io::Error>(Bytes::from(chunk));
        }
    };
    let mut reply = (status, Body::from_stream(stream)).into_response();
    reply.headers_mut().extend(reply_headers);
    Ok(reply)
}
