//! The hub's HTTP surface (batch 40 H4) against two or three
//! `tests/hub_fake_node.py` nodes: the `test_hub.py` dispatch/proxy cases —
//! page in hub mode, `/api/meta`, `/api/nodes`, the gate, resolve
//! uniqueness (400), offline 503 shape, `_build` 409, JSON/SSE rewrite, raw
//! WebSocket pass-through, chunked uploads, explicit node routes, display /
//! order writes with their audit records, bulk writes, browser audit
//! routing and the NDJSON search. Nothing here touches a session root.
use std::{
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};

use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    http::{HeaderName, Request, StatusCode},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sessiondock::{
    hub::{
        Client, Registry, proxy,
        registry::{Registration, parse_networks},
    },
    hub_api::{self, HubApp},
    hub_config::HubConfig,
};
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

const NID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NID_C: &str = "cccccccccccccccccccccccccccccccc";
const PNG_LEN: usize = 68;

fn python3() -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("python3"))
                .find(|candidate| candidate.is_file())
        })
        .unwrap_or_else(|| PathBuf::from("/usr/bin/python3"))
}

struct FakeNode {
    child: Child,
    port: u16,
    name: String,
    token: String,
}

impl FakeNode {
    fn start(id: &str, name: &str) -> Self {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/hub_fake_node.py");
        let mut child = Command::new(python3())
            .arg(&script)
            .args(["--id", id, "--name", name])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("python3 tests/hub_fake_node.py");
        let mut line = String::new();
        let mut stdout = child.stdout.take().unwrap();
        let mut byte = [0u8; 1];
        while stdout.read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {
            line.push(byte[0] as char);
        }
        let info: Value = serde_json::from_str(&line).expect("fake node start line");
        FakeNode {
            child,
            port: info["port"].as_u64().unwrap() as u16,
            name: name.to_string(),
            token: info["token"].as_str().unwrap().to_string(),
        }
    }

    fn registration(&self) -> Registration {
        Registration {
            name: self.name.clone(),
            url: format!("http://127.0.0.1:{}", self.port),
            token: self.token.clone(),
            color: String::new(),
            id: None,
        }
    }

    /// Plain blocking HTTP/1.0 to the control endpoints.
    fn http(&self, method: &str, path: &str, body: Option<&Value>) -> Value {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let payload = body.map(|body| body.to_string()).unwrap_or_default();
        write!(
            stream,
            "{method} {path} HTTP/1.0\r\nContent-Length: {}\r\n\r\n{payload}",
            payload.len()
        )
        .unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let end = raw
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        serde_json::from_slice(&raw[end + 4..]).unwrap()
    }

    fn set(&self, values: Value) {
        assert_eq!(
            self.http("POST", "/__control", Some(&json!({"set": values})))["ok"],
            true
        );
    }

    fn pop(&self, keys: &[&str]) {
        assert_eq!(
            self.http("POST", "/__control", Some(&json!({"pop": keys})))["ok"],
            true
        );
    }

    fn state(&self) -> Value {
        self.http("GET", "/__state", None)
    }

