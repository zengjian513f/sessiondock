//! Hub → node HTTP/1.1 client: one connection per request over a plain or
//! system-validated TLS socket, written by hand.
//!
//! The client is hand-written instead of pulling `hyper-util`'s legacy
//! client (pool, resolver, tower,
//! tracing) into the lock file for a handful of GET/POSTs. What the hand-written
//! client guarantees: the target is always a literal `SocketAddr` taken from a
//! validated registry URL (no DNS, no rebinding); a 3xx is just a non-200
//! status, never followed; `Accept-Encoding: identity` keeps bodies plain;
//! every socket operation carries an idle
//! timeout (connect 5 s; reads 5 s for JSON, 60 s while a search streams,
//! 10/45 s for proxied requests); JSON bodies are capped at `JSON_LIMIT` and an
//! oversize body is an invalid response, never a partial parse.
//!
//! Failures carry a public code and a Chinese reason (`hub.py`
//! `request_failure`) and never the URL, the token or upstream text.

use std::{
    collections::HashMap,
    fmt, io,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant},
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
    net::TcpStream,
    time::timeout,
};

use super::identity::PROTOCOL;

/// Largest JSON body accepted from a node (`hub.py` `JSON_LIMIT`).
pub const JSON_LIMIT: usize = 64 * 1024 * 1024;
/// Default per-operation timeout for registry requests.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// A streaming search may stay silent this long between lines.
pub const SEARCH_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Proxied requests; `/api/watch` SSE heartbeats are 20 s apart.
pub const PROXY_TIMEOUT: Duration = Duration::from_secs(10);
pub const WATCH_TIMEOUT: Duration = Duration::from_secs(45);
/// Idle timeout while a node works through a bulk write (delete, fork
/// visibility, purge): it answers only once every session is handled, and
/// removing OpenCode sessions or moving large files takes seconds each. Kept
/// under the fronting proxy's usual 60 s read timeout.
pub const BULK_WRITE_TIMEOUT: Duration = Duration::from_secs(50);

/// `http.client` allows 64 KiB for each status/header/chunk line and 100 headers.
const LINE_LIMIT: usize = 64 * 1024;
const HEADER_COUNT_LIMIT: usize = 100;
const READ_CHUNK: usize = 64 * 1024;

/// Where a request goes and the credential it carries. `Debug` hides the token.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub addr: SocketAddr,
    pub token: String,
    /// Use TLS with system roots and the literal IP as the certificate name.
    pub tls: bool,
}

impl Target {
    /// The three headers every hub→node request carries (`Registry.headers`).
    pub fn headers(&self) -> [(&'static str, String); 3] {
        [
            ("X-SessionDock-Node-Token", self.token.clone()),
            ("X-SessionDock-Protocol", PROTOCOL.to_string()),
            ("Accept-Encoding", "identity".to_string()),
        ]
    }
}

impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Target({}://{}, token <redacted>)",
            if self.tls { "https" } else { "http" },
            self.addr
        )
    }
}

/// Public failure classes (`request_failure`). Private detail stays in logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    /// No answer within the idle timeout (connect, headers or one read).
    Timeout,
    ConnectionRefused,
    Unreachable,
    /// The peer closed the connection before a complete response.
    ConnectionClosed,
    /// Malformed status line/headers/framing, JSON that does not parse, a body
    /// over the limit, an unexpected event, or a request we cannot encode.
    Invalid(&'static str),
    ConnectionFailed,
}

impl ClientError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::ConnectionRefused => "connection_refused",
            Self::Unreachable => "unreachable",
            Self::ConnectionClosed => "connection_closed",
            Self::Invalid(_) => "invalid_response",
            Self::ConnectionFailed => "connection_failed",
        }
    }

    /// User-facing reason; `timeout` is the idle timeout the request used.
    pub fn message(&self, timeout: Duration) -> String {
        match self {
            Self::Timeout => format!("节点连接或响应超时（等待超过 {} 秒）", seconds(timeout)),
            Self::ConnectionRefused => "节点拒绝连接，目标端口未接受请求".to_string(),
            Self::Unreachable => "节点网络不可达".to_string(),
            Self::ConnectionClosed => "节点连接中断，未收到完整响应".to_string(),
            Self::Invalid(_) => "节点返回无效或不完整的响应".to_string(),
            Self::ConnectionFailed => "无法建立节点连接".to_string(),
        }
    }

    fn from_io(error: &io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Self::Timeout,
            io::ErrorKind::ConnectionRefused => Self::ConnectionRefused,
            io::ErrorKind::HostUnreachable | io::ErrorKind::NetworkUnreachable => Self::Unreachable,
            io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof => Self::ConnectionClosed,
            _ => match error.raw_os_error() {
                // EHOSTUNREACH / ENETUNREACH on Linux, before std maps them.
                Some(113) | Some(101) => Self::Unreachable,
                _ => Self::ConnectionFailed,
            },
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(detail) => write!(f, "{}: {detail}", self.code()),
            other => f.write_str(other.code()),
        }
    }
}

