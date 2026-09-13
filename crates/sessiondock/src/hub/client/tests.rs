//! Wire-level cases against in-process tokio listeners: framing, limits,
//! timeouts, no redirects. The fake node (`tests/hub_fake_node.py`) covers the
//! node protocol in the integration tests.
use super::*;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{net::TcpListener, task::JoinHandle};

fn target(addr: SocketAddr) -> Target {
    Target {
        addr,
        token: "t".repeat(40),
        tls: false,
    }
}

/// Accept one connection, read the request head (+ `Content-Length` body),
/// answer with `response` and close. Returns what the server received.
async fn serve(response: Vec<u8>) -> (SocketAddr, JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        stream.write_all(&response).await.unwrap();
        stream.shutdown().await.ok();
        request
    });
    (addr, task)
}

async fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut scratch = [0u8; 4096];
    loop {
        let count = stream.read(&mut scratch).await.unwrap();
        if count == 0 {
            break;
        }
        request.extend_from_slice(&scratch[..count]);
        if let Some(end) = find(&request, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
            let length: usize = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            if request.len() >= end + 4 + length {
                break;
            }
        }
    }
    request
}

fn client() -> Client {
    Client {
        timeout: Duration::from_millis(500),
        search_idle: Duration::from_millis(500),
        recheck: Duration::from_millis(300),
    }
}

#[tokio::test]
async fn json_round_trip_sends_node_headers_and_reads_content_length_body() {
    let body = br#"{"mode":"local","protocol":1}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes()
    .into_iter()
    .chain(body.iter().copied())
    .collect();
    let (addr, task) = serve(response).await;
    let (status, value) = client()
        .json(
            &target(addr),
            "GET",
            "/api/meta?",
            None,
            Duration::from_millis(500),
        )
        .await
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(value, json!({"mode": "local", "protocol": 1}));
    let request = String::from_utf8(task.await.unwrap()).unwrap();
    let mut lines = request.split("\r\n");
    assert_eq!(lines.next(), Some("GET /api/meta? HTTP/1.1"));
    let headers: Vec<&str> = lines.take_while(|line| !line.is_empty()).collect();
    assert!(
        headers.contains(&format!("Host: {addr}").as_str()),
        "{headers:?}"
    );
    assert!(headers.contains(&format!("X-SessionDock-Node-Token: {}", "t".repeat(40)).as_str()));
    assert!(headers.contains(&"X-SessionDock-Protocol: 1"));
    assert!(headers.contains(&"Accept-Encoding: identity"));
    assert!(headers.contains(&"Connection: close"));
    assert!(
        !headers
            .iter()
            .any(|line| line.starts_with("Content-Length")),
        "GET has no body"
    );
}

#[tokio::test]
async fn post_json_body_carries_content_type_and_length() {
    let (addr, task) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".to_vec()).await;
    let payload = json!({"uids": ["claude:x"]});
    let (status, value) = client()
        .json(
            &target(addr),
            "POST",
            "/api/sessions/delete",
            Some(&payload),
            Duration::from_millis(500),
        )
        .await
        .unwrap();
    assert_eq!((status, value), (200, json!({})));
    let request = String::from_utf8(task.await.unwrap()).unwrap();
    let (head, body) = request.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("POST /api/sessions/delete HTTP/1.1\r\n"));
    assert!(head.contains("\r\nContent-Type: application/json\r\n"));
    assert!(
        head.ends_with(&format!("\r\nContent-Length: {}", body.len())),
        "{head}"
    );
    assert_eq!(serde_json::from_str::<Value>(body).unwrap(), payload);
}

