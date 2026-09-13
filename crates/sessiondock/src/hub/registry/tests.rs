//! Registry rules against an in-process fake node (tokio listener) so faults
//! — silence, 503, refused, malformed meta — are injected deterministically.
//! The Python fixture port (`tests/hub_fake_node.py`) is exercised by the
//! `hub_registry` integration tests.
use super::*;
use std::os::unix::fs::PermissionsExt;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const NID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[derive(Clone)]
enum Mode {
    Online,
    /// 503 on every `/api/` path.
    Offline,
    /// Accept, then say nothing for this long.
    Silent(Duration),
    /// `/api/meta` answers this instead of a local node.
    Meta(Value),
    /// `/api/search` NDJSON script (lines) or a JSON answer.
    Search(Vec<Value>),
    SearchJson(Value),
}

struct Fake {
    addr: SocketAddr,
    mode: Arc<Mutex<Mode>>,
    /// Request targets in order.
    hits: Arc<Mutex<Vec<String>>>,
    rows: Arc<Mutex<Vec<Value>>>,
}

fn sig_of(rows: &[Value]) -> String {
    format!("fixture-{}", rows.len())
}

impl Fake {
    async fn start(nid: &'static str) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mode = Arc::new(Mutex::new(Mode::Online));
        let hits = Arc::new(Mutex::new(Vec::new()));
        let rows = Arc::new(Mutex::new(vec![
            json!({"uid": "claude:same-file-hash", "title": "session", "updated": "2026-09-07T00:00:00Z"}),
        ]));
        let (mode2, hits2, rows2) = (mode.clone(), hits.clone(), rows.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let (mode, hits, rows) = (mode2.clone(), hits2.clone(), rows2.clone());
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut scratch = [0u8; 4096];
                    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut scratch).await {
                            Ok(0) | Err(_) => return,
                            Ok(count) => buffer.extend_from_slice(&scratch[..count]),
                        }
                    }
                    let head = String::from_utf8_lossy(&buffer).to_string();
                    let target = head.split(' ').nth(1).unwrap_or("").to_string();
                    let token_ok = head.contains("X-AgentHub-Node-Token: ")
                        && head.contains("X-AgentHub-Protocol: 1\r\n");
                    hits.lock().unwrap().push(target.clone());
                    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
                    let mode = mode.lock().unwrap().clone();
                    let (status, body): (u16, Vec<u8>) = match mode {
                        _ if !token_ok => (403, b"{\"error\":\"forbidden\"}".to_vec()),
                        Mode::Silent(delay) => {
                            tokio::time::sleep(delay).await;
                            return;
                        }
                        Mode::Offline => (503, b"{\"error\":\"offline\"}".to_vec()),
                        Mode::Meta(meta) if path == "/api/meta" => (200, meta.to_string().into_bytes()),
                        Mode::Search(lines) if path == "/api/search" => {
                            let _ = stream
                                .write_all(b"HTTP/1.0 200 OK\r\nContent-Type: application/x-ndjson\r\n\r\n")
                                .await;
                            for line in lines {
                                if let Some(ms) = line.get("sleep_ms").and_then(Value::as_u64) {
                                    tokio::time::sleep(Duration::from_millis(ms)).await;
                                    continue;
                                }
                                let mut bytes = line.to_string().into_bytes();
                                bytes.push(b'\n');
                                if stream.write_all(&bytes).await.is_err() {
                                    return;
                                }
                            }
                            let _ = stream.shutdown().await;
                            return;
                        }
                        Mode::SearchJson(data) if path == "/api/search" => (200, data.to_string().into_bytes()),
                        _ => match path {
                            "/api/meta" => (200, json!({"mode": "local", "protocol": 1, "node_id": nid}).to_string().into_bytes()),
                            "/api/sessions" => {
                                let rows = rows.lock().unwrap().clone();
                                let sig = sig_of(&rows);
                                let wanted = query.split('&').find_map(|pair| pair.strip_prefix("sig="));
                                let forced = query.split('&').any(|pair| pair == "force=1");
                                if wanted == Some(sig.as_str()) && !forced {
                                    (200, json!({"unchanged": true, "sig": sig}).to_string().into_bytes())
                                } else {
                                    (200, json!({"sessions": rows, "sig": sig, "built_at": 0}).to_string().into_bytes())
                                }
                            }
                            "/api/live" => (200, json!({"uids": [], "tmux_uids": [], "started_at": {}}).to_string().into_bytes()),
                            "/api/term/list" => (
                                200,
                                json!({"enabled": true, "sources": {"claude": true}, "sessions": [{"uid": "claude:t"}], "pending": []})
                                    .to_string()
                                    .into_bytes(),
                            ),
                            _ => (404, b"{\"error\":\"not found\"}".to_vec()),
                        },
                    };
                    let head = format!(
                        "HTTP/1.0 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Fake {
            addr,
            mode,
            hits,
            rows,
        }
    }

    fn set(&self, mode: Mode) {
        *self.mode.lock().unwrap() = mode;
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }

    fn registration(&self, name: &str) -> Registration {
        Registration {
            name: name.to_string(),
            url: self.url(),
            token: name.repeat(32),
            color: String::new(),
            id: None,
        }
    }
}