impl std::error::Error for ClientError {}

/// `hub.py request_failure`: a non-200 status wins over the transport error.
pub fn request_failure(
    error: Option<&ClientError>,
    status: Option<u16>,
    timeout: Duration,
) -> (&'static str, String) {
    if let Some(status) = status.filter(|status| *status != 200) {
        let detail = match status {
            401 => "节点认证失败",
            403 => "节点拒绝访问，请检查认证或访问权限",
            404 => "节点接口不存在",
            429 => "节点请求过于频繁",
            503 => "节点服务暂不可用",
            _ => "节点返回错误响应",
        };
        return ("http_error", format!("{detail}（HTTP {status}）"));
    }
    match error {
        Some(error) => (error.code(), error.message(timeout)),
        None => (
            "connection_failed",
            ClientError::ConnectionFailed.message(timeout),
        ),
    }
}

/// `{timeout}`: `5` for an integer, `0.2` for a float.
fn seconds(timeout: Duration) -> String {
    let secs = timeout.as_secs_f64();
    if secs.fract() == 0.0 {
        format!("{}", secs as u64)
    } else {
        format!("{secs}")
    }
}

/// One request to build. `target` is the already-encoded path plus query.
pub struct Request<'a> {
    pub method: &'a str,
    pub target: &'a str,
    /// Extra headers after the three node headers; `Connection: close` is added
    /// unless the caller sets `Connection` (WebSocket upgrades do).
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<&'a [u8]>,
    pub connect: Duration,
    pub idle: Duration,
}

const POOL_MAX_IDLE_PER_TARGET: usize = 4;
const POOL_MAX_TARGETS: usize = 64;
const POOL_IDLE_TTL: Duration = Duration::from_secs(30);

#[derive(Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    addr: SocketAddr,
    tls: bool,
    token_hash: [u8; 32],
}

impl PoolKey {
    fn from_target(target: &Target) -> Self {
        Self {
            addr: target.addr,
            tls: target.tls,
            token_hash: Sha256::digest(target.token.as_bytes()).into(),
        }
    }
}

struct PooledEntry {
    stream: UpstreamStream,
    idle_since: Instant,
}

#[derive(Default)]
struct ConnectionPool {
    buckets: HashMap<PoolKey, Vec<PooledEntry>>,
}

impl fmt::Debug for ConnectionPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionPool")
            .field("targets", &self.buckets.len())
            .finish()
    }
}

impl ConnectionPool {
    fn evict_expired(&mut self, now: Instant) {
        self.buckets.retain(|_, entries| {
            entries.retain(|entry| now.saturating_duration_since(entry.idle_since) < POOL_IDLE_TTL);
            !entries.is_empty()
        });
    }

    fn checkout(&mut self, key: &PoolKey) -> Option<UpstreamStream> {
        let now = Instant::now();
        self.evict_expired(now);
        let mut selected = None;
        if let Some(entries) = self.buckets.get_mut(key) {
            while let Some(mut entry) = entries.pop() {
                if entry.stream.is_reusable_idle() {
                    selected = Some(entry.stream);
                    break;
                }
            }
            if entries.is_empty() {
                self.buckets.remove(key);
            }
        }
        selected
    }

    fn checkin(&mut self, key: PoolKey, mut stream: UpstreamStream) {
        if !stream.is_reusable_idle() {
            return;
        }
        let now = Instant::now();
        self.evict_expired(now);
        if !self.buckets.contains_key(&key)
            && self.buckets.len() >= POOL_MAX_TARGETS
            && let Some(oldest_key) = self
                .buckets
                .iter()
                .filter_map(|(k, v)| v.first().map(|entry| (k.clone(), entry.idle_since)))
                .min_by_key(|(_, since)| *since)
                .map(|(k, _)| k)
        {
            self.buckets.remove(&oldest_key);
        }
        let entries = self.buckets.entry(key).or_default();
        if entries.len() >= POOL_MAX_IDLE_PER_TARGET {
            entries.remove(0);
        }
        entries.push(PooledEntry {
            stream,
            idle_since: now,
        });
    }
}

/// Complete buffered response from a short Hub → node HTTP request.
pub(crate) struct BufferedResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Timeouts every registry request uses; tests shorten them.
#[derive(Clone, Debug)]
pub struct Client {
    /// Connect timeout and the idle timeout of JSON requests.
    pub timeout: Duration,
    /// Idle timeout between search stream lines.
    pub search_idle: Duration,
    /// The inline re-check before refusing an action on an offline node.
    pub recheck: Duration,
    /// Shared admission for JSON decoding and fleet aggregation. The permit
    /// belongs to the actual blocking job, including after HTTP cancellation.
    jobs: Arc<tokio::sync::Semaphore>,
    /// Bounded HTTP/1.1 keep-alive pool strictly for short buffered requests.
    pool: Arc<Mutex<ConnectionPool>>,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            timeout: REQUEST_TIMEOUT,
            search_idle: SEARCH_IDLE_TIMEOUT,
            recheck: RECHECK_TIMEOUT,
            jobs: Arc::new(tokio::sync::Semaphore::new(
                std::thread::available_parallelism()
                    .map_or(4, usize::from)
                    .clamp(2, 8),
            )),
            pool: Arc::new(Mutex::new(ConnectionPool::default())),
        }
    }
}

