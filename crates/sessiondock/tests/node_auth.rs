//! Node listener gate against the in-process routers: peer,
//! protocol and token are each required; the loopback router keeps refusing
//! hub headers; `/api/meta` and `/api/nodes` report the configured identity.
//! The `Config` fields are the ones `SESSIONDOCK_NODE_BIND`,
//! `SESSIONDOCK_NODE_TOKEN_FILE`, `SESSIONDOCK_NODE_ID_FILE` and
//! `SESSIONDOCK_NODE_PEERS` fill (`tests/node_auth_suite.py` drives the binary).
//! Synthetic Claude corpus in a private temporary directory; no network.

#![cfg(unix)]
use std::{
    fs,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{HeaderName, Request, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use sessiondock::{config::Config, sessions::SessionRoots};
use tower::ServiceExt;

const TOKEN: &str = "node-t0ken.node-t0ken.node-t0ken.node-t0ken~";

fn corpus(temp: &Path) -> Config {
    let root = temp.join("claude");
    fs::create_dir_all(root.join("project")).unwrap();
    let records = [
        json!({"type":"user", "uuid":"u1", "parentUuid":null, "sessionId":"synthetic",
            "cwd":"/example/project", "timestamp":"2026-01-01T00:00:00.000Z",
            "message":{"role":"user", "content":"节点鉴权测试"}}),
        json!({"type":"assistant", "uuid":"a1", "parentUuid":"u1", "sessionId":"synthetic",
            "timestamp":"2026-01-01T00:00:01.000Z", "message":{"role":"assistant",
                "content":[{"type":"text", "text":"人工合成回答"}]}}),
    ];
    fs::write(
        root.join("project/synthetic.jsonl"),
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    Config {
        web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
        roots: SessionRoots {
            claude: Some(root),
            ..Default::default()
        },
        // The default is the system host name; pin the
        // Python-shaped node name this test asserts.
        hostname: "SessionDock".into(),
        ..Config::default()
    }
}

fn node_config(temp: &Path, peers: &str) -> Config {
    let token = temp.join("node-token");
    fs::write(&token, format!("{TOKEN}\n")).unwrap();
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    Config {
        node_bind: Some("127.0.0.1:0".parse().unwrap()),
        node_token_file: Some(token),
        node_id_file: Some(temp.join("node-id")),
        node_peers: peers.split(',').map(|s| s.parse().unwrap()).collect(),
        ..corpus(temp)
    }
}

fn request(uri: &str, peer: Option<&str>, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder()
        .uri(uri)
        .header("Host", "10.100.100.2:8742");
    for (name, value) in headers {
        builder = builder.header(HeaderName::from_bytes(name.as_bytes()).unwrap(), *value);
    }
    let mut request = builder.body(Body::empty()).unwrap();
    if let Some(peer) = peer {
        let address: SocketAddr = format!("{peer}:40000").parse().unwrap();
        request.extensions_mut().insert(ConnectInfo(address));
    }
    request
}

async fn send(router: &Router, request: Request<Body>) -> Response {
    router.clone().oneshot(request).await.unwrap()
}

async fn body(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn good() -> [(&'static str, &'static str); 2] {
    [
        ("x-sessiondock-protocol", "1"),
        ("x-sessiondock-node-token", TOKEN),
    ]
}

#[tokio::test]
async fn without_identity_there_is_no_node_router_and_meta_stays_protocol_zero() {
    let temp = tempfile::tempdir().unwrap();
    let (loopback, node) = sessiondock::app_pair(corpus(temp.path())).unwrap();
    assert!(node.is_none());
    let meta = body(send(&loopback, request_loopback("/api/meta", &[])).await).await;
    assert_eq!(meta["protocol"], 0);
    assert_eq!(meta["node_id"], Value::Null);
    assert_eq!(meta["capabilities"]["hub"], false);
    let nodes = body(send(&loopback, request_loopback("/api/nodes", &[])).await).await;
    assert_eq!(nodes["nodes"][0]["id"], "rs-local");
    assert!(!temp.path().join("node-id").exists());
}

#[tokio::test]
async fn node_router_requires_peer_protocol_and_token_each() {
    let temp = tempfile::tempdir().unwrap();
    let (loopback, node) =
        sessiondock::app_pair(node_config(temp.path(), "10.100.100.0/24,127.0.0.0/8")).unwrap();
    let node = node.expect("node router with the identity configured");
    let node_id = fs::read_to_string(temp.path().join("node-id")).unwrap();
    let node_id = node_id.trim();
    assert_eq!(node_id.len(), 32, "{node_id}");
    assert_eq!(
        fs::metadata(temp.path().join("node-id"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    // Peer first: outside the networks (or unknown) never reaches the credential.
    for peer in [None, Some("10.100.101.9"), Some("192.168.2.5")] {
        let response = send(&node, request("/api/sessions", peer, &good())).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{peer:?}");
        let error = body(response).await;
        assert_eq!(
            error,
            json!({"error": "forbidden", "code": "node_peer_denied"}),
            "{peer:?}"
        );
    }
    // Then the two headers, both exact.
    let wrong_length = format!("{TOKEN}x");
    let same_length = TOKEN.replace('~', "-");
    let cases: [&[(&str, &str)]; 6] = [
        &[],
        &[("x-sessiondock-protocol", "1")],
        &[("x-sessiondock-node-token", TOKEN)],
        &[
            ("x-sessiondock-protocol", "2"),
            ("x-sessiondock-node-token", TOKEN),
        ],
        &[
            ("x-sessiondock-protocol", "1"),
            ("x-sessiondock-node-token", &wrong_length),
        ],
        &[
            ("x-sessiondock-protocol", "1"),
            ("x-sessiondock-node-token", &same_length),
        ],
    ];
    for (index, case) in cases.iter().enumerate() {
        let response = send(&node, request("/api/sessions", Some("10.100.100.2"), case)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "case {index}");
        assert_eq!(
            body(response).await,
            json!({"error": "node authentication required", "code": "node_auth_required"}),
            "case {index}"
        );
    }
    // All three: the same listing the loopback router serves.
    let response = send(
        &node,
        request("/api/sessions", Some("10.100.100.2"), &good()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let through_node = body(response).await;
    let local = body(send(&loopback, request_loopback("/api/sessions", &[])).await).await;
    assert_eq!(through_node["sessions"], local["sessions"]);
    assert_eq!(through_node["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(through_node["sig"], local["sig"]);

    // Identity on both listeners; the node is not a hub.
    for (name, router, req) in [
        (
            "node",
            &node,
            request("/api/meta", Some("10.100.100.2"), &good()),
        ),
        ("loopback", &loopback, request_loopback("/api/meta", &[])),
    ] {
        let meta = body(send(router, req).await).await;
        assert_eq!(meta["protocol"], 1, "{name}");
        assert_eq!(meta["node_id"], node_id, "{name}");
        assert_eq!(meta["mode"], "local", "{name}");
        assert_eq!(meta["capabilities"]["hub"], false, "{name}");
    }
    let nodes = body(send(&loopback, request_loopback("/api/nodes", &[])).await).await;
    assert_eq!(nodes["mode"], "local");
    assert_eq!(nodes["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(nodes["nodes"][0]["id"], node_id);
    assert_eq!(nodes["nodes"][0]["online"], true);
    assert_eq!(nodes["nodes"][0]["name"], "SessionDock");

    // No static page on the node listener, even with the credential.
    for path in ["/", "/index.html", "/app.js"] {
        let response = send(&node, request(path, Some("10.100.100.2"), &good())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(body(response).await["code"], "not_found", "{path}");
    }
    // Everything else of the shared policy still applies.
    let mut headers = good().to_vec();
    headers.push(("sec-fetch-site", "cross-site"));
    let response = send(
        &node,
        request("/api/sessions", Some("10.100.100.2"), &headers),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(body(response).await["code"], "cross_site");
    // `debug_run` is the list-view selector here too: an unknown run is
    // an empty view, never a refusal.
    let response = send(
        &node,
        request("/api/sessions?debug_run=x", Some("10.100.100.2"), &good()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await["sessions"], serde_json::json!([]));

    // The loopback listener keeps refusing hub headers, right token included.
    for headers in [
        &[("x-sessiondock-protocol", "1")][..],
        &[("x-sessiondock-node-token", TOKEN)][..],
        &good()[..],
    ] {
        let response = send(&loopback, request_loopback("/api/sessions", headers)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(body(response).await["code"], "hub_unsupported");
    }
    // A second start reuses the minted id.
    drop((loopback, node));
    let (loopback, _node) = sessiondock::app_pair(node_config(temp.path(), "127.0.0.0/8")).unwrap();
    let meta = body(send(&loopback, request_loopback("/api/meta", &[])).await).await;
    assert_eq!(meta["node_id"], node_id);
}

fn request_loopback(uri: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).header("Host", "127.0.0.1:8741");
    for (name, value) in headers {
        builder = builder.header(HeaderName::from_bytes(name.as_bytes()).unwrap(), *value);
    }
    let mut request = builder.body(Body::empty()).unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:40001".parse::<SocketAddr>().unwrap(),
    ));
    request
}

#[tokio::test]
async fn a_partial_node_configuration_never_builds_an_app() {
    let temp = tempfile::tempdir().unwrap();
    let full = node_config(temp.path(), "127.0.0.0/8");
    for missing in 0..4 {
        let mut config = Config {
            node_bind: full.node_bind,
            node_token_file: full.node_token_file.clone(),
            node_id_file: full.node_id_file.clone(),
            node_peers: full.node_peers.clone(),
            ..corpus(temp.path())
        };
        match missing {
            0 => config.node_bind = None,
            1 => config.node_token_file = None,
            2 => config.node_id_file = None,
            _ => config.node_peers.clear(),
        }
        let error = sessiondock::app_pair(config).unwrap_err();
        assert!(
            error.to_string().contains("set together"),
            "{missing}: {error}"
        );
    }
    assert!(!temp.path().join("node-id").exists());
}
