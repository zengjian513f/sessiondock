//! Hub aggregation against two `tests/hub_fake_node.py` nodes (the port of the
//! Python project's `hub_fixture.NodeHandler`): the `test_hub.py` aggregate
//! cases — identity across machines, `sig`/`unchanged`, the `nodes` filter,
//! offline nodes as partial answers with stale cached rows, term/list
//! capabilities, trash totals, the NDJSON search stream and the writes split
//! per machine. Nothing here touches a session root.
use std::{
    ffi::OsStr,
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use serde_json::{Value, json};
use sessiondock::hub::{
    Client, Registry,
    aggregate::{self, Params},
    namespace,
    registry::{Registration, parse_networks},
};

const NID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NID_C: &str = "cccccccccccccccccccccccccccccccc";

fn python3() -> PathBuf {
    python_from_path(std::env::var_os("PATH").as_deref())
}

fn python_from_path(path: Option<&OsStr>) -> PathBuf {
    // Windows does not apply PATHEXT to a constructed absolute path.
    let names: &[&str] = if cfg!(windows) {
        &["python3.exe", "python.exe"]
    } else {
        &["python3"]
    };
    path.and_then(|paths| first_file_on_path(paths, names))
        .unwrap_or_else(|| {
            PathBuf::from(if cfg!(windows) {
                "python.exe"
            } else {
                "/usr/bin/python3"
            })
        })
}

fn first_file_on_path(paths: &OsStr, names: &[&str]) -> Option<PathBuf> {
    names.iter().find_map(|name| {
        std::env::split_paths(paths)
            .map(|dir| dir.join(name))
            .find(|candidate| usable_python(candidate))
    })
}

fn usable_python(candidate: &Path) -> bool {
    let Ok(metadata) = candidate.metadata() else {
        return false;
    };
    // Windows App Execution Aliases are zero-byte placeholders that only
    // print a Store-install message when launched outside the desktop shell.
    metadata.is_file() && metadata.len() != 0
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

    /// `(path, body)` of every POST the node has answered.
    fn writes(&self) -> Vec<(String, Value)> {
        self.state()["writes"]
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

struct Hub {
    _dir: tempfile::TempDir,
    registry: Arc<Registry>,
    client: Arc<Client>,
    a: FakeNode,
    b: FakeNode,
}

impl Hub {
    async fn new() -> Self {
        Self::with_search_idle(Duration::from_millis(400)).await
    }

    /// `search_idle` is the hub's idle limit between search stream lines.
    async fn with_search_idle(search_idle: Duration) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let registry = Registry::open(
            &dir.path().join("nodes.json"),
            parse_networks("127.0.0.0/8").unwrap(),
            &dir.path().join("hub-cache"),
        )
        .unwrap()
        .with_public_payload(namespace::public_payload);
        let client = Client {
            timeout: Duration::from_millis(400),
            search_idle,
            recheck: Duration::from_millis(400),
        };
        let a = FakeNode::start(NID_A, "NodeA");
        let b = FakeNode::start(NID_B, "NodeB");
        registry.register(&client, a.registration()).await.unwrap();
        registry.register(&client, b.registration()).await.unwrap();
        Hub {
            _dir: dir,
            registry: Arc::new(registry),
            client: Arc::new(client),
            a,
            b,
        }
    }

    /// Take B down until the monitor has struck it out (two failed checks).
    async fn b_offline(&self) {
        self.b.set(json!({"offline": true}));
        self.registry.check_all(&self.client).await;
        self.registry.check_all(&self.client).await;
        assert!(self.registry.offline(NID_B));
    }

    async fn b_online(&self) {
        self.b.pop(&["offline"]);
        self.registry.check_all(&self.client).await;
        assert!(!self.registry.offline(NID_B));
    }

    async fn sessions(&self, query: &Params) -> Value {
        aggregate::sessions(&self.registry, &self.client, query)
            .await
            .unwrap()
    }

    async fn search_lines(&self, query: &Params) -> Vec<Value> {
        let stream =
            aggregate::search_stream(self.registry.clone(), self.client.clone(), query).unwrap();
        stream
            .collect::<Vec<String>>()
            .await
            .into_iter()
            .map(|line| {
                assert!(line.ends_with('\n'), "{line:?}");
                serde_json::from_str(&line).unwrap()
            })
            .collect()
    }
}

fn q(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn names(rows: &Value) -> Vec<&str> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|row| row["node_name"].as_str().unwrap())
        .collect()
}

fn scoped(nid: &str, local: &str) -> String {
    namespace::qualify(nid, local, true).unwrap()
}

fn node_row<'a>(rows: &'a Value, name: &str) -> &'a Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap()
}