/// `hub.py RECHECK_TIMEOUT`.
pub const RECHECK_TIMEOUT: Duration = Duration::from_secs(5);

impl Client {
    pub(crate) async fn work<T, F>(&self, work: F) -> Result<T, ClientError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let permit = self
            .jobs
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ClientError::Invalid("hub worker pool closed"))?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let result = work();
            // List decoding, aggregation and UI snapshots repeatedly allocate
            // and drop full JSON projections on different worker arenas. The
            // node already returns those free pages to glibc; the Hub needs the
            // same coalesced release after its CPU jobs. Delay it so the result
            // and replaced baselines can finish leaving their callers too.
            crate::sessions::memory::release_soon();
            result
        })
        .await
        .map_err(|_| ClientError::Invalid("hub worker failed"))
    }

    fn pool_checkout(&self, key: &PoolKey) -> Option<UpstreamStream> {
        self.pool
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .checkout(key)
    }

    fn pool_checkin(&self, key: PoolKey, stream: UpstreamStream) {
        self.pool
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .checkin(key, stream);
    }

    /// Execute one short buffered HTTP/1.1 request up to `limit` body bytes,
    /// reusing an idle pooled connection when safe and never sharing streams
    /// with SSE/NDJSON/tar streams or WebSocket upgrades.
    pub(crate) async fn request(
        &self,
        target: &Target,
        request: Request<'_>,
        limit: usize,
    ) -> Result<BufferedResponse, ClientError> {
        let caller_set_connection = request
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("connection"));
        let caller_wants_close = request.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("connection") && value_has_token(value, "close")
        });
        let use_pool = !caller_wants_close;
        let head = encode_head_mode(target, &request, !caller_set_connection)?;
        let head_only = request.method.eq_ignore_ascii_case("HEAD");
        let idempotent = head_only || request.method.eq_ignore_ascii_case("GET");
        let pool_key = PoolKey::from_target(target);

        let pooled = if use_pool {
            self.pool_checkout(&pool_key)
        } else {
            None
        };

        let response = if let Some(stream) = pooled {
            match send_and_read_response(stream, &head, request.body, request.idle, head_only).await
            {
                Ok(response) => response,
                Err((ClientError::ConnectionClosed, false)) if idempotent => {
                    let fresh = match timeout(request.connect, connect_stream(target)).await {
                        Ok(result) => result?,
                        Err(_) => return Err(ClientError::Timeout),
                    };
                    send_and_read_response(fresh, &head, request.body, request.idle, head_only)
                        .await
                        .map_err(|(error, _)| error)?
                }
                Err((error, _)) => return Err(error),
            }
        } else {
            let fresh = match timeout(request.connect, connect_stream(target)).await {
                Ok(result) => result?,
                Err(_) => return Err(ClientError::Timeout),
            };
            send_and_read_response(fresh, &head, request.body, request.idle, head_only)
                .await
                .map_err(|(error, _)| error)?
        };

        let status = response.status;
        let keep_alive_ok =
            use_pool && response_allows_keep_alive(status, response.http11, &response.headers);
        let (raw, reusable_stream) = response.body.read_to_end_pooled(limit).await?;
        if keep_alive_ok && let Some(stream) = reusable_stream {
            self.pool_checkin(pool_key, stream);
        }
        Ok(BufferedResponse { status, body: raw })
    }

    /// `Registry.request`: one JSON round trip, `timeout` for connect and every
    /// read, body capped at `JSON_LIMIT`. Any status is returned with its
    /// parsed body; the caller decides what a non-200 means.
    pub async fn json(
        &self,
        target: &Target,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<(u16, Value), ClientError> {
        self.json_idle(target, method, path, body, timeout, timeout)
            .await
    }

    /// `json` with a separate idle timeout for the answer.
    pub async fn json_idle(
        &self,
        target: &Target,
        method: &str,
        path: &str,
        body: Option<&Value>,
        connect: Duration,
        idle: Duration,
    ) -> Result<(u16, Value), ClientError> {
        let encoded = body.map(|body| serde_json::to_vec(body).expect("JSON value serializes"));
        let headers: &[(&str, &str)] = if encoded.is_some() {
            &[("Content-Type", "application/json")]
        } else {
            &[]
        };
        let response = self
            .request(
                target,
                Request {
                    method,
                    target: path,
                    headers,
                    body: encoded.as_deref(),
                    connect,
                    idle,
                },
                JSON_LIMIT,
            )
            .await?;
        let status = response.status;
        let raw = response.body;
        let value = self
            .work(move || {
                serde_json::from_slice(&raw).map_err(|_| ClientError::Invalid("body is not JSON"))
            })
            .await??;
        Ok((status, value))
    }

    /// Connect, send the whole request and read the response head.
    pub async fn open(
        &self,
        target: &Target,
        request: Request<'_>,
    ) -> Result<Response, ClientError> {
        let mut pending = self.connect(target, &request).await?;
        if let Some(body) = request.body {
            pending.send(body).await?;
        }
        pending.response().await
    }

    /// Reuse HTTP framing for an already authenticated, locally owned transport.
    /// Hub requests continue to use `open` and system-validated TLS.
    pub(crate) async fn open_on_stream(
        &self,
        target: &Target,
        request: Request<'_>,
        stream: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
    ) -> Result<Response, ClientError> {
        let head = encode_head(target, &request)?;
        let mut pending = Pending {
            stream: UpstreamStream::new(stream),
            idle: request.idle,
            head_only: request.method.eq_ignore_ascii_case("HEAD"),
        };
        pending.send(&head).await?;
        if let Some(body) = request.body {
            pending.send(body).await?;
        }
        pending.response().await
    }

    /// Connect and send the request line and headers only. The caller streams
    /// the body with `Pending::send` (its length must be in `request.headers`)
    /// and then reads `Pending::response`.
    pub async fn connect(
        &self,
        target: &Target,
        request: &Request<'_>,
    ) -> Result<Pending, ClientError> {
        let head = encode_head(target, request)?;
        let stream = match timeout(request.connect, connect_stream(target)).await {
            Ok(result) => result?,
            Err(_) => return Err(ClientError::Timeout),
        };
        let mut pending = Pending {
            stream,
            idle: request.idle,
            head_only: request.method.eq_ignore_ascii_case("HEAD"),
        };
        pending.send(&head).await?;
        Ok(pending)
    }
}