    fn writes(&self) -> Vec<(String, Value)> {
        self.state()["writes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| (entry[0].as_str().unwrap().to_string(), entry[1].clone()))
            .collect()
    }

    fn gets(&self) -> Vec<String> {
        self.state()["gets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry[0].as_str().unwrap().to_string())
            .collect()
    }
}

impl Drop for FakeNode {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Hub {
    dir: tempfile::TempDir,
    router: Router,
    registry: Arc<Registry>,
    client: Arc<Client>,
    build: String,
    shutdown: tokio_util::sync::CancellationToken,
    a: FakeNode,
    b: FakeNode,
}

impl Hub {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let audit = dir.path().join("audit");
        std::fs::create_dir(&audit).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&audit, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = HubConfig {
            nodes_file: dir.path().join("hub-nodes.json"),
            cache_dir: dir.path().join("hub-cache"),
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            audit_dir: Some(audit),
            ..HubConfig::default()
        };
        let shutdown = tokio_util::sync::CancellationToken::new();
        let HubApp {
            router,
            registry,
            client,
            monitor: _monitor,
        } = hub_api::hub_app(&config, shutdown.clone()).unwrap();
        let a = FakeNode::start(NID_A, "NodeA");
        let b = FakeNode::start(NID_B, "NodeB");
        registry.register(&client, a.registration()).await.unwrap();
        registry.register(&client, b.registration()).await.unwrap();
        registry.check_all(&client).await;
        let meta = json_of(request(&router, "GET", "/api/meta", &[], None).await)
            .await
            .1;
        Hub {
            dir,
            router,
            registry,
            client,
            build: meta["build"].as_str().unwrap().to_string(),
            shutdown,
            a,
            b,
        }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        json_of(request(&self.router, method, path, &[], body).await).await
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.call("POST", path, Some(body)).await
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.call("GET", path, None).await
    }

    /// Serve the router on a real loopback listener (for the WebSocket
    /// upgrade, which needs a hyper connection).
    async fn listen(&self) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let router = self.router.clone();
        let stop = self.shutdown.clone();
        tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async move { stop.cancelled().await })
            .await
            .unwrap();
        });
        port
    }

    async fn b_offline(&self) {
        self.b.set(json!({"offline": true}));
        self.registry.check_all(&self.client).await;
        self.registry.check_all(&self.client).await;
        assert!(self.registry.offline(NID_B));
    }

    fn audit_events(&self) -> Vec<Value> {
        let mut events = Vec::new();
        for entry in std::fs::read_dir(self.dir.path().join("audit")).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            events.extend(
                text.lines()
                    .map(|line| serde_json::from_str::<Value>(line).unwrap()),
            );
        }
        events
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn request(
    router: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(path);
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("host"))
    {
        builder = builder.header("Host", "127.0.0.1:8742");
    }
    for (name, value) in headers {
        builder = builder.header(HeaderName::from_bytes(name.as_bytes()).unwrap(), *value);
    }
    let body = match body {
        Some(value) => {
            let bytes = value.to_string();
            builder = builder
                .header("Content-Type", "application/json")
                .header("Content-Length", bytes.len());
            Body::from(bytes)
        }
        None => Body::empty(),
    };
    router
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn json_of(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&bytes)}))
    };
    (status, value)
}

fn scoped(nid: &str, uid: &str) -> String {
    let (source, tail) = uid.split_once(':').unwrap();
    format!("{source}:{nid}~{tail}")
}