#[tokio::test]
async fn chunked_and_close_delimited_bodies_are_decoded() {
    let chunked =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\n\r\n\
        5;ext=1\r\n{\"a\":\r\n7\r\n[1,2,3]\r\n1\r\n}\r\n0\r\nTrailer: x\r\n\r\n";
    let (addr, _) = serve(chunked.to_vec()).await;
    let (status, value) = client()
        .json(&target(addr), "GET", "/x", None, Duration::from_millis(500))
        .await
        .unwrap();
    assert_eq!((status, value), (200, json!({"a": [1, 2, 3]})));

    let (addr, _) =
        serve(b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n[true]".to_vec()).await;
    let (status, value) = client()
        .json(&target(addr), "GET", "/x", None, Duration::from_millis(500))
        .await
        .unwrap();
    assert_eq!((status, value), (200, json!([true])));

    let (addr, _) = serve(b"HTTP/1.1 204 No Content\r\nContent-Length: 999\r\n\r\n".to_vec()).await;
    let response = client()
        .open(
            &target(addr),
            Request {
                method: "GET",
                target: "/x",
                headers: &[],
                body: None,
                connect: Duration::from_millis(500),
                idle: Duration::from_millis(500),
            },
        )
        .await
        .unwrap();
    assert_eq!(response.status, 204);
    assert_eq!(
        response.into_body().read_to_end(10).await.unwrap(),
        b"",
        "204 never has a body"
    );
}

#[tokio::test]
async fn response_header_limit_is_per_line_not_aggregate() {
    let first = "a".repeat(40 * 1024);
    let second = "b".repeat(40 * 1024);
    let response = format!(
        "HTTP/1.1 200 OK\r\nX-First: {first}\r\nX-Second: {second}\r\nContent-Length: 2\r\n\r\n{{}}"
    )
    .into_bytes();
    let (addr, _) = serve(response).await;
    let (status, value) = client()
        .json(&target(addr), "GET", "/x", None, Duration::from_millis(500))
        .await
        .unwrap();
    assert_eq!((status, value), (200, json!({})));

    let oversized = format!(
        "HTTP/1.1 200 OK\r\nX-Big: {}\r\nContent-Length: 2\r\n\r\n{{}}",
        "x".repeat(LINE_LIMIT)
    )
    .into_bytes();
    let (addr, _) = serve(oversized).await;
    assert_eq!(
        client()
            .json(&target(addr), "GET", "/x", None, Duration::from_millis(500))
            .await,
        Err(ClientError::Invalid("response line too long"))
    );
}

#[tokio::test]
async fn read_line_streams_ndjson_and_100_continue_is_skipped() {
    let response = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\n\
        Transfer-Encoding: chunked\r\n\r\n10\r\n{\"type\":\"a\"}\n{\"t\r\na\r\nype\":\"b\"}\n\r\n0\r\n\r\n";
    let (addr, _) = serve(response.to_vec()).await;
    let response = client()
        .open(
            &target(addr),
            Request {
                method: "GET",
                target: "/api/search?q=x&progress=1",
                headers: &[],
                body: None,
                connect: Duration::from_millis(500),
                idle: Duration::from_millis(500),
            },
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("content-type"),
        Some("application/x-ndjson")
    );
    let mut body = response.into_body();
    assert_eq!(
        body.read_line(1024).await.unwrap().unwrap(),
        b"{\"type\":\"a\"}\n"
    );
    assert_eq!(
        body.read_line(1024).await.unwrap().unwrap(),
        b"{\"type\":\"b\"}\n"
    );
    assert_eq!(body.read_line(1024).await.unwrap(), None);

    let (addr, _) = serve(b"HTTP/1.0 200 OK\r\n\r\nno newline at all".to_vec()).await;
    let mut body = client()
        .open(
            &target(addr),
            Request {
                method: "GET",
                target: "/x",
                headers: &[],
                body: None,
                connect: Duration::from_millis(500),
                idle: Duration::from_millis(500),
            },
        )
        .await
        .unwrap()
        .into_body();
    assert_eq!(
        body.read_line(4).await,
        Err(ClientError::Invalid("节点响应过大"))
    );
}

#[tokio::test]
async fn oversize_body_is_invalid_not_partial() {
    let big = "x".repeat(2048);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{big}",
        big.len()
    )
    .into_bytes();
    let (addr, _) = serve(response).await;
    let body = client()
        .open(
            &target(addr),
            Request {
                method: "GET",
                target: "/x",
                headers: &[],
                body: None,
                connect: Duration::from_millis(500),
                idle: Duration::from_millis(500),
            },
        )
        .await
        .unwrap()
        .into_body();
    let error = body.read_to_end(1024).await.unwrap_err();
    assert_eq!(error, ClientError::Invalid("节点响应过大"));
    assert_eq!(error.code(), "invalid_response");
    assert_eq!(JSON_LIMIT, 64 * 1024 * 1024);
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let connections = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = connections.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            seen.fetch_add(1, Ordering::SeqCst);
            read_request(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/elsewhere\r\nContent-Length: 12\r\n\r\n{\"moved\":1}\n")
                .await
                .unwrap();
        }
    });
    let (status, value) = client()
        .json(&target(addr), "GET", "/x", None, Duration::from_millis(500))
        .await
        .unwrap();
    assert_eq!((status, value), (302, json!({"moved": 1})));
    assert_eq!(connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn transport_failures_map_to_public_codes() {
    // Refused: a port nothing listens on.
    let closed = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = closed.local_addr().unwrap();
    drop(closed);
    let error = client()
        .json(&target(addr), "GET", "/x", None, Duration::from_millis(500))
        .await
        .unwrap_err();
    // A port that was just released: refused, or closed by the kernel's
    // TIME_WAIT/backlog handling while the machine is under test load.
    #[cfg(not(windows))]
    assert!(
        matches!(
            error,
            ClientError::ConnectionRefused | ClientError::ConnectionClosed
        ),
        "{error:?}"
    );
    // A just-released Windows loopback port can remain pending until the
    // caller's connect deadline; preserve that distinct timeout result.
    #[cfg(windows)]
    assert!(
        matches!(
            error,
            ClientError::ConnectionRefused | ClientError::ConnectionClosed | ClientError::Timeout
        ),
        "{error:?}"
    );

    // Silent peer: accepted, never answers.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hold = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
        drop(stream);
    });
    let started = std::time::Instant::now();
    let error = client()
        .json(&target(addr), "GET", "/x", None, Duration::from_millis(200))
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::Timeout);
    assert!(started.elapsed() < Duration::from_secs(2));
    hold.abort();

    // Closed before any byte, truncated body, malformed status line.
    let (addr, _) = serve(Vec::new()).await;
    let error = client()
        .json(&target(addr), "GET", "/x", None, Duration::from_millis(500))
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::ConnectionClosed);

    let (addr, _) =
        serve(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\n{\"short\":true}".to_vec()).await;
    let error = client()
        .json(&target(addr), "GET", "/x", None, Duration::from_millis(500))
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::ConnectionClosed);

    let (addr, _) = serve(b"SMTP 220 hello\r\n\r\n".to_vec()).await;
    let error = client()
        .json(&target(addr), "GET", "/x", None, Duration::from_millis(500))
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::Invalid("bad status line"));

    let (addr, _) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nnotjs".to_vec()).await;
    let error = client()
        .json(&target(addr), "GET", "/x", None, Duration::from_millis(500))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        ClientError::ConnectionClosed,
        "5 of 6 promised bytes arrived"
    );

    let (addr, _) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nnotjs".to_vec()).await;
    let error = client()
        .json(&target(addr), "GET", "/x", None, Duration::from_millis(500))
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::Invalid("body is not JSON"));
}