async fn send_and_read_response(
    stream: UpstreamStream,
    head: &[u8],
    body: Option<&[u8]>,
    idle: Duration,
    head_only: bool,
) -> Result<Response, (ClientError, bool)> {
    let mut pending = Pending {
        stream,
        idle,
        head_only,
    };
    pending.send(head).await.map_err(|error| (error, false))?;
    if let Some(body) = body {
        pending.send(body).await.map_err(|error| (error, false))?;
    }
    pending.response_tracked().await
}

/// `TlsConnector::new` uses the platform trust store and verifies both the
/// certificate chain and the literal IP passed to `connect` (the registry never
/// permits DNS). Loading the trust store is the expensive part, so one
/// connector serves the whole process. A failed construction is retried by the
/// next request.
fn tls_connector() -> Result<&'static tokio_native_tls::TlsConnector, ClientError> {
    static CONNECTOR: std::sync::OnceLock<tokio_native_tls::TlsConnector> =
        std::sync::OnceLock::new();
    if let Some(connector) = CONNECTOR.get() {
        return Ok(connector);
    }
    let connector = native_tls::TlsConnector::new().map_err(|_| ClientError::ConnectionFailed)?;
    Ok(CONNECTOR.get_or_init(|| tokio_native_tls::TlsConnector::from(connector)))
}

async fn connect_stream(target: &Target) -> Result<UpstreamStream, ClientError> {
    let stream = TcpStream::connect(target.addr)
        .await
        .map_err(|error| ClientError::from_io(&error))?;
    let _ = stream.set_nodelay(true);
    if !target.tls {
        return Ok(UpstreamStream::plain(stream));
    }
    #[cfg(unix)]
    let raw_fd = std::os::fd::AsRawFd::as_raw_fd(&stream);
    let connector = tls_connector()?;
    let domain = target.addr.ip().to_string();
    let stream = connector
        .connect(&domain, stream)
        .await
        .map_err(|_| ClientError::ConnectionFailed)?;
    #[cfg(unix)]
    return Ok(UpstreamStream::tls(stream, raw_fd));
    #[cfg(not(unix))]
    return Ok(UpstreamStream::tls(stream));
}

fn encode_head(target: &Target, request: &Request<'_>) -> Result<Vec<u8>, ClientError> {
    encode_head_mode(target, request, false)
}

