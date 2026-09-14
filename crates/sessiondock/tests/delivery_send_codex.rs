//! Codex reliable send end to end: the HTTP router, an isolated
//! ptyhost, the lifecycle launcher and the delivery ledger, with the fake
//! Codex CLI from `tests/fake_codex_cli.py` as the only "CLI". Corpus rollouts
//! are resumed through a schema-2 profile (`resume_args ["resume","{sid}"]`);
//! the fake renders a Codex-style composer (dim placeholder, braille
//! particles, model footer) and appends the real rollout shape for every
//! submitted line. No model binary, native CLI home or production host is
//! touched. Skips when ptyhost is not built.
#![cfg(unix)]

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sessiondock::{
    config::Config,
    delivery::{codex, engine::DeliveryEngine},
    lifecycle::store::LifecycleStore,
    prepare_app,
    sessions::SessionRoots,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const FAKE_CLI: &str = include_str!("../../../tests/fake_codex_cli.py");
const SIDS: [(&str, &str); 5] = [
    ("codex-cli-v1", "5d1e2f3a-4b5c-4d6e-8f70-1a2b3c4d5e01"),
    ("codex-slow-v1", "5d1e2f3a-4b5c-4d6e-8f70-1a2b3c4d5e02"),
    ("codex-swallow-v1", "5d1e2f3a-4b5c-4d6e-8f70-1a2b3c4d5e03"),
    ("codex-noturn-v1", "5d1e2f3a-4b5c-4d6e-8f70-1a2b3c4d5e04"),
    ("codex-dup-v1", "5d1e2f3a-4b5c-4d6e-8f70-1a2b3c4d5e05"),
];

struct Fixture {
    _temp: tempfile::TempDir,
    lifecycle: PathBuf,
    host: PathBuf,
    codex_root: PathBuf,
    codex_area: PathBuf,
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
fn sha1_uid(path: &Path) -> String {
    use sha1::Digest;
    let digest = sha1::Sha1::digest(path.to_str().unwrap().as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("codex:{}", &hex[..16])
}

impl Fixture {
    fn new(host_binary: &Path, selected_profile: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let lifecycle = root.join("lifecycle");
        let host = root.join("hosts");
        let work = root.join("work");
        let codex_area = work.join("codex-area");
        let codex_root = root.join("codex");
        let delivery = root.join("delivery");
        let web = root.join("web");
        let bin = root.join("bin");
        for path in [
            &lifecycle,
            &host,
            &codex_area,
            &codex_root.join("2026/09/12"),
            &delivery,
            &web,
            &bin,
            &root.join("home"),
        ] {
            directory(path);
        }
        file(&web.join("index.html"),b"<!doctype html><meta name=\"sessiondock-mode\" content=\"local\"><title>synthetic</title>",0o600);
        let script = bin.join("fake_codex_cli.py");
        file(&script, FAKE_CLI.as_bytes(), 0o600);
        let wrapper = format!(
            "#!/bin/sh\nexec {} {} \"$@\"\n",
            python3().display(),
            script.display()
        );
        let env = json!({"PATH":"/usr/bin:/bin","HOME":root.join("home"),
            "SESSIONDOCK_TEST_CODEX_ROOT":codex_root,"LANG":"C.UTF-8"});
        let mut profiles = Vec::new();
        for (profile, sid) in SIDS {
            let executable = bin.join(format!("fake-{profile}"));
            file(&executable, wrapper.as_bytes(), 0o700);
            let args = match profile {
                "codex-cli-v1" => json!(["--reply", "-c", "model_reasoning_effort=\"low\""]),
                "codex-slow-v1" => json!(["--delay", "1500", "--busy-footer", "--reply"]),
                "codex-swallow-v1" => json!(["--swallow", "1"]),
                "codex-noturn-v1" => json!(["--no-turn-id"]),
                _ => json!(["--duplicate"]),
            };
            profiles.push(
                json!({"id":profile,"source":"codex","executable":executable,
                "args":args,"new_args":[],"resume_args":["resume","{sid}"],
                "env":env}),
            );
            let rows = [
                json!({"timestamp":"2026-09-12T09:00:00.000Z","type":"session_meta",
                    "payload":{"id":sid,"timestamp":"2026-09-12T09:00:00.000Z","cwd":codex_area}}),
                json!({"timestamp":"2026-09-12T09:00:01.000Z","type":"response_item",
                    "payload":{"type":"message","role":"user","turn_id":"seed-turn",
                    "content":[{"type":"input_text","text":"seed prompt typed earlier"}]}}),
            ];
            file(
                &codex_root
                    .join("2026/09/12")
                    .join(format!("rollout-2026-09-12T09-00-00-{sid}.jsonl")),
                rows.iter()
                    .map(|row| format!("{row}\n"))
                    .collect::<String>()
                    .as_bytes(),
                0o600,
            );
        }
        let launcher = root.join("launcher.json");
        let selected = profiles
            .iter()
            .find(|profile| profile["id"] == selected_profile)
            .cloned()
            .expect("selected test profile");
        let config = json!({"schema":2,"host_binary":host_binary,"host_dir":host,
        "adapters":[],"profiles":[selected],"test_profiles":profiles});
        file(&launcher, config.to_string().as_bytes(), 0o600);
        drop(LifecycleStore::initialize(&lifecycle).unwrap());
        drop(DeliveryEngine::initialize(&delivery).unwrap());
        Self {
            _temp: temp,
            lifecycle,
            host,
            codex_root,
            codex_area,
            delivery,
            web,
            launcher,
        }
    }
    fn select_profile(&self, profile: &str) {
        let mut config: Value = serde_json::from_slice(&fs::read(&self.launcher).unwrap()).unwrap();
        let selected = config["test_profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == profile)
            .cloned()
            .expect("selected test profile");
        config["profiles"] = json!([selected]);
        fs::write(&self.launcher, serde_json::to_vec(&config).unwrap()).unwrap();
    }
    fn config(&self) -> Config {
        Config {
            web_dir: self.web.clone(),
            ptyhost_dir: Some(self.host.clone()),
            lifecycle_dir: Some(self.lifecycle.clone()),
            launcher_config: Some(self.launcher.clone()),
            delivery_dir: Some(self.delivery.clone()),
            roots: SessionRoots {
                codex: Some(self.codex_root.clone()),
                ..Default::default()
            },
            ..Default::default()
        }
    }
    fn rollout(&self, sid: &str) -> PathBuf {
        self.codex_root
            .join("2026/09/12")
            .join(format!("rollout-2026-09-12T09-00-00-{sid}.jsonl"))
    }
    fn uid(&self, sid: &str) -> String {
        sha1_uid(&self.rollout(sid))
    }
    fn rows(&self, sid: &str) -> Vec<Value> {
        fs::read_to_string(self.rollout(sid))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn user_records(&self, sid: &str) -> Vec<Value> {
        self.rows(sid)
            .into_iter()
            .filter(|row| {
                row["type"] == "response_item"
                    && row["payload"]["type"] == "message"
                    && row["payload"]["role"] == "user"
            })
            .collect()
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
    let build = meta["build"].as_str().unwrap().to_owned();
    App {
        shutdown,
        prepared,
        build,
    }
}

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

/// Resume a corpus rollout through the profile, wait for the runtime catalog
/// to list the managed instance and for the fake composer to be readable;
/// returns (uid, name, instance, record_id).
async fn resume(app: &App, fixture: &Fixture, profile: &str) -> (String, String, String, String) {
    let sid = SIDS.iter().find(|(id, _)| *id == profile).unwrap().1;
    let uid = fixture.uid(sid);
    let router = &app.prepared.router;
    let (status, receipt) = post(
        router,
        "/api/term/create",
        json!({"source":"codex","cwd":fixture.codex_area,"request_id":format!("resume-{profile}"),
        "adapter_id":profile,"resume_uid":uid}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["running"], true, "{receipt}");
    let record_id = receipt["record_id"].as_str().unwrap().to_owned();
    let name = receipt["name"].as_str().unwrap().to_owned();
    let instance = receipt["instance_id"].as_str().unwrap().to_owned();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let (status, list) = get(router, "/api/term/list").await;
        assert_eq!(status, StatusCode::OK, "{list}");
        if list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["uid"] == uid && row["instance_id"] == instance)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "resumed instance never listed: {list}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // The fake draws its `›` composer with the dim placeholder and particle
    // glyphs; the driver must read it as an empty composer, not `unknown`.
    loop {
        let (status, probe) = post(
            router,
            "/api/session/draft-status",
            json!({"uid":uid,"name":name}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{probe}");
        if probe["draft_state"] == "empty" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fake Codex composer never became readable: {probe}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    (uid, name, instance, record_id)
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

fn user_texts(messages: &Value) -> Vec<String> {
    messages["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user")
        .map(|message| message["text"].as_str().unwrap_or("").to_owned())
        .collect()
}

/// Open the closed ledger read-only for assertions on the private receipt
/// (association evidence is never published over HTTP).
fn receipt(fixture: &Fixture, id: &str) -> codex::Receipt {
    let mut engine = DeliveryEngine::open(&fixture.delivery).unwrap();
    engine
        .codex_receipt(id)
        .unwrap()
        .expect("receipt persisted")
}

#[tokio::test]
async fn codex_send_confirms_with_operation_turn_replays_and_needs_draft_consent() {
    let Some(host_binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    let fixture = Fixture::new(&host_binary, "codex-cli-v1");
    let app = open(&fixture).await;
    let router = app.prepared.router.clone();
    let sid = SIDS[0].1;
    let (uid, name, instance, record) = resume(&app, &fixture, "codex-cli-v1").await;
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

    let body = json!({"uid":uid,"name":name,"text":"hello from the web composer","media":[],
        "activity":null,"request_id":request_id,"page_id":"page-one",
        "overwrite_draft":"","cursor":null,"_build":app.build});
    let (status, reply) = post(&router, "/api/session/send", body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["item"]["id"], request_id);
    assert_eq!(reply["item"]["uid"], uid);
    assert_eq!(reply["item"]["text"], "hello from the web composer");
    assert_eq!(reply["item"]["server"], true);
    // Python's Codex projection: an attempted, unconfirmed row is
    // `failed` with `attempts: 1` (legacy "终端写入待核对", 检查终端/移除).
    assert_eq!(reply["item"]["state"], "failed");
    assert_eq!(reply["item"]["attempts"], 1);
    assert_eq!(reply["item"]["error"], "发送结果待核对；禁止自动重试");
    let epoch = reply["outbox_version"]["epoch"]
        .as_str()
        .unwrap()
        .to_owned();
    let revision = reply["outbox_version"]["revision"].as_u64().unwrap();
    assert_eq!(reply["outbox"][0]["id"], request_id);

    // The native user record (with its turn ID) confirms the receipt.
    let confirmed = wait_confirmed(&app, &uid, &request_id, Duration::from_secs(10)).await;
    assert_eq!(confirmed["outbox_version"]["epoch"], epoch);
    assert!(confirmed["outbox_version"]["revision"].as_u64().unwrap() > revision);
    let users = fixture.user_records(sid);
    assert_eq!(users.len(), 2, "{users:?}");
    assert_eq!(
        users[1]["payload"]["content"][0]["text"],
        "hello from the web composer"
    );
    let turn = users[1]["payload"]["internal_chat_message_metadata_passthrough"]["turn_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!turn.is_empty());
    let (status, messages) = get(&router, &format!("/api/messages/{uid}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        user_texts(&messages),
        vec![
            "seed prompt typed earlier".to_owned(),
            "hello from the web composer".to_owned()
        ]
    );
    assert!(
        messages["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "assistant" && m["text"] == "OK: hello from the web composer"),
        "{messages}"
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
    assert_eq!(fixture.user_records(sid).len(), 2, "replay must not inject");

    // Media is opaque preview metadata; the uploaded path is already in text.
    let (status, media) = post(
        &router,
        "/api/session/send",
        json!({"uid":uid,"name":name,"text":"with media","request_id":"codex-send-0002",
        "media":[{"kind":"image","token":"abc"}],"_build":app.build}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{media}");
    assert_eq!(media["item"]["media"][0]["token"], "abc");
    wait_confirmed(&app, &uid, "codex-send-0002", Duration::from_secs(10)).await;
    let (status, unlinked) = post(&router, "/api/session/send",
        json!({"uid":uid,"name":"sessiondock-codex-other","text":"x","request_id":"codex-send-0003","_build":app.build})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(unlinked["code"], "terminal_unlinked");
    assert!(
        unlinked["error"]
            .as_str()
            .unwrap()
            .starts_with("Codex 终端会话未连接"),
        "{unlinked}"
    );
    let (status, unknown) = post(&router, "/api/session/send",
        json!({"uid":"codex:0000000000000000","name":name,"text":"x","request_id":"codex-send-0004","_build":app.build})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        unknown["error"],
        "服务端发送账本只用于已有 Claude/Codex 会话"
    );

    // Draft consent: text typed at the console needs the token.
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
    let (status, owned) = post(&router, "/api/session/send",
        json!({"uid":uid,"name":name,"text":"blocked","request_id":"codex-send-0005","_build":app.build})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{owned}");
    assert_eq!(owned["code"], "terminal_ownership");
    let lease = json!({"page":"page-console","token":token,"instance_id":instance});
    let (status, probe) = post(
        &router,
        "/api/session/draft-status",
        json!({"uid":uid,"name":name,"lease":lease}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{probe}");
    assert_eq!(probe["draft_state"], "editing", "{probe}");
    assert_eq!(probe["draft_conflict"], true);
    let draft_token = probe["draft_token"].as_str().unwrap().to_owned();
    assert_eq!(draft_token.len(), 64);
    let (status, refused) = post(
        &router,
        "/api/session/send",
        json!({"uid":uid,"name":name,"text":"overwrite me","request_id":"codex-send-0006",
        "overwrite_draft":"","lease":lease,"_build":app.build}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["draft_conflict"], true);
    assert_eq!(refused["draft_token"], draft_token);
    assert!(refused["outbox"].as_array().unwrap().is_empty());
    let (status, sent) = post(
        &router,
        "/api/session/send",
        json!({"uid":uid,"name":name,"text":"overwrite me","request_id":"codex-send-0006",
        "overwrite_draft":draft_token,"lease":lease,"_build":app.build}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    assert_eq!(sent["item"]["state"], "failed");
    assert_eq!(sent["item"]["attempts"], 1);
    wait_confirmed(&app, &uid, "codex-send-0006", Duration::from_secs(10)).await;
    let users = fixture.user_records(sid);
    assert_eq!(users.len(), 4, "{users:?}");
    assert_eq!(users[3]["payload"]["content"][0]["text"], "overwrite me");
    kill(&app, &instances).await;
    close(app).await;

    // The persisted association follows Python's causal text match.
    let row = receipt(&fixture, &request_id);
    assert!(
        matches!(
            row.state,
            codex::State::Acknowledged | codex::State::Completed
        ),
        "{row:?}"
    );
    let accepted = row.accepted.clone().unwrap();
    assert_eq!(accepted.turn_id, turn);
    assert_eq!(row.association, Some(codex::Correlation::PossibleTextMatch));
    assert!(accepted.start >= row.confirmation.unwrap().position);

    // A slow TUI (record 1.5 s after Enter behind a Working footer) confirms late.
    fixture.select_profile("codex-slow-v1");
    let app = open(&fixture).await;
    let router = app.prepared.router.clone();
    let (slow_uid, slow_name, slow_instance, slow_record) =
        resume(&app, &fixture, "codex-slow-v1").await;
    let instances = vec![(slow_record, slow_instance)];
    let started = Instant::now();
    let (status, reply) = post(&router, "/api/session/send",
        json!({"uid":slow_uid,"name":slow_name,"text":"slow prompt","request_id":"codex-send-slow1","_build":app.build})).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    wait_confirmed(&app, &slow_uid, "codex-send-slow1", Duration::from_secs(15)).await;
    assert!(
        started.elapsed() >= Duration::from_millis(1400),
        "confirmed before the CLI wrote"
    );
    kill(&app, &instances).await;
    close(app).await;
    assert!(receipt(&fixture, "codex-send-slow1").accepted.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_swallowed_line_stays_uncertain_and_causal_text_records_confirm() {
    let Some(host_binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    let fixture = Fixture::new(&host_binary, "codex-swallow-v1");
    let app = open(&fixture).await;
    let router = app.prepared.router.clone();

    // Swallowing instance: the first submitted line (our send) is dropped.
    let sid = SIDS[2].1;
    let (uid, name, instance, record) = resume(&app, &fixture, "codex-swallow-v1").await;
    let swallow_instance = (record, instance);
    let (status, reply) = post(&router, "/api/session/send",
        json!({"uid":uid,"name":name,"text":"lost in the tui","request_id":"codex-send-lost1","_build":app.build})).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["item"]["state"], "failed");
    assert_eq!(reply["item"]["attempts"], 1);
    let epoch = reply["outbox_version"]["epoch"]
        .as_str()
        .unwrap()
        .to_owned();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let snapshot = outbox(&app, &uid).await;
    assert_eq!(snapshot["outbox"][0]["id"], "codex-send-lost1");
    assert_eq!(snapshot["outbox"][0]["state"], "failed");
    assert_eq!(snapshot["outbox"][0]["attempts"], 1);
    assert_eq!(
        snapshot["outbox"][0]["error"],
        "发送结果待核对；禁止自动重试"
    );
    let (status, retry) = post(&router, "/api/session/outbox/retry",
        json!({"uid":uid,"id":"codex-send-lost1","activity":null,"overwrite_draft":"","_build":app.build})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{retry}");
    assert_eq!(retry["error"], "消息已经写入终端或仍在确认，禁止重复发送");
    assert_eq!(retry["outbox"][0]["id"], "codex-send-lost1");
    let (status, missing) = post(
        &router,
        "/api/session/outbox/retry",
        json!({"uid":uid,"id":"codex-send-none","_build":app.build}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing["error"], "待发送消息不存在");
    assert_eq!(
        fixture.user_records(sid).len(),
        1,
        "swallowed line re-injected"
    );

    close(app).await;

    // Python accepts the first causal matching record without a turn ID.
    fixture.select_profile("codex-noturn-v1");
    let app = open(&fixture).await;
    let router = app.prepared.router.clone();
    let noturn_sid = SIDS[3].1;
    let (noturn_uid, noturn_name, noturn_instance, noturn_record) =
        resume(&app, &fixture, "codex-noturn-v1").await;
    let (status, reply) = post(&router, "/api/session/send",
        json!({"uid":noturn_uid,"name":noturn_name,"text":"record without turn","request_id":"codex-send-noturn","_build":app.build})).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    tokio::time::sleep(Duration::from_secs(2)).await;
    let users = fixture.user_records(noturn_sid);
    assert_eq!(users.len(), 2, "{users:?}");
    assert!(users[1]["payload"]["turn_id"].is_null());
    assert!(users[1]["payload"]["internal_chat_message_metadata_passthrough"].is_null());
    assert!(
        fixture
            .rows(noturn_sid)
            .iter()
            .any(|row| row["payload"]["type"] == "task_started"),
        "task_started remains completion-only evidence"
    );
    let snapshot = outbox(&app, &noturn_uid).await;
    assert!(snapshot["outbox"].as_array().unwrap().is_empty());
    kill(&app, &[(noturn_record, noturn_instance)]).await;
    close(app).await;

    // Two identical causal records still confirm the same receipt.
    fixture.select_profile("codex-dup-v1");
    let app = open(&fixture).await;
    let router = app.prepared.router.clone();
    let dup_sid = SIDS[4].1;
    let (dup_uid, dup_name, dup_instance, dup_record) =
        resume(&app, &fixture, "codex-dup-v1").await;
    let (status, reply) = post(&router, "/api/session/send",
        json!({"uid":dup_uid,"name":dup_name,"text":"written twice","request_id":"codex-send-dup","_build":app.build})).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        fixture.user_records(dup_sid).len(),
        3,
        "two identical records"
    );
    let snapshot = outbox(&app, &dup_uid).await;
    assert!(snapshot["outbox"].as_array().unwrap().is_empty());
    kill(&app, &[(dup_record, dup_instance)]).await;
    close(app).await;

    // Web restart: the uncertain receipt survives with a fresh epoch and is
    // never pasted again.
    fixture.select_profile("codex-swallow-v1");
    let app = open(&fixture).await;
    let router = app.prepared.router.clone();
    let snapshot = outbox(&app, &uid).await;
    assert_ne!(snapshot["outbox_version"]["epoch"], epoch);
    assert_eq!(snapshot["outbox"][0]["id"], "codex-send-lost1");
    assert_eq!(snapshot["outbox"][0]["state"], "failed");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        fixture.user_records(sid).len(),
        1,
        "restart must not re-inject"
    );
    let (status, list) = get(&router, "/api/term/list").await;
    assert_eq!(status, StatusCode::OK);
    let row = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["uid"] == uid)
        .cloned()
        .unwrap_or_else(|| panic!("{list}"));
    let (status, claim) = post(
        &router,
        "/api/term/claim",
        json!({"name":name,"page":"page-after-restart","uid":uid,"instance_id":row["instance_id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claim}");
    let lease = json!({"page":"page-after-restart","token":claim["token"],"instance_id":row["instance_id"]});
    let (status, replay) = post(&router, "/api/session/send",
        json!({"uid":uid,"name":name,"text":"lost in the tui","request_id":"codex-send-lost1","_build":app.build,"lease":lease})).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["item"]["state"], "failed");
    assert_eq!(fixture.user_records(sid).len(), 1);

    // Discard hides the receipt (Python `9b1c2fd`: a confirming receipt may
    // be removed, it never resends); a second discard is idempotent `ok`
    // like Python's Codex handler.
    let (status, discard) = post(
        &router,
        "/api/session/outbox/discard",
        json!({"uid":uid,"id":"codex-send-lost1"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{discard}");
    assert_eq!(discard["ok"], true);
    assert_eq!(discard["uid"], uid);
    assert!(discard["outbox"].as_array().unwrap().is_empty());
    let (status, again) = post(
        &router,
        "/api/session/outbox/discard",
        json!({"uid":uid,"id":"codex-send-lost1"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["ok"], true);
    assert!(again["outbox"].as_array().unwrap().is_empty());
    // The tombstone keeps the deduplication identity: a replay is a lookup.
    let (status, replay) = post(&router, "/api/session/send",
        json!({"uid":uid,"name":name,"text":"lost in the tui","request_id":"codex-send-lost1","_build":app.build,"lease":lease})).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["item"]["state"], "confirmed");
    assert_eq!(fixture.user_records(sid).len(), 1);
    // A later identical send is a new request; the fake no longer swallows.
    let (status, fresh) = post(&router, "/api/session/send",
        json!({"uid":uid,"name":name,"text":"lost in the tui","request_id":"codex-send-lost2","_build":app.build,"lease":lease})).await;
    assert_eq!(status, StatusCode::OK, "{fresh}");
    wait_confirmed(&app, &uid, "codex-send-lost2", Duration::from_secs(10)).await;
    assert_eq!(fixture.user_records(sid).len(), 2);
    let (status, list) = get(&router, "/api/term/list").await;
    assert_eq!(status, StatusCode::OK);
    let live: Vec<(String, String)> = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["instance_id"] == swallow_instance.1)
        .map(|row| {
            (
                swallow_instance.0.clone(),
                row["instance_id"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    kill(&app, &live).await;
    close(app).await;
    let lost = receipt(&fixture, "codex-send-lost1");
    assert_eq!(lost.state, codex::State::Uncertain, "{lost:?}");
    assert!(lost.accepted.is_none() && lost.association.is_none());
    assert!(lost.dismissed);
    for id in ["codex-send-noturn", "codex-send-dup"] {
        let row = receipt(&fixture, id);
        assert!(
            matches!(
                row.state,
                codex::State::Acknowledged | codex::State::Completed
            ),
            "{id}: {row:?}"
        );
        assert!(row.accepted.is_some(), "{id}");
        assert_eq!(
            row.association,
            Some(codex::Correlation::PossibleTextMatch),
            "{id}"
        );
        assert!(!row.dismissed, "{id}");
    }
    assert!(receipt(&fixture, "codex-send-lost2").accepted.is_some());
}

#[tokio::test]
async fn codex_send_routes_are_501_without_the_ledger_or_transport() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let web = root.join("web");
    directory(&web);
    file(
        &web.join("index.html"),
        b"<!doctype html><meta name=\"sessiondock-mode\" content=\"local\"><title>synthetic</title>",
        0o600,
    );
    let codex = root.join("codex");
    directory(&codex);
    let shutdown = CancellationToken::new();
    let prepared = prepare_app(
        Config {
            web_dir: web,
            roots: SessionRoots {
                codex: Some(codex),
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
        let (status, body) =
            post(&prepared.router, route, json!({"uid":"codex:x","name":"n"})).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{route}: {body}");
        assert_eq!(body["code"], "delivery_send_disabled");
    }
    shutdown.cancel();
}