async fn wait_for(mut condition: impl FnMut() -> bool) {
    for _ in 0..100 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("condition not met in time");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn page_meta_nodes_and_gate() {
    let hub = Hub::new().await;
    let page = request(&hub.router, "GET", "/", &[], None).await;
    assert_eq!(page.status(), StatusCode::OK);
    let html =
        String::from_utf8(to_bytes(page.into_body(), 32 << 20).await.unwrap().to_vec()).unwrap();
    assert!(
        html.contains("<meta name=\"agenthub-mode\" content=\"hub\">"),
        "hub mode meta"
    );
    assert!(
        html.contains("<title>SessionDock · 会话管理</title>"),
        "hub hostname"
    );
    assert!(
        html.contains("&quot;storage_namespace&quot;:&quot;sessiondock.hub.&quot;"),
        "namespace"
    );
    assert!(
        html.contains("c.storage_namespace=\"sessiondock.hub.\"+location.pathname+'.'"),
        "path suffix script"
    );
    assert!(html.contains("&quot;hub&quot;:true"), "hub capability");
    assert!(
        !html.contains("media_lazy"),
        "lazy media stays undeclared on the hub page"
    );

    let (status, meta) = hub.get("/api/meta").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["mode"], "hub");
    assert_eq!(meta["protocol"], 1);
    assert_eq!(meta["hostname"], "SessionDock");
    assert_eq!(meta["build"].as_str().unwrap().len(), 12);
    assert_eq!(meta["capabilities"]["hub"], true);

    let (status, nodes) = hub.get("/api/nodes").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(nodes["mode"], "hub");
    let ids: Vec<&str> = nodes["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [NID_A, NID_B]);
    for row in nodes["nodes"].as_array().unwrap() {
        assert!(
            row.get("url").is_none() && row.get("token").is_none(),
            "{row}"
        );
        assert_eq!(row["online"], true);
    }
    assert!(!nodes.to_string().contains(&hub.a.token));
    assert_eq!(nodes["machines"][1]["enabled"], true);

    // Gate: loopback Host, no hub headers, same-origin writes; a POST to
    // `/api/nodes` is not a route but a request that names no machine.
    let foreign = request(
        &hub.router,
        "GET",
        "/api/meta",
        &[("Host", "example.lan")],
        None,
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    let hub_header = request(
        &hub.router,
        "GET",
        "/api/meta",
        &[("X-AgentHub-Protocol", "1")],
        None,
    )
    .await;
    assert_eq!(hub_header.status(), StatusCode::FORBIDDEN);
    let (status, body) = json_of(
        request(
            &hub.router,
            "POST",
            "/api/term/attach",
            &[("Origin", "https://other.invalid")],
            Some(json!({})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "cross-origin write rejected");
    let (status, body) = hub.post("/api/nodes", json!({"name": "X"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], proxy::ONE_MACHINE);
    assert_eq!(hub.registry.all().len(), 2);
    let (status, _) = json_of(
        request(
            &hub.router,
            "POST",
            "/api/session/star",
            &[("Transfer-Encoding", "chunked")],
            Some(json!({"uid": "x"})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resolve_routes_writes_and_rejects_mixed_targets() {
    let hub = Hub::new().await;
    let a = scoped(NID_A, "claude:same-file-hash");
    let (status, body) = hub
        .post("/api/session/star", json!({"uid": a, "starred": true}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        hub.a.writes().last().unwrap().1["uid"],
        "claude:same-file-hash"
    );
    // The answer comes back scoped.
    assert_eq!(body["uid"], a);
    let b_name = format!("{NID_B}~same-terminal");
    let (status, body) = hub
        .post("/api/session/rewind", json!({"uid": a, "name": b_name}))
        .await;
    assert_eq!(
        (status, body["error"].as_str().unwrap()),
        (StatusCode::BAD_REQUEST, proxy::ONE_MACHINE)
    );
    let (status, _) = hub
        .post(
            "/api/term/create",
            json!({"source": "claude", "cwd": "/same"}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = hub
        .post(
            "/api/term/create",
            json!({"_node": NID_B, "_build": "stale"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reload"], true);
    assert_eq!(body["build"], hub.build);
    let (status, body) = hub
        .post(
            "/api/session/send",
            json!({"uid": a, "name": format!("{NID_A}~same-terminal"), "text": "keep exact text",
                "request_id": "request-123", "_build": hub.build,
                "media": [{"src": format!("/api/nodes/{NID_A}/api/media/{}", "d".repeat(32))}]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let forwarded = hub.a.writes().last().unwrap().1.clone();
    assert_eq!(forwarded["name"], "same-terminal");
    assert_eq!(
        forwarded["media"][0]["src"],
        format!("/api/media/{}", "d".repeat(32))
    );
    assert_eq!(forwarded["text"], "keep exact text");
    assert_eq!(forwarded["request_id"], "request-123");
    assert!(forwarded.get("_node").is_none());
    let (status, body) = hub
        .post(
            "/api/session/send",
            json!({"uid": a, "_build": hub.build,
            "media": [{"src": format!("/api/nodes/{NID_B}/api/media/{}", "d".repeat(32))}]}),
        )
        .await;
    assert_eq!(
        (status, body["error"].as_str().unwrap()),
        (StatusCode::BAD_REQUEST, proxy::FOREIGN_ATTACHMENT)
    );
    // Unknown machine ids are 404, not proxied.
    let (status, body) = hub
        .post(
            "/api/session/star",
            json!({"uid": scoped(NID_C, "claude:x"), "starred": true}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "机器未注册或已移除");
    // Explicit routes carry local references and reach one machine only.
    let (status, body) = hub
        .post(
            &format!("/api/nodes/{NID_B}/api/term/backend"),
            json!({"backend": "ptyhost"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["backend"], "ptyhost");
    let (_, listing) = hub.get("/api/term/list").await;
    assert_eq!(listing["capabilities"][NID_B]["backend"], "ptyhost");
    assert_eq!(listing["capabilities"][NID_A]["backend"], "tmux");
    let (status, body) = hub
        .post(
            &format!("/api/nodes/{NID_B}/api/term/backend"),
            json!({"backend": "nope"}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("未知终端后端"));
    let (status, _) = hub.get(&format!("/api/nodes/{NID_C}/api/live")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // `DELETE /api/session/<global>` names its machine through the path; the
    // fixture has no DELETE handler, so its 501 HTML page is streamed back.
    let (status, body) = hub.call("DELETE", &format!("/api/session/{a}"), None).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert!(body["raw"].as_str().unwrap().contains("DELETE"), "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_machine_is_503_until_it_answers_again() {
    let hub = Hub::new().await;
    hub.b_offline().await;
    let b = scoped(NID_B, "claude:same-file-hash");
    let (status, body) = hub
        .post("/api/session/star", json!({"uid": b, "starred": true}))
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["node_offline"], true);
    assert_eq!(body["node_id"], NID_B);
    assert!(body["offline_since"].as_f64().is_some(), "{body}");
    assert!(
        body["error"].as_str().unwrap().starts_with("NodeB 离线："),
        "{body}"
    );
    // The list still carries B's cached rows as stale, and A is untouched.
    let (_, sessions) = hub.get("/api/sessions?force=1").await;
    assert_eq!(sessions["partial"], true);
    assert!(
        sessions["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["node_id"] == NID_B && row["stale"] == true)
    );
    // Back online: the inline recheck lets the write through at once.
    hub.b.pop(&["offline"]);
    let (status, _) = hub
        .post("/api/session/star", json!({"uid": b, "starred": false}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!hub.registry.offline(NID_B));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn json_sse_media_and_history_pages_are_rewritten_or_streamed() {
    let hub = Hub::new().await;
    let a = scoped(NID_A, "claude:same-file-hash");
    let (status, body) = hub.get(&format!("/api/messages/{a}?start=0")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["meta"]["uid"], a);
    assert_eq!(body["meta"]["node_name"], "NodeA");
    assert_eq!(
        body["messages"][1]["media"][0]["src"],
        format!("/api/nodes/{NID_A}/api/media/{}", "d".repeat(32))
    );
    assert!(
        hub.a
            .gets()
            .contains(&"/api/messages/claude:same-file-hash".to_string())
    );
    // Rust-only sub-routes keep their suffix on the way to the node.
    let _ = hub.get(&format!("/api/messages/{a}/page?cursor=x")).await;
    assert!(
        hub.a
            .gets()
            .contains(&"/api/messages/claude:same-file-hash/page".to_string())
    );
    // Media through the explicit route is streamed with its framing.
    let media = request(
        &hub.router,
        "GET",
        &format!("/api/nodes/{NID_A}/api/media/{}", "d".repeat(32)),
        &[],
        None,
    )
    .await;
    assert_eq!(media.status(), StatusCode::OK);
    assert_eq!(media.headers()["content-type"], "image/png");
    assert_eq!(media.headers()["content-length"], PNG_LEN.to_string());
    assert_eq!(media.headers()["connection"], "close");
    let bytes = to_bytes(media.into_body(), 1 << 20).await.unwrap();
    assert_eq!(bytes.len(), PNG_LEN);
    assert!(bytes.starts_with(b"\x89PNG"));
    // SSE: only `data:` payloads are rewritten; the stream keeps flowing.
    let watch = request(
        &hub.router,
        "GET",
        &format!("/api/watch?uid={a}&start=0"),
        &[],
        None,
    )
    .await;
    assert_eq!(watch.status(), StatusCode::OK);
    assert!(
        watch.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    assert_eq!(watch.headers()["x-accel-buffering"], "no");
    let mut stream = watch.into_body().into_data_stream();
    let mut collected = Vec::new();
    while !collected.windows(2).any(|w| w == b"\n\n") {
        let chunk = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        collected.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(collected).unwrap();
    let line = text
        .lines()
        .find(|line| line.starts_with("data: "))
        .unwrap();
    let event: Value = serde_json::from_str(&line[6..]).unwrap();
    assert_eq!(event["meta"]["uid"], a);
    assert_eq!(event["meta"]["node_id"], NID_A);
    assert_eq!(
        event["messages"][1]["media"][0]["src"],
        format!("/api/nodes/{NID_A}/api/media/{}", "d".repeat(32))
    );
    hub.a.set(json!({"pause_stream": true}));
    drop(stream);
    hub.a.pop(&["pause_stream"]);
    // A file navigation from a browser is redirected to the reader page.
    let nav = request(
        &hub.router,
        "GET",
        &format!("/api/session/file?uid={a}&path=%2Fetc%2Fhosts"),
        &[("Accept", "text/html")],
        None,
    )
    .await;
    assert_eq!(nav.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        nav.headers()["location"],
        format!(
            "../../file.html?uid={}&path=%2Fetc%2Fhosts",
            proxy::quote(&a, "")
        )
    );
    let nav = request(
        &hub.router,
        "GET",
        &format!("/api/nodes/{NID_A}/api/session/file?uid=claude:same-file-hash&raw=0"),
        &[("Accept", "text/html")],
        None,
    )
    .await;
    assert_eq!(nav.status(), StatusCode::SEE_OTHER);
    assert!(
        nav.headers()["location"]
            .to_str()
            .unwrap()
            .starts_with(&format!(
                "../../../../../file.html?uid={}",
                proxy::quote(&a, "")
            ))
    );
    let api = request(
        &hub.router,
        "GET",
        &format!("/api/session/file?uid={a}&path=x"),
        &[("Accept", "application/json")],
        None,
    )
    .await;
    assert_eq!(api.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uploads_stream_in_chunks_and_route_by_node_query() {
    let hub = Hub::new().await;
    let a = scoped(NID_A, "claude:same-file-hash");
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let chunks: Vec<Bytes> = payload.chunks(7_000).map(Bytes::copy_from_slice).collect();
    let body = Body::from_stream(futures_util::stream::iter(
        chunks.into_iter().map(Ok::<_, std::io::Error>),
    ));
    let request = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/session/attachment?uid={a}&name=same-name.png"
        ))
        .header("Host", "127.0.0.1")
        .header("Content-Type", "image/png")
        .header("Content-Length", payload.len())
        .body(body)
        .unwrap();
    let (status, answer) = json_of(hub.router.clone().oneshot(request).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["name"], "same-name.png");
    assert_eq!(answer["size"], payload.len());
    assert_eq!(
        answer["media"]["src"],
        format!("/api/nodes/{NID_A}/api/media/{}", "d".repeat(32))
    );
    let uploads = hub.a.state()["uploads"].clone();
    let last = uploads.as_array().unwrap().last().unwrap().clone();
    assert_eq!(last[0]["uid"], json!(["claude:same-file-hash"]));
    assert_eq!(last[1], payload.len());
    // Bug-report uploads name the machine with ?node= and no session.
    let request = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/session/attachment?uid=bug-report&node={NID_B}&name=%E6%88%AA%E5%9B%BE.png"
        ))
        .header("Host", "127.0.0.1")
        .header("Content-Type", "image/png")
        .header("Content-Length", 4)
        .body(Body::from(&b"\x89PNG"[..]))
        .unwrap();
    let (status, answer) = json_of(hub.router.clone().oneshot(request).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["name"], "截图.png");
    let uploads = hub.b.state()["uploads"].clone();
    let last = uploads.as_array().unwrap().last().unwrap().clone();
    assert_eq!(last[0]["uid"], json!(["bug-report"]));
    assert!(last[0].get("node").is_none());
    assert_eq!(last[1], 4);
    // No node and no scoped uid: refused, never guessed.
    let request = Request::builder()
        .method("POST")
        .uri("/api/session/attachment?uid=bug-report&name=x.png")
        .header("Host", "127.0.0.1")
        .header("Content-Length", 1)
        .body(Body::from("x"))
        .unwrap();
    let (status, _) = json_of(hub.router.clone().oneshot(request).await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // A declared size beyond the attachment limit is refused before reading.
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/session/attachment?uid={a}&name=big.bin"))
        .header("Host", "127.0.0.1")
        .header("Content-Length", proxy::ATTACHMENT_MAX_BYTES + 1)
        .body(Body::from("x"))
        .unwrap();
    let (status, answer) = json_of(hub.router.clone().oneshot(request).await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(answer["error"], "invalid attachment size");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn display_and_order_are_the_users_and_audited() {
    let hub = Hub::new().await;
    let (status, body) = hub
        .post(
            &format!("/api/nodes/{NID_A}/display"),
            json!({"name": "机房 A", "color": "teal"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["node"],
        json!({"id": NID_A, "name": "机房 A", "color": "teal", "enabled": true})
    );
    let (_, listing) = hub.get("/api/nodes").await;
    assert_eq!(
        (
            listing["nodes"][0]["name"].as_str().unwrap(),
            listing["nodes"][0]["color"].as_str().unwrap()
        ),
        ("机房 A", "teal")
    );
    assert_eq!(
        hub.post(
            &format!("/api/nodes/{NID_A}/display"),
            json!({"color": "rose"})
        )
        .await
        .1["node"]["name"],
        "机房 A"
    );
    assert_eq!(
        hub.post(&format!("/api/nodes/{NID_A}/display"), json!({"color": ""}))
            .await
            .1["node"]["color"],
        ""
    );
    for (bad, hint) in [
        (json!({"name": ""}), "不能为空"),
        (json!({"name": "x".repeat(81)}), "80"),
        (json!({"color": "#ff0000"}), "机器颜色"),
        (json!({"name": "NodeB"}), "已有机器"),
        (json!({"enabled": "yes"}), "enabled"),
    ] {
        let (status, body) = hub
            .post(&format!("/api/nodes/{NID_A}/display"), bad.clone())
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        assert!(
            body["error"].as_str().unwrap().contains(hint),
            "{bad}: {body}"
        );
    }
    // Address and credential cannot change through the page.
    hub.post(
        &format!("/api/nodes/{NID_A}/display"),
        json!({"name": "机房 A", "url": "http://127.0.0.1:1", "token": "x".repeat(40)}),
    )
    .await;
    let stored = hub.registry.find(NID_A).unwrap();
    assert_eq!(stored.url, format!("http://127.0.0.1:{}", hub.a.port));
    assert_eq!(stored.token, hub.a.token);
    assert_eq!(
        hub.post(&format!("/api/nodes/{NID_C}/display"), json!({"name": "x"}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    // Unticking a machine: gone from the public list, kept in machines.
    let (status, body) = hub
        .post(
            &format!("/api/nodes/{NID_B}/display"),
            json!({"enabled": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["node"]["enabled"], false);
    let (_, listing) = hub.get("/api/nodes").await;
    assert_eq!(listing["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(listing["machines"][1]["enabled"], false);
    assert_eq!(listing["machines"][1]["online"], Value::Null);
    assert_eq!(
        hub.get(&format!("/api/nodes/{NID_B}/api/live")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        hub.get(&format!("/api/sessions?nodes={NID_B}")).await.0,
        StatusCode::BAD_REQUEST
    );
    let (status, _) = hub
        .post(
            &format!("/api/nodes/{NID_B}/display"),
            json!({"enabled": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // Order: a permutation of every machine, persisted, else 400.
    let (status, body) = hub
        .post("/api/nodes/order", json!({"ids": [NID_B, NID_A]}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids: Vec<&str> = body["machines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [NID_B, NID_A]);
    let (_, listing) = hub.get("/api/nodes").await;
    assert_eq!(listing["nodes"][0]["id"], NID_B);
    for bad in [
        json!([NID_A]),
        json!([NID_A, NID_B, NID_A]),
        json!([NID_A, NID_C]),
        json!("ab"),
        Value::Null,
    ] {
        let (status, _) = hub.post("/api/nodes/order", json!({"ids": bad})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let fresh = Registry::open(
        &hub.dir.path().join("hub-nodes.json"),
        parse_networks("127.0.0.0/8").unwrap(),
        &hub.dir.path().join("hub-cache"),
    )
    .unwrap();
    let ids: Vec<Value> = fresh.machines().iter().map(|m| m["id"].clone()).collect();
    assert_eq!(ids, [json!(NID_B), json!(NID_A)]);
    // The audit directory holds one record per effective change.
    wait_for(|| hub.audit_events().len() >= 6).await;
    let events = hub.audit_events();
    let display: Vec<&Value> = events
        .iter()
        .filter(|e| e["event"] == "hub.node.display.changed")
        .collect();
    assert_eq!(display[0]["category"], "terminal");
    assert_eq!(display[0]["data"]["node_id"], NID_A);
    assert_eq!(display[0]["data"]["from"]["name"], "NodeA");
    assert_eq!(
        display[0]["data"]["to"],
        json!({"name": "机房 A", "color": "teal", "enabled": true})
    );
    let order: Vec<&Value> = events
        .iter()
        .filter(|e| e["event"] == "hub.node.order.changed")
        .collect();
    assert_eq!(order.len(), 1);
    assert_eq!(order[0]["data"]["to"], json!([NID_B, NID_A]));
    assert!(events.iter().all(|e| e["ts"].as_str().is_some()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bulk_writes_trash_and_audit_route_by_machine() {
    let hub = Hub::new().await;
    let uids = [
        scoped(NID_A, "claude:same-file-hash"),
        scoped(NID_B, "claude:same-file-hash"),
    ];
    let (status, data) = hub
        .post("/api/sessions/delete", json!({"uids": uids}))
        .await;
    assert_eq!(status, StatusCode::OK, "{data}");
    let deleted: Vec<&str> = data["deleted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uid"].as_str().unwrap())
        .collect();
    assert_eq!(deleted, uids.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(data["errors"], json!([]));
    hub.a.set(json!({"deleted": false}));
    hub.b.set(json!({"deleted": false}));
    let (status, _) = hub.post("/api/sessions/delete", json!({"uids": []})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, trash) = hub.get("/api/trash").await;
    assert_eq!(trash["items"].as_array().unwrap().len(), 2);
    assert_eq!(trash["dir"], "所选机器的本地回收站");
    let item = trash["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["node_name"] == "NodeB")
        .unwrap();
    let (status, _) = hub
        .post("/api/trash/restore", json!({"id": item["id"]}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hub.b.writes().last().unwrap().1["id"], "claude/same-trash");
    let (status, purge) = hub
        .post(
            &format!("/api/trash/purge?nodes={NID_A}"),
            json!({"all": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{purge}");
    assert_eq!(purge["removed"], 1);
    let (status, data) = hub.post("/api/sessions/fork-visibility", json!({"uids": [scoped(NID_A, "codex:parent"), scoped(NID_B, "codex:parent")], "visible": true})).await;
    assert_eq!(status, StatusCode::OK, "{data}");
    assert_eq!(data["updated"].as_array().unwrap().len(), 2);
    assert_eq!(
        hub.a.writes().last().unwrap(),
        &(
            "/api/sessions/fork-visibility".to_string(),
            json!({"uids": ["codex:parent"], "visible": true})
        )
    );
    // Browser audit: by scoped uid, then a pending uid follows the page's machine.
    let page_id = "page-audit-pending-uid";
    let uid = scoped(NID_A, "claude:same-file-hash");
    let (status, _) = hub.post("/api/audit/browser", json!({"page_id": page_id, "uid": uid, "events": [{"event": "page.loaded", "uid": uid}]})).await;
    assert_eq!(status, StatusCode::OK);
    let before = hub
        .a
        .writes()
        .iter()
        .filter(|w| w.0 == "/api/audit/browser")
        .count();
    assert!(before >= 1);
    let (status, _) = hub.post("/api/audit/browser", json!({"page_id": page_id, "uid": "pending:xxx", "events": [{"event": "dom.snapshot", "uid": "pending:xxx"}]})).await;
    assert_eq!(status, StatusCode::OK);
    let writes: Vec<_> = hub
        .a
        .writes()
        .into_iter()
        .filter(|w| w.0 == "/api/audit/browser")
        .collect();
    assert_eq!(writes.len(), before + 1);
    assert_eq!(
        writes.last().unwrap().1["events"][0]["event"],
        "dom.snapshot"
    );
    assert_eq!(writes.last().unwrap().1["events"][0]["uid"], "");
    // Aggregates: identity across machines, filter, sig.
    let (_, sessions) = hub.get("/api/sessions?force=1").await;
    let ids: Vec<&str> = sessions["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["node_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    let sig = sessions["sig"].as_str().unwrap();
    let (_, unchanged) = hub.get(&format!("/api/sessions?sig={sig}")).await;
    assert_eq!(unchanged["unchanged"], true);
    let (_, filtered) = hub
        .get(&format!("/api/sessions?force=1&nodes={NID_B}"))
        .await;
    assert_eq!(filtered["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        hub.get(&format!("/api/sessions?nodes={NID_C}")).await.0,
        StatusCode::BAD_REQUEST
    );
    // The NDJSON search stream: progress first, matches, then the result.
    let search = request(
        &hub.router,
        "GET",
        "/api/search?q=needle&progress=1",
        &[],
        None,
    )
    .await;
    assert_eq!(search.status(), StatusCode::OK);
    assert!(
        search.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/x-ndjson")
    );
    let text = String::from_utf8(
        to_bytes(search.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines[0]["type"], "progress");
    assert_eq!(lines.last().unwrap()["type"], "result");
    let results = lines.last().unwrap()["data"]["results"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(results.len(), 2);
    assert!(lines.iter().any(|line| line["type"] == "matches"));
    let (_, plain) = hub.get("/api/search?q=needle").await;
    assert_eq!(plain["results"].as_array().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_is_copied_raw_after_the_101() {
    let hub = Hub::new().await;
    let port = hub.listen().await;
    let url = format!("ws://127.0.0.1:{port}/api/term/attach?name={NID_A}~same-terminal");
    let (mut socket, response) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(url),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    // The node greets with its name, then echoes every frame.
    let first = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first, Message::Binary(Bytes::from_static(b"NodeA")));
    socket
        .send(Message::Text("terminal-echo".into()))
        .await
        .unwrap();
    let echoed = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(echoed, Message::Text("terminal-echo".into()));
    socket
        .send(Message::Binary(Bytes::from_static(&[0, 255, 17])))
        .await
        .unwrap();
    let echoed = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(echoed, Message::Binary(Bytes::from_static(&[0, 255, 17])));
    socket.close(None).await.unwrap();
    wait_for(|| hub.a.state()["frames"].as_array().map(Vec::len) == Some(2)).await;
    // Over the wire, the hub's own gate still applies.
    let rejected = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/api/term/attach?name=nope"
    ))
    .await;
    assert!(rejected.is_err());
}

/// `SESSIONDOCK_PUBLIC_HOSTS` opens the hub gate to the exact forwarded
/// authorities a reverse proxy sends, exactly like the node gate; anything else
/// stays `local_only`.
#[tokio::test]
async fn public_hosts_pass_the_hub_gate_like_the_node_gate() {
    let dir = tempfile::tempdir().unwrap();
    let config = HubConfig {
        nodes_file: dir.path().join("hub-nodes.json"),
        cache_dir: dir.path().join("hub-cache"),
        web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
        public_hosts: vec!["203.0.113.177".into(), "hub.lan:8443".into()],
        ..HubConfig::default()
    };
    let shutdown = tokio_util::sync::CancellationToken::new();
    let HubApp { router, .. } = hub_api::hub_app(&config, shutdown.clone()).unwrap();
    for (host, status) in [
        ("203.0.113.177", StatusCode::OK),
        ("HUB.LAN:8443", StatusCode::OK),
        ("127.0.0.1:8742", StatusCode::OK),
        ("203.0.113.177:8443", StatusCode::FORBIDDEN),
        ("hub.lan", StatusCode::FORBIDDEN),
        ("example.lan", StatusCode::FORBIDDEN),
    ] {
        let response = request(&router, "GET", "/api/meta", &[("Host", host)], None).await;
        assert_eq!(response.status(), status, "Host {host}");
    }
    shutdown.cancel();
}