fn encode_head_mode(
    target: &Target,
    request: &Request<'_>,
    keep_alive: bool,
) -> Result<Vec<u8>, ClientError> {
    fn clean(value: &str) -> bool {
        !value
            .bytes()
            .any(|byte| byte == b'\r' || byte == b'\n' || byte == 0)
    }
    if request.method.is_empty()
        || !request
            .method
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic())
        || request.target.is_empty()
        || !request.target.starts_with('/')
        || !request.target.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(ClientError::Invalid("malformed request target"));
    }
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\n",
        request.method, request.target, target.addr
    );
    for (name, value) in target.headers() {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    let mut connection = false;
    let mut length = false;
    for (name, value) in request.headers {
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && byte != b':')
            || !clean(value)
        {
            return Err(ClientError::Invalid("malformed request header"));
        }
        connection |= name.eq_ignore_ascii_case("connection");
        length |= name.eq_ignore_ascii_case("content-length")
            || name.eq_ignore_ascii_case("transfer-encoding");
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if !connection && !keep_alive {
        head.push_str("Connection: close\r\n");
    }
    if !length {
        let expects_body = matches!(
            request.method.to_ascii_uppercase().as_str(),
            "POST" | "PUT" | "PATCH"
        );
        if let Some(body) = request.body {
            head.push_str(&format!("Content-Length: {}\r\n", body.len()));
        } else if expects_body {
            head.push_str("Content-Length: 0\r\n");
        }
    }
    head.push_str("\r\n");
    Ok(head.into_bytes())
}

trait HubIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> HubIo for T {}

enum StreamInner {
    Plain(TcpStream),
    Tls(
        Box<tokio_native_tls::TlsStream<TcpStream>>,
        #[cfg(unix)] std::os::fd::RawFd,
    ),
    Custom(Box<dyn HubIo>),
}

/// Plain TCP or TLS transport retained after a WebSocket upgrade or keep-alive reuse.
pub struct UpstreamStream {
    inner: StreamInner,
}

impl UpstreamStream {
    fn new(stream: impl HubIo + 'static) -> Self {
        Self {
            inner: StreamInner::Custom(Box::new(stream)),
        }
    }

    fn plain(stream: TcpStream) -> Self {
        Self {
            inner: StreamInner::Plain(stream),
        }
    }

    #[cfg(unix)]
    fn tls(stream: tokio_native_tls::TlsStream<TcpStream>, raw_fd: std::os::fd::RawFd) -> Self {
        Self {
            inner: StreamInner::Tls(Box::new(stream), raw_fd),
        }
    }

    #[cfg(not(unix))]
    fn tls(stream: tokio_native_tls::TlsStream<TcpStream>) -> Self {
        Self {
            inner: StreamInner::Tls(Box::new(stream)),
        }
    }

    fn is_reusable_idle(&mut self) -> bool {
        #[cfg(unix)]
        match &mut self.inner {
            StreamInner::Plain(tcp) => {
                use std::os::fd::AsRawFd;
                let mut byte = 0_u8;
                let rc = unsafe {
                    libc::recv(
                        tcp.as_raw_fd(),
                        (&mut byte as *mut u8).cast(),
                        1,
                        libc::MSG_PEEK | libc::MSG_DONTWAIT,
                    )
                };
                if rc != -1 {
                    return false;
                }
                let error = io::Error::last_os_error();
                error.kind() == io::ErrorKind::WouldBlock
                    || matches!(error.raw_os_error(), Some(libc::EAGAIN))
            }
            StreamInner::Tls(tls, raw_fd) => {
                let mut byte = 0_u8;
                let rc = unsafe {
                    libc::recv(
                        *raw_fd,
                        (&mut byte as *mut u8).cast(),
                        1,
                        libc::MSG_PEEK | libc::MSG_DONTWAIT,
                    )
                };
                if rc == 0 {
                    return false;
                }
                if rc < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::WouldBlock
                        && !matches!(error.raw_os_error(), Some(libc::EAGAIN | libc::EINTR))
                    {
                        return false;
                    }
                    return true;
                }
                let mut scratch = [0_u8; 1];
                let mut read_buf = ReadBuf::new(&mut scratch);
                let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());
                matches!(
                    Pin::new(tls.as_mut()).poll_read(&mut cx, &mut read_buf),
                    Poll::Pending
                )
            }
            StreamInner::Custom(_) => false,
        }
        #[cfg(not(unix))]
        match &mut self.inner {
            StreamInner::Plain(_) | StreamInner::Tls(_) => {
                let mut scratch = [0_u8; 1];
                let mut read_buf = ReadBuf::new(&mut scratch);
                let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());
                matches!(
                    Pin::new(self).poll_read(&mut cx, &mut read_buf),
                    Poll::Pending
                )
            }
            StreamInner::Custom(_) => false,
        }
    }
}

impl AsyncRead for UpstreamStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut self.inner {
            StreamInner::Plain(tcp) => Pin::new(tcp).poll_read(cx, buffer),
            #[cfg(unix)]
            StreamInner::Tls(tls, _) => Pin::new(tls.as_mut()).poll_read(cx, buffer),
            #[cfg(not(unix))]
            StreamInner::Tls(tls) => Pin::new(tls.as_mut()).poll_read(cx, buffer),
            StreamInner::Custom(custom) => Pin::new(custom.as_mut()).poll_read(cx, buffer),
        }
    }
}

