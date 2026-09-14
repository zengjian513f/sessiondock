//! Synthetic TCP ptyhost peers only: no native homes, PTYs, shells, or CLI jobs.
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sessiondock::{app_with_shutdown, config::Config};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, Notify},
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const NAME: &str = "sessiondock-synthetic-terminal";
const HOST_TOKEN: &str = "SYNTHETIC_HOST_SECRET_NEVER_PUBLIC";
const READY: &[u8] = b"synthetic ready\r\n";
const ECHO: u8 = 0;
const EXIT: u8 = 1;
const REJECT_ATTACH: u8 = 2;
const SILENT_INFO: u8 = 3;
const INVALID_FRAME: u8 = 4;
const BURST: u8 = 5;
const EXITED_INFO: u8 = 6;
const INCOMPLETE_EXIT: u8 = 7;
const UNMARKED_EOF: u8 = 8;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
type SeenFrames = Arc<Mutex<Vec<(u8, Vec<u8>)>>>;

struct FakeHost {
    directory: TempDir,
    mode: Arc<AtomicU8>,
    seen: SeenFrames,
    active: Arc<AtomicUsize>,
    changed: Arc<Notify>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl FakeHost {
    async fn new(mode: u8) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let record = json!({"name":NAME,"host_pid":42,"pid":43,"created":123,
            "cwd":"/synthetic","cmd":"synthetic-peer","cols":80,"rows":24,
            "port":listener.local_addr().unwrap().port(),"token":HOST_TOKEN,
            "argv":["SECRET_ARGV"],"meta":{"secret":"SECRET_META"}});
        tokio::fs::write(
            directory.path().join(format!("{NAME}.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .await
        .unwrap();
        let mode = Arc::new(AtomicU8::new(mode));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let active = Arc::new(AtomicUsize::new(0));
        let changed = Arc::new(Notify::new());
        let cancel = CancellationToken::new();
        let task = {
            let (mode, seen, active, changed, cancel) = (
                mode.clone(),
                seen.clone(),
                active.clone(),
                changed.clone(),
                cancel.clone(),
            );
            tokio::spawn(async move {
                let mut jobs = JoinSet::new();
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        result = listener.accept() => {
                            let Ok((stream, _)) = result else { break };
                            jobs.spawn(peer(stream, record.clone(), mode.load(Ordering::SeqCst), seen.clone(), active.clone(), changed.clone()));
                        },
                        _ = jobs.join_next(), if !jobs.is_empty() => {},
                    }
                }
                jobs.abort_all();
                while jobs.join_next().await.is_some() {}
            })
        };
        Self {
            directory,
            mode,
            seen,
            active,
            changed,
            cancel,
            task,
        }
    }

    fn set(&self, mode: u8) {
        self.mode.store(mode, Ordering::SeqCst);
    }

    async fn wait_detached(&self) {
        timeout(Duration::from_secs(5), async {
            while self.active.load(Ordering::SeqCst) != 0 {
                self.changed.notified().await;
            }
        })
        .await
        .expect("bridge must drop only its local attachment");
    }
}

impl Drop for FakeHost {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

struct AttachmentCount {
    active: Arc<AtomicUsize>,
    changed: Arc<Notify>,
}
impl Drop for AttachmentCount {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.changed.notify_one();
    }
}

fn frame(kind: u8, bytes: &[u8]) -> Vec<u8> {
    let mut frame = vec![kind];
    frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(bytes);
    frame
}

async fn peer(
    mut stream: TcpStream,
    record: Value,
    mode: u8,
    seen: SeenFrames,
    active: Arc<AtomicUsize>,
    changed: Arc<Notify>,
) {
    let mut line = Vec::new();
    loop {
        let Ok(byte) = stream.read_u8().await else {
            return;
        };
        if byte == b'\n' {
            break;
        }
        line.push(byte);
        if line.len() > 8192 {
            return;
        }
    }
    let request: Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(request["token"], HOST_TOKEN);
    if request["op"] == "info" {
        if mode == SILENT_INFO {
            let _ = stream.read_u8().await;
            return;
        }
        let reply = json!({"ok":true,"info":record,"exited":mode == EXITED_INFO});
        let _ = stream.write_all(format!("{reply}\n").as_bytes()).await;
        return;
    }
    assert_eq!(
        request["op"], "attach",
        "bridge must never send kill/create/send control operations"
    );
    if mode == REJECT_ATTACH {
        let _ = stream
            .write_all(format!("{{\"ok\":false,\"error\":\"{HOST_TOKEN}\"}}\n").as_bytes())
            .await;
        return;
    }
    active.fetch_add(1, Ordering::SeqCst);
    let _active = AttachmentCount { active, changed };
    let mut reply = format!(
        "{{\"ok\":true,\"cols\":{},\"rows\":{}}}\n",
        request["cols"], request["rows"]
    )
    .into_bytes();
    if mode == EXIT || mode == INCOMPLETE_EXIT || mode == UNMARKED_EOF {
        reply.extend(frame(1, b"FIRST\xff"));
        reply.extend(frame(1, b"LAST\x1b[0m"));
        if mode != UNMARKED_EOF {
            reply.extend(frame(
                3,
                if mode == INCOMPLETE_EXIT {
                    b"{\"code\":0,\"output_complete\":false,\"reason\":\"pty_drain_timeout\"}"
                } else {
                    b"{\"code\":0}"
                },
            ));
        }
        let _ = stream.write_all(&reply).await;
        return;
    }
    if mode == INVALID_FRAME {
        reply.extend([1, 255, 255, 255, 255]);
        let _ = stream.write_all(&reply).await;
        return;
    }
    reply.extend(frame(1, READY));
    if stream.write_all(&reply).await.is_err() {
        return;
    }
    if mode == BURST {
        let data = frame(1, &vec![b'x'; 64 * 1024]);
        for _ in 0..1024 {
            if stream.write_all(&data).await.is_err() {
                return;
            }
        }
    }
    loop {
        let Ok(kind) = stream.read_u8().await else {
            return;
        };
        let Ok(length) = stream.read_u32().await else {
            return;
        };
        if length > 1024 * 1024 {
            return;
        }
        let mut data = vec![0; length as usize];
        if stream.read_exact(&mut data).await.is_err() {
            return;
        }
        seen.lock().await.push((kind, data.clone()));
        let response = match kind {
            1 => data,
            2 => b"RESIZED".to_vec(),
            _ => panic!("unexpected frame kind"),
        };
        if stream.write_all(&frame(1, &response)).await.is_err() {
            return;
        }
    }
}

struct Server {
    app: Router,
    address: SocketAddr,
    cancel: CancellationToken,
    task: JoinHandle<Result<(), std::io::Error>>,
}

impl Server {
    async fn start(directory: Option<&Path>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let cancel = CancellationToken::new();
        let config = Config {
            bind: address,
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            ptyhost_dir: directory.map(Path::to_owned),
            ..Default::default()
        };
        let app = app_with_shutdown(config, cancel.clone()).unwrap();
        let task = {
            let app = app.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(cancel.cancelled_owned())
                .await
            })
        };
        Self {
            app,
            address,
            cancel,
            task,
        }
    }

    async fn request(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", self.address.to_string())
            .header("content-type", "application/json")
            .body(if method == "GET" {
                Body::empty()
            } else {
                Body::from(body.to_string())
            })
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.headers()["cache-control"], "no-store");
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn claim(&self, page: &str, force: bool) -> String {
        let (status, body) = self
            .request(
                "POST",
                "/api/term/claim",
                json!({"name":NAME,"page":page,"force":force,
            "_build":"synthetic","_trace_id":"synthetic-trace","_page_id":page}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(!body.to_string().contains(HOST_TOKEN));
        body["token"].as_str().unwrap().to_owned()
    }

    async fn connect(&self, page: &str, token: &str) -> Ws {
        let uri = format!(
            "ws://{}/api/term/attach?name={NAME}&page={page}&token={token}&connection=synthetic-client-id&cols=80&rows=24",
            self.address
        );
        let mut request = uri.into_client_request().unwrap();
        request.headers_mut().insert(
            "origin",
            format!("http://{}", self.address).parse().unwrap(),
        );
        timeout(Duration::from_secs(5), connect_async(request))
            .await
            .unwrap()
            .unwrap()
            .0
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

async fn next(ws: &mut Ws) -> Message {
    timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .expect("WS event")
        .unwrap()
}

async fn closed(ws: &mut Ws) -> (u16, String, Vec<u8>) {
    let mut bytes = Vec::new();
    for _ in 0..4096 {
        match next(ws).await {
            Message::Binary(chunk) => bytes.extend_from_slice(&chunk),
            Message::Close(Some(frame)) => {
                return (u16::from(frame.code), frame.reason.to_string(), bytes);
            }
            Message::Text(text) => assert_eq!(
                serde_json::from_str::<Value>(&text).unwrap()["t"],
                "revoked"
            ),
            _ => {}
        }
    }
    panic!("bounded WS stream did not close")
}

#[tokio::test]
async fn transport_defaults_off_and_host_listing_never_enables_legacy_cli_actions() {
    let server = Server::start(None).await;
    assert_eq!(
        server
            .request(
                "POST",
                "/api/term/claim",
                json!({"name":NAME,"page":"page"})
            )
            .await
            .0,
        StatusCode::NOT_IMPLEMENTED
    );
    let (_, disabled) = server.request("GET", "/api/term/list", Value::Null).await;
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["transport_enabled"], false);
    let host = FakeHost::new(ECHO).await;
    let server = Server::start(Some(host.directory.path())).await;
    let (_, list) = server.request("GET", "/api/term/list", Value::Null).await;
    assert_eq!(list["enabled"], false);
    assert_eq!(list["transport_enabled"], true);
    assert_eq!(list["sessions"], json!([]));
    assert_eq!(list["hosts"][0]["name"], NAME);
    let encoded = list.to_string();
    for secret in [
        HOST_TOKEN,
        "SECRET_ARGV",
        "SECRET_META",
        "\"port\"",
        "\"token\"",
    ] {
        assert!(!encoded.contains(secret));
    }
}

#[tokio::test]
async fn binary_input_output_and_serial_resize_preserve_raw_bytes() {
    let host = FakeHost::new(ECHO).await;
    let server = Server::start(Some(host.directory.path())).await;
    let token = server.claim("page-a", false).await;
    let mut ws = server.connect("page-a", &token).await;
    assert_eq!(next(&mut ws).await, Message::Binary(READY.to_vec().into()));
    let input = b"raw\xff\x00\x1b[31m\xe4\xb8\xad\r";
    ws.send(Message::Binary(input.to_vec().into()))
        .await
        .unwrap();
    assert_eq!(next(&mut ws).await, Message::Binary(input.to_vec().into()));
    ws.send(Message::Text(
        "{\"t\":\"resize\",\"cols\":100,\"rows\":40}".into(),
    ))
    .await
    .unwrap();
    assert_eq!(
        next(&mut ws).await,
        Message::Binary(b"RESIZED".to_vec().into())
    );
    let seen = host.seen.lock().await;
    assert_eq!(seen[0], (1, input.to_vec()));
    assert_eq!(seen[1].0, 2);
    assert_eq!(
        serde_json::from_slice::<Value>(&seen[1].1).unwrap(),
        json!({"cols":100,"rows":40})
    );
    drop(seen);
    ws.close(None).await.unwrap();
    host.wait_detached().await;
    server.claim("page-b", false).await;
}

#[tokio::test]
async fn replay_and_final_bytes_are_drained_before_coalesced_host_exit() {
    let host = FakeHost::new(EXIT).await;
    let server = Server::start(Some(host.directory.path())).await;
    let token = server.claim("page", false).await;
    let mut ws = server.connect("page", &token).await;
    let (code, _, bytes) = closed(&mut ws).await;
    assert_eq!(code, 1000);
    assert_eq!(bytes, b"FIRST\xffLAST\x1b[0m");
}

#[tokio::test]
async fn incomplete_exit_preserves_prior_bytes_and_reports_abnormal_close() {
    let host = FakeHost::new(INCOMPLETE_EXIT).await;
    let server = Server::start(Some(host.directory.path())).await;
    let token = server.claim("page", false).await;
    let mut ws = server.connect("page", &token).await;
    let (code, reason, bytes) = closed(&mut ws).await;
    assert_eq!(code, 1011);
    assert_eq!(reason, "host output incomplete: PTY drain timeout");
    assert_eq!(bytes, b"FIRST\xffLAST\x1b[0m");
}

#[tokio::test]
async fn unmarked_socket_eof_does_not_claim_that_the_host_process_exited() {
    let host = FakeHost::new(UNMARKED_EOF).await;
    let server = Server::start(Some(host.directory.path())).await;
    let token = server.claim("page", false).await;
    let mut ws = server.connect("page", &token).await;
    let (code, reason, bytes) = closed(&mut ws).await;
    assert_eq!(code, 1011);
    assert_eq!(reason, "host stream closed without exit marker");
    assert_eq!(bytes, b"FIRST\xffLAST\x1b[0m");
}

#[tokio::test]
async fn takeover_revokes_old_socket_and_old_cleanup_cannot_release_replacement() {
    let host = FakeHost::new(ECHO).await;
    let server = Server::start(Some(host.directory.path())).await;
    let old = server.claim("page-a", false).await;
    let mut first = server.connect("page-a", &old).await;
    next(&mut first).await;
    let (conflict, body) = server
        .request(
            "POST",
            "/api/term/claim",
            json!({"name":NAME,"page":"page-b"}),
        )
        .await;
    assert_eq!(conflict, StatusCode::CONFLICT);
    assert_eq!(body["conflict"], true);
    assert!(body.get("token").is_none());
    let token = server.claim("page-b", true).await;
    let (code, reason, _) = closed(&mut first).await;
    assert_eq!(code, 4001);
    // Both pages claim from the same address, so the notice carries no label.
    assert_eq!(reason, "revoked:");
    let mut second = server.connect("page-b", &token).await;
    assert_eq!(
        next(&mut second).await,
        Message::Binary(READY.to_vec().into())
    );
    second
        .send(Message::Binary(b"new owner".to_vec().into()))
        .await
        .unwrap();
    assert_eq!(
        next(&mut second).await,
        Message::Binary(b"new owner".to_vec().into())
    );
    assert_eq!(host.seen.lock().await.len(), 1);
}

/// `X-Real-IP` labels a claim only through the authenticated node listener
/// (the hub forwards the browser's address); the loopback listener ignores it.
/// A conflict says whether the holder sits at the claimant's own address.
#[cfg(unix)]
#[tokio::test]
async fn claim_labels_hub_traffic_by_forwarded_address_and_flags_same_address_conflicts() {
    use std::os::unix::fs::PermissionsExt;

    use axum::extract::ConnectInfo;
    use sessiondock::app_pair_with_shutdown;

    const NODE_TOKEN: &str = "node-t0ken.node-t0ken.node-t0ken.node-t0ken~";
    let host = FakeHost::new(ECHO).await;
    let temp = tempfile::tempdir().unwrap();
    let token_file = temp.path().join("node-token");
    std::fs::write(&token_file, format!("{NODE_TOKEN}\n")).unwrap();
    std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let config = Config {
        web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
        ptyhost_dir: Some(host.directory.path().to_owned()),
        node_bind: Some("127.0.0.1:0".parse().unwrap()),
        node_token_file: Some(token_file),
        node_id_file: Some(temp.path().join("node-id")),
        node_peers: vec!["10.100.100.0/24".parse().unwrap()],
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let (browser, node) = app_pair_with_shutdown(config, cancel.clone()).unwrap();
    let node = node.expect("node router");
    let claim = |router: &Router, hub: bool, real_ip: &str, page: &str, force: bool| {
        let router = router.clone();
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/term/claim")
            .header("content-type", "application/json")
            .header("x-real-ip", real_ip);
        builder = if hub {
            builder
                .header("host", "10.100.100.2:8742")
                .header("x-sessiondock-protocol", "1")
                .header("x-sessiondock-node-token", NODE_TOKEN)
        } else {
            builder.header("host", "127.0.0.1:8742")
        };
        let mut request = builder
            .body(Body::from(
                json!({"name":NAME,"page":page,"force":force}).to_string(),
            ))
            .unwrap();
        let peer: SocketAddr = if hub {
            "10.100.100.1:40000"
        } else {
            "127.0.0.1:40000"
        }
        .parse()
        .unwrap();
        request.extensions_mut().insert(ConnectInfo(peer));
        async move {
            let response = router.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        }
    };
    let (status, body) = claim(&node, true, "203.0.113.7", "page-a", false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["owner"]["ip"], "203.0.113.7");
    assert!(body.get("same_address").is_none());
    let (status, body) = claim(&node, true, "203.0.113.7", "page-b", false).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["conflict"], true);
    assert_eq!(body["owner"]["ip"], "203.0.113.7");
    assert_eq!(body["same_address"], true);
    let (status, body) = claim(&node, true, "198.51.100.4", "page-b", false).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["same_address"], false);
    let (status, body) = claim(&browser, false, "203.0.113.9", "page-c", true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["owner"]["ip"], "127.0.0.1");
    cancel.cancel();
}

#[tokio::test]
async fn same_page_reconnect_is_silent_and_binds_only_once() {
    let host = FakeHost::new(ECHO).await;
    let server = Server::start(Some(host.directory.path())).await;
    let old = server.claim("page", false).await;
    let mut first = server.connect("page", &old).await;
    next(&mut first).await;
    let token = server.claim("page", false).await;
    let event = next(&mut first).await;
    let Message::Close(Some(close)) = event else {
        panic!("same-page reconnect must not emit revoked notice")
    };
    assert_eq!(u16::from(close.code), 4001);
    assert_eq!(close.reason, "replaced");
    let mut second = server.connect("page", &token).await;
    next(&mut second).await;
    let url = format!(
        "ws://{}/api/term/attach?name={NAME}&page=page&token={token}",
        server.address
    );
    let error = connect_async(url).await.err().unwrap();
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP ownership rejection")
    };
    assert_eq!(response.status(), StatusCode::CONFLICT);
    second
        .send(Message::Binary(b"still-current".to_vec().into()))
        .await
        .unwrap();
    assert_eq!(
        next(&mut second).await,
        Message::Binary(b"still-current".to_vec().into())
    );
}

