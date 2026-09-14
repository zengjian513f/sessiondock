//! Isolated HTTP reads of preexisting synthetic ledgers; no CLI/model execution.
#![cfg(unix)]

use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sessiondock::{
    config::Config,
    delivery::{claude, codex, engine::DeliveryEngine, store},
    prepare_app,
    sessions::SessionRoots,
};
use sha1::{Digest, Sha1};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

static TEST_GATE: Semaphore = Semaphore::const_new(1);
const CLAUDE_SID: &str = "claude-native-parent";
const AGENT: &str = "helper-full-agent-id";
const CODEX_SID: &str = "codex-real-session-id";
const CODEX_CHILD: &str = "codex-real-child-id";

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    ledger: PathBuf,
    claude_path: PathBuf,
    codex_path: PathBuf,
    claude_uid: String,
    codex_uid: String,
    codex_child_uid: String,
    grok_uid: String,
}
fn uid(source: &str, path: &Path) -> String {
    format!(
        "{source}:{}",
        &format!("{:x}", Sha1::digest(path.to_string_lossy().as_bytes()))[..16]
    )
}
fn jsonl(path: &Path, records: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let ledger = root.join("ledger");
        fs::create_dir(&ledger).unwrap();
        fs::set_permissions(&ledger, fs::Permissions::from_mode(0o700)).unwrap();
        let claude_path = root.join("claude/project/claude-file.jsonl");
        jsonl(
            &claude_path,
            &[
                json!({"type":"user", "uuid":"parent-user", "parentUuid":null, "sessionId":CLAUDE_SID, "cwd":"/synthetic/project", "timestamp":"2026-09-11T10:00:00Z", "message":{"role":"user", "content":"synthetic native parent"}}),
            ],
        );
        jsonl(
            &root.join(format!(
                "claude/project/claude-file/subagents/agent-{AGENT}.jsonl"
            )),
            &[
                json!({"type":"assistant", "uuid":"child-assistant", "parentUuid":null, "sessionId":CLAUDE_SID, "agentId":AGENT, "isSidechain":true, "timestamp":"2026-09-11T10:00:01Z", "message":{"role":"assistant", "content":"synthetic child history"}}),
            ],
        );
        let codex_path = root.join("codex/2026/09/11/rollout-file-label.jsonl");
        jsonl(
            &codex_path,
            &[
                json!({"type":"session_meta", "payload":{"id":CODEX_SID, "session_id":CODEX_SID, "cwd":"/synthetic/project", "timestamp":"2026-09-11T10:00:00Z"}}),
                json!({"type":"response_item", "timestamp":"2026-09-11T10:00:01Z", "payload":{"type":"message", "role":"user", "content":[{"type":"input_text", "text":"synthetic codex native input"}]}}),
            ],
        );
        let child_path = root.join("codex/2026/09/11/rollout-child-file.jsonl");
        jsonl(
            &child_path,
            &[
                json!({"type":"session_meta", "payload":{"id":CODEX_CHILD, "session_id":CODEX_SID, "forked_from_id":CODEX_SID, "thread_source":"subagent", "parent_thread_id":CODEX_SID, "source":{"subagent":{"thread_spawn":{"parent_thread_id":CODEX_SID, "agent_path":"/root/synthetic-child", "agent_role":"reviewer"}}}, "cwd":"/synthetic/project", "timestamp":"2026-09-11T10:00:00Z"}}),
                json!({"type":"response_item", "timestamp":"2026-09-11T10:00:01Z", "payload":{"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"synthetic codex child"}]}}),
            ],
        );
        let grok_path = root.join("grok/project/grok-real-session");
        fs::create_dir_all(&grok_path).unwrap();
        fs::write(grok_path.join("summary.json"), json!({"info":{"id":"grok-real-session", "cwd":"/synthetic/project"}, "generated_title":"synthetic Grok"}).to_string()).unwrap();
        jsonl(
            &grok_path.join("chat_history.jsonl"),
            &[json!({"type":"user", "prompt_index":1, "content":"synthetic grok input"})],
        );
        fs::create_dir(root.join("files")).unwrap();
        fs::write(root.join("files/untouched.txt"), b"synthetic file content").unwrap();
        let fixture = Self {
            claude_uid: uid("claude", &claude_path),
            codex_uid: uid("codex", &codex_path),
            codex_child_uid: uid("codex", &child_path),
            grok_uid: uid("grok", &grok_path),
            _temp: temp,
            root,
            ledger,
            claude_path,
            codex_path,
        };
        let mut engine = DeliveryEngine::initialize(&fixture.ledger).unwrap();
        for (id, sid, agent, text) in [
            (
                "claude-main-request",
                CLAUDE_SID,
                None,
                "MAIN_ONLY_PRIVATE_PROMPT",
            ),
            (
                "claude-child-request",
                CLAUDE_SID,
                Some(AGENT),
                "CHILD_ONLY_PRIVATE_PROMPT",
            ),
            (
                "claude-wrong-scope",
                "claude-file",
                None,
                "WRONG_SCOPE_PRIVATE_PROMPT",
            ),
            (
                "claude-tombstone",
                CLAUDE_SID,
                None,
                "TOMBSTONE_PRIVATE_PROMPT",
            ),
        ] {
            engine
                .apply_claude(claude::Command::Enqueue {
                    request: claude::Request {
                        id: id.into(),
                        payload: claude::Payload {
                            scope: claude::Scope {
                                uid: fixture.claude_uid.clone(),
                                session_id: sid.into(),
                                agent_id: agent.map(str::to_owned),
                            },
                            target: claude::Target {
                                host_instance: "PRIVATE_HOST_INSTANCE".into(),
                                terminal_id: "private-terminal".into(),
                                ownership_epoch: "PRIVATE_OWNERSHIP".into(),
                            },
                            text: text.into(),
                            attachments: vec![],
                        },
                    },
                    now_ms: 1234,
                })
                .unwrap()
                .claim()
                .unwrap();
        }
        engine
            .apply_claude(claude::Command::Cancel {
                id: "claude-tombstone".into(),
                scope: claude::Scope {
                    uid: fixture.claude_uid.clone(),
                    session_id: CLAUDE_SID.into(),
                    agent_id: None,
                },
            })
            .unwrap()
            .claim()
            .unwrap();
        engine
            .apply_codex(
                codex::Command::Submit {
                    request: codex::Request {
                        request_id: "codex-main-request".into(),
                        payload: codex::Payload {
                            uid: fixture.codex_uid.clone(),
                            target: codex::Target {
                                host_instance: "PRIVATE_HOST_INSTANCE".into(),
                                session_id: CODEX_SID.into(),
                                ownership_epoch: "PRIVATE_OWNERSHIP".into(),
                            },
                            text: "CODEX_ONLY_PRIVATE_PROMPT".repeat(3000),
                            media: vec![],
                        },
                    },
                    now_ms: 2345,
                },
                None,
            )
            .unwrap()
            .claim()
            .unwrap();
        drop(engine);
        fixture
    }
    fn config(&self) -> Config {
        Config {
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            delivery_dir: Some(self.ledger.clone()),
            roots: SessionRoots {
                claude: Some(self.root.join("claude")),
                codex: Some(self.root.join("codex")),
                grok: Some(self.root.join("grok")),
            },
            file_roots: vec![self.root.join("files")],
            ..Config::default()
        }
    }
    fn uri(&self, source: &str, agent: Option<&str>) -> String {
        let uid = match source {
            "claude" => &self.claude_uid,
            "codex" => &self.codex_uid,
            "grok" => &self.grok_uid,
            _ => panic!("fixture source"),
        };
        format!(
            "/api/session/outbox?uid={uid}{}",
            agent
                .map(|agent| format!("&agent={agent}"))
                .unwrap_or_default()
        )
    }
    fn inputs(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(&path, files);
                } else {
                    files.insert(path.clone(), fs::read(path).unwrap());
                }
            }
        }
        let mut files = BTreeMap::new();
        for root in ["claude", "codex", "grok", "files"] {
            visit(&self.root.join(root), &mut files);
        }
        files
    }
    fn sanitized(&self, value: &Value) {
        let body = value.to_string();
        for secret in [
            self.root.to_str().unwrap(),
            "PRIVATE_HOST_INSTANCE",
            "PRIVATE_OWNERSHIP",
            "TOMBSTONE_PRIVATE_PROMPT",
            "WRONG_SCOPE_PRIVATE_PROMPT",
        ] {
            assert!(!body.contains(secret), "private data exposed");
        }
    }
}
async fn fixture() -> (tokio::sync::SemaphorePermit<'static>, Fixture) {
    let gate = TEST_GATE.acquire().await.unwrap();
    let fixture = tokio::task::spawn_blocking(Fixture::new).await.unwrap();
    (gate, fixture)
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
async fn body(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}
async fn ok(app: &Router, uri: &str) -> Value {
    let response = get(app, uri).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    body(response).await
}

#[tokio::test]
async fn outbox_uses_uid_and_ignores_the_legacy_agent_selector() {
    let (_gate, f) = fixture().await;
    let original = f.inputs();
    let prepared = prepare_app(f.config(), CancellationToken::new())
        .await
        .unwrap();
    let committed = fs::read(f.ledger.join(store::LEDGER_FILENAME)).unwrap();
    let main = ok(&prepared.router, &f.uri("claude", None)).await;
    assert_eq!(
        main["outbox"],
        json!([{"id":"claude-main-request","uid":f.claude_uid,"text":"MAIN_ONLY_PRIVATE_PROMPT","media":[],"created":1234,"state":"persisted","attempts":0,"server":true}])
    );
    let child = ok(&prepared.router, &f.uri("claude", Some(AGENT))).await;
    assert_eq!(
        child["outbox"],
        json!([{"id":"claude-main-request","uid":f.claude_uid,"text":"MAIN_ONLY_PRIVATE_PROMPT","media":[],"created":1234,"state":"persisted","attempts":0,"server":true}])
    );
    let codex = ok(&prepared.router, &f.uri("codex", None)).await;
    assert_eq!(codex["outbox"].as_array().unwrap().len(), 1);
    assert_eq!(codex["outbox"][0]["id"], "codex-main-request");
    assert_eq!(codex["outbox"][0]["state"], "failed");
    assert_eq!(codex["outbox"][0]["attempts"], 0);
    for projected in [&main, &child, &codex] {
        f.sanitized(projected);
        assert!(projected["outbox_version"]["epoch"].is_string());
        assert!(projected["outbox_version"]["revision"].is_u64());
        assert!(projected["outbox"][0].get("afterTs").is_none());
    }
    assert!(!child.to_string().contains("CHILD_ONLY_PRIVATE_PROMPT"));
    assert!(!main.to_string().contains("CHILD_ONLY_PRIVATE_PROMPT"));
    let meta = body(get(&prepared.router, "/api/meta").await).await;
    assert_eq!(meta["capabilities"]["outbox_read"], true);
    assert_eq!(meta["capabilities"]["outbox"], false);
    assert_eq!(meta["capabilities"]["mutations"], false);
    for route in [
        "send",
        "draft-status",
        "outbox/retry",
        "outbox/discard",
        "stop",
    ] {
        let mut req = request(&format!("/api/session/{route}"));
        *req.method_mut() = Method::POST;
        let response = prepared.router.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }
    prepared
        .delivery
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    assert_eq!(f.inputs(), original);
    assert_eq!(
        fs::read(f.ledger.join(store::LEDGER_FILENAME)).unwrap(),
        committed
    );
}

#[tokio::test]
async fn outbox_query_matches_python_first_uid_and_ignores_other_fields() {
    let (_gate, f) = fixture().await;
    let prepared = prepare_app(f.config(), CancellationToken::new())
        .await
        .unwrap();
    for uri in [
        "/api/session/outbox".into(),
        "/api/session/outbox?uid=".into(),
        "/api/session/outbox?uid=claude:0000000000000000".into(),
        format!("/api/session/outbox?uid={}", "x".repeat(257)),
        f.uri("grok", None),
    ] {
        let response = get(&prepared.router, &uri).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let snapshot = body(response).await;
        assert_eq!(snapshot["outbox"], json!([]));
        assert_eq!(snapshot["outbox_version"]["epoch"], "none");
        f.sanitized(&snapshot);
    }

    for uri in [
        format!("{}&unknown=1", f.uri("claude", None)),
        format!("{}&uid=duplicate", f.uri("claude", None)),
        format!("{}&agent={}", f.uri("claude", None), "x".repeat(257)),
        f.uri("claude", Some("..%2F..%2Fescape")),
        f.uri("codex", Some(CODEX_CHILD)),
    ] {
        let response = get(&prepared.router, &uri).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let snapshot = body(response).await;
        assert!(snapshot["outbox"].is_array());
        f.sanitized(&snapshot);
    }
    // Outbox rows are keyed by the supplied UID. An unrepresented child UID
    // therefore has the same empty unsupported-source snapshot as any miss.
    let response = get(
        &prepared.router,
        &format!("/api/session/outbox?uid={}", f.codex_child_uid),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = body(response).await;
    assert_eq!(snapshot["outbox"], json!([]));
    f.sanitized(&snapshot);
    prepared
        .delivery
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}

#[tokio::test]
async fn missing_and_conflicting_native_identity_return_python_empty_outbox() {
    let (_gate, f) = fixture().await;
    for records in [
        vec![
            json!({"type":"user","uuid":"missing-id","message":{"role":"user","content":"synthetic"}}),
        ],
        vec![
            json!({"type":"user","uuid":"first-id","sessionId":CLAUDE_SID,"message":{"role":"user","content":"synthetic"}}),
            json!({"type":"user","uuid":"second-id","sessionId":"different-native-session","message":{"role":"user","content":"synthetic"}}),
        ],
    ] {
        jsonl(&f.claude_path, &records);
        let prepared = prepare_app(f.config(), CancellationToken::new())
            .await
            .unwrap();
        for agent in [None, Some(AGENT)] {
            let response = get(&prepared.router, &f.uri("claude", agent)).await;
            assert_eq!(response.status(), StatusCode::OK);
            let snapshot = body(response).await;
            f.sanitized(&snapshot);
            assert_eq!(snapshot["outbox"], json!([]));
            assert_eq!(snapshot["outbox_version"]["epoch"], "none");
            assert_eq!(snapshot["outbox_version"]["revision"], 0);
        }
        prepared
            .delivery
            .as_ref()
            .unwrap()
            .shutdown()
            .await
            .unwrap();
    }
    jsonl(
        &f.codex_path,
        &[
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"missing session_meta"}]}}),
        ],
    );
    let prepared = prepare_app(f.config(), CancellationToken::new())
        .await
        .unwrap();
    let response = get(&prepared.router, &f.uri("codex", None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = body(response).await;
    assert_eq!(snapshot["outbox"], json!([]));
    assert_eq!(snapshot["outbox_version"]["epoch"], "none");
    prepared
        .delivery
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}

#[tokio::test]
async fn disabled_read_stays_disabled_while_missing_and_shared_ledgers_open_normally() {
    let (_gate, f) = fixture().await;
    let mut config = f.config();
    config.delivery_dir = None;
    let prepared = prepare_app(config, CancellationToken::new()).await.unwrap();
    assert!(prepared.delivery.is_none());
    let response = get(&prepared.router, &f.uri("claude", None)).await;
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body(response).await["code"], "delivery_disabled");
    assert_eq!(
        body(get(&prepared.router, "/api/meta").await).await["capabilities"]["outbox_read"],
        false
    );
    let empty = f.root.join("uninitialized");
    fs::create_dir(&empty).unwrap();
    fs::set_permissions(&empty, fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = f.config();
    config.delivery_dir = Some(empty.clone());
    let initialized = prepare_app(config, CancellationToken::new()).await.unwrap();
    assert!(empty.join(store::LEDGER_FILENAME).is_file());
    initialized.delivery.unwrap().shutdown().await.unwrap();
    let prepared = prepare_app(f.config(), CancellationToken::new())
        .await
        .unwrap();
    let shared = prepare_app(f.config(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        ok(&shared.router, &f.uri("claude", None)).await["outbox"],
        ok(&prepared.router, &f.uri("claude", None)).await["outbox"]
    );
    shared.delivery.unwrap().shutdown().await.unwrap();
    prepared
        .delivery
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    let opened = prepare_app(f.config(), CancellationToken::new())
        .await
        .unwrap();
    opened.delivery.unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn restart_changes_both_epochs_and_shutdown_releases_lock_with_router_still_alive() {
    let (_gate, f) = fixture().await;
    let first = prepare_app(f.config(), CancellationToken::new())
        .await
        .unwrap();
    let old_c = ok(&first.router, &f.uri("codex", None)).await;
    let old_h = ok(&first.router, &f.uri("claude", None)).await;
    first.delivery.as_ref().unwrap().shutdown().await.unwrap();
    let response = get(&first.router, &f.uri("claude", None)).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body(response).await["code"], "delivery_closed");
    let next = prepare_app(f.config(), CancellationToken::new())
        .await
        .unwrap();
    let new_c = ok(&next.router, &f.uri("codex", None)).await;
    let new_h = ok(&next.router, &f.uri("claude", None)).await;
    for (before, after) in [(&old_c, &new_c), (&old_h, &new_h)] {
        assert_ne!(
            before["outbox_version"]["epoch"],
            after["outbox_version"]["epoch"]
        );
        assert_eq!(before["outbox"], after["outbox"]);
    }
    next.delivery.unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn syntactic_external_ledger_edit_reloads_and_preserves_external_bytes() {
    let (_gate, f) = fixture().await;
    let prepared = prepare_app(f.config(), CancellationToken::new())
        .await
        .unwrap();
    let path = f.ledger.join(store::LEDGER_FILENAME);
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b' ');
    fs::write(&path, &bytes).unwrap();
    for source in ["claude", "codex"] {
        let response = get(&prepared.router, &f.uri(source, None)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let snapshot = body(response).await;
        assert!(snapshot["outbox"].is_array());
        f.sanitized(&snapshot);
    }
    assert_eq!(fs::read(path).unwrap(), bytes);
    prepared.delivery.unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn unconsumed_response_bodies_do_not_reject_more_reads() {
    let (_gate, f) = fixture().await;
    let prepared = prepare_app(f.config(), CancellationToken::new())
        .await
        .unwrap();
    let uri = f.uri("codex", None);
    let mut responses = Vec::new();
    for _ in 0..8 {
        let response = get(&prepared.router, &uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        responses.push(response);
    }
    let ninth = get(&prepared.router, &uri).await;
    assert_eq!(ninth.status(), StatusCode::OK);
    responses.push(ninth);
    // A partially consumed multi-frame stream must still own its response guard.
    let response = responses.pop().unwrap();
    let mut partial = response.into_body();
    let frame = partial.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(frame.len(), 32 * 1024);
    assert_eq!(get(&prepared.router, &uri).await.status(), StatusCode::OK);
    drop(partial);
    let replacement = get(&prepared.router, &uri).await;
    assert_eq!(replacement.status(), StatusCode::OK);
    responses.push(replacement);
    let consumed = body(responses.pop().unwrap()).await;
    assert_eq!(consumed["outbox"][0]["id"], "codex-main-request");
    let replacement = get(&prepared.router, &uri).await;
    assert_eq!(replacement.status(), StatusCode::OK);
    responses.push(replacement);
    // Existing bodies cannot keep the ledger lock after explicit service shutdown.
    prepared
        .delivery
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    let next = prepare_app(f.config(), CancellationToken::new())
        .await
        .unwrap();
    next.delivery.unwrap().shutdown().await.unwrap();
    drop(responses);
}

#[tokio::test]
async fn static_asset_failure_after_ledger_open_waits_for_lock_release_before_returning() {
    let (_gate, f) = fixture().await;
    let native = f.inputs();
    let path = f.ledger.join(store::LEDGER_FILENAME);
    let before: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let empty_web = f.root.join("empty-web");
    fs::create_dir(&empty_web).unwrap();
    let mut config = f.config();
    config.web_dir = empty_web;
    let result = prepare_app(config, CancellationToken::new()).await;
    assert!(matches!(result, Err(ref error) if error.kind() == std::io::ErrorKind::NotFound));
    let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    // Prove failure happened after both durable recovery commits, rather than
    // before the service obtained the lock.
    assert_ne!(
        before["codex"]["version"]["epoch"],
        after["codex"]["version"]["epoch"]
    );
    assert_ne!(
        before["claude"]["version"]["epoch"],
        after["claude"]["version"]["epoch"]
    );
    let reopened = prepare_app(f.config(), CancellationToken::new())
        .await
        .unwrap();
    reopened.delivery.unwrap().shutdown().await.unwrap();
    assert_eq!(f.inputs(), native);
}