impl AsyncWrite for UpstreamStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        match &mut self.inner {
            StreamInner::Plain(tcp) => Pin::new(tcp).poll_write(cx, buffer),
            #[cfg(unix)]
            StreamInner::Tls(tls, _) => Pin::new(tls.as_mut()).poll_write(cx, buffer),
            #[cfg(not(unix))]
            StreamInner::Tls(tls) => Pin::new(tls.as_mut()).poll_write(cx, buffer),
            StreamInner::Custom(custom) => Pin::new(custom.as_mut()).poll_write(cx, buffer),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match &mut self.inner {
            StreamInner::Plain(tcp) => Pin::new(tcp).poll_flush(cx),
            #[cfg(unix)]
            StreamInner::Tls(tls, _) => Pin::new(tls.as_mut()).poll_flush(cx),
            #[cfg(not(unix))]
            StreamInner::Tls(tls) => Pin::new(tls.as_mut()).poll_flush(cx),
            StreamInner::Custom(custom) => Pin::new(custom.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        match &mut self.inner {
            StreamInner::Plain(tcp) => Pin::new(tcp).poll_shutdown(cx),
            #[cfg(unix)]
            StreamInner::Tls(tls, _) => Pin::new(tls.as_mut()).poll_shutdown(cx),
            #[cfg(not(unix))]
            StreamInner::Tls(tls) => Pin::new(tls.as_mut()).poll_shutdown(cx),
            StreamInner::Custom(custom) => Pin::new(custom.as_mut()).poll_shutdown(cx),
        }
    }
}

/// A connection whose request head is on the wire.
pub struct Pending {
    stream: UpstreamStream,
    idle: Duration,
    head_only: bool,
}

impl Pending {
    pub async fn send(&mut self, bytes: &[u8]) -> Result<(), ClientError> {
        match timeout(self.idle, self.stream.write_all(bytes)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(ClientError::from_io(&error)),
            Err(_) => Err(ClientError::Timeout),
        }
    }

    /// Read the status line and headers; `100 Continue` is skipped like
    /// `http.client`. Anything else, including 101, is the response.
    pub async fn response(self) -> Result<Response, ClientError> {
        self.response_tracked().await.map_err(|(error, _)| error)
    }

    async fn response_tracked(self) -> Result<Response, (ClientError, bool)> {
        let mut body = Body {
            stream: self.stream,
            buffer: Vec::new(),
            pos: 0,
            pending: Vec::new(),
            framing: Framing::UntilClose,
            idle: self.idle,
            closed: false,
            received_any: false,
        };
        loop {
            let head = match body.read_head().await {
                Ok(head) => head,
                Err(error) => return Err((error, body.received_any)),
            };
            let (http11, status, headers) =
                parse_head(&head).map_err(|error| (error, body.received_any))?;
            if status == 100 {
                continue;
            }
            body.framing =
                framing(status, &headers, self.head_only).map_err(|error| (error, true))?;
            return Ok(Response {
                status,
                http11,
                headers,
                body,
            });
        }
    }
}

type ParsedHead = (bool, u16, Vec<(String, String)>);

fn parse_head(head: &[u8]) -> Result<ParsedHead, ClientError> {
    let text = std::str::from_utf8(head).map_err(|_| ClientError::Invalid("non-UTF-8 head"))?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    let status = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/1.") || status.len() != 3 {
        return Err(ClientError::Invalid("bad status line"));
    }
    let http11 = version == "HTTP/1.1";
    let status: u16 = status
        .parse()
        .map_err(|_| ClientError::Invalid("bad status line"))?;
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or(ClientError::Invalid("bad header line"))?;
        if name.is_empty() || name.starts_with([' ', '\t']) {
            return Err(ClientError::Invalid("bad header line"));
        }
        headers.push((name.to_string(), value.trim().to_string()));
        if headers.len() > HEADER_COUNT_LIMIT {
            return Err(ClientError::Invalid("too many headers"));
        }
    }
    Ok((http11, status, headers))
}

fn value_has_token(value: &str, expected: &str) -> bool {
    value
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case(expected))
}

fn headers_have_token(headers: &[(String, String)], name: &str, token: &str) -> bool {
    headers
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case(name))
        .any(|(_, value)| value_has_token(value, token))
}

fn response_allows_keep_alive(status: u16, http11: bool, headers: &[(String, String)]) -> bool {
    if (100..200).contains(&status) {
        return false;
    }
    if headers_have_token(headers, "connection", "close") {
        return false;
    }
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("transfer-encoding")
            && !value.trim().eq_ignore_ascii_case("chunked")
        {
            return false;
        }
    }
    http11 || headers_have_token(headers, "connection", "keep-alive")
}

