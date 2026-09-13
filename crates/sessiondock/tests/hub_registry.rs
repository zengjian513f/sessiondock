//! Hub registry against `tests/hub_fake_node.py` (the port of the Python
//! project's `hub_fixture.NodeHandler`): the `test_hub.py` registry cases —
//! conditional `sig` probes, snapshot across a restart, strikes → offline,
//! recheck, streaming search, JSON cap — with the fake node driven through
//! its `/__control` endpoint. Nothing here touches a session root.

#![cfg(unix)]
use std::{
    io::{Read, Write},
    net::TcpStream,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use sessiondock::hub::{
    Client, JSON_LIMIT, Registry,
    registry::{Fetched, Node, OFFLINE_STRIKES, Registration, SearchEvent, parse_networks},
};
use tokio::sync::mpsc;

const NID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

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

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn registration(&self) -> Registration {
        Registration {
            name: self.name.clone(),
            url: self.url(),
            token: self.token.clone(),
            color: String::new(),
            id: None,
        }
    }

    /// Plain blocking HTTP/1.0 to the control endpoints, independent of the
    /// client under test.
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

    /// `(path, query)` of every `/api` GET the node has answered.
    fn gets(&self) -> Vec<(String, Value)> {
        self.state()["gets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| (entry[0].as_str().unwrap().to_string(), entry[1].clone()))
            .collect()
    }
}

impl Drop for FakeNode {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn client() -> Client {
    Client {
        timeout: Duration::from_millis(300),
        search_idle: Duration::from_millis(300),
        recheck: Duration::from_millis(300),
    }
}

fn open(dir: &Path) -> Registry {
    Registry::open(
        &dir.join("nodes.json"),
        parse_networks("127.0.0.0/8").unwrap(),
        &dir.join("hub-cache"),
    )
    .unwrap()
}

struct Hub {
    _dir: tempfile::TempDir,
    registry: Registry,
    client: Client,
    a: FakeNode,
    b: FakeNode,
}

impl Hub {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let registry = open(dir.path());
        let client = client();
        let a = FakeNode::start(NID_A, "NodeA");
        let b = FakeNode::start(NID_B, "NodeB");
        registry.register(&client, a.registration()).await.unwrap();
        registry.register(&client, b.registration()).await.unwrap();
        Hub {
            _dir: dir,
            registry,
            client,
            a,
            b,
        }
    }

    fn node(&self, nid: &str) -> Node {
        self.registry.get(nid).unwrap()
    }

    async fn fetch(&self, nid: &str, path: &str, query: &[(String, String)]) -> Fetched {
        self.registry
            .fetch(&self.client, &self.node(nid), path, query, None)
            .await
    }

    fn public(&self, nid: &str) -> Value {
        self.registry
            .public()
            .into_iter()
            .find(|row| row["id"] == nid)
            .unwrap()
    }
}

fn q(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[tokio::test]
async fn registration_uses_the_node_meta_and_public_rows_never_carry_credentials() {
    let hub = Hub::new().await;
    assert_eq!(
        hub.registry
            .all()
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        [NID_A, NID_B]
    );
    assert_eq!(
        hub.a
            .gets()
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        ["/api/meta"]
    );
    let public = serde_json::to_string(&hub.registry.public()).unwrap();
    assert!(
        !public.contains(&hub.a.token) && !public.contains("url") && !public.contains(&hub.a.url()),
        "{public}"
    );
    assert_eq!(
        hub.public(NID_A),
        json!({"id": NID_A, "name": "NodeA", "color": "", "online": null})
    );

    // The wrong credential is a 403 from the node: registration fails without changing the registry.
    let wrong = Registration {
        token: "w".repeat(40),
        ..hub.a.registration()
    };
    let error = hub.registry.register(&hub.client, wrong).await.unwrap_err();
    assert!(
        error.to_string().contains("节点认证或协议检查失败"),
        "{error}"
    );
    assert_eq!(hub.registry.get(NID_A).unwrap().token, hub.a.token);
}

#[tokio::test]
async fn session_polls_are_conditional_and_served_from_cache_when_unchanged() {
    let hub = Hub::new().await;
    hub.registry.check_all(&hub.client).await;
    let seen = hub.b.gets().len();
    hub.registry.check_all(&hub.client).await;
    let fetched = hub.fetch(NID_B, "/api/sessions", &[]).await;
    let probes = hub.b.gets()[seen..].to_vec();
    assert_eq!(
        probes
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        ["/api/sessions", "/api/sessions"]
    );
    assert!(
        probes.iter().all(|(_, query)| query["sig"][0]
            .as_str()
            .is_some_and(|sig| sig.starts_with("fixture-"))),
        "{probes:?}"
    );
    assert!(fetched.ok());
    let rows = fetched.data["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["uid"], "claude:same-file-hash");
    assert!(rows[0].get("stale").is_none());
    assert_eq!(hub.public(NID_B)["online"], true);

    // A forced refresh and a changed list both bypass the cache.
    hub.fetch(NID_B, "/api/sessions", &q(&[("force", "1")]))
        .await;
    assert!(hub.b.gets().last().unwrap().1.get("sig").is_none());
    hub.b.set(json!({"deleted": true}));
    let fetched = hub.fetch(NID_B, "/api/sessions", &[]).await;
    assert_eq!(fetched.data["sessions"], json!([]));
    assert!(hub.b.gets().last().unwrap().1.get("sig").is_some());
    hub.b.pop(&["deleted"]);
    let fetched = hub.fetch(NID_B, "/api/sessions", &[]).await;
    assert_eq!(fetched.data["sessions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn session_snapshot_survives_hub_restart_for_offline_machine() {
    let hub = Hub::new().await;
    hub.registry.check_all(&hub.client).await;
    let snapshot = hub.registry.snapshot_path(NID_B);
    assert!(snapshot.exists());
    assert_eq!(
        std::fs::metadata(&snapshot).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let fresh = open(hub._dir.path());
    let public = fresh
        .public()
        .into_iter()
        .find(|row| row["id"] == NID_B)
        .unwrap();
    assert_eq!(public["online"], Value::Null);
    assert!(public["last_seen"].is_number());
    let node = fresh.get(NID_B).unwrap();
    hub.b.set(json!({"offline": true}));
    fresh.check_all(&hub.client).await;
    for query in [q(&[]), q(&[("force", "1")])] {
        let fetched = fresh
            .fetch(&hub.client, &node, "/api/sessions", &query, None)
            .await;
        let failure = fetched.failure.unwrap();
        assert_eq!(failure["error_code"], "http_error");
        assert_eq!(failure["name"], "NodeB");
        let rows = fetched.data["sessions"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["uid"], "claude:same-file-hash");
        assert_eq!(rows[0]["stale"], true);
        assert_eq!(rows[0]["last_seen"], public["last_seen"]);
    }
    hub.b.pop(&["offline"]);
    fresh.check_all(&hub.client).await;
    assert_eq!(fresh.state(NID_B).unwrap().online, Some(true));
}

#[tokio::test]
async fn offline_node_is_skipped_by_fetch_and_recovers_through_recheck() {
    let hub = Hub::new().await;
    hub.registry.check_all(&hub.client).await;
    assert!(hub.fetch(NID_B, "/api/sessions", &[]).await.ok());
    // Every answer from B now takes longer than the client waits.
    hub.b.set(json!({"slow": 0.8}));
    let started = Instant::now();
    for _ in 0..OFFLINE_STRIKES {
        hub.registry.check_all(&hub.client).await;
    }
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(hub.registry.offline(NID_B));
    assert!(!hub.registry.offline(NID_A));

    // Page requests never wait for a node the monitor knows to be down.
    for path in ["/api/sessions", "/api/live", "/api/term/list", "/api/trash"] {
        let started = Instant::now();
        let fetched = hub.fetch(NID_B, path, &[]).await;
        assert!(started.elapsed() < Duration::from_millis(150), "{path}");
        let failure = fetched.failure.expect(path);
        assert_eq!(failure["name"], "NodeB");
        assert_eq!(failure["error_code"], "timeout");
        assert_eq!(failure["error"], "节点连接或响应超时（等待超过 0.3 秒）");
        assert!(failure["offline_since"].is_number());
        let public = hub.public(NID_B);
        assert_eq!(public["online"], false);
        assert!(
            public["offline_since"].is_number() && public["checked_at"].is_number(),
            "{public}"
        );
        assert!(!public.to_string().contains("private"));
    }
    let stale = hub.fetch(NID_B, "/api/sessions", &[]).await;
    assert_eq!(stale.data["sessions"][0]["stale"], true);
    let stale = hub
        .fetch(NID_B, "/api/sessions", &q(&[("force", "1")]))
        .await;
    assert_eq!(stale.data["sessions"][0]["stale"], true);
    let term = hub.fetch(NID_B, "/api/term/list", &[]).await;
    assert_eq!(term.data["enabled"], false);
    let started = Instant::now();
    let search = hub
        .fetch(NID_B, "/api/search", &q(&[("q", "needle")]))
        .await;
    assert!(started.elapsed() < Duration::from_millis(150));
    assert_eq!(search.data, json!({}));
    assert!(search.failure.is_some());
    // A healthy node is untouched by its neighbour's outage.
    let healthy = hub.fetch(NID_A, "/api/sessions", &[]).await;
    assert!(healthy.ok());
    assert!(healthy.data["sessions"][0].get("stale").is_none());

    // Explicit actions re-check once, then refuse with the known reason.
    let started = Instant::now();
    assert!(!hub.registry.recheck(&hub.client, &hub.node(NID_B)).await);
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(300) && elapsed < Duration::from_millis(1500),
        "{elapsed:?}"
    );
    assert_eq!(hub.public(NID_B)["online"], false);
    let health = hub.registry.state(NID_B).unwrap();
    assert_eq!(health.failed_path.as_deref(), Some("/api/live"));
    assert_eq!(health.error_code.as_deref(), Some("timeout"));

    // The machine is back: the re-check succeeds at once, the next pass clears the offline cache.
    hub.b.pop(&["slow"]);
    assert!(hub.registry.recheck(&hub.client, &hub.node(NID_B)).await);
    assert!(!hub.registry.offline(NID_B));
    hub.registry.check_all(&hub.client).await;
    let public = hub.public(NID_B);
    assert_eq!(public["online"], true);
    assert!(
        public.get("offline_since").is_none() && public.get("error").is_none(),
        "{public}"
    );
    let fetched = hub.fetch(NID_B, "/api/sessions", &[]).await;
    assert!(fetched.ok());
    assert!(
        fetched.data["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row.get("stale").is_none())
    );
}

#[tokio::test]
async fn node_health_exposes_safe_failure_reasons_and_clears_on_recovery() {
    let hub = Hub::new().await;
    let node = hub.node(NID_B);
    // 503 from the node, then a credential the node no longer accepts (403).
    hub.b.set(json!({"offline": true}));
    for _ in 0..OFFLINE_STRIKES {
        hub.registry
            .query(
                &hub.client,
                &node,
                "/api/live",
                &[],
                hub.client.timeout,
                None,
            )
            .await;
    }
    let public = hub.public(NID_B);
    assert_eq!(public["online"], false);
    assert_eq!(public["failed_path"], "/api/live");
    assert_eq!(public["error_code"], "http_error");
    assert_eq!(public["error"], "节点服务暂不可用（HTTP 503）");
    hub.b.pop(&["offline"]);
    hub.b.set(json!({"token": "changed-on-the-node"}));
    let fetched = hub
        .registry
        .query(
            &hub.client,
            &node,
            "/api/term/list",
            &[],
            hub.client.timeout,
            None,
        )
        .await;
    let failure = fetched.failure.unwrap();
    assert!(failure["error"].as_str().unwrap().contains("HTTP 403"));
    assert!(failure["error"].as_str().unwrap().contains("认证"));
    assert!(
        !serde_json::to_string(&hub.registry.public())
            .unwrap()
            .contains("forbidden"),
        "upstream body never leaks"
    );
    hub.b.set(json!({"token": hub.b.token}));
    assert!(
        hub.registry
            .query(
                &hub.client,
                &node,
                "/api/live",
                &[],
                hub.client.timeout,
                None
            )
            .await
            .ok()
    );
    let healthy = hub.public(NID_B);
    assert_eq!(healthy["online"], true);
    assert!(
        healthy.get("error").is_none()
            && healthy.get("failed_path").is_none()
            && healthy.get("strikes").is_none(),
        "{healthy}"
    );
}

#[tokio::test]
async fn one_slow_answer_does_not_gray_out_a_reachable_node() {
    let hub = Hub::new().await;
    let node = hub.node(NID_B);
    assert!(
        hub.registry
            .query(
                &hub.client,
                &node,
                "/api/live",
                &[],
                hub.client.timeout,
                None
            )
            .await
            .ok()
    );
    hub.b.set(json!({"slow": 0.8}));
    let failed = hub
        .registry
        .query(
            &hub.client,
            &node,
            "/api/live",
            &[],
            hub.client.timeout,
            None,
        )
        .await;
    assert_eq!(failed.failure.unwrap()["error_code"], "timeout");
    let public = hub.public(NID_B);
    assert_eq!(public["online"], true);
    assert!(!hub.registry.offline(NID_B));
    assert!(public.get("offline_since").is_none());
    let first_failure = hub.registry.state(NID_B).unwrap().failed_since.unwrap();
    for _ in 0..OFFLINE_STRIKES - 1 {
        hub.registry
            .query(
                &hub.client,
                &node,
                "/api/live",
                &[],
                hub.client.timeout,
                None,
            )
            .await;
    }
    let public = hub.public(NID_B);
    assert_eq!(public["online"], false);
    assert!(hub.registry.offline(NID_B));
    assert_eq!(public["offline_since"], json!(first_failure));
    hub.b.pop(&["slow"]);
    hub.registry
        .query(
            &hub.client,
            &node,
            "/api/live",
            &[],
            hub.client.timeout,
            None,
        )
        .await;
    let recovered = hub.registry.state(NID_B).unwrap();
    assert_eq!(recovered.online, Some(true));
    assert_eq!((recovered.strikes, recovered.failed_since), (None, None));
}

#[tokio::test]
async fn a_node_never_reached_is_offline_on_its_first_failure() {
    let dir = tempfile::tempdir().unwrap();
    let registry = open(dir.path());
    let client = client();
    let a = FakeNode::start(NID_A, "NodeA");
    registry.register(&client, a.registration()).await.unwrap();
    assert_eq!(registry.state(NID_A), None);
    a.set(json!({"offline": true}));
    registry
        .query(
            &client,
            &registry.get(NID_A).unwrap(),
            "/api/sessions",
            &[],
            client.timeout,
            None,
        )
        .await;
    assert!(registry.offline(NID_A));
    assert!(registry.state(NID_A).unwrap().offline_since.is_some());
}

async fn search_events(hub: &Hub, nid: &str) -> (Fetched, Vec<SearchEvent>) {
    let (tx, mut rx) = mpsc::channel(64);
    let fetched = hub
        .registry
        .fetch(
            &hub.client,
            &hub.node(nid),
            "/api/search",
            &q(&[("q", "needle")]),
            Some(&tx),
        )
        .await;
    drop(tx);
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    (fetched, events)
}

#[tokio::test]
async fn search_stream_progress_outlives_the_idle_timeout_and_reports_every_step() {
    let hub = Hub::new().await;
    hub.registry.check_all(&hub.client).await;
    // 8 steps 40 ms apart with a 200 ms idle timeout: progress keeps the stream alive.
    hub.b.set(json!({"search_steps": 8, "search_delay": 0.04}));
    let client = Client {
        search_idle: Duration::from_millis(200),
        ..hub.client.clone()
    };
    let (tx, mut rx) = mpsc::channel(64);
    let fetched = hub
        .registry
        .fetch(
            &client,
            &hub.node(NID_B),
            "/api/search",
            &q(&[("q", "needle")]),
            Some(&tx),
        )
        .await;
    drop(tx);
    assert!(fetched.ok(), "{:?}", fetched.failure);
    let mut progress = Vec::new();
    while let Some(event) = rx.recv().await {
        if let SearchEvent::Progress { done, total } = event {
            progress.push((done, total));
        }
    }
    assert_eq!(progress, (0..8).map(|done| (done, 8)).collect::<Vec<_>>());
    assert_eq!(fetched.data["results"][0]["snippet"], "NodeB needle");
    assert_eq!(fetched.data["total_pool"], 1);
    let (_, query) = hub.b.gets().last().unwrap().clone();
    assert_eq!(
        query,
        json!({"q": ["needle"], "progress": ["1"]}),
        "the node is asked for a progress stream"
    );
    hub.b.pop(&["search_steps", "search_delay"]);

    // A node that still answers plain JSON is read whole.
    hub.b.set(json!({"search_json": true}));
    let (fetched, events) = search_events(&hub, NID_B).await;
    assert!(fetched.ok());
    assert_eq!(fetched.data["results"][0]["uid"], "claude:same-file-hash");
    assert!(events.is_empty());
}

#[tokio::test]
async fn search_failures_keep_health_and_matches_already_streamed() {
    let hub = Hub::new().await;
    hub.registry.check_all(&hub.client).await;
    for option in ["search_error", "search_incomplete", "search_delay"] {
        hub.b
            .set(json!({option: if option == "search_delay" { json!(1.0) } else { json!(true) }}));
        let (fetched, _) = search_events(&hub, NID_B).await;
        let failure = fetched.failure.expect(option);
        assert_eq!(failure["name"], "NodeB");
        assert_eq!(
            failure["error_code"],
            if option == "search_delay" {
                "timeout"
            } else {
                "invalid_response"
            },
            "{option}"
        );
        assert!(!failure.to_string().contains("private upstream details"));
        assert_eq!(
            hub.public(NID_B)["online"],
            true,
            "a failed search says nothing about the node"
        );
        assert!(hub.registry.state(NID_B).unwrap().strikes.is_none());
        hub.b.pop(&[option]);
    }
    hub.b
        .set(json!({"search_matches": true, "search_error": true}));
    let (fetched, events) = search_events(&hub, NID_B).await;
    assert!(fetched.failure.is_some());
    assert_eq!(
        fetched.data["results"][0]["uid"], "claude:same-file-hash",
        "matches streamed before the error stay"
    );
    assert!(matches!(events.first(), Some(SearchEvent::Matches(rows)) if rows.len() == 1));
    hub.b.pop(&["search_matches", "search_error"]);
}

#[tokio::test]
async fn a_body_over_the_json_cap_is_an_invalid_response_not_a_partial_list() {
    let hub = Hub::new().await;
    hub.registry.check_all(&hub.client).await;
    hub.b.set(json!({"pad": JSON_LIMIT + 1}));
    let client = Client {
        timeout: Duration::from_secs(30),
        ..hub.client.clone()
    };
    let fetched = hub
        .registry
        .fetch(
            &client,
            &hub.node(NID_B),
            "/api/sessions",
            &q(&[("force", "1")]),
            None,
        )
        .await;
    let failure = fetched.failure.unwrap();
    assert_eq!(failure["error_code"], "invalid_response");
    assert_eq!(failure["error"], "节点返回无效或不完整的响应");
    assert_eq!(
        fetched.data["sessions"][0]["stale"], true,
        "the last good list is served instead"
    );
    assert!(fetched.data["sessions"][0].get("pad").is_none());
    hub.b.pop(&["pad"]);
}
