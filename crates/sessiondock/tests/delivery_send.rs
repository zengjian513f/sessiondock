//! Claude reliable send end to end (batch 31): the HTTP router, an isolated
//! ptyhost, the lifecycle launcher and the delivery ledger, with the fake
//! Claude CLI from `tests/fake_claude_cli.py` as the only "CLI". It renders a
//! Claude-like composer and appends synthetic native `user` records for every
//! submitted line into the isolated Claude root. No model binary, native CLI
//! home or production host is touched. Skips when ptyhost is not built.
#![cfg(unix)]

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use ptyhost_client::{ControlOp, HostClient, Limits};
use serde_json::{Value, json};
use sessiondock::{
    config::Config, delivery::engine::DeliveryEngine, lifecycle::store::LifecycleStore,
    prepare_app, sessions::SessionRoots,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const FAKE_CLI: &str = include_str!("../../../tests/fake_claude_cli.py");

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    lifecycle: PathBuf,
    host: PathBuf,
    claude_root: PathBuf,
    claude_area: PathBuf,
    delivery: PathBuf,
    web: PathBuf,
    launcher: PathBuf,
}

fn directory(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn file(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
fn ptyhost_binary() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/ptyhost");
    path.canonicalize().ok().filter(|path| path.is_file())
}
fn python3() -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("python3"))
                .find(|candidate| candidate.is_file())
        })
        .unwrap_or_else(|| PathBuf::from("/usr/bin/python3"))
}

impl Fixture {
    fn new(host_binary: &Path, profile: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let lifecycle = root.join("lifecycle");
        let host = root.join("hosts");
        let work = root.join("work");
        let claude_area = work.join("claude-area");
        let claude_root = root.join("claude");
        let delivery = root.join("delivery");
        let web = root.join("web");
        let bin = root.join("bin");
        for path in [
            &lifecycle,
            &host,
            &claude_area,
            &claude_root,
            &delivery,
            &web,
            &bin,
        ] {
            directory(path);
        }
        file(&web.join("index.html"),b"<!doctype html><meta name=\"sessiondock-mode\" content=\"local\"><title>synthetic</title>",0o600);
        let script = bin.join("fake_claude_cli.py");
        file(&script, FAKE_CLI.as_bytes(), 0o600);
        let wrapper = format!(
            "#!/bin/sh\nexec {} {} \"$@\"\n",
            python3().display(),
            script.display()
        );
        for name in ["fake-claude", "fake-claude-slow", "fake-claude-swallow"] {
            file(&bin.join(name), wrapper.as_bytes(), 0o700);
        }
        let env = json!({"PATH":"/usr/bin:/bin","HOME":root.join("home"),
            "SESSIONDOCK_TEST_CLAUDE_ROOT":claude_root,"LANG":"C.UTF-8"});
        directory(&root.join("home"));
        let launcher = root.join("launcher.json");
        let (executable, args, resume_args) = match profile {
            "normal" => (
                bin.join("fake-claude"),
                json!(["--settings", "/synthetic/bridge-settings.json"]),
                json!(["--resume", "{sid}"]),
            ),
            "slow" => (
                bin.join("fake-claude-slow"),
                json!(["--delay", "1500", "--busy-footer", "--reply"]),
                json!([]),
            ),
            "swallow" => (
                bin.join("fake-claude-swallow"),
                json!(["--swallow", "2"]),
                json!([]),
            ),
            _ => panic!("unknown fake profile"),
        };
        let config = json!({"schema":2,"host_binary":host_binary,"host_dir":host,
        "adapters":[],
        "profiles":[
            {"id":"claude-test-v1","source":"claude","executable":executable,
             "args":args,
             "new_args":["--session-id","{session_id}"],
             "resume_args":resume_args,
             "env":env}
        ]});
        file(&launcher, config.to_string().as_bytes(), 0o600);
        drop(LifecycleStore::initialize(&lifecycle).unwrap());
        drop(DeliveryEngine::initialize(&delivery).unwrap());
        Self {
            _temp: temp,
            root,
            lifecycle,
            host,
            claude_root,
            claude_area,
            delivery,
            web,
            launcher,
        }
    }
    fn config(&self) -> Config {
        Config {
            web_dir: self.web.clone(),
            ptyhost_dir: Some(self.host.clone()),
            lifecycle_dir: Some(self.lifecycle.clone()),
            launcher_config: Some(self.launcher.clone()),
            delivery_dir: Some(self.delivery.clone()),
            roots: SessionRoots {
                claude: Some(self.claude_root.clone()),
                ..Default::default()
            },
            ..Default::default()
        }
    }
    fn jsonl(&self, sid: &str) -> PathBuf {
        self.claude_root
            .join("project-history")
            .join(format!("{sid}.jsonl"))
    }
    fn client(&self) -> HostClient {
        HostClient::new(
            &self.host,
            Limits {
                max_line_bytes: 64 * 1024,
                operation_timeout: Duration::from_secs(2),
                ..Default::default()
            },
        )
        .unwrap()
    }
}