/// `http.client` rules: no body for 1xx/204/304/HEAD, else chunked, else
/// `Content-Length` (all copies equal), else until the peer closes.
fn framing(
    status: u16,
    headers: &[(String, String)],
    head_only: bool,
) -> Result<Framing, ClientError> {
    if head_only || status == 204 || status == 304 || (100..200).contains(&status) {
        return Ok(Framing::Empty);
    }
    if let Some(encoding) = header(headers, "transfer-encoding")
        && encoding.eq_ignore_ascii_case("chunked")
    {
        return Ok(Framing::Chunked(Chunk::Size));
    }
    let mut length = None;
    for (name, value) in headers {
        if !name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        for piece in value.split(',') {
            let parsed: u64 = match piece.trim().parse() {
                Ok(parsed) => parsed,
                Err(_) => return Ok(Framing::UntilClose),
            };
            match length {
                None => length = Some(parsed),
                Some(seen) if seen == parsed => {}
                Some(_) => return Err(ClientError::Invalid("conflicting Content-Length")),
            }
        }
    }
    Ok(match length {
        Some(length) => Framing::Length(length),
        None => Framing::UntilClose,
    })
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

pub struct Response {
    pub status: u16,
    http11: bool,
    headers: Vec<(String, String)>,
    body: Body,
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish_non_exhaustive()
    }
}

impl Response {
    /// First header with this name, ASCII case-insensitive.
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    pub fn body(&mut self) -> &mut Body {
        &mut self.body
    }

    pub fn into_body(self) -> Body {
        self.body
    }
}

enum Framing {
    Empty,
    Length(u64),
    Chunked(Chunk),
    UntilClose,
}

enum Chunk {
    Size,
    Data(u64),
    DataEnd,
    Trailers,
    Done,
}

/// Decoded response body. Every socket read waits at most `idle`.
pub struct Body {
    stream: UpstreamStream,
    /// Raw bytes from the socket not yet decoded (`pos` consumed).
    buffer: Vec<u8>,
    pos: usize,
    /// Decoded bytes not yet handed out (`read_line` keeps partial lines here).
    pending: Vec<u8>,
    framing: Framing,
    idle: Duration,
    closed: bool,
    received_any: bool,
}

impl Body {
    pub fn set_idle(&mut self, idle: Duration) {
        self.idle = idle;
    }

    /// Hand back the socket plus raw bytes already read beyond the response
    /// head (for a 101 upgrade; nothing has been decoded from them).
    pub fn into_raw(self) -> (UpstreamStream, Vec<u8>) {
        let mut raw = self.pending;
        raw.extend_from_slice(&self.buffer[self.pos..]);
        (self.stream, raw)
    }

    /// Next decoded piece; `None` once the body is complete.
    pub async fn read(&mut self) -> Result<Option<Vec<u8>>, ClientError> {
        if !self.pending.is_empty() {
            return Ok(Some(std::mem::take(&mut self.pending)));
        }
        loop {
            match self.framing {
                Framing::Empty => return Ok(None),
                Framing::Length(0) => return Ok(None),
                Framing::Length(remaining) => {
                    if self.available() == 0 && !self.fill().await? {
                        return Err(ClientError::ConnectionClosed);
                    }
                    let take = (self.available() as u64).min(remaining) as usize;
                    self.framing = Framing::Length(remaining - take as u64);
                    return Ok(Some(self.take(take)));
                }
                Framing::UntilClose => {
                    if self.available() == 0 && !self.fill().await? {
                        return Ok(None);
                    }
                    let take = self.available();
                    return Ok(Some(self.take(take)));
                }
                Framing::Chunked(Chunk::Done) => return Ok(None),
                Framing::Chunked(Chunk::Size) => {
                    let line = self.raw_line().await?;
                    let text = std::str::from_utf8(&line)
                        .map_err(|_| ClientError::Invalid("bad chunk size"))?;
                    let digits = text.split(';').next().unwrap_or("").trim();
                    let size = u64::from_str_radix(digits, 16)
                        .map_err(|_| ClientError::Invalid("bad chunk size"))?;
                    self.framing = Framing::Chunked(if size == 0 {
                        Chunk::Trailers
                    } else {
                        Chunk::Data(size)
                    });
                }
                Framing::Chunked(Chunk::Data(remaining)) => {
                    if self.available() == 0 && !self.fill().await? {
                        return Err(ClientError::ConnectionClosed);
                    }
                    let take = (self.available() as u64).min(remaining) as usize;
                    let left = remaining - take as u64;
                    self.framing = Framing::Chunked(if left == 0 {
                        Chunk::DataEnd
                    } else {
                        Chunk::Data(left)
                    });
                    return Ok(Some(self.take(take)));
                }
                Framing::Chunked(Chunk::DataEnd) => {
                    let line = self.raw_line().await?;
                    if !line.is_empty() {
                        return Err(ClientError::Invalid("bad chunk end"));
                    }
                    self.framing = Framing::Chunked(Chunk::Size);
                }
                Framing::Chunked(Chunk::Trailers) => {
                    let line = self.raw_line().await?;
                    if line.is_empty() {
                        self.framing = Framing::Chunked(Chunk::Done);
                    }
                }
            }
        }
    }