#[tokio::test]
async fn sessions_merge_machines_and_the_signature_answers_unchanged() {
    let hub = Hub::new().await;
    let data = hub.sessions(&[]).await;
    assert_eq!(data["partial"], false);
    assert_eq!(data["errors"], json!([]));
    let rows = data["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let uids: Vec<&str> = rows
        .iter()
        .map(|row| row["uid"].as_str().unwrap())
        .collect();
    assert_eq!(uids.len(), 2);
    assert_ne!(uids[0], uids[1]);
    assert!(rows.iter().all(|row| row["cwd"] == "/same/project"));
    for row in rows {
        let (nid, local) = namespace::split(row["uid"].as_str().unwrap(), true).unwrap();
        assert_eq!(row["node_id"], nid);
        assert_eq!(local, "claude:same-file-hash");
    }
    assert_eq!(data["truncated"], false);
    assert_eq!(data["truncated_nodes"], json!([]));
    assert_eq!(data["total_pool"], 0);
    assert_eq!(data["nodes"].as_array().unwrap().len(), 2);
    assert!(data["nodes"][0]["online"].as_bool().unwrap());
    assert!(data["built_at"].as_f64().unwrap() > 1.0e9);
    let sig = data["sig"].as_str().unwrap();
    assert_eq!(sig.len(), 24);

    let same = hub.sessions(&q(&[("sig", sig)])).await;
    assert_eq!(same["unchanged"], true);
    assert_eq!(same["sig"], sig);
    assert_eq!(same["errors"], json!([]));
    // The node rows travel with the short answer (health stamps move; identity does not).
    let identity = |rows: &Value| -> Vec<(String, String, Value)> {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["id"].to_string(),
                    row["name"].to_string(),
                    row["online"].clone(),
                )
            })
            .collect()
    };
    assert_eq!(identity(&same["nodes"]), identity(&data["nodes"]));
    assert!(same["nodes"][0]["last_seen"].as_f64().is_some());
    assert!(same.get("sessions").is_none());
    assert_eq!(same.as_object().unwrap().len(), 4);

    // Any change on a node changes the signature; the old one no longer short-circuits.
    hub.b.set(json!({"deleted": true}));
    let changed = hub.sessions(&q(&[("sig", sig), ("force", "1")])).await;
    assert!(changed.get("unchanged").is_none(), "{changed}");
    assert_ne!(changed["sig"], sig);
    assert_eq!(names(&changed["sessions"]), ["NodeA"]);
    hub.b.pop(&["deleted"]);
}

#[tokio::test]
async fn the_nodes_filter_selects_machines_and_rejects_unregistered_ids() {
    let hub = Hub::new().await;
    let only_b = hub.sessions(&q(&[("nodes", NID_B), ("force", "1")])).await;
    assert_eq!(names(&only_b["sessions"]), ["NodeB"]);
    // The public node list is always complete; only rows are filtered.
    assert_eq!(only_b["nodes"].as_array().unwrap().len(), 2);
    let none = hub.sessions(&q(&[("nodes", "")])).await;
    assert_eq!(none["sessions"], json!([]));
    assert_eq!(none["partial"], false);
    for (path, query) in [
        ("sessions", q(&[("nodes", NID_C)])),
        (
            "search",
            q(&[("q", "needle"), ("nodes", &format!("{NID_A},{NID_C}"))]),
        ),
    ] {
        let error = match path {
            "sessions" => aggregate::sessions(&hub.registry, &hub.client, &query)
                .await
                .unwrap_err(),
            _ => aggregate::search(&hub.registry, &hub.client, &query)
                .await
                .unwrap_err(),
        };
        assert_eq!(error.message(), "筛选包含未注册的机器");
    }
    assert_eq!(
        aggregate::search_stream(
            hub.registry.clone(),
            hub.client.clone(),
            &q(&[("nodes", NID_C)])
        )
        .err()
        .map(|error| error.message().to_string()),
        Some("筛选包含未注册的机器".to_string())
    );
    let search = aggregate::search(
        &hub.registry,
        &hub.client,
        &q(&[("q", "needle"), ("nodes", NID_B)]),
    )
    .await
    .unwrap();
    assert_eq!(names(&search["results"]), ["NodeB"]);
    assert_eq!(search["total_pool"], 1);
    assert!(search.get("sig").is_none());
    let empty = aggregate::search(
        &hub.registry,
        &hub.client,
        &q(&[("q", "needle"), ("nodes", "")]),
    )
    .await
    .unwrap();
    assert_eq!(empty["results"], json!([]));
}

