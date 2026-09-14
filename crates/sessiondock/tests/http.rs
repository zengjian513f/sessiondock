use std::{fs, io::Write, path::PathBuf, time::Duration};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sessiondock::{config::Config, sessions::SessionRoots};
use tower::ServiceExt;

fn config() -> Config {
    Config {
        web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
        ..Config::default()
    }
}

fn request(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("Host", "127.0.0.1:8741")
        .body(Body::empty())
        .unwrap()
}

async fn get(app: &Router, uri: &str) -> Response {
    app.clone().oneshot(request(uri)).await.unwrap()
}

async fn json_body(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn fixture() -> (tempfile::TempDir, Config, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("claude");
    fs::create_dir_all(root.join("project")).unwrap();
    let path = root.join("project/synthetic.jsonl");
    let records = [
        json!({"type":"user", "uuid":"u1", "parentUuid":null, "sessionId":"synthetic",
            "cwd":"/example/project", "timestamp":"2026-01-01T00:00:00.000Z",
            "message":{"role":"user", "content":"迁移测试问题"}}),
        json!({"type":"assistant", "uuid":"a1", "parentUuid":"u1", "sessionId":"synthetic",
            "timestamp":"2026-01-01T00:00:01.000Z", "message":{"role":"assistant",
                "content":[{"type":"text", "text":"人工合成回答"}]}}),
    ];
    fs::write(
        &path,
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    let mut cfg = config();
    cfg.roots = SessionRoots {
        claude: Some(root),
        ..Default::default()
    };
    (temp, cfg, path)
}

async fn first_uid(app: &Router) -> String {
    json_body(get(app, "/api/sessions").await).await["sessions"][0]["uid"]
        .as_str()
        .unwrap()
        .into()
}

async fn frame(body: &mut Body) -> String {
    let bytes = tokio::time::timeout(Duration::from_secs(3), body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_data()
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn health_meta_and_default_empty_sources_are_honest() {
    let app = sessiondock::app(config()).unwrap();
    let response = get(&app, "/api/health").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["service"], "sessiondock");
    let meta = json_body(get(&app, "/api/meta").await).await;
    assert_eq!(meta["protocol"], 0); // Must not pass the old Hub registration handshake.
    assert_eq!(meta["capabilities"]["read_only"], false);
    assert_eq!(meta["capabilities"]["stage"], "replacement");
    assert!(meta.get("migration").is_none(), "{meta}");
    assert_eq!(meta["capabilities"]["outbox"], false);
    let sessions = json_body(get(&app, "/api/sessions").await).await;
    assert_eq!(sessions["sessions"], json!([]));
    let unchanged = json_body(
        get(
            &app,
            &format!("/api/sessions?sig={}", sessions["sig"].as_str().unwrap()),
        )
        .await,
    )
    .await;
    assert_eq!(unchanged["unchanged"], true);
    assert!(unchanged.get("sessions").is_none());
    assert_eq!(
        json_body(get(&app, "/api/live").await).await["known"],
        cfg!(target_os = "linux")
    );
    let term = json_body(get(&app, "/api/term/list").await).await;
    assert_eq!(term["enabled"], false);
    assert!(
        term["unavailable_reason"]
            .as_str()
            .unwrap()
            .contains("只读")
    );
}

#[tokio::test]
async fn static_snapshot_replaces_templates_and_prevents_traversal() {
    let mut cfg = config();
    cfg.hostname = "<script>not executable</script>".into();
    let app = sessiondock::app(cfg).unwrap();
    let meta = json_body(get(&app, "/api/meta").await).await;
    for uri in ["/", "/index.html", "/files.html", "/file.html"] {
        let response = get(&app, uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let html = String::from_utf8(
            to_bytes(response.into_body(), 100_000)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!html.contains("__SESSIONDOCK_"));
        assert!(!html.contains("<script>not executable</script>"));
        assert!(html.contains(meta["build"].as_str().unwrap()));
        assert!(html.contains("sessiondock-capabilities"));
        assert!(
            html.find("sessiondock-capabilities").unwrap()
                < html.find("<script>").unwrap_or(html.len())
        );
    }
    for uri in [
        "/../Cargo.toml",
        "/%2e%2e/Cargo.toml",
        "/reference/legacy-web/app.js",
        "/AGENTS.md",
    ] {
        assert_eq!(get(&app, uri).await.status(), StatusCode::NOT_FOUND);
    }
    let js = get(&app, "/capabilities.js").await;
    assert_eq!(js.status(), StatusCode::OK);
    let mut req = request("/capabilities.js");
    req.headers_mut()
        .insert("if-none-match", js.headers()["etag"].clone());
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::NOT_MODIFIED
    );
    let redirect = get(&app, "/files.html?open=1&ref=example").await;
    assert_eq!(redirect.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        redirect.headers()["location"],
        "file.html?open=1&ref=example"
    );
}

#[tokio::test]
async fn unsupported_and_unknown_routes_are_not_fake_success() {
    let app = sessiondock::app(config()).unwrap();
    for (path, code) in [
        ("/api/session/send", "delivery_send_disabled"),
        ("/api/audit/browser", "not_implemented"),
        ("/api/session/star", "metadata_disabled"),
        ("/api/term/create", "not_implemented"),
    ] {
        let mut req = request(path);
        *req.method_mut() = axum::http::Method::POST;
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED, "{path}");
        assert_eq!(json_body(response).await["code"], code);
    }
    let response = get(&app, "/api/does-not-exist").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(response).await["code"], "not_found");
    assert_eq!(
        get(&app, "/api/session/outbox?uid=test").await.status(),
        StatusCode::NOT_IMPLEMENTED
    );
}

#[tokio::test]
async fn request_security_rejects_rebinding_cross_site_hub_bypass_and_oversize() {
    let app = sessiondock::app(config()).unwrap();
    for (name, value) in [
        ("host", "attacker.example"),
        ("origin", "https://attacker.example"),
        ("origin", "null"),
        ("sec-fetch-site", "cross-site"),
        ("x-sessiondock-protocol", "1"),
        ("x-sessiondock-node-token", "not-a-real-token"),
    ] {
        let mut req = request("/api/meta");
        req.headers_mut().insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::FORBIDDEN,
            "{name}"
        );
    }
    let mut req = request("/api/meta");
    req.headers_mut()
        .insert("origin", "http://127.0.0.1:8741".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );
    let req = Request::builder()
        .method("POST")
        .uri("/api/session/resolve-files")
        .header("Host", "127.0.0.1:8741")
        .header("Content-Type", "application/json")
        .body(Body::from(" ".repeat(4 * 1024 * 1024 + 1)))
        .unwrap();
    assert_eq!(
        app.oneshot(req).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn native_input_is_parsed_and_bad_queries_are_json_errors() {
    let (_temp, cfg, _) = fixture();
    let app = sessiondock::app(cfg).unwrap();
    let uid = first_uid(&app).await;
    let batch = json_body(get(&app, &format!("/api/messages/{uid}?window=1")).await).await;
    assert_eq!(batch["reset"], true);
    assert_eq!(batch["messages"].as_array().unwrap().len(), 2);
    assert_eq!(batch["messages"][0]["text"], "迁移测试问题");
    assert!(batch.get("outbox").is_none());
    // Python sets `prompt` on every main view (null without a live card).
    assert!(batch["prompt"].is_null());
    let history =
        json_body(get(&app, &format!("/api/session/input-history?uid={uid}")).await).await;
    assert_eq!(history["history"][0]["text"], "迁移测试问题");
    for suffix in ["start=-1", "start=nope", "window=true", "append=2"] {
        let response = get(&app, &format!("/api/messages/{uid}?{suffix}")).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(json_body(response).await["error"].is_string());
    }
    assert_eq!(
        get(&app, "/api/messages/claude:missing").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, &format!("/api/messages/{uid}?agent=..%2fsecret"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    // `debug_run` is a list-view selector (Python `filter_rows`), not a
    // gate: the detail routes ignore it, and the list answers an empty
    // view for an id no registry knows.
    assert_eq!(
        get(&app, &format!("/api/messages/{uid}?debug_run=unknown"))
            .await
            .status(),
        StatusCode::OK
    );
    let response = get(&app, "/api/sessions?debug_run=unknown").await;
    assert_eq!(response.status(), StatusCode::OK);
    let listed = json_body(response).await;
    assert_eq!(listed["sessions"], json!([]));
    assert!(listed["sig"].is_string());
}

#[tokio::test]
async fn sse_advances_metadata_only_cursor_and_closes_on_shutdown() {
    let (_temp, cfg, path) = fixture();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let app = sessiondock::app_with_shutdown(cfg, shutdown.clone()).unwrap();
    let uid = first_uid(&app).await;
    let response = get(&app, &format!("/api/watch?uid={uid}&start=0")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut body = response.into_body();
    let initial = frame(&mut body).await;
    assert!(initial.contains("人工合成回答"));
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file).unwrap();
    let watermark = frame(&mut body).await;
    let value: Value =
        serde_json::from_str(watermark.trim().strip_prefix("data: ").unwrap()).unwrap();
    assert_eq!(value["messages"], json!([]));
    assert!(value["end"].as_u64().unwrap() > value["start"].as_u64().unwrap());
    shutdown.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), body.frame())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn sse_publishes_changed_agent_menu_even_when_leaf_cursor_is_unchanged() {
    let (_temp, cfg, path) = fixture();
    let app = sessiondock::app(cfg).unwrap();
    let uid = first_uid(&app).await;
    let mut body = get(&app, &format!("/api/watch?uid={uid}"))
        .await
        .into_body();
    let initial = frame(&mut body).await;
    let before: Value =
        serde_json::from_str(initial.trim().strip_prefix("data: ").unwrap()).unwrap();
    let agent = path
        .parent()
        .unwrap()
        .join("synthetic/subagents/agent-later.jsonl");
    fs::create_dir_all(agent.parent().unwrap()).unwrap();
    fs::write(
        &agent,
        format!(
            "{}\n",
            json!({"type":"assistant","uuid":"agent-a",
        "parentUuid":null,"isSidechain":true,"agentId":"later",
        "timestamp":"2026-01-01T00:00:02Z","message":{"content":"New isolated agent"}})
        ),
    )
    .unwrap();
    // The publisher reuses a list up to 3 s old (`OPEN_TTL`)
    // instead of walking the roots every 500 ms, so a new agent file shows
    // up within ~3.5 s rather than ~1 s.
    let packet = tokio::time::timeout(Duration::from_secs(8), body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_data()
        .unwrap();
    let packet = String::from_utf8(packet.to_vec()).unwrap();
    let after: Value = serde_json::from_str(packet.trim().strip_prefix("data: ").unwrap()).unwrap();
    assert_eq!(after["reset"], false);
    assert_eq!(after["messages"], json!([]));
    assert_eq!(after["anchor"], before["anchor"]);
    assert_eq!(after["end"], before["end"]);
    assert!(
        after["meta"]["agent_items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == "later")
    );
}

#[tokio::test]
async fn watch_subscribers_are_not_rejected_at_the_former_limit() {
    let (_temp, cfg, _) = fixture();
    let app = sessiondock::app(cfg).unwrap();
    let uid = first_uid(&app).await;
    let uri = format!("/api/watch?uid={uid}");
    let mut bodies = Vec::new();
    for _ in 0..32 {
        let response = get(&app, &uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        bodies.push(response.into_body());
    }
    assert_eq!(get(&app, &uri).await.status(), StatusCode::OK);
    bodies.pop();
    assert_eq!(get(&app, &uri).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn sse_branch_switch_resets_each_subscriber_with_its_own_cursor() {
    let (_temp, cfg, path) = fixture();
    let app = sessiondock::app(cfg).unwrap();
    let uid = first_uid(&app).await;
    let before = json_body(get(&app, &format!("/api/messages/{uid}")).await).await;
    let uri = format!(
        "/api/watch?uid={uid}&start={}&head={}&anchor={}",
        before["end"],
        before["version"]["head"].as_str().unwrap(),
        before["anchor"].as_str().unwrap()
    );
    let mut current = get(&app, &uri).await.into_body();
    let aligned = frame(&mut current).await;
    let aligned: Value =
        serde_json::from_str(aligned.trim().strip_prefix("data: ").unwrap()).unwrap();
    assert_eq!(aligned["reset"], false);
    assert_eq!(aligned["messages"], json!([]));
    let mut fresh = get(&app, &format!("/api/watch?uid={uid}"))
        .await
        .into_body();
    let first = frame(&mut fresh).await;
    assert!(first.contains("人工合成回答"));
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file, "{}", json!({"type":"last-prompt","leafUuid":"u1"})).unwrap();
    for body in [&mut current, &mut fresh] {
        let packet = frame(body).await;
        let value: Value =
            serde_json::from_str(packet.trim().strip_prefix("data: ").unwrap()).unwrap();
        assert_eq!(value["reset"], true);
        assert_eq!(value["messages"].as_array().unwrap().len(), 1);
        assert_eq!(value["messages"][0]["text"], "迁移测试问题");
    }
    let inputs = json_body(get(&app, &format!("/api/session/input-history?uid={uid}")).await).await;
    assert_eq!(inputs["history"].as_array().unwrap().len(), 1);
    assert_eq!(inputs["history"][0]["text"], "迁移测试问题");
}

#[tokio::test]
async fn subagent_http_view_has_independent_byte_cursor_and_stays_read_only() {
    let (_temp, cfg, path) = fixture();
    let agent = path
        .parent()
        .unwrap()
        .join("synthetic/subagents/agent-helper.jsonl");
    fs::create_dir_all(agent.parent().unwrap()).unwrap();
    fs::write(
        &agent,
        format!(
            "{}\n",
            json!({"type":"assistant","uuid":"agent-a",
        "parentUuid":null,"isSidechain":true,"agentId":"helper",
        "timestamp":"2026-01-01T00:00:02Z","message":{"content":"子代理人工回答"}})
        ),
    )
    .unwrap();
    let app = sessiondock::app(cfg).unwrap();
    let uid = first_uid(&app).await;
    let response = get(&app, &format!("/api/messages/{uid}?agent=helper")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let value = json_body(response).await;
    assert_eq!(value["messages"][0]["text"], "子代理人工回答");
    assert_eq!(value["end"], fs::metadata(&agent).unwrap().len());
    assert_ne!(
        get(
            &app,
            &format!("/api/messages/{uid}?agent=..%2F..%2Foutside")
        )
        .await
        .status(),
        StatusCode::OK
    );
    let watch = get(&app, &format!("/api/watch?uid={uid}&agent=helper")).await;
    assert_eq!(watch.status(), StatusCode::OK);
    let mut body = watch.into_body();
    assert!(frame(&mut body).await.contains("子代理人工回答"));
    let mut req = request("/api/session/rewind");
    *req.method_mut() = axum::http::Method::POST;
    assert_eq!(
        app.oneshot(req).await.unwrap().status(),
        StatusCode::NOT_IMPLEMENTED
    );
}