    /// One decoded line including its `\n`; `None` at the end of the body. A
    /// line longer than `limit` bytes is an invalid response.
    pub async fn read_line(&mut self, limit: usize) -> Result<Option<Vec<u8>>, ClientError> {
        let mut line = Vec::new();
        loop {
            if let Some(index) = self.pending.iter().position(|byte| *byte == b'\n') {
                line.extend(self.pending.drain(..=index));
                if line.len() > limit {
                    return Err(ClientError::Invalid("节点响应过大"));
                }
                return Ok(Some(line));
            }
            line.append(&mut self.pending);
            if line.len() > limit {
                return Err(ClientError::Invalid("节点响应过大"));
            }
            match self.read().await? {
                Some(piece) => self.pending = piece,
                None => return Ok((!line.is_empty()).then_some(line)),
            }
        }
    }

    /// The whole body; more than `limit` bytes is an invalid response.
    pub async fn read_to_end(self, limit: usize) -> Result<Vec<u8>, ClientError> {
        let (out, _) = self.read_to_end_pooled(limit).await?;
        Ok(out)
    }

    async fn read_to_end_pooled(
        mut self,
        limit: usize,
    ) -> Result<(Vec<u8>, Option<UpstreamStream>), ClientError> {
        let mut out = match self.framing {
            Framing::Length(len) if len <= limit as u64 => Vec::with_capacity(len as usize),
            _ => Vec::new(),
        };
        while let Some(piece) = self.read().await? {
            out.extend_from_slice(&piece);
            if out.len() > limit {
                return Err(ClientError::Invalid("节点响应过大"));
            }
        }
        let clean = !self.closed
            && self.pending.is_empty()
            && self.available() == 0
            && matches!(
                self.framing,
                Framing::Empty | Framing::Length(0) | Framing::Chunked(Chunk::Done)
            );
        Ok((out, clean.then_some(self.stream)))
    }

    async fn read_head(&mut self) -> Result<Vec<u8>, ClientError> {
        let mut head = self.raw_line().await?;
        let mut count = 0;
        loop {
            let line = self.raw_line().await?;
            if line.is_empty() {
                return Ok(head);
            }
            count += 1;
            if count > HEADER_COUNT_LIMIT {
                return Err(ClientError::Invalid("too many headers"));
            }
            head.extend_from_slice(b"\r\n");
            head.extend_from_slice(&line);
        }
    }

    /// One raw (undecoded) line without its CRLF, for chunk framing.
    async fn raw_line(&mut self) -> Result<Vec<u8>, ClientError> {
        loop {
            let available = self.available();
            if let Some(index) = self.buffer[self.pos..]
                .iter()
                .position(|byte| *byte == b'\n')
            {
                if index + 1 > LINE_LIMIT {
                    return Err(ClientError::Invalid("response line too long"));
                }
                let mut line = self.take(index);
                self.skip(1);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(line);
            }
            if available > LINE_LIMIT {
                return Err(ClientError::Invalid("response line too long"));
            }
            if !self.fill().await? {
                return Err(ClientError::ConnectionClosed);
            }
        }
    }

    fn skip(&mut self, count: usize) {
        self.pos += count;
        if self.pos == self.buffer.len() {
            self.buffer.clear();
            self.pos = 0;
        }
    }

    fn available(&self) -> usize {
        self.buffer.len() - self.pos
    }

    fn take(&mut self, count: usize) -> Vec<u8> {
        let out = self.buffer[self.pos..self.pos + count].to_vec();
        self.pos += count;
        if self.pos == self.buffer.len() {
            self.buffer.clear();
            self.pos = 0;
        }
        out
    }

    /// Read more raw bytes; `false` at EOF.
    async fn fill(&mut self) -> Result<bool, ClientError> {
        if self.closed {
            return Ok(false);
        }
        if self.pos > 0 && self.pos == self.buffer.len() {
            self.buffer.clear();
            self.pos = 0;
        }
        // Read straight into the heap buffer: a stack scratch array would be
        // embedded in every future up the chain.
        let start = self.buffer.len();
        self.buffer.resize(start + READ_CHUNK, 0);
        let outcome = timeout(self.idle, self.stream.read(&mut self.buffer[start..])).await;
        let count = match outcome {
            Ok(Ok(count)) => count,
            Ok(Err(error)) => {
                self.buffer.truncate(start);
                return Err(ClientError::from_io(&error));
            }
            Err(_) => {
                self.buffer.truncate(start);
                return Err(ClientError::Timeout);
            }
        };
        self.buffer.truncate(start + count);
        if count == 0 {
            self.closed = true;
        } else {
            self.received_any = true;
        }
        Ok(count > 0)
    }
}