async fn request(
    router: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let response: Response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("host", "localhost")
                .header("content-type", "application/json")
                .body(match body {
                    Some(body) => Body::from(body.to_string()),
                    None => Body::empty(),
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn post(router: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    request(router, Method::POST, uri, Some(body)).await
}
async fn get(router: &Router, uri: &str) -> (StatusCode, Value) {
    request(router, Method::GET, uri, None).await
}

struct App {
    shutdown: CancellationToken,
    prepared: sessiondock::PreparedApp,
    build: String,
}

async fn open(fixture: &Fixture) -> App {
    let shutdown = CancellationToken::new();
    let prepared = prepare_app(fixture.config(), shutdown.clone())
        .await
        .unwrap();
    let (status, meta) = get(&prepared.router, "/api/meta").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["capabilities"]["outbox"], true, "{meta}");
    assert_eq!(meta["capabilities"]["outbox_read"], true);
    let build = meta["build"].as_str().unwrap().to_owned();
    App {
        shutdown,
        prepared,
        build,
    }
}

/// Cleanup only the exact instances a test created.
async fn kill(app: &App, instances: &[(String, String)]) {
    for (record_id, instance_id) in instances {
        let (status, cancelled) = post(
            &app.prepared.router,
            "/api/term/kill",
            json!({"record_id":record_id,"instance_id":instance_id}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{cancelled}");
        assert_eq!(cancelled["state"], "exited", "{cancelled}");
    }
}

async fn close(app: App) {
    app.shutdown.cancel();
    if let Some(lifecycle) = &app.prepared.lifecycle {
        let _ = lifecycle.shutdown().await;
    }
    if let Some(delivery) = &app.prepared.delivery {
        let _ = delivery.shutdown().await;
    }
}

/// Type raw bytes into a pending (launch-guarded) instance, like a person at
/// the console before the session has any native history.
async fn type_raw(app: &App, fixture: &Fixture, record_id: &str, text: &str) {
    let lifecycle = app.prepared.lifecycle.as_ref().unwrap();
    let target = lifecycle.target(record_id.into()).await.unwrap();
    fixture
        .client()
        .request_launch(&target, ControlOp::Send { text: text.into() })
        .await
        .unwrap();
}

/// Launch a fake Claude through the given profile, type the first prompt in
/// the console so the native file exists, and return (uid, name, instance,
/// record_id, sid).
async fn create(
    app: &App,
    fixture: &Fixture,
    request_id: &str,
) -> (String, String, String, String, String) {
    let (status, receipt) = post(&app.prepared.router, "/api/term/create",
        json!({"source":"claude","cwd":fixture.claude_area,"request_id":request_id,"adapter_id":"ignored-browser-selector"})).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["running"], true, "{receipt}");
    let record_id = receipt["record_id"].as_str().unwrap().to_owned();
    let sid = receipt["declared_sid"].as_str().unwrap().to_owned();
    let name = receipt["name"].as_str().unwrap().to_owned();
    let instance = receipt["instance_id"].as_str().unwrap().to_owned();
    // Wait for the fake CLI to draw its composer, then type the first prompt.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let lifecycle = app.prepared.lifecycle.as_ref().unwrap();
        let target = lifecycle.target(record_id.clone()).await.unwrap();
        let reply = fixture
            .client()
            .request_launch(
                &target,
                ControlOp::Capture {
                    kind: ptyhost_client::CaptureKind::Screen,
                    styled: false,
                    join: false,
                    lines: 0,
                },
            )
            .await
            .unwrap();
        if let ptyhost_client::ControlReply::Capture(capture) = reply
            && capture.text.contains("FAKE_CLAUDE_READY")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fake CLI never drew its composer"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    type_raw(app, fixture, &record_id, "first prompt typed in console\r").await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fixture.jsonl(&sid).is_file() {
        assert!(
            Instant::now() < deadline,
            "no native file after the first prompt"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The runtime catalog now associates the host by its declared SID.
    let uid = loop {
        let (status, list) = get(&app.prepared.router, "/api/term/list").await;
        assert_eq!(status, StatusCode::OK, "{list}");
        if let Some(row) = list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == name)
        {
            assert_eq!(row["instance_id"], instance);
            assert_eq!(row["sid"], sid);
            break row["uid"].as_str().unwrap().to_owned();
        }
        assert!(
            Instant::now() < deadline,
            "session never became a managed instance: {list}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    (uid, name, instance, record_id, sid)
}

async fn outbox(app: &App, uid: &str) -> Value {
    let (status, body) = get(
        &app.prepared.router,
        &format!("/api/session/outbox?uid={uid}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn wait_confirmed(app: &App, uid: &str, id: &str, within: Duration) -> Value {
    let deadline = Instant::now() + within;
    loop {
        let snapshot = outbox(app, uid).await;
        if !snapshot["outbox"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == id)
        {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "receipt {id} never confirmed: {snapshot}"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

fn user_texts(app_messages: &Value) -> Vec<String> {
    app_messages["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user")
        .map(|message| message["text"].as_str().unwrap_or("").to_owned())
        .collect()
}

#[tokio::test]
async fn send_confirms_from_native_record_and_replays_by_request_id() {
    let Some(host_binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    let fixture = Fixture::new(&host_binary, "normal");
    let app = open(&fixture).await;
    let router = app.prepared.router.clone();
    let (uid, name, instance, record, sid) = create(&app, &fixture, "send-create-one").await;
    let instances = vec![(record.clone(), instance.clone())];

    let request_id = " 短?🦀 ".repeat(20); // Under 128 characters, over 128 UTF-8 bytes.

    // Stale build gate before any terminal access.
    let (status, stale) = post(
        &router,
        "/api/session/send",
        json!({"uid":uid,"name":name,"text":"x","request_id":request_id,"_build":"old"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(stale["code"], "stale_build");
    assert_eq!(stale["reload"], true);
    assert_eq!(stale["build"], app.build);

    let body = json!({"uid":uid,"name":name,"text":"hello from the web composer","media":[],
        "activity":null,"request_id":request_id,"page_id":"page-one",
        "overwrite_draft":"","cursor":null,"_build":app.build,"_trace_id":"t","_page_id":"page-one"});
    let (status, reply) = post(&router, "/api/session/send", body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["item"]["id"], request_id);
    assert_eq!(reply["item"]["uid"], uid);
    assert_eq!(reply["item"]["text"], "hello from the web composer");
    assert_eq!(reply["item"]["server"], true);
    assert_eq!(reply["item"]["attempts"], 1);
    assert_eq!(reply["item"]["state"], "ambiguous");
    let epoch = reply["outbox_version"]["epoch"]
        .as_str()
        .unwrap()
        .to_owned();
    let revision = reply["outbox_version"]["revision"].as_u64().unwrap();
    assert_eq!(reply["outbox"][0]["id"], request_id);

    // The native user record confirms the receipt; the row leaves the outbox.
    let confirmed = wait_confirmed(&app, &uid, &request_id, Duration::from_secs(10)).await;
    assert_eq!(confirmed["outbox_version"]["epoch"], epoch);
    assert!(confirmed["outbox_version"]["revision"].as_u64().unwrap() > revision);
    let raw = fs::read_to_string(fixture.jsonl(&sid)).unwrap();
    let rows: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2, "{raw}");
    assert_eq!(rows[1]["message"]["content"], "hello from the web composer");
    assert_eq!(rows[1]["sessionId"], sid);
    let (status, messages) = get(&router, &format!("/api/messages/{uid}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        user_texts(&messages),
        vec![
            "first prompt typed in console".to_owned(),
            "hello from the web composer".to_owned()
        ]
    );

    // Replaying the same request ID is a status lookup, never a second paste.
    let (status, replay) = post(&router, "/api/session/send", body).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["item"]["id"], request_id);
    assert_eq!(replay["item"]["state"], "confirmed");
    assert!(replay["item"].get("text").is_none());
    assert!(replay["outbox"].as_array().unwrap().is_empty());
    let (status, conflict) = post(&router, "/api/session/send",
        json!({"uid":uid,"name":name,"text":"another text","request_id":request_id,"_build":app.build})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(conflict["error"], "重复发送 ID 对应了不同消息");
    let raw = fs::read_to_string(fixture.jsonl(&sid)).unwrap();
    assert_eq!(raw.lines().count(), 2, "replay must not inject");

    // Uploaded file paths are already in text; media is opaque preview metadata.
    let (status, media) = post(
        &router,
        "/api/session/send",
        json!({"uid":uid,"name":name,"text":"with media","request_id":"send-request-0002",
        "media":[{"kind":"image","token":"abc"}],"_build":app.build}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{media}");
    assert_eq!(media["item"]["media"][0]["token"], "abc");
    wait_confirmed(&app, &uid, "send-request-0002", Duration::from_secs(10)).await;

    // Wrong terminal name and unknown session follow the Python codes.
    let (status, unlinked) = post(&router, "/api/session/send",
        json!({"uid":uid,"name":"sessiondock-claude-other","text":"x","request_id":"send-request-0003","_build":app.build})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(unlinked["code"], "terminal_unlinked");
    let (status, unknown) = post(&router, "/api/session/send",
        json!({"uid":"claude:0000000000000000","name":name,"text":"x","request_id":"send-request-0004","_build":app.build})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        unknown["error"],
        "服务端发送账本只用于已有 Claude/Codex 会话"
    );

    // Draft conflict: text typed at the console needs consent before sending.
    let (status, empty) = post(
        &router,
        "/api/session/draft-status",
        json!({"uid":uid,"name":name}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty, json!({"ok": true, "draft_state": "empty"}));
    {
        let (status, list) = get(&router, "/api/term/list").await;
        assert_eq!(status, StatusCode::OK);
        let _ = list;
    }
    // Type a draft through the native-bound path: claim a browser lease, send
    // raw text, release by letting the reservation lapse is too slow, so use
    // the page lease for the composer send afterwards.
    let (status, claim) = post(
        &router,
        "/api/term/claim",
        json!({"name":name,"page":"page-console","uid":uid,"instance_id":instance}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claim}");
    let token = claim["token"].as_str().unwrap().to_owned();
    let (status, typed) = post(
        &router,
        "/api/term/send",
        json!({"name":name,"page":"page-console","token":token,"uid":uid,"instance_id":instance,
        "data":"unsent draft","_build":app.build}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{typed}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Another page (no lease) is refused by the ownership registry.
    let (status, owned) = post(&router, "/api/session/send",
        json!({"uid":uid,"name":name,"text":"blocked","request_id":"send-request-0005","_build":app.build})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{owned}");
    assert_eq!(owned["code"], "terminal_ownership");
    assert_eq!(owned["owner"]["ip"], "127.0.0.1");
    let lease = json!({"page":"page-console","token":token,"instance_id":instance});
    let (status, probe) = post(
        &router,
        "/api/session/draft-status",
        json!({"uid":uid,"name":name,"lease":lease}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{probe}");
    assert_eq!(probe["draft_state"], "editing");
    assert_eq!(probe["draft_conflict"], true);
    let draft_token = probe["draft_token"].as_str().unwrap().to_owned();
    assert_eq!(draft_token.len(), 64);
    let (status, refused) = post(
        &router,
        "/api/session/send",
        json!({"uid":uid,"name":name,"text":"overwrite me","request_id":"send-request-0006",
        "overwrite_draft":"","lease":lease,"_build":app.build}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["draft_conflict"], true);
    assert_eq!(refused["draft_token"], draft_token);
    assert!(
        refused["outbox"].as_array().unwrap().is_empty(),
        "no receipt without consent"
    );
    let (status, sent) = post(
        &router,
        "/api/session/send",
        json!({"uid":uid,"name":name,"text":"overwrite me","request_id":"send-request-0006",
        "overwrite_draft":draft_token,"lease":lease,"_build":app.build}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    assert_eq!(sent["item"]["state"], "ambiguous");
    wait_confirmed(&app, &uid, "send-request-0006", Duration::from_secs(10)).await;
    let raw = fs::read_to_string(fixture.jsonl(&sid)).unwrap();
    let rows: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 4, "{raw}");
    assert_eq!(rows[3]["message"]["content"], "overwrite me");
    kill(&app, &instances).await;
    close(app).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn busy_tui_confirms_late_and_swallowed_line_stays_uncertain_across_restart() {
    let Some(host_binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    let slow_fixture = Fixture::new(&host_binary, "slow");
    let app = open(&slow_fixture).await;
    let router = app.prepared.router.clone();

    // Slow instance: the record appears 1.5 s after Enter behind a busy footer.
    let (slow_uid, slow_name, slow_instance, slow_record, slow_sid) =
        create(&app, &slow_fixture, "send-create-slow").await;
    let started = Instant::now();
    let (status, reply) = post(&router, "/api/session/send",
        json!({"uid":slow_uid,"name":slow_name,"text":"slow prompt","request_id":"send-request-slow1","_build":app.build})).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["item"]["state"], "ambiguous");
    wait_confirmed(
        &app,
        &slow_uid,
        "send-request-slow1",
        Duration::from_secs(15),
    )
    .await;
    assert!(
        started.elapsed() >= Duration::from_millis(1400),
        "confirmed before the CLI wrote"
    );
    let raw = fs::read_to_string(slow_fixture.jsonl(&slow_sid)).unwrap();
    assert!(raw.contains("\"slow prompt\""), "{raw}");
    assert!(
        raw.contains("OK: slow prompt"),
        "assistant reply missing: {raw}"
    );
    kill(&app, &[(slow_record, slow_instance)]).await;
    close(app).await;

    // Swallowing instance: the second submitted line (our send) is dropped.
    let fixture = Fixture::new(&host_binary, "swallow");
    let app = open(&fixture).await;
    let router = app.prepared.router.clone();
    let (uid, name, instance, record, sid) = create(&app, &fixture, "send-create-swallow").await;
    let instances = vec![(record, instance)];
    let (status, reply) = post(&router, "/api/session/send",
        json!({"uid":uid,"name":name,"text":"lost in the tui","request_id":"send-request-lost1","_build":app.build})).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["item"]["state"], "ambiguous");
    let epoch = reply["outbox_version"]["epoch"]
        .as_str()
        .unwrap()
        .to_owned();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let snapshot = outbox(&app, &uid).await;
    assert_eq!(snapshot["outbox"][0]["id"], "send-request-lost1");
    assert_eq!(snapshot["outbox"][0]["state"], "ambiguous");
    assert_eq!(
        snapshot["outbox"][0]["error"],
        "发送状态待核对；禁止自动重试"
    );
    let (status, retry) = post(&router, "/api/session/outbox/retry",
        json!({"uid":uid,"id":"send-request-lost1","activity":null,"overwrite_draft":"","_build":app.build})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{retry}");
    assert_eq!(
        retry["error"],
        "消息已经提交到终端，禁止盲目重发；请等待原生记录或打开终端检查"
    );
    assert_eq!(retry["outbox"][0]["id"], "send-request-lost1");
    let (status, missing) = post(
        &router,
        "/api/session/outbox/retry",
        json!({"uid":uid,"id":"send-request-none","_build":app.build}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing["error"], "待核对消息不存在");
    let raw = fs::read_to_string(fixture.jsonl(&sid)).unwrap();
    assert_eq!(
        raw.lines().count(),
        1,
        "swallowed line must not be re-injected: {raw}"
    );

    // Web restart: the uncertain receipt survives with a fresh epoch and is
    // never pasted again; the fake CLI's transcript shows exactly two lines.
    close(app).await;
    let app = open(&fixture).await;
    let router = app.prepared.router.clone();
    let snapshot = outbox(&app, &uid).await;
    assert_ne!(snapshot["outbox_version"]["epoch"], epoch);
    assert_eq!(snapshot["outbox"][0]["id"], "send-request-lost1");
    assert_eq!(snapshot["outbox"][0]["state"], "ambiguous");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let raw = fs::read_to_string(fixture.jsonl(&sid)).unwrap();
    assert_eq!(raw.lines().count(), 1, "restart must not re-inject: {raw}");
    let (status, list) = get(&router, "/api/term/list").await;
    assert_eq!(status, StatusCode::OK);
    let row = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["uid"] == uid)
        .cloned();
    let row = row.unwrap_or_else(|| panic!("{list}"));
    let (status, claim) = post(
        &router,
        "/api/term/claim",
        json!({"name":name,"page":"page-after-restart","uid":uid,"instance_id":row["instance_id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claim}");
    let (status, replay) = post(&router, "/api/session/send",
        json!({"uid":uid,"name":name,"text":"lost in the tui","request_id":"send-request-lost1","_build":app.build,
        "lease":{"page":"page-after-restart","token":claim["token"],"instance_id":row["instance_id"]}})).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["item"]["state"], "ambiguous");
    let raw = fs::read_to_string(fixture.jsonl(&sid)).unwrap();
    assert_eq!(raw.lines().count(), 1);

    // Discard retires the receipt; a second discard is 404 like Python.
    let (status, discard) = post(
        &router,
        "/api/session/outbox/discard",
        json!({"uid":uid,"id":"send-request-lost1"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{discard}");
    assert_eq!(discard["ok"], true);
    assert_eq!(discard["uid"], uid);
    assert!(discard["outbox"].as_array().unwrap().is_empty());
    let (status, again) = post(
        &router,
        "/api/session/outbox/discard",
        json!({"uid":uid,"id":"send-request-lost1"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(again["error"], "待核对消息不存在");
    // A later identical send under the page lease is a new request and works.
    let (status, fresh) = post(&router, "/api/session/send",
        json!({"uid":uid,"name":name,"text":"lost in the tui","request_id":"send-request-lost2","_build":app.build,
        "lease":{"page":"page-after-restart","token":claim["token"],"instance_id":row["instance_id"]}})).await;
    assert_eq!(status, StatusCode::OK, "{fresh}");
    wait_confirmed(&app, &uid, "send-request-lost2", Duration::from_secs(10)).await;
    let raw = fs::read_to_string(fixture.jsonl(&sid)).unwrap();
    assert_eq!(raw.lines().count(), 2, "{raw}");
    assert_eq!(fixture.root.join("claude"), fixture.claude_root);
    kill(&app, &instances).await;
    close(app).await;
}

#[tokio::test]
async fn send_routes_are_501_without_the_ledger_or_transport() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let web = root.join("web");
    directory(&web);
    file(
        &web.join("index.html"),
        b"<!doctype html><meta name=\"sessiondock-mode\" content=\"local\"><title>synthetic</title>",
        0o600,
    );
    let claude = root.join("claude");
    directory(&claude);
    let shutdown = CancellationToken::new();
    let prepared = prepare_app(
        Config {
            web_dir: web,
            roots: SessionRoots {
                claude: Some(claude),
                ..Default::default()
            },
            ..Default::default()
        },
        shutdown.clone(),
    )
    .await
    .unwrap();
    let (status, meta) = get(&prepared.router, "/api/meta").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["capabilities"]["outbox"], false);
    for route in [
        "/api/session/send",
        "/api/session/draft-status",
        "/api/session/outbox/retry",
        "/api/session/outbox/discard",
    ] {
        let (status, body) = post(
            &prepared.router,
            route,
            json!({"uid":"claude:x","name":"n"}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{route}: {body}");
        assert_eq!(body["code"], "delivery_send_disabled");
    }
    shutdown.cancel();
}

/// Frozen Python server uses str(value or "") before enqueue's character slice.
/// Raw request bodies preserve large integer spelling and duplicate map keys.
#[tokio::test]
async fn python_json_request_ids_and_ignored_outbox_query_fields() {
    let host_binary = ptyhost_binary().expect("build ptyhost for this HTTP integration test");
    let fixture = Fixture::new(&host_binary, "normal");
    let app = open(&fixture).await;
    let router = app.prepared.router.clone();
    let (uid, name, instance, record, _) = create(&app, &fixture, "json-id-create").await;
    async fn raw_post(router: &Router, path: &str, body: String) -> (StatusCode, Value) {
        use tower::ServiceExt;
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(path)
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    let cases: &[(&str, Option<&str>)] = &[
        (r##"null"##, None),
        (r##"false"##, None),
        (r##"0"##, None),
        (r##"-0"##, None),
        (r##"0.0"##, None),
        (r##""""##, None),
        (r##"[]"##, None),
        (r##"{}"##, None),
        (r##"true"##, Some(r##"True"##)),
        (r##"42"##, Some(r##"42"##)),
        (r##"-7"##, Some(r##"-7"##)),
        (r##"1.25"##, Some(r##"1.25"##)),
        (r##"1e-7"##, Some(r##"1e-07"##)),
        (r##"1e16"##, Some(r##"1e+16"##)),
        (r##"0.0001"##, Some(r##"0.0001"##)),
        (
            r##"18446744073709551617001"##,
            Some(r##"18446744073709551617001"##),
        ),
        (
            r##"[true, null, 1.0, -0.0, "短", [], {}]"##,
            Some(r##"[True, None, 1.0, -0.0, '短', [], {}]"##),
        ),
        (
            r##"{"z": 1, "a": false, "z": 2}"##,
            Some(r##"{'z': 2, 'a': False}"##),
        ),
        (
            r##"["has'quote", "both'\"quotes", " ‍́️\b", "🦀"]"##,
            Some(r##"["has'quote", 'both\'"quotes', '\xa0\u200d́️\x7f\x08', '🦀']"##),
        ),
        (
            r##"["汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉"]"##,
            Some(
                r##"['汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉"##,
            ),
        ),
    ];
    let mut generated = std::collections::BTreeSet::new();
    for (index, (raw, expected)) in cases.iter().enumerate() {
        let body = format!(
            r#"{{"uid":{},"name":{},"text":{},"_build":{},"request_id":{raw}}}"#,
            json!(uid),
            json!(name),
            json!(format!("json type case {index}")),
            json!(app.build)
        );
        let (status, reply) = raw_post(&router, "/api/session/send", body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{raw}: {reply}");
        let id = reply["item"]["id"].as_str().unwrap().to_owned();
        if let Some(expected) = expected {
            assert_eq!(&id, expected, "{raw}");
            let (status, replay) = raw_post(&router, "/api/session/send", body).await;
            assert_eq!(status, StatusCode::OK, "{raw}: {replay}");
            assert_eq!(replay["item"]["id"], id);
            assert_eq!(replay["outbox_version"], reply["outbox_version"]);
        } else {
            assert_eq!(id.len(), 36, "{raw}: {id}");
            assert_eq!(&id[14..15], "4");
            assert!(
                generated.insert(id.clone()),
                "falsy IDs must mint a fresh UUID"
            );
        }
        wait_confirmed(&app, &uid, &id, Duration::from_secs(10)).await;
    }
    let baseline = outbox(&app, &uid).await;
    let (status, queried) = get(
        &router,
        &format!("/api/session/outbox?uid={uid}&future=one&future=two&ignored=%E6%96%B0"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{queried}");
    assert_eq!(queried, baseline);
    // Unknown UIDs use Python's unsupported-source empty snapshot.
    let (status, unknown) = get(
        &router,
        "/api/session/outbox?uid=claude:does-not-exist&future=1",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unknown["outbox"], json!([]));
    assert_eq!(unknown["outbox_version"]["epoch"], "none");
    // Retry/discard coerce IDs too, but never mint or truncate lookup keys.
    for path in ["/api/session/outbox/retry", "/api/session/outbox/discard"] {
        let (status, numeric) = raw_post(
            &router,
            path,
            format!(
                r#"{{"uid":{},"id":42,"_build":{}}}"#,
                json!(uid),
                json!(app.build)
            ),
        )
        .await;
        let (string_status, string) = post(
            &router,
            path,
            json!({"uid":uid,"id":"42","_build":app.build}),
        )
        .await;
        assert_eq!(status, string_status, "{path}: {numeric}");
        assert_eq!(numeric, string, "{path}");
    }
    kill(&app, &[(record, instance)]).await;
    close(app).await;
}