#[test]
fn request_failure_texts_match_python_and_never_carry_private_detail() {
    let five = Duration::from_secs(5);
    assert_eq!(
        request_failure(None, Some(403), five),
        (
            "http_error",
            "节点拒绝访问，请检查认证或访问权限（HTTP 403）".to_string()
        )
    );
    assert_eq!(
        request_failure(None, Some(401), five).1,
        "节点认证失败（HTTP 401）"
    );
    assert_eq!(
        request_failure(None, Some(404), five).1,
        "节点接口不存在（HTTP 404）"
    );
    assert_eq!(
        request_failure(None, Some(429), five).1,
        "节点请求过于频繁（HTTP 429）"
    );
    assert_eq!(
        request_failure(None, Some(503), five).1,
        "节点服务暂不可用（HTTP 503）"
    );
    assert_eq!(
        request_failure(None, Some(500), five).1,
        "节点返回错误响应（HTTP 500）"
    );
    // A 200 status defers to the transport error.
    assert_eq!(
        request_failure(
            Some(&ClientError::Invalid("private upstream details")),
            Some(200),
            five
        ),
        ("invalid_response", "节点返回无效或不完整的响应".to_string())
    );
    assert_eq!(
        request_failure(Some(&ClientError::Timeout), None, five),
        ("timeout", "节点连接或响应超时（等待超过 5 秒）".to_string())
    );
    assert_eq!(
        ClientError::Timeout.message(Duration::from_millis(200)),
        "节点连接或响应超时（等待超过 0.2 秒）"
    );
    assert_eq!(
        ClientError::Timeout.message(Duration::from_secs(60)),
        "节点连接或响应超时（等待超过 60 秒）"
    );
    assert_eq!(
        request_failure(Some(&ClientError::ConnectionRefused), None, five),
        (
            "connection_refused",
            "节点拒绝连接，目标端口未接受请求".to_string()
        )
    );
    assert_eq!(
        request_failure(Some(&ClientError::Unreachable), None, five).1,
        "节点网络不可达"
    );
    assert_eq!(
        request_failure(Some(&ClientError::ConnectionClosed), None, five),
        (
            "connection_closed",
            "节点连接中断，未收到完整响应".to_string()
        )
    );
    assert_eq!(
        request_failure(None, None, five),
        ("connection_failed", "无法建立节点连接".to_string())
    );
    assert_eq!(
        ClientError::from_io(&io::Error::from_raw_os_error(113)),
        ClientError::Unreachable
    );
    assert_eq!(
        ClientError::from_io(&io::Error::from_raw_os_error(101)),
        ClientError::Unreachable
    );
    assert_eq!(
        ClientError::from_io(&io::Error::from(io::ErrorKind::ConnectionReset)),
        ClientError::ConnectionClosed
    );
    assert_eq!(
        format!("{:?}", target("127.0.0.1:1".parse().unwrap())),
        "Target(http://127.0.0.1:1, token <redacted>)"
    );
}