#[tokio::test]
async fn claim_accepts_large_ignored_fields_without_echoing_payload() {
    let host = FakeHost::new(ECHO).await;
    let server = Server::start(Some(host.directory.path())).await;
    let (status, body) = server
        .request(
            "POST",
            "/api/term/claim",
            json!({
        "name":NAME,"page":"page","_trace_id":"sensitive-test-marker".repeat(256 * 1024)}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.to_string().contains("sensitive-test-marker"));
    server.claim("other-page", true).await;
}

#[tokio::test]
async fn handshake_and_attach_failure_release_lease_without_exposing_host_token() {
    let host = FakeHost::new(REJECT_ATTACH).await;
    let server = Server::start(Some(host.directory.path())).await;
    let token = server.claim("page-a", false).await;
    let path = format!("/api/term/attach?name={NAME}&page=page-a&token={token}");
    assert_eq!(
        server.request("GET", &path, Value::Null).await.0,
        StatusCode::BAD_REQUEST
    );
    let token = server.claim("page-b", false).await;
    let mut ws = server.connect("page-b", &token).await;
    let (code, reason, _) = closed(&mut ws).await;
    assert_eq!(code, 1011);
    assert!(!reason.contains(HOST_TOKEN));
    host.set(ECHO);
    let token = server.claim("page-c", false).await;
    let mut recovered = server.connect("page-c", &token).await;
    assert_eq!(
        next(&mut recovered).await,
        Message::Binary(READY.to_vec().into())
    );
}

#[tokio::test]
async fn claim_checks_missing_exited_and_timed_out_hosts_before_issuing_token() {
    let host = FakeHost::new(EXITED_INFO).await;
    let server = Server::start(Some(host.directory.path())).await;
    let (status, body) = server
        .request(
            "POST",
            "/api/term/claim",
            json!({"name":"absent","page":"page"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.get("token").is_none());
    let (status, _) = server
        .request(
            "POST",
            "/api/term/claim",
            json!({"name":NAME,"page":"page"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    host.set(SILENT_INFO);
    let (status, _) = server
        .request(
            "POST",
            "/api/term/claim",
            json!({"name":NAME,"page":"page"}),
        )
        .await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    host.set(ECHO);
    server.claim("other-page", false).await;
}

#[tokio::test]
async fn browser_text_is_literal_input_and_malformed_host_frames_fail_closed() {
    let host = FakeHost::new(ECHO).await;
    let server = Server::start(Some(host.directory.path())).await;
    for text in [
        "not-json",
        "{\"t\":\"resize\",\"cols\":0,\"rows\":24}",
        "{\"t\":\"send\",\"text\":\"should not run\"}",
    ] {
        let token = server.claim("page", false).await;
        let mut ws = server.connect("page", &token).await;
        next(&mut ws).await;
        ws.send(Message::Text(text.into())).await.unwrap();
        assert_eq!(next(&mut ws).await, Message::Binary(text.as_bytes().into()));
    }
    assert_eq!(host.seen.lock().await.len(), 3);
    let token = server.claim("page", false).await;
    let mut oversized = server.connect("page", &token).await;
    next(&mut oversized).await;
    oversized
        .send(Message::Binary(vec![b'x'; 1024 * 1024 + 1].into()))
        .await
        .unwrap();
    assert_eq!(closed(&mut oversized).await.0, 1011);
    assert_eq!(host.seen.lock().await.len(), 3);
    host.set(INVALID_FRAME);
    let token = server.claim("page", false).await;
    let mut ws = server.connect("page", &token).await;
    assert_eq!(closed(&mut ws).await.0, 1011);
}

#[tokio::test]
async fn web_shutdown_detaches_but_host_remains_available_for_next_server() {
    let host = FakeHost::new(ECHO).await;
    let server = Server::start(Some(host.directory.path())).await;
    let token = server.claim("page", false).await;
    let mut ws = server.connect("page", &token).await;
    next(&mut ws).await;
    server.cancel.cancel();
    assert_eq!(closed(&mut ws).await.0, 1001);
    host.wait_detached().await;
    let replacement = Server::start(Some(host.directory.path())).await;
    let token = replacement.claim("page-new", false).await;
    let mut recovered = replacement.connect("page-new", &token).await;
    assert_eq!(
        next(&mut recovered).await,
        Message::Binary(READY.to_vec().into())
    );
}

#[tokio::test]
async fn stalled_browser_waits_until_takeover_and_does_not_block_recovery() {
    let host = FakeHost::new(BURST).await;
    let server = Server::start(Some(host.directory.path())).await;
    let token = server.claim("slow-page", false).await;
    let mut ws = server.connect("slow-page", &token).await;
    assert_eq!(next(&mut ws).await, Message::Binary(READY.to_vec().into()));
    // Stop consuming WS output. Python has no fixed downstream backpressure
    // deadline, so an otherwise healthy attachment remains active.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(host.active.load(Ordering::SeqCst), 1);
    host.set(ECHO);
    let token = server.claim("new-page", true).await;
    host.wait_detached().await;
    let mut recovered = server.connect("new-page", &token).await;
    assert_eq!(
        next(&mut recovered).await,
        Message::Binary(READY.to_vec().into())
    );
}