#[tokio::test]
async fn an_offline_node_is_partial_with_stale_cached_rows_and_never_waited_on() {
    let hub = Hub::new().await;
    hub.sessions(&[]).await;
    hub.b_offline().await;
    let started = Instant::now();
    let data = hub.sessions(&[]).await;
    assert!(
        started.elapsed() < hub.client.timeout,
        "waited on the offline node"
    );
    assert_eq!(data["partial"], true);
    let error = &data["errors"][0];
    assert_eq!(error["node_id"], NID_B);
    assert_eq!(error["name"], "NodeB");
    assert_eq!(error["error_code"], "http_error");
    assert!(error["offline_since"].as_f64().is_some(), "{error}");
    assert!(error["last_seen"].as_f64().is_some(), "{error}");
    let stale = data["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["node_name"] == "NodeB")
        .unwrap();
    assert_eq!(stale["stale"], true);
    assert!(stale["last_seen"].as_f64().is_some());
    assert_eq!(stale["uid"], scoped(NID_B, "claude:same-file-hash"));
    assert_eq!(node_row(&data["nodes"], "NodeB")["online"], false);
    assert!(
        node_row(&data["nodes"], "NodeA")["online"]
            .as_bool()
            .unwrap()
    );
    // The search aggregate has no cache: only the healthy node answers.
    let search = aggregate::search(&hub.registry, &hub.client, &q(&[("q", "needle")]))
        .await
        .unwrap();
    assert_eq!(search["partial"], true);
    assert_eq!(names(&search["results"]), ["NodeA"]);
    // The live aggregate merges only nodes that answered.
    let live = aggregate::live(&hub.registry, &hub.client, &[])
        .await
        .unwrap();
    assert_eq!(live["partial"], true);
    assert_eq!(live["uids"], json!([]));
    assert_eq!(live["tmux_uids"], json!([]));
    assert_eq!(live["started_at"], json!({}));
    hub.b_online().await;
    let back = hub.sessions(&[]).await;
    assert_eq!(back["partial"], false);
    assert_eq!(back["sessions"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn term_list_reports_capabilities_per_machine_and_unions_sources() {
    let hub = Hub::new().await;
    let listing = aggregate::term_list(&hub.registry, &hub.client, &[])
        .await
        .unwrap();
    assert_eq!(listing["enabled"], true);
    assert_eq!(listing["home"], "");
    assert_eq!(listing["sources"], json!({"claude": true, "codex": true}));
    assert_eq!(listing["sessions"], json!([]));
    assert_eq!(listing["pending"], json!([]));
    let a = &listing["capabilities"][NID_A];
    assert_eq!(a["enabled"], true);
    assert_eq!(a["backend"], "tmux");
    // The fake node reports `<home>/<name>`; only the node-specific tail matters here.
    assert!(
        a["home"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/{}", hub.a.name)),
        "{a}"
    );
    assert_eq!(a["unavailable_reason"], "");
    let mut backends: Vec<&str> = a["backends"]
        .as_array()
        .unwrap()
        .iter()
        .map(|backend| backend["name"].as_str().unwrap())
        .collect();
    backends.sort();
    assert_eq!(backends, ["ptyhost", "tmux"]);

    // The backend is each machine's own setting; sources union like Python's `or`.
    hub.b.set(
        json!({"backend": "ptyhost", "term_sources": {"claude": false, "grok": true},
                     "term_enabled": false, "term_reason": "no tmux"}),
    );
    let listing = aggregate::term_list(&hub.registry, &hub.client, &[])
        .await
        .unwrap();
    assert_eq!(listing["capabilities"][NID_B]["backend"], "ptyhost");
    assert_eq!(listing["capabilities"][NID_A]["backend"], "tmux");
    assert_eq!(
        listing["capabilities"][NID_B]["backends"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|backend| backend["name"] == "ptyhost")
            .map(|backend| backend["current"].clone())
            .collect::<Vec<_>>(),
        [json!(true)]
    );
    assert_eq!(listing["capabilities"][NID_B]["enabled"], false);
    assert_eq!(
        listing["capabilities"][NID_B]["unavailable_reason"],
        "no tmux"
    );
    assert_eq!(
        listing["sources"],
        json!({"claude": true, "codex": true, "grok": true})
    );
    assert_eq!(listing["enabled"], true);
    hub.b
        .pop(&["backend", "term_sources", "term_enabled", "term_reason"]);

    // A pending terminal row is scoped and stamped; an offline machine's
    // capabilities carry the failure reason and no backends.
    hub.b.set(
        json!({"term_sessions": [{"name": "same-terminal", "uid": "claude:same-file-hash",
                                         "source": "claude", "cwd": "/same"}]}),
    );
    aggregate::term_list(&hub.registry, &hub.client, &[])
        .await
        .unwrap();
    hub.b_offline().await;
    let listing = aggregate::term_list(&hub.registry, &hub.client, &[])
        .await
        .unwrap();
    assert_eq!(listing["partial"], true);
    let b = &listing["capabilities"][NID_B];
    assert_eq!(b["enabled"], false);
    assert_eq!(b["backends"], json!([]));
    assert_eq!(b["sources"], json!({}));
    assert_eq!(b["unavailable_reason"], listing["errors"][0]["error"]);
    assert!(
        b["unavailable_reason"]
            .as_str()
            .unwrap()
            .contains("HTTP 503"),
        "{b}"
    );
    assert_eq!(listing["sources"], json!({"claude": true, "codex": true}));
    let row = &listing["sessions"][0];
    assert_eq!(row["name"], format!("{NID_B}~same-terminal"));
    assert_eq!(row["uid"], scoped(NID_B, "claude:same-file-hash"));
    assert_eq!(row["node_name"], "NodeB");
    assert_eq!(row["stale"], true);
    hub.b_online().await;
    hub.b.pop(&["term_sessions"]);
}

#[tokio::test]
async fn trash_items_are_scoped_per_machine_and_sizes_summed() {
    let hub = Hub::new().await;
    let trash = aggregate::trash(&hub.registry, &hub.client, &[])
        .await
        .unwrap();
    assert_eq!(trash["size"], 20);
    assert_eq!(trash["dir"], "所选机器的本地回收站");
    assert_eq!(trash["partial"], false);
    let items = trash["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let ids: Vec<&str> = items
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_ne!(ids[0], ids[1]);
    for item in items {
        let (nid, local) = namespace::split(item["id"].as_str().unwrap(), false).unwrap();
        assert_eq!(item["node_id"], nid);
        assert_eq!(local, "claude/same-trash");
        assert_eq!(
            namespace::split(item["uid"].as_str().unwrap(), true)
                .unwrap()
                .0,
            nid
        );
    }
    let only_b = aggregate::trash(&hub.registry, &hub.client, &q(&[("nodes", NID_B)]))
        .await
        .unwrap();
    assert_eq!(only_b["size"], 10);
    assert_eq!(names(&only_b["items"]), ["NodeB"]);
    hub.b_offline().await;
    let partial = aggregate::trash(&hub.registry, &hub.client, &[])
        .await
        .unwrap();
    assert_eq!(partial["partial"], true);
    assert_eq!(partial["size"], 10);
    assert_eq!(names(&partial["items"]), ["NodeA"]);
    hub.b_online().await;
}

#[tokio::test]
async fn search_stream_orders_progress_matches_heartbeat_and_result() {
    // B stays silent for longer than the heartbeat interval but within the idle limit.
    let hub = Hub::with_search_idle(Duration::from_secs(3)).await;
    hub.b
        .set(json!({"search_steps": 8, "search_delay": 0.04, "search_prepare_delay": 1.3}));
    let started = Instant::now();
    let events = hub
        .search_lines(&q(&[("q", "needle"), ("progress", "1")]))
        .await;
    hub.b
        .pop(&["search_steps", "search_delay", "search_prepare_delay"]);
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    // First line: every node preparing, totals unknown.
    assert_eq!(kinds[0], "progress");
    assert_eq!(
        events[0],
        json!({"type": "progress", "done": 0, "total": 0, "total_known": false, "nodes": [
            {"id": NID_A, "name": "NodeA", "done": 0, "total": null, "state": "preparing"},
            {"id": NID_B, "name": "NodeB", "done": 0, "total": null, "state": "preparing"}]})
    );
    // Last line: the JSON search body.
    assert_eq!(*kinds.last().unwrap(), "result");
    let result = &events.last().unwrap()["data"];
    assert_eq!(result["partial"], false);
    assert_eq!(result["errors"], json!([]));
    let mut found = names(&result["results"]);
    found.sort();
    assert_eq!(found, ["NodeA", "NodeB"]);
    assert_eq!(result["total_pool"], 2);
    assert_eq!(result["truncated"], false);
    assert_eq!(result["truncated_nodes"], json!([]));
    assert_eq!(result["nodes"].as_array().unwrap().len(), 2);
    // A's matches arrive before B has produced anything; a heartbeat fills B's silence.
    let a_result = kinds
        .iter()
        .position(|kind| *kind == "matches")
        .expect("A's matches");
    let heartbeat = kinds
        .iter()
        .position(|kind| *kind == "heartbeat")
        .expect("heartbeat");
    let b_scanning = events
        .iter()
        .position(|event| {
            event["type"] == "progress" && node_row(&event["nodes"], "NodeB")["state"] == "scanning"
        })
        .expect("B scanning");
    assert!(a_result < heartbeat && heartbeat < b_scanning, "{kinds:?}");
    assert!(
        events.iter().any(|event| event["type"] == "progress"
            && event["done"]
                .as_u64()
                .is_some_and(|done| done > 0 && done < event["total"].as_u64().unwrap())),
        "{events:?}"
    );
    let last_progress = events
        .iter()
        .rev()
        .find(|event| event["type"] == "progress")
        .unwrap();
    assert_eq!(last_progress["total_known"], true);
    assert_eq!(node_row(&last_progress["nodes"], "NodeB")["state"], "done");
    assert_eq!(node_row(&last_progress["nodes"], "NodeA")["state"], "done");
    assert_eq!(last_progress["done"], 9);
    assert_eq!(last_progress["total"], 9);
    // Result matches are emitted after the node's final progress, before the result.
    assert_eq!(kinds[kinds.len() - 2], "matches");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn search_progress_waits_for_all_totals_and_reports_a_truncated_scan() {
    let hub = Hub::new().await;
    hub.b.set(
        json!({"search_prepare_delay": 0.15, "search_steps": 10, "search_stop": 3,
                     "search_pool": 10, "search_scanned": 2, "search_truncated": true}),
    );
    let events = hub
        .search_lines(&q(&[("q", "needle"), ("progress", "1")]))
        .await;
    hub.b.pop(&[
        "search_prepare_delay",
        "search_steps",
        "search_stop",
        "search_pool",
        "search_scanned",
        "search_truncated",
    ]);
    let progress: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == "progress")
        .collect();
    let ready_a = progress
        .iter()
        .find(|event| event["done"] == 1 && event["total_known"] == false)
        .expect("A done while B still preparing");
    assert_eq!(node_row(&ready_a["nodes"], "NodeB")["state"], "preparing");
    assert_eq!(node_row(&ready_a["nodes"], "NodeA")["state"], "done");
    let last = progress.last().unwrap();
    assert_eq!(last["total_known"], true);
    assert_eq!(
        (last["done"].clone(), last["total"].clone()),
        (json!(3), json!(11))
    );
    let limited = node_row(&last["nodes"], "NodeB");
    assert_eq!(
        (
            limited["state"].clone(),
            limited["done"].clone(),
            limited["total"].clone()
        ),
        (json!("limited"), json!(2), json!(10))
    );
    let result = &events.last().unwrap()["data"];
    assert_eq!(result["truncated"], true);
    assert_eq!(result["truncated_nodes"], json!([NID_B]));
    assert_eq!(result["total_pool"], 11);
}

#[tokio::test]
async fn search_failures_keep_health_and_retain_matches_already_streamed() {
    let hub = Hub::new().await;
    aggregate::live(&hub.registry, &hub.client, &[])
        .await
        .unwrap();
    for option in ["search_error", "search_incomplete", "search_delay"] {
        hub.b
            .set(json!({option: if option == "search_delay" { json!(0.6) } else { json!(true) }}));
        let events = hub
            .search_lines(&q(&[("q", "needle"), ("progress", "1")]))
            .await;
        hub.b.pop(&[option]);
        let result = &events.last().unwrap()["data"];
        assert_eq!(result["partial"], true, "{option}");
        assert_eq!(names(&result["results"]), ["NodeA"], "{option}");
        assert_eq!(result["errors"][0]["name"], "NodeB", "{option}");
        assert!(
            node_row(&result["nodes"], "NodeB")["online"]
                .as_bool()
                .unwrap(),
            "{option}"
        );
        let last_progress = events
            .iter()
            .rev()
            .find(|event| event["type"] == "progress")
            .unwrap();
        assert_eq!(
            node_row(&last_progress["nodes"], "NodeB")["state"],
            "error",
            "{option}"
        );
        assert_eq!(
            node_row(&last_progress["nodes"], "NodeA")["state"],
            "done",
            "{option}"
        );
        let text = serde_json::to_string(&events).unwrap();
        assert!(!text.contains("private upstream details"), "{option}");
    }
    hub.b
        .set(json!({"search_matches": true, "search_error": true}));
    let events = hub
        .search_lines(&q(&[("q", "needle"), ("progress", "1")]))
        .await;
    hub.b.pop(&["search_matches", "search_error"]);
    let result = &events.last().unwrap()["data"];
    assert_eq!(result["partial"], true);
    let mut found = names(&result["results"]);
    found.sort();
    assert_eq!(found, ["NodeA", "NodeB"]);
    for row in result["results"].as_array().unwrap() {
        assert_eq!(
            namespace::split(row["uid"].as_str().unwrap(), true)
                .unwrap()
                .0,
            row["node_id"]
        );
    }
    // B's streamed matches were forwarded as a `matches` line before its error.
    assert!(
        events
            .iter()
            .any(|event| event["type"] == "matches" && names(&event["results"]) == ["NodeB"]),
        "{events:?}"
    );
}

#[tokio::test]
async fn search_stream_empty_selection_offline_node_and_json_node_compatibility() {
    let hub = Hub::new().await;
    let events = hub
        .search_lines(&q(&[("q", "needle"), ("progress", "1"), ("nodes", "")]))
        .await;
    assert_eq!(
        events[0],
        json!({"type": "progress", "done": 0, "total": 0, "total_known": true, "nodes": []})
    );
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["data"]["results"], json!([]));
    assert_eq!(events[1]["data"]["partial"], false);

    hub.b.set(json!({"search_json": true}));
    let events = hub
        .search_lines(&q(&[("q", "needle"), ("progress", "1"), ("nodes", NID_B)]))
        .await;
    hub.b.pop(&["search_json"]);
    let result = &events.last().unwrap()["data"];
    assert_eq!(result["partial"], false);
    assert_eq!(names(&result["results"]), ["NodeB"]);
    assert_eq!(
        node_row(&events[events.len() - 3]["nodes"], "NodeB")["state"],
        "done"
    );

    hub.b_offline().await;
    let started = Instant::now();
    let events = hub
        .search_lines(&q(&[("q", "needle"), ("progress", "1")]))
        .await;
    assert!(
        started.elapsed() < hub.client.search_idle,
        "waited on the offline node"
    );
    let first = node_row(&events[0]["nodes"], "NodeB");
    assert_eq!(
        (first["state"].clone(), first["total"].clone()),
        (json!("offline"), json!(0))
    );
    let result = &events.last().unwrap()["data"];
    assert_eq!(result["partial"], true);
    assert_eq!(result["errors"][0]["node_id"], NID_B);
    assert!(result["errors"][0]["offline_since"].as_f64().is_some());
    assert_eq!(names(&result["results"]), ["NodeA"]);
    let last_progress = events
        .iter()
        .rev()
        .find(|event| event["type"] == "progress")
        .unwrap();
    assert_eq!(
        node_row(&last_progress["nodes"], "NodeB")["state"],
        "offline"
    );
    hub.b_online().await;
}

#[tokio::test]
async fn dropping_the_search_stream_cancels_the_scans_and_leaves_the_registry_usable() {
    let hub = Hub::new().await;
    hub.b.set(json!({"search_steps": 40, "search_delay": 0.05}));
    {
        let mut stream = Box::pin(
            aggregate::search_stream(
                hub.registry.clone(),
                hub.client.clone(),
                &q(&[("q", "needle"), ("progress", "1")]),
            )
            .unwrap(),
        );
        let first = stream.next().await.unwrap();
        assert!(first.starts_with("{\"type\":\"progress\""));
    }
    hub.b.pop(&["search_steps", "search_delay"]);
    let started = Instant::now();
    let live = aggregate::live(&hub.registry, &hub.client, &[])
        .await
        .unwrap();
    assert_eq!(live["partial"], false);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        node_row(&live["nodes"], "NodeB")["online"]
            .as_bool()
            .unwrap()
    );
}

#[tokio::test]
async fn bulk_delete_is_split_per_machine_and_reports_failed_machines_per_uid() {
    let hub = Hub::new().await;
    let a = scoped(NID_A, "claude:same-file-hash");
    let b = scoped(NID_B, "claude:same-file-hash");
    let c = scoped(NID_C, "codex:elsewhere");
    let data = aggregate::delete(&hub.registry, &hub.client, &json!({"uids": [&a, &b, &a]}))
        .await
        .unwrap();
    assert_eq!(data["ok"], true);
    assert_eq!(data["errors"], json!([]));
    assert_eq!(data["deleted"], json!([{"uid": a}, {"uid": a}, {"uid": b}]));
    assert_eq!(
        hub.a.writes().last().unwrap(),
        &(
            "/api/sessions/delete".to_string(),
            json!({"uids": ["claude:same-file-hash", "claude:same-file-hash"], "force": false})
        )
    );
    assert_eq!(
        hub.b.writes().last().unwrap().1,
        json!({"uids": ["claude:same-file-hash"], "force": false})
    );
    hub.a.pop(&["deleted"]);
    hub.b.pop(&["deleted"]);

    // Unknown machine and a machine that answers 503: every uid of that group is an error.
    hub.b.set(json!({"offline": true}));
    let data = aggregate::delete(&hub.registry, &hub.client, &json!({"uids": [&c, &b, &a]}))
        .await
        .unwrap();
    hub.b.pop(&["offline"]);
    assert_eq!(data["deleted"], json!([{"uid": a}]));
    assert_eq!(
        data["errors"],
        json!([{"uid": c, "error": "机器请求失败，请核对结果"},
               {"uid": b, "error": "机器请求失败，请核对结果"}])
    );
    hub.a.pop(&["deleted"]);

    let _ = aggregate::delete(
        &hub.registry,
        &hub.client,
        &json!({"uids": [&a], "force": true}),
    )
    .await
    .unwrap();
    assert_eq!(
        hub.a.writes().last().unwrap().1,
        json!({"uids": ["claude:same-file-hash"], "force": true})
    );
    hub.a.pop(&["deleted"]);

    for (body, message) in [
        (json!({}), "没有选中任何会话"),
        (json!({"uids": []}), "没有选中任何会话"),
        (
            json!({"uids": ["claude:same-file-hash"]}),
            "missing or invalid machine reference",
        ),
        (json!({"uids": ["nosource"]}), "missing session source"),
        (json!({"uids": [&a], "force": "true"}), "需要布尔值 force"),
    ] {
        assert_eq!(
            aggregate::delete(&hub.registry, &hub.client, &body)
                .await
                .unwrap_err()
                .message(),
            message,
            "{body}"
        );
    }
}

#[tokio::test]
async fn fork_visibility_is_grouped_by_machine_and_requalified() {
    let hub = Hub::new().await;
    let uids = [scoped(NID_A, "codex:parent"), scoped(NID_B, "codex:parent")];
    let data = aggregate::fork_visibility(
        &hub.registry,
        &hub.client,
        &json!({"uids": uids, "visible": true}),
    )
    .await
    .unwrap();
    assert_eq!(data["ok"], true);
    assert_eq!(data["errors"], json!([]));
    let updated = data["updated"].as_array().unwrap();
    assert_eq!(
        updated
            .iter()
            .map(|row| row["uid"].as_str().unwrap())
            .collect::<Vec<_>>(),
        uids.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert!(updated.iter().all(|row| row["fork_parent_visible"] == true));
    assert_eq!(
        hub.a.writes().last().unwrap(),
        &(
            "/api/sessions/fork-visibility".to_string(),
            json!({"uids": ["codex:parent"], "visible": true})
        )
    );
    assert_eq!(
        hub.b.writes().last().unwrap().0,
        "/api/sessions/fork-visibility"
    );
    for (body, message) in [
        (json!({"uids": [uids[0].clone()]}), "需要布尔值 visible"),
        (
            json!({"uids": [uids[0].clone()], "visible": "yes"}),
            "需要布尔值 visible",
        ),
        (json!({"visible": false}), "没有选中任何父会话"),
    ] {
        assert_eq!(
            aggregate::fork_visibility(&hub.registry, &hub.client, &body)
                .await
                .unwrap_err()
                .message(),
            message,
            "{body}"
        );
    }
}

#[tokio::test]
async fn purge_all_sums_every_selected_machine_and_prefixes_failures_with_the_name() {
    let hub = Hub::new().await;
    let data = aggregate::purge_all(&hub.registry, &hub.client, &[])
        .await
        .unwrap();
    assert_eq!(
        data,
        json!({"ok": true, "removed": 2, "freed": 20, "errors": []})
    );
    assert_eq!(
        hub.a.writes().last().unwrap(),
        &("/api/trash/purge".to_string(), json!({"all": true}))
    );
    assert_eq!(hub.b.writes().last().unwrap().0, "/api/trash/purge");
    let only_b = aggregate::purge_all(&hub.registry, &hub.client, &q(&[("nodes", NID_B)]))
        .await
        .unwrap();
    assert_eq!(only_b["removed"], 1);
    assert_eq!(
        aggregate::purge_all(&hub.registry, &hub.client, &q(&[("nodes", NID_C)]))
            .await
            .unwrap_err()
            .message(),
        "筛选包含未注册的机器"
    );
    hub.b.set(json!({"offline": true}));
    let data = aggregate::purge_all(&hub.registry, &hub.client, &[])
        .await
        .unwrap();
    hub.b.pop(&["offline"]);
    assert_eq!(
        data,
        json!({"ok": true, "removed": 1, "freed": 10, "errors": ["NodeB: 请求失败，请核对结果"]})
    );
}

#[test]
fn python_lookup_prefers_python3_exe_then_python_exe_in_isolated_dirs() {
    let early = tempfile::tempdir().unwrap();
    let late = tempfile::tempdir().unwrap();
    std::fs::write(early.path().join("python3.exe"), []).unwrap();
    std::fs::write(early.path().join("python.exe"), b"python").unwrap();
    std::fs::write(late.path().join("python3.exe"), b"python3").unwrap();
    std::fs::write(late.path().join("python3"), b"python3").unwrap();
    let path = std::env::join_paths([early.path(), late.path()]).unwrap();

    assert_eq!(
        first_file_on_path(&path, &["python3.exe", "python.exe"]),
        Some(late.path().join("python3.exe"))
    );
    std::fs::remove_file(late.path().join("python3.exe")).unwrap();
    assert_eq!(
        first_file_on_path(&path, &["python3.exe", "python.exe"]),
        Some(early.path().join("python.exe"))
    );
    assert_eq!(
        first_file_on_path(&path, &["python3"]),
        Some(late.path().join("python3"))
    );
    assert_eq!(
        python_from_path(None),
        PathBuf::from(if cfg!(windows) {
            "python.exe"
        } else {
            "/usr/bin/python3"
        })
    );
}