#[tokio::test]
async fn upgrade_hands_back_the_socket_with_prefetched_bytes() {
    let response = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n\x81\x02hi".to_vec();
    let (addr, task) = serve(response).await;
    let response = client()
        .open(
            &target(addr),
            Request {
                method: "GET",
                target: "/api/term/attach?name=x",
                headers: &[
                    ("Connection", "Upgrade"),
                    ("Upgrade", "websocket"),
                    ("Sec-WebSocket-Key", "abc="),
                ],
                body: None,
                connect: Duration::from_millis(500),
                idle: Duration::from_millis(500),
            },
        )
        .await
        .unwrap();
    assert_eq!(response.status, 101);
    assert_eq!(response.header("upgrade"), Some("websocket"));
    let (mut stream, prefetched) = response.into_body().into_raw();
    let mut rest = prefetched;
    let mut scratch = [0u8; 16];
    loop {
        let count = stream.read(&mut scratch).await.unwrap();
        if count == 0 {
            break;
        }
        rest.extend_from_slice(&scratch[..count]);
    }
    assert_eq!(rest, b"\x81\x02hi");
    let request = String::from_utf8(task.await.unwrap()).unwrap();
    assert!(
        request.contains("\r\nConnection: Upgrade\r\n"),
        "caller's Connection header wins: {request}"
    );
    assert!(!request.contains("Connection: close"));
}

#[tokio::test]
async fn malformed_requests_are_rejected_before_any_byte_is_sent() {
    let (addr, _) = serve(Vec::new()).await;
    let error = client()
        .json(
            &target(addr),
            "GET",
            "/x\r\nInjected: 1",
            None,
            Duration::from_millis(500),
        )
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::Invalid("malformed request target"));
    let error = client()
        .open(
            &target(addr),
            Request {
                method: "GET",
                target: "/x",
                headers: &[("X-Bad", "a\r\nb")],
                body: None,
                connect: Duration::from_millis(500),
                idle: Duration::from_millis(500),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::Invalid("malformed request header"));
}