fn client() -> Client {
    Client {
        timeout: Duration::from_millis(300),
        search_idle: Duration::from_millis(300),
        recheck: Duration::from_millis(300),
    }
}

fn networks() -> Vec<Network> {
    parse_networks("127.0.0.0/8").unwrap()
}

fn open(dir: &Path) -> Registry {
    Registry::open(
        &dir.join("hub-nodes.json"),
        networks(),
        &dir.join("hub-cache"),
    )
    .unwrap()
}

fn public_row<'a>(rows: &'a [Value], nid: &str) -> &'a Value {
    rows.iter().find(|row| row["id"] == nid).unwrap()
}

#[test]
fn networks_parse_strictly_and_match_by_address_family() {
    // The default is loopback only; the private range is explicit configuration.
    let nets = parse_networks(&format!("{DEFAULT_NETWORKS},192.0.2.0/24")).unwrap();
    assert_eq!(nets.len(), 3);
    assert!(nets[0].contains("127.0.0.1".parse().unwrap()));
    assert!(nets[0].contains("127.255.255.255".parse().unwrap()));
    assert!(!nets[0].contains("128.0.0.1".parse().unwrap()));
    assert!(nets[1].contains("::1".parse().unwrap()));
    assert!(!nets[1].contains("::2".parse().unwrap()));
    assert!(nets[2].contains("192.0.2.9".parse().unwrap()));
    assert!(!nets[2].contains("192.0.3.9".parse().unwrap()));
    assert!(
        !nets[2].contains("::ffff:192.0.2.9".parse().unwrap()),
        "no cross-family matches"
    );
    assert_eq!(
        "192.0.2.0".parse::<Network>().unwrap(),
        Network {
            addr: "192.0.2.0".parse().unwrap(),
            prefix: 32
        }
    );
    assert!(
        "0.0.0.0/0"
            .parse::<Network>()
            .unwrap()
            .contains("198.51.100.8".parse().unwrap())
    );
    for bad in [
        "192.0.2.1/24",
        "192.0.2.0/33",
        "::1/129",
        "example.com/8",
        "",
        "192.0.2.0/x",
    ] {
        assert!(bad.parse::<Network>().is_err(), "{bad}");
    }
}

#[test]
fn validate_url_accepts_only_literal_ip_http_without_path_or_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let registry = open(dir.path());
    assert_eq!(
        registry.validate_url("http://127.0.0.1:8710").unwrap(),
        "127.0.0.1:8710".parse().unwrap()
    );
    assert_eq!(
        registry.validate_url("http://127.0.0.1:8710/").unwrap(),
        "127.0.0.1:8710".parse().unwrap()
    );
    assert_eq!(
        registry.validate_url("HTTP://127.0.0.1").unwrap(),
        "127.0.0.1:80".parse().unwrap()
    );
    assert_eq!(
        registry.validate_url("http://127.0.0.1:").unwrap(),
        "127.0.0.1:80".parse().unwrap()
    );
    let v6 = Registry::open(
        &dir.path().join("v6.json"),
        parse_networks("::1/128").unwrap(),
        dir.path(),
    )
    .unwrap();
    assert_eq!(
        v6.validate_url("http://[::1]:8000").unwrap(),
        "[::1]:8000".parse().unwrap()
    );
    for (url, hint) in [
        ("http://169.254.169.254", "允许的网络"),
        ("http://example.com", "节点地址必须是"),
        ("http://127.0.0.1/a", "节点地址必须是"),
        ("http://user:pass@127.0.0.1", "节点地址必须是"),
        ("http://127.0.0.1?x=1", "节点地址必须是"),
        ("http://127.0.0.1#f", "节点地址必须是"),
        ("http://127.0.0.1//", "节点地址必须是"),
        ("https://127.0.0.1:8710", "节点地址必须是"),
        ("ftp://127.0.0.1", "节点地址必须是"),
        ("127.0.0.1:8710", "节点地址必须是"),
        ("http://", "节点地址必须是"),
        ("http://::1:8000", "节点地址必须是"),
        ("http://127.0.0.1:0", "invalid port"),
        ("http://127.0.0.1:99999", "invalid port"),
        ("http://127.0.0.1:abc", "invalid port"),
    ] {
        match registry.validate_url(url) {
            Err(RegistryError::Invalid(message)) => {
                assert!(message.contains(hint), "{url}: {message}")
            }
            other => panic!("{url}: {other:?}"),
        }
    }
}

#[test]
fn encode_query_matches_python_urlencode() {
    let query = vec![
        ("q".to_string(), "a b&c=d/é".to_string()),
        ("sig".to_string(), "fixture-1~x.y_z".to_string()),
    ];
    assert_eq!(
        encode_query(&query),
        "q=a+b%26c%3Dd%2F%C3%A9&sig=fixture-1~x.y_z"
    );
    assert_eq!(encode_query(&[]), "");
}

