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

#[cfg(test)]
mod tests;

use std::{
    fmt, io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use serde_json::Value;
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

/// Timeouts every registry request uses; tests shorten them.
#[derive(Clone, Debug)]
pub struct Client {
    /// Connect timeout and the idle timeout of JSON requests.
    pub timeout: Duration,
    /// Idle timeout between search stream lines.
    pub search_idle: Duration,
    /// The inline re-check before refusing an action on an offline node.
    pub recheck: Duration,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            timeout: REQUEST_TIMEOUT,
            search_idle: SEARCH_IDLE_TIMEOUT,
            recheck: RECHECK_TIMEOUT,
        }
    }
}

/// `hub.py RECHECK_TIMEOUT`.
pub const RECHECK_TIMEOUT: Duration = Duration::from_secs(5);

impl Client {
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
        let encoded = body.map(|body| serde_json::to_vec(body).expect("JSON value serializes"));
        let headers: &[(&str, &str)] = if encoded.is_some() {
            &[("Content-Type", "application/json")]
        } else {
            &[]
        };
        let response = self
            .open(
                target,
                Request {
                    method,
                    target: path,
                    headers,
                    body: encoded.as_deref(),
                    connect: timeout,
                    idle: timeout,
                },
            )
            .await?;
        let status = response.status;
        let raw = response.into_body().read_to_end(JSON_LIMIT).await?;
        let value =
            serde_json::from_slice(&raw).map_err(|_| ClientError::Invalid("body is not JSON"))?;
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

async fn connect_stream(target: &Target) -> Result<UpstreamStream, ClientError> {
    let stream = TcpStream::connect(target.addr)
        .await
        .map_err(|error| ClientError::from_io(&error))?;
    let _ = stream.set_nodelay(true);
    if !target.tls {
        return Ok(UpstreamStream::new(stream));
    }
    // `TlsConnector::new` uses the platform trust store and verifies both the
    // certificate chain and this literal IP (the registry never permits DNS).
    let connector = native_tls::TlsConnector::new().map_err(|_| ClientError::ConnectionFailed)?;
    let connector = tokio_native_tls::TlsConnector::from(connector);
    let domain = target.addr.ip().to_string();
    let stream = connector
        .connect(&domain, stream)
        .await
        .map_err(|_| ClientError::ConnectionFailed)?;
    Ok(UpstreamStream::new(stream))
}

fn encode_head(target: &Target, request: &Request<'_>) -> Result<Vec<u8>, ClientError> {
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
    if !connection {
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

/// Plain TCP or TLS transport retained after a WebSocket upgrade.
pub struct UpstreamStream {
    inner: Box<dyn HubIo>,
}

impl UpstreamStream {
    fn new(stream: impl HubIo + 'static) -> Self {
        Self {
            inner: Box::new(stream),
        }
    }
}

impl AsyncRead for UpstreamStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_read(cx, buffer)
    }
}

impl AsyncWrite for UpstreamStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut *self.inner).poll_write(cx, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut *self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut *self.inner).poll_shutdown(cx)
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
        let mut body = Body {
            stream: self.stream,
            buffer: Vec::new(),
            pos: 0,
            pending: Vec::new(),
            framing: Framing::UntilClose,
            idle: self.idle,
            closed: false,
        };
        loop {
            let head = body.read_head().await?;
            let (status, headers) = parse_head(&head)?;
            if status == 100 {
                continue;
            }
            body.framing = framing(status, &headers, self.head_only)?;
            return Ok(Response {
                status,
                headers,
                body,
            });
        }
    }
}

fn parse_head(head: &[u8]) -> Result<(u16, Vec<(String, String)>), ClientError> {
    let text = std::str::from_utf8(head).map_err(|_| ClientError::Invalid("non-UTF-8 head"))?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    let status = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/1.") || status.len() != 3 {
        return Err(ClientError::Invalid("bad status line"));
    }
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
    Ok((status, headers))
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
    pub async fn read_to_end(mut self, limit: usize) -> Result<Vec<u8>, ClientError> {
        let mut out = Vec::new();
        while let Some(piece) = self.read().await? {
            out.extend_from_slice(&piece);
            if out.len() > limit {
                return Err(ClientError::Invalid("节点响应过大"));
            }
        }
        Ok(out)
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
        }
        Ok(count > 0)
    }
}

#[cfg(test)]
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