#[test]
fn stale_payload_marks_rows_and_disables_terminal_lists() {
    let cached = (
        1700000000.5,
        json!({"sessions": [{"uid": "a"}], "pending": [{"name": "p"}], "sig": "s", "enabled": true, "sources": {"claude": true}}),
    );
    let data = stale_payload("/api/sessions", Some(&cached));
    assert_eq!(
        data["sessions"][0],
        json!({"uid": "a", "stale": true, "last_seen": 1700000000.5})
    );
    assert_eq!(
        data["pending"][0],
        json!({"name": "p", "stale": true, "last_seen": 1700000000.5})
    );
    assert_eq!(
        data["enabled"],
        json!(true),
        "sessions keep their other keys"
    );
    let term = stale_payload("/api/term/list", Some(&cached));
    assert_eq!(term["enabled"], json!(false));
    assert_eq!(term["sources"], json!({}));
    assert_eq!(stale_payload("/api/live", None), json!({}));
    assert_eq!(
        stale_payload("/api/term/list", None),
        json!({"enabled": false, "sources": {}})
    );
}

#[tokio::test]
async fn register_validates_then_asks_meta_and_keys_by_node_id() {
    let dir = tempfile::tempdir().unwrap();
    let registry = open(dir.path());
    let fake = Fake::start(NID_A).await;
    let client = client();
    let base = fake.registration("NodeA");

    for (bad, hint) in [
        (
            Registration {
                name: "  ".into(),
                ..base.clone()
            },
            "请输入机器名称",
        ),
        (
            Registration {
                name: "x".repeat(81),
                ..base.clone()
            },
            "请输入机器名称",
        ),
        (
            Registration {
                token: "short".into(),
                ..base.clone()
            },
            "请输入机器名称",
        ),
        (
            Registration {
                token: format!("{}!", "a".repeat(40)),
                ..base.clone()
            },
            "请输入机器名称",
        ),
        (
            Registration {
                url: "http://example.com".into(),
                ..base.clone()
            },
            "节点地址必须是",
        ),
        (
            Registration {
                url: "http://198.51.100.3:1".into(),
                ..base.clone()
            },
            "允许的网络",
        ),
    ] {
        match registry.register(&client, bad).await {
            Err(RegistryError::Invalid(message)) => assert!(message.contains(hint), "{message}"),
            other => panic!("{hint}: {other:?}"),
        }
    }
    assert!(
        fake.hits().is_empty(),
        "nothing is asked before the input is valid"
    );

    // The node must prove it is a local node speaking protocol 1 with a real id.
    for meta in [
        json!({"mode": "hub", "protocol": 1, "node_id": NID_A}),
        json!({"mode": "local", "protocol": 2, "node_id": NID_A}),
        json!({"mode": "local", "protocol": 1, "node_id": "short"}),
        json!({"mode": "local", "protocol": 1}),
        json!([]),
    ] {
        fake.set(Mode::Meta(meta.clone()));
        match registry.register(&client, base.clone()).await {
            Err(RegistryError::Invalid(message)) => assert!(
                message.contains("节点认证或协议检查失败"),
                "{meta}: {message}"
            ),
            other => panic!("{meta}: {other:?}"),
        }
    }
    fake.set(Mode::Offline);
    assert!(
        matches!(
            registry.register(&client, base.clone()).await,
            Err(RegistryError::Invalid(_))
        ),
        "503 meta"
    );
    fake.set(Mode::Silent(Duration::from_secs(2)));
    assert!(matches!(
        registry.register(&client, base.clone()).await,
        Err(RegistryError::Node(ClientError::Timeout))
    ));
    fake.set(Mode::Online);
    assert!(registry.all().is_empty());
    assert!(!dir.path().join("hub-nodes.json").exists());

    let row = registry
        .register(
            &client,
            Registration {
                url: format!("{}//", fake.url()),
                ..base.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        row,
        NodeRow {
            id: NID_A.into(),
            name: "NodeA".into(),
            color: String::new(),
            enabled: None
        }
    );
    assert_eq!(fake.hits().last().map(String::as_str), Some("/api/meta"));
    let stored = registry.get(NID_A).unwrap();
    assert_eq!(stored.url, fake.url(), "trailing slashes are stripped");
    assert_eq!(stored.token, "NodeA".repeat(32));
    let file = dir.path().join("hub-nodes.json");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let text = fs::read_to_string(&file).unwrap();
    assert!(
        text.starts_with("[\n  {\n    \"url\""),
        "pretty list like json.dump(indent=2): {text}"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!([{"url": fake.url(), "token": "NodeA".repeat(32), "id": NID_A, "name": "NodeA"}])
    );
    assert!(!dir.path().join("hub-nodes.tmp").exists());

    // Colour: palette only, stored and reloaded; re-registering replaces the entry by node id.
    let bad = registry
        .register(
            &client,
            Registration {
                color: "#ff0000".into(),
                ..base.clone()
            },
        )
        .await
        .unwrap_err();
    assert!(
        bad.to_string().contains("机器颜色只能取 blue、violet"),
        "{bad}"
    );
    let row = registry
        .register(
            &client,
            Registration {
                name: "Renamed".into(),
                color: " TEAL ".into(),
                ..base.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(row.color, "teal");
    assert_eq!(registry.all().len(), 1);
    assert_eq!(registry.get(NID_A).unwrap().name, "Renamed");
    let public = registry.public();
    assert_eq!(
        public,
        vec![json!({"id": NID_A, "name": "Renamed", "color": "teal", "online": null})]
    );
    assert!(!public[0].to_string().contains("token") && public[0].get("url").is_none());
    let reopened = open(dir.path());
    assert_eq!(reopened.public()[0]["color"], "teal");

    // An expected id that is not the node's own id never overwrites another entry.
    let mismatch = registry
        .register(
            &client,
            Registration {
                id: Some(NID_B.into()),
                ..base.clone()
            },
        )
        .await
        .unwrap_err();
    assert!(
        mismatch.to_string().contains("地址对应另一台机器"),
        "{mismatch}"
    );
    assert_eq!(
        registry.get(NID_A).unwrap().name,
        "Renamed",
        "the failed registration changed nothing"
    );
    registry
        .register(
            &client,
            Registration {
                id: Some(NID_A.into()),
                ..base.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(registry.get(NID_A).unwrap().name, "NodeA");
}

#[tokio::test]
async fn display_enabled_and_order_follow_python_rules_and_persist() {
    let dir = tempfile::tempdir().unwrap();
    let registry = open(dir.path());
    let a = Fake::start(NID_A).await;
    let b = Fake::start(NID_B).await;
    let client = client();
    registry
        .register(&client, a.registration("NodeA"))
        .await
        .unwrap();
    registry
        .register(&client, b.registration("NodeB"))
        .await
        .unwrap();

    let row = registry
        .update_display(NID_A, Some(" 机房 A "), Some("teal"), None)
        .unwrap();
    assert_eq!(
        row,
        NodeRow {
            id: NID_A.into(),
            name: "机房 A".into(),
            color: "teal".into(),
            enabled: Some(true)
        }
    );
    assert_eq!(
        registry
            .update_display(NID_A, None, Some("rose"), None)
            .unwrap()
            .name,
        "机房 A",
        "one field leaves the other"
    );
    assert_eq!(
        registry
            .update_display(NID_A, None, Some(""), None)
            .unwrap()
            .color,
        "",
        "empty colour clears"
    );
    for (name, color, hint) in [
        (Some(""), None, "不能为空"),
        (Some("x".repeat(81).as_str()), None, "80"),
        (None, Some("#ff0000"), "机器颜色"),
        (Some("NodeB"), None, "已有机器"),
    ] {
        match registry.update_display(NID_A, name, color, None) {
            Err(RegistryError::Invalid(message)) => assert!(message.contains(hint), "{message}"),
            other => panic!("{hint}: {other:?}"),
        }
    }
    assert!(
        registry
            .update_display(NID_A, Some("x".repeat(80).as_str()), None, None)
            .is_ok(),
        "80 characters, not bytes"
    );
    assert!(matches!(
        registry.update_display(&"c".repeat(32), Some("x"), None, None),
        Err(RegistryError::NotFound(_))
    ));
    registry
        .update_display(NID_A, Some("NodeA"), None, None)
        .unwrap();

    // Disabled = does not exist: no health, no cache, not probed, not public; order unchanged.
    registry.check_all(&client).await;
    assert!(registry.state(NID_B).is_some());
    let probes_before = b.hits().len();
    let row = registry
        .update_display(NID_B, None, None, Some(false))
        .unwrap();
    assert_eq!(row.enabled, Some(false));
    assert_eq!(registry.state(NID_B), None);
    assert_eq!(
        registry
            .all()
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        [NID_A]
    );
    assert!(registry.get(NID_B).is_none() && registry.find(NID_B).is_some());
    assert_eq!(
        registry
            .public()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [NID_A]
    );
    let machines = registry.machines();
    assert_eq!(
        machines
            .iter()
            .map(|row| (
                row["id"].as_str().unwrap(),
                row["enabled"].as_bool().unwrap(),
                row["online"].clone()
            ))
            .collect::<Vec<_>>(),
        [(NID_A, true, json!(true)), (NID_B, false, Value::Null)]
    );
    registry.check_all(&client).await;
    assert_eq!(
        b.hits().len(),
        probes_before,
        "the monitor no longer probes a disabled machine"
    );
    let reopened = open(dir.path());
    assert_eq!(
        reopened
            .all()
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        [NID_A]
    );
    assert_eq!(
        reopened
            .machines()
            .iter()
            .map(|row| (
                row["id"].as_str().unwrap(),
                row["enabled"].as_bool().unwrap()
            ))
            .collect::<Vec<_>>(),
        [(NID_A, true), (NID_B, false)]
    );
    assert_eq!(
        registry
            .update_display(NID_B, None, None, Some(true))
            .unwrap()
            .enabled,
        Some(true)
    );
    assert_eq!(registry.all().len(), 2);
    registry.check_all(&client).await;
    assert_eq!(registry.state(NID_B).unwrap().online, Some(true));

    // Order is the user's, a permutation of every machine, and survives toggling and restarts.
    assert_eq!(
        registry.reorder(&[NID_B.into(), NID_A.into()]).unwrap(),
        [NID_B, NID_A]
    );
    assert_eq!(
        registry
            .public()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [NID_B, NID_A]
    );
    registry
        .update_display(NID_B, None, None, Some(false))
        .unwrap();
    registry
        .update_display(NID_B, None, None, Some(true))
        .unwrap();
    assert_eq!(
        registry
            .machines()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [NID_B, NID_A]
    );
    assert_eq!(
        open(dir.path())
            .machines()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [NID_B, NID_A]
    );
    for bad in [
        vec![NID_A.to_string()],
        vec![NID_A.into(), NID_B.into(), NID_A.into()],
        vec![NID_A.into(), "c".repeat(32)],
    ] {
        match registry.reorder(&bad) {
            Err(RegistryError::Invalid(message)) => {
                assert_eq!(message, "顺序必须包含每台机器各一次")
            }
            other => panic!("{bad:?}: {other:?}"),
        }
    }
    for bad in [json!("ab"), Value::Null, json!([1, 2])] {
        match registry.reorder_json(&bad) {
            Err(RegistryError::Invalid(message)) => assert_eq!(message, "ids 必须是机器 id 列表"),
            other => panic!("{bad}: {other:?}"),
        }
    }
    assert_eq!(
        registry.reorder_json(&json!([NID_A, NID_B])).unwrap(),
        [NID_A, NID_B]
    );

    registry.remove(NID_B).unwrap();
    assert!(registry.find(NID_B).is_none());
    assert_eq!(open(dir.path()).machines().len(), 1);
}

#[tokio::test]
async fn one_slow_answer_keeps_a_reachable_node_online_until_the_strikes_run_out() {
    let dir = tempfile::tempdir().unwrap();
    let registry = open(dir.path());
    let fake = Fake::start(NID_A).await;
    let client = client();
    registry
        .register(&client, fake.registration("NodeA"))
        .await
        .unwrap();
    let node = registry.get(NID_A).unwrap();
    assert!(
        registry
            .query(&client, &node, "/api/live", &[], client.timeout, None)
            .await
            .ok()
    ); // known reachable

    fake.set(Mode::Silent(Duration::from_secs(3)));
    let failed = registry
        .query(&client, &node, "/api/live", &[], client.timeout, None)
        .await;
    let failure = failed.failure.unwrap();
    assert_eq!(failure["error_code"], "timeout");
    assert_eq!(failure["error"], "节点连接或响应超时（等待超过 0.3 秒）");
    assert_eq!(failure["name"], "NodeA");
    assert!(failure["last_seen"].is_number());
    let health = registry.state(NID_A).unwrap();
    assert_eq!(
        health.online,
        Some(true),
        "one failure of a reachable node is tentative"
    );
    assert!(!registry.offline(NID_A));
    assert_eq!(health.offline_since, None);
    assert_eq!(health.strikes, Some(1));
    assert_eq!(health.failed_path.as_deref(), Some("/api/live"));
    let first_failure = health.failed_since.unwrap();
    let public = registry.public();
    assert_eq!(public_row(&public, NID_A)["online"], json!(true));
    assert!(public_row(&public, NID_A).get("offline_since").is_none());

    for _ in 0..OFFLINE_STRIKES - 1 {
        registry
            .query(&client, &node, "/api/live", &[], client.timeout, None)
            .await;
    }
    let health = registry.state(NID_A).unwrap();
    assert_eq!(health.online, Some(false));
    assert!(registry.offline(NID_A));
    assert_eq!(
        health.offline_since,
        Some(first_failure),
        "the outage is dated from its first failure"
    );
    assert_eq!(health.strikes, Some(OFFLINE_STRIKES));
    let row = public_row(&registry.public(), NID_A).clone();
    assert_eq!(row["online"], json!(false));
    assert_eq!(row["offline_since"], json!(first_failure));
    assert!(row.get("checked_at").is_some() && row.get("failed_path").is_some());
    assert!(!row.to_string().contains("private"), "{row}");

    // A node the monitor knows to be down is never waited on by the aggregate.
    let started = std::time::Instant::now();
    let fetched = registry.fetch(&client, &node, "/api/live", &[], None).await;
    assert!(started.elapsed() < Duration::from_millis(100));
    let failure = fetched.failure.unwrap();
    assert_eq!(failure["error_code"], "timeout");
    assert_eq!(failure["offline_since"], json!(first_failure));
    assert_eq!(fetched.data, json!({}));

    // Recovery clears every failure field.
    fake.set(Mode::Online);
    assert!(registry.recheck(&client, &node).await);
    let recovered = registry.state(NID_A).unwrap();
    assert_eq!(recovered.online, Some(true));
    assert_eq!(
        (
            recovered.strikes,
            recovered.failed_since,
            recovered.offline_since,
            recovered.error
        ),
        (None, None, None, None)
    );
    let row = public_row(&registry.public(), NID_A).clone();
    assert!(
        row.get("error").is_none()
            && row.get("failed_path").is_none()
            && row.get("offline_since").is_none()
    );
    assert_eq!(
        fake.hits().last().map(String::as_str),
        Some("/api/live?"),
        "recheck asks /api/live"
    );
}

#[tokio::test]
async fn a_node_never_reached_is_offline_on_its_first_failure() {
    let dir = tempfile::tempdir().unwrap();
    let registry = open(dir.path());
    let fake = Fake::start(NID_A).await;
    let client = client();
    registry
        .register(&client, fake.registration("NodeA"))
        .await
        .unwrap();
    let node = registry.get(NID_A).unwrap();
    assert_eq!(registry.state(NID_A), None);
    assert_eq!(public_row(&registry.public(), NID_A)["online"], Value::Null);

    // Point the stored node at a closed port: connection refused on the first probe.
    let closed = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let unreachable = Node {
        url: format!("http://{}", closed.local_addr().unwrap()),
        ..node.clone()
    };
    drop(closed);
    let fetched = registry
        .query(
            &client,
            &unreachable,
            "/api/sessions",
            &[],
            client.timeout,
            None,
        )
        .await;
    assert_eq!(
        fetched.failure.as_ref().unwrap()["error_code"],
        "connection_refused"
    );
    assert_eq!(
        fetched.failure.as_ref().unwrap()["error"],
        "节点拒绝连接，目标端口未接受请求"
    );
    assert!(registry.offline(NID_A));
    let health = registry.state(NID_A).unwrap();
    assert_eq!(health.strikes, Some(1));
    assert_eq!(health.offline_since, health.failed_since);
    assert!(health.offline_since.is_some());

    // Every failure class is public and safe.
    fake.set(Mode::Offline);
    let fetched = registry
        .query(&client, &node, "/api/term/list", &[], client.timeout, None)
        .await;
    let failure = fetched.failure.unwrap();
    assert_eq!(failure["error_code"], "http_error");
    assert_eq!(failure["error"], "节点服务暂不可用（HTTP 503）");
    assert_eq!(fetched.data, json!({"enabled": false, "sources": {}}));
    fake.set(Mode::Meta(json!("not an object")));
    let fetched = registry
        .query(&client, &node, "/api/meta", &[], client.timeout, None)
        .await;
    assert_eq!(fetched.failure.unwrap()["error_code"], "invalid_response");
    assert_eq!(
        registry.state(NID_A).unwrap().failed_path.as_deref(),
        Some("/api/meta")
    );
}

#[tokio::test]
async fn snapshot_is_private_reloaded_unknown_and_served_stale_while_offline() {
    let dir = tempfile::tempdir().unwrap();
    let registry = open(dir.path());
    let fake = Fake::start(NID_A).await;
    let client = client();
    registry
        .register(&client, fake.registration("NodeA"))
        .await
        .unwrap();
    registry.check_all(&client).await;
    let snapshot = registry.snapshot_path(NID_A);
    assert_eq!(
        snapshot,
        dir.path()
            .join("hub-cache")
            .join(format!("{NID_A}.sessions.json"))
    );
    assert_eq!(
        fs::metadata(&snapshot).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let raw: Value = serde_json::from_slice(&fs::read(&snapshot).unwrap()).unwrap();
    assert!(raw["stamp"].is_f64());
    assert_eq!(raw["data"]["sessions"][0]["uid"], "claude:same-file-hash");
    let written = fs::metadata(&snapshot).unwrap().modified().unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    registry.check_all(&client).await;
    assert_eq!(
        fs::metadata(&snapshot).unwrap().modified().unwrap(),
        written,
        "same sig: not rewritten"
    );

    let fresh = open(dir.path());
    let public = fresh.public();
    assert_eq!(public_row(&public, NID_A)["online"], Value::Null);
    let last_seen = public_row(&public, NID_A)["last_seen"].clone();
    assert!(last_seen.is_number());
    let node = fresh.get(NID_A).unwrap();
    fake.set(Mode::Offline);
    fresh.check_all(&client).await;
    assert!(
        fresh.offline(NID_A),
        "never seen online by this registry: offline at once"
    );
    for query in [vec![], vec![("force".to_string(), "1".to_string())]] {
        let fetched = fresh
            .fetch(&client, &node, "/api/sessions", &query, None)
            .await;
        let failure = fetched.failure.unwrap();
        assert_eq!(failure["error_code"], "http_error");
        assert_eq!(fetched.data["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(fetched.data["sessions"][0]["stale"], json!(true));
        assert_eq!(fetched.data["sessions"][0]["last_seen"], last_seen);
    }
    fake.set(Mode::Online);
    fresh.check_all(&client).await;
    assert_eq!(fresh.state(NID_A).unwrap().online, Some(true));
    let fetched = fresh
        .fetch(&client, &node, "/api/sessions", &[], None)
        .await;
    assert!(fetched.ok());
    assert!(fetched.data["sessions"][0].get("stale").is_none());

    // Snapshots that are not a session list are ignored; a corrupt one too.
    fs::write(
        &snapshot,
        b"{\"stamp\": 1, \"data\": {\"sessions\": \"no\"}}",
    )
    .unwrap();
    let ignored = open(dir.path());
    assert_eq!(ignored.state(NID_A), None);
    fs::write(&snapshot, b"garbage").unwrap();
    assert_eq!(open(dir.path()).state(NID_A), None);

    // Removing / re-registering a node drops its snapshot.
    registry.remove(NID_A).unwrap();
    assert!(!snapshot.exists());
}

#[tokio::test]
async fn session_polls_are_conditional_and_served_from_cache_when_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let registry = open(dir.path());
    let fake = Fake::start(NID_A).await;
    let client = client();
    registry
        .register(&client, fake.registration("NodeA"))
        .await
        .unwrap();
    let node = registry.get(NID_A).unwrap();
    registry.check_all(&client).await;
    let seen = fake.hits().len();
    registry.check_all(&client).await;
    let fetched = registry
        .fetch(&client, &node, "/api/sessions", &[], None)
        .await;
    let probes: Vec<String> = fake.hits()[seen..].to_vec();
    assert_eq!(probes.len(), 2);
    assert!(
        probes
            .iter()
            .all(|target| target.starts_with("/api/sessions?sig=fixture-1")),
        "{probes:?}"
    );
    assert!(fetched.ok());
    assert_eq!(fetched.data["sessions"].as_array().unwrap().len(), 1);
    assert!(fetched.data["sessions"][0].get("stale").is_none());
    assert_eq!(registry.state(NID_A).unwrap().online, Some(true));

    // A forced refresh bypasses the conditional probe; a changed list arrives through it.
    registry
        .fetch(
            &client,
            &node,
            "/api/sessions",
            &[("force".into(), "1".into())],
            None,
        )
        .await;
    assert_eq!(
        fake.hits().last().map(String::as_str),
        Some("/api/sessions?force=1")
    );
    fake.rows.lock().unwrap().clear();
    let fetched = registry
        .fetch(&client, &node, "/api/sessions", &[], None)
        .await;
    assert!(fake.hits().last().unwrap().contains("sig=fixture-1"));
    assert_eq!(fetched.data["sessions"], json!([]));
    assert_eq!(fetched.data["sig"], "fixture-0");
    registry.check_all(&client).await;
    assert!(
        fake.hits().last().unwrap().contains("sig=fixture-0"),
        "the new signature is what the next probe sends"
    );

    // Offline: a cached variant answer is served for term/list, the plain list
    // for any sessions variant, nothing for /api/live.
    let variant = vec![("v".to_string(), "1".to_string())];
    assert!(
        registry
            .query(
                &client,
                &node,
                "/api/term/list",
                &variant,
                client.timeout,
                None
            )
            .await
            .ok()
    );
    fake.set(Mode::Offline);
    for _ in 0..OFFLINE_STRIKES {
        registry
            .query(&client, &node, "/api/live", &[], client.timeout, None)
            .await;
    }
    assert!(registry.offline(NID_A));
    let fetched = registry
        .fetch(&client, &node, "/api/term/list", &variant, None)
        .await;
    assert_eq!(fetched.data["sessions"][0]["stale"], json!(true));
    assert_eq!(fetched.data["enabled"], json!(false));
    let fetched = registry
        .fetch(
            &client,
            &node,
            "/api/term/list",
            &[("v".into(), "2".into())],
            None,
        )
        .await;
    assert_eq!(
        fetched.data,
        json!({"enabled": false, "sources": {}}),
        "unknown variant: nothing cached"
    );
    let fetched = registry
        .fetch(
            &client,
            &node,
            "/api/sessions",
            &[("window".into(), "9".into())],
            None,
        )
        .await;
    assert_eq!(
        fetched.data["sessions"],
        json!([]),
        "an unknown sessions variant falls back to the plain list"
    );
    assert_eq!(fetched.data["sig"], "fixture-0");
    let live = registry.fetch(&client, &node, "/api/live", &[], None).await;
    assert_eq!(live.data, json!({}), "/api/live is never cached");
    assert_eq!(
        live.failure.unwrap()["error"],
        "节点服务暂不可用（HTTP 503）"
    );

    // Variant caches are bounded to 128 entries, oldest first (Python keeps the same bound).
    fake.set(Mode::Online);
    for index in 0..140 {
        registry
            .query(
                &client,
                &node,
                "/api/term/list",
                &[("v".into(), index.to_string())],
                client.timeout,
                None,
            )
            .await;
    }
    let inner = registry.lock();
    assert_eq!(inner.cache.len(), CACHE_LIMIT);
    assert!(inner.cache.contains_key(&(
        NID_A.to_string(),
        "/api/term/list".to_string(),
        "v=139".to_string()
    )));
    assert!(!inner.cache.contains_key(&(
        NID_A.to_string(),
        "/api/term/list".to_string(),
        "v=1".to_string()
    )));
}

#[tokio::test]
async fn search_streams_events_keeps_health_and_retains_streamed_matches() {
    let dir = tempfile::tempdir().unwrap();
    let registry = open(dir.path());
    let fake = Fake::start(NID_A).await;
    let client = client();
    registry
        .register(&client, fake.registration("NodeA"))
        .await
        .unwrap();
    let node = registry.get(NID_A).unwrap();
    registry.check_all(&client).await;
    let row = json!({"uid": "claude:same-file-hash", "hits": 1, "snippet": "needle"});
    let query = vec![("q".to_string(), "needle".to_string())];

    fake.set(Mode::Search(vec![
        json!({"type": "progress", "done": 0, "total": 3}),
        json!({"type": "heartbeat"}),
        json!({"type": "matches", "results": [row]}),
        json!({"type": "progress", "done": "2", "total": 3.0}),
        json!({"type": "result", "data": {"results": [row], "total_pool": 3, "truncated": false}}),
    ]));
    let (tx, mut rx) = mpsc::channel(8);
    let fetched = registry
        .query(
            &client,
            &node,
            "/api/search",
            &query,
            client.timeout,
            Some(&tx),
        )
        .await;
    drop(tx);
    assert!(fetched.ok(), "{:?}", fetched.failure);
    assert_eq!(fetched.data["results"], json!([row]));
    assert_eq!(
        fake.hits().last().map(String::as_str),
        Some("/api/search?q=needle&progress=1")
    );
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    assert_eq!(
        events,
        [
            SearchEvent::Progress { done: 0, total: 3 },
            SearchEvent::Matches(vec![row.clone()]),
            SearchEvent::Progress { done: 2, total: 3 },
        ]
    );

    // Old nodes answer JSON: returned whole.
    fake.set(Mode::SearchJson(json!({"results": [], "total_pool": 0})));
    let fetched = registry
        .query(&client, &node, "/api/search", &query, client.timeout, None)
        .await;
    assert_eq!(fetched.data, json!({"results": [], "total_pool": 0}));

    // Failures: an error event, an incomplete stream, and idle silence — none touch health,
    // all keep the matches already streamed, and none leak upstream text.
    for (script, code, text) in [
        (
            vec![
                json!({"type": "matches", "results": [row]}),
                json!({"type": "error", "error": "private upstream details"}),
            ],
            "invalid_response",
            "节点返回无效或不完整的响应",
        ),
        (
            vec![json!({"type": "matches", "results": [row]})],
            "invalid_response",
            "节点返回无效或不完整的响应",
        ),
        (
            vec![
                json!({"type": "matches", "results": [row]}),
                json!({"sleep_ms": 2000}),
            ],
            "timeout",
            "节点连接或响应超时（等待超过 0.3 秒）",
        ),
    ] {
        fake.set(Mode::Search(script));
        let fetched = registry
            .query(&client, &node, "/api/search", &query, client.timeout, None)
            .await;
        let failure = fetched.failure.unwrap();
        assert_eq!(failure["error_code"], code);
        assert_eq!(failure["error"], text);
        assert_eq!(
            fetched.data["results"],
            json!([row]),
            "matches already streamed are kept"
        );
        assert!(!failure.to_string().contains("private"));
        let health = registry.state(NID_A).unwrap();
        assert_eq!(
            (health.online, health.strikes),
            (Some(true), None),
            "a failed search says nothing about the node"
        );
    }

    // The offline gate applies to searches too: nothing is asked, no stale rows for search.
    fake.set(Mode::Offline);
    for _ in 0..OFFLINE_STRIKES {
        registry
            .query(&client, &node, "/api/live", &[], client.timeout, None)
            .await;
    }
    assert!(registry.offline(NID_A));
    let seen = fake.hits().len();
    let fetched = registry
        .fetch(&client, &node, "/api/search", &query, None)
        .await;
    assert_eq!(fake.hits().len(), seen);
    assert_eq!(fetched.data, json!({}));
    assert_eq!(fetched.failure.unwrap()["error_code"], "http_error");
}

#[tokio::test]
async fn monitor_polls_on_start_wakes_on_nudge_and_stops_on_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(open(dir.path()));
    let fake = Fake::start(NID_A).await;
    let client = Arc::new(client());
    registry
        .register(&client, fake.registration("NodeA"))
        .await
        .unwrap();
    let probes = || {
        fake.hits()
            .iter()
            .filter(|target| target.starts_with("/api/sessions"))
            .count()
    };
    let wait_for = |expected: usize| {
        let fake = &fake;
        async move {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while std::time::Instant::now() < deadline {
                if fake
                    .hits()
                    .iter()
                    .filter(|target| target.starts_with("/api/sessions"))
                    .count()
                    >= expected
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("expected {expected} session probes, saw {:?}", fake.hits());
        }
    };
    let shutdown = CancellationToken::new();
    let monitor = Monitor::spawn_every(
        registry.clone(),
        client.clone(),
        shutdown.clone(),
        Duration::from_secs(30),
    );
    wait_for(1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        probes(),
        1,
        "the nudge from registration does not schedule a second pass"
    );
    assert_eq!(registry.state(NID_A).unwrap().online, Some(true));
    registry.nudge();
    wait_for(2).await;
    // A recheck nudges as well.
    registry
        .recheck(&client, &registry.get(NID_A).unwrap())
        .await;
    wait_for(3).await;
    shutdown.cancel();
    monitor.stop().await;
    let after = probes();
    registry.nudge();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(probes(), after, "a stopped monitor ignores nudges");
}
