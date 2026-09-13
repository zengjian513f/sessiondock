//! `POST /api/bug-report` end to end (batch 41): the HTTP router, an isolated
//! ptyhost, the lifecycle launcher with a cheapest-model profile, the audit
//! log and the fake Claude CLI from `tests/fake_claude_cli.py`. The worker's
//! prompt is pasted into the fake composer, confirmed from the synthetic
//! native `user` record, and the bundle on disk is checked. No model binary,
//! native CLI home or production host is touched. Skips when ptyhost is not
//! built.
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
    config::Config, lifecycle::store::LifecycleStore, prepare_app, sessions::SessionRoots,
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
const MODEL: &str = "claude-haiku-4-5-20251001";

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    lifecycle: PathBuf,
    host: PathBuf,
    claude_root: PathBuf,
    work: PathBuf,
    repo: PathBuf,
    reports: PathBuf,
    audit: PathBuf,
    web: PathBuf,
    launcher: PathBuf,
    launcher_bad: PathBuf,
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
    fn new(host_binary: &Path) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let lifecycle = root.join("lifecycle");
        let host = root.join("hosts");
        let work = root.join("work");
        let repo = work.join("repo");
        let claude_root = root.join("claude");
        let reports = root.join("reports");
        let audit = root.join("audit");
        let web = root.join("web");
        let bin = root.join("bin");
        for path in [
            &lifecycle,
            &host,
            &claude_root,
            &reports,
            &audit,
            &web,
            &bin,
        ] {
            directory(path);
        }
        fs::create_dir_all(&repo).unwrap();
        fs::set_permissions(&work, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&repo, fs::Permissions::from_mode(0o755)).unwrap();
        file(&web.join("index.html"),b"<!doctype html><meta name=\"agenthub-mode\" content=\"local\"><title>synthetic</title>",0o600);
        let script = bin.join("fake_claude_cli.py");
        file(&script, FAKE_CLI.as_bytes(), 0o600);
        let wrapper = format!(
            "#!/bin/sh\nexec {} {} \"$@\"\n",
            python3().display(),
            script.display()
        );
        file(&bin.join("fake-claude"), wrapper.as_bytes(), 0o700);
        let env = json!({"PATH":"/usr/bin:/bin","HOME":root.join("home"),
            "AGENTHUB_TEST_CLAUDE_ROOT":claude_root,"LANG":"C.UTF-8"});
        directory(&root.join("home"));
        let profile = |id: &str, args: Value| {
            json!({"id":id,"source":"claude","executable":bin.join("fake-claude"),
             "args":args,
             "new_args":["--session-id","{session_id}"],
             "resume_args":["--resume","{sid}"],
             "env":env})
        };
        let launcher = root.join("launcher.json");
        file(
            &launcher,
            json!({"schema":2,"host_binary":host_binary,"host_dir":host,
            "adapters":[],
            "profiles":[
                profile("claude-plain-v1", json!(["--reply"])),
                profile("claude-cheap-v1", json!(["--model", MODEL, "--effort", "low", "--reply"]))
            ],
            "bug_report_profiles":{"claude":"claude-cheap-v1"}})
            .to_string()
            .as_bytes(),
            0o600,
        );
        // A profile that names the wrong model: refused at the route.
        let launcher_bad = root.join("launcher-bad.json");
        file(
            &launcher_bad,
            json!({"schema":2,"host_binary":host_binary,"host_dir":host,
            "adapters":[],
            "profiles":[profile("claude-pricey-v1", json!(["--model", "claude-opus-4-1", "--effort", "high"]))],
            "bug_report_profiles":{"claude":"claude-pricey-v1"}})
            .to_string()
            .as_bytes(),
            0o600,
        );
        drop(LifecycleStore::initialize(&lifecycle).unwrap());
        Self {
            _temp: temp,
            root,
            lifecycle,
            host,
            claude_root,
            work,
            repo,
            reports,
            audit,
            web,
            launcher,
            launcher_bad,
        }
    }
    fn config(&self, bug_report: bool, launcher: &Path) -> Config {
        Config {
            web_dir: self.web.clone(),
            ptyhost_dir: Some(self.host.clone()),
            lifecycle_dir: Some(self.lifecycle.clone()),
            launcher_config: Some(launcher.to_path_buf()),
            audit_dir: Some(self.audit.clone()),
            file_roots: vec![self.work.clone()],
            file_write_roots: vec![self.work.clone()],
            bug_report_dir: bug_report.then(|| self.reports.clone()),
            bug_report_repo: bug_report.then(|| self.repo.clone()),
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
}

async fn request(
    router: &Router,
    method: Method,
    uri: &str,
    content_type: &str,
    body: Body,
) -> (StatusCode, Value) {
    let response: Response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("host", "localhost")
                .header("content-type", content_type)
                .body(body)
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
    request(
        router,
        Method::POST,
        uri,
        "application/json",
        Body::from(body.to_string()),
    )
    .await
}
async fn get(router: &Router, uri: &str) -> (StatusCode, Value) {
    request(router, Method::GET, uri, "application/json", Body::empty()).await
}

struct App {
    shutdown: CancellationToken,
    prepared: sessiondock::PreparedApp,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

/// The lifecycle service admits two concurrent opens per process; the three
/// tests here each open one, so they run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn open(config: Config) -> App {
    let serial = SERIAL.lock().await;
    let shutdown = CancellationToken::new();
    let prepared = prepare_app(config, shutdown.clone()).await.unwrap();
    App {
        shutdown,
        prepared,
        _serial: serial,
    }
}

async fn close(app: App) {
    app.shutdown.cancel();
    if let Some(lifecycle) = &app.prepared.lifecycle {
        let _ = lifecycle.shutdown().await;
    }
    if let Some(audit) = &app.prepared.audit {
        audit.shutdown().await;
    }
}

async fn kill(app: &App, record_id: &str, instance_id: &str) {
    let (status, cancelled) = post(
        &app.prepared.router,
        "/api/term/kill",
        json!({"record_id":record_id,"instance_id":instance_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(cancelled["state"], "exited", "{cancelled}");
}

fn manifest(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path.join("manifest.json")).unwrap()).unwrap()
}

async fn wait_final(path: &Path, within: Duration) -> Value {
    let deadline = Instant::now() + within;
    loop {
        let document = manifest(path);
        if matches!(
            document["status"].as_str(),
            Some("submitted" | "submitted_unconfirmed" | "failed")
        ) {
            return document;
        }
        assert!(
            Instant::now() < deadline,
            "worker never finished: {document}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn unconfigured_route_is_501_and_capability_false() {
    let Some(host_binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    let fixture = Fixture::new(&host_binary);
    let app = open(fixture.config(false, &fixture.launcher)).await;
    let (status, meta) = get(&app.prepared.router, "/api/meta").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["capabilities"]["bug_report"], false, "{meta}");
    let (status, body) = post(
        &app.prepared.router,
        "/api/bug-report",
        json!({"description": "x"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert_eq!(body["code"], "bug_report_disabled");
    // The raw upload branch is gated the same way; the JSON branch is untouched.
    let (status, body) = request(
        &app.prepared.router,
        Method::POST,
        "/api/session/attachment?uid=bug-report&name=a.png",
        "image/png",
        Body::from("png"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert_eq!(body["code"], "bug_report_disabled");
    let (status, body) = post(
        &app.prepared.router,
        "/api/session/attachment",
        json!({"uid":"claude:none","ref":"x","job":"j"}),
    )
    .await;
    assert_ne!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert_ne!(body["code"], "bug_report_disabled");
    close(app).await;
}

#[tokio::test]
async fn model_policy_mismatch_is_501_before_any_bundle() {
    let Some(host_binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    let fixture = Fixture::new(&host_binary);
    let app = open(fixture.config(true, &fixture.launcher_bad)).await;
    let (status, meta) = get(&app.prepared.router, "/api/meta").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["capabilities"]["bug_report"], true, "{meta}");
    let (status, body) = post(
        &app.prepared.router,
        "/api/bug-report",
        json!({"description": "pricey", "source": "claude"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert_eq!(body["code"], "bug_report_model_policy");
    assert_eq!(fs::read_dir(&fixture.reports).unwrap().count(), 0);
    // Codex has no worker profile here: Python's 503 "本机找不到 codex 命令".
    let (status, body) = post(
        &app.prepared.router,
        "/api/bug-report",
        json!({"description": "x"}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body["error"].as_str().unwrap().contains("codex"));
    let (status, body) = post(
        &app.prepared.router,
        "/api/bug-report",
        json!({"description": "x", "source": "bash"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("不支持的处理会话类型")
    );
    close(app).await;
}

#[tokio::test]
async fn report_is_captured_injected_and_confirmed_from_the_native_record() {
    let Some(host_binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    let fixture = Fixture::new(&host_binary);
    let app = open(fixture.config(true, &fixture.launcher)).await;
    let router = app.prepared.router.clone();
    let (status, meta) = get(&router, "/api/meta").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["capabilities"]["bug_report"], true, "{meta}");

    // Validation before any bundle: empty description, bad attachments.
    let (status, body) = post(
        &router,
        "/api/bug-report",
        json!({"description": "  ", "source": "claude"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "请描述遇到的问题");
    let (status, body) = post(
        &router,
        "/api/bug-report",
        json!({"description": "x", "source": "claude", "attachments": [{"path": "/etc/passwd"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("附件"), "{body}");
    let (status, body) = post(
        &router,
        "/api/bug-report",
        json!({"description": "x".repeat(50_001), "source": "claude"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(fs::read_dir(&fixture.reports).unwrap().count(), 0);

    // A raw upload lands under <repo>/agenthub_attachments/<id>/<name>.
    let (status, upload) = request(
        &router,
        Method::POST,
        "/api/session/attachment?uid=bug-report&name=%E5%B1%8F%E5%B9%95%E6%88%AA%E5%9B%BE.png",
        "image/png",
        Body::from(vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{upload}");
    assert_eq!(upload["ok"], true);
    assert_eq!(upload["attachment_id"], "1");
    assert_eq!(upload["kind"], "image");
    assert_eq!(upload["mime"], "image/png");
    assert_eq!(
        upload["relative_path"],
        "agenthub_attachments/1/屏幕截图.png"
    );
    let shot = PathBuf::from(upload["path"].as_str().unwrap());
    assert_eq!(
        shot,
        fixture.repo.join("agenthub_attachments/1/屏幕截图.png")
    );
    assert_eq!(
        fs::read(&shot).unwrap(),
        vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4]
    );
    assert_eq!(
        fs::metadata(fixture.repo.join("agenthub_attachments"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    // Same batch id, same content: reused; different content: numbered.
    let (status, again) = request(
        &router,
        Method::POST,
        "/api/session/attachment?uid=bug-report&name=%E5%B1%8F%E5%B9%95%E6%88%AA%E5%9B%BE.png&id=1",
        "image/png",
        Body::from(vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["reused"], true);
    let (status, other) = request(
        &router,
        Method::POST,
        "/api/session/attachment?uid=bug-report&name=%E5%B1%8F%E5%B9%95%E6%88%AA%E5%9B%BE.png&id=1",
        "image/png",
        Body::from(vec![0x89, b'P', b'N', b'G', 9]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{other}");
    assert_eq!(other["name"], "屏幕截图__1.png");
    let (status, bad_id) = request(
        &router,
        Method::POST,
        "/api/session/attachment?uid=bug-report&name=a.png&id=07",
        "image/png",
        Body::from("x"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad_id}");
    let (status, empty) = request(
        &router,
        Method::POST,
        "/api/session/attachment?uid=bug-report&name=a.png",
        "image/png",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{empty}");

    // Some audit context for the window: a browser event for the page.
    let (status, accepted) = post(
        &router,
        "/api/audit/browser",
        json!({"page_id": "page-1", "uid": "claude:none", "events": [{"event": "dom.snapshot", "ts": "t"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");

    let (status, reply) = post(
        &router,
        "/api/bug-report",
        json!({"description": "点了按钮没反应，见 [附件1]", "uid": "claude:none",
            "page_id": "page-1", "source": "claude", "snapshot": {"data": {"selected": "claude:none"}},
            "terminal_name": "agenthub-nope", "cols": 100, "rows": 30, "_build": meta["build"],
            "attachments": [{"path": shot, "number": 1, "name": "屏幕截图.png", "kind": "image",
                "mime": "image/png", "size": 8, "attachment_id": "1"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{reply}");
    assert_eq!(reply["ok"], true);
    let report_id = reply["report_id"].as_str().unwrap().to_owned();
    assert!(report_id.starts_with("BUG-"), "{report_id}");
    let path = PathBuf::from(reply["path"].as_str().unwrap());
    assert_eq!(path, fixture.reports.join(&report_id));
    let worker = &reply["worker"];
    for key in [
        "name",
        "source",
        "sid",
        "cwd",
        "token",
        "title",
        "kind",
        "report_id",
    ] {
        assert!(!worker[key].is_null(), "worker.{key} missing: {worker}");
    }
    assert_eq!(worker["source"], "claude");
    assert_eq!(worker["kind"], "bug-report");
    assert_eq!(worker["title"], format!("处理 {report_id}"));
    assert_eq!(worker["report_id"], report_id);
    assert_eq!(worker["cwd"], fixture.repo.to_str().unwrap());
    let sid = worker["sid"].as_str().unwrap().to_owned();
    let record_id = worker["record_id"].as_str().unwrap().to_owned();
    let instance_id = worker["instance_id"].as_str().unwrap().to_owned();

    // The bundle is complete and private before the worker finishes.
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in [
        "description.md",
        "browser-state.json",
        "events.jsonl",
        "environment.json",
        "worker-prompt.md",
        "manifest.json",
        "attachments/01-屏幕截图.png",
    ] {
        assert!(path.join(name).is_file(), "{name} missing");
    }
    assert!(
        !path.join("terminal.txt").exists(),
        "unknown terminal name yields no capture"
    );
    let prompt = fs::read_to_string(path.join("worker-prompt.md")).unwrap();
    assert!(prompt.contains("附件1: ./agenthub_attachments/1/屏幕截图.png"));
    assert!(prompt.contains("不要 push"));
    let events: Vec<Value> = fs::read_to_string(path.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        events
            .iter()
            .any(|row| row["event"] == "bug_report.created"),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|row| row["event"] == "browser.dom.snapshot"),
        "{events:?}"
    );
    let environment: Value =
        serde_json::from_str(&fs::read_to_string(path.join("environment.json")).unwrap()).unwrap();
    assert_eq!(environment["build"], meta["build"]);
    assert_eq!(environment["repository"], fixture.repo.to_str().unwrap());

    let final_manifest = wait_final(&path, Duration::from_secs(60)).await;
    assert_eq!(final_manifest["status"], "submitted", "{final_manifest}");
    // The confirmed uid is the index's identity for the declared sid.
    let (status, listed) = get(&router, "/api/sessions?force=1").await;
    assert_eq!(status, StatusCode::OK);
    let row = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sid"] == sid)
        .unwrap_or_else(|| panic!("session {sid} not listed: {listed}"));
    assert_eq!(final_manifest["confirmed_from"]["uid"], row["uid"]);
    assert_eq!(final_manifest["confirmed_from"]["text_match"], "report_id");
    assert_eq!(
        final_manifest["confirmed_from"]["method"],
        "native_user_record"
    );
    assert_eq!(final_manifest["worker_source"], "claude");
    assert_eq!(final_manifest["worker"]["record_id"], record_id);
    assert!(
        final_manifest["injection"]["pasted_at"].is_string(),
        "{final_manifest}"
    );
    assert!(
        final_manifest["injection"]["entered_at"].is_string(),
        "{final_manifest}"
    );
    assert_eq!(final_manifest["injection"]["enter_acknowledged"], true);
    assert_eq!(final_manifest["session"], json!({}));
    // The native record carries the exact prompt.
    let jsonl = fs::read_to_string(fixture.jsonl(&sid)).unwrap();
    let users: Vec<Value> = jsonl
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|row| row["type"] == "user")
        .collect();
    assert_eq!(users.len(), 1, "{users:?}");
    assert_eq!(
        users[0]["message"]["content"].as_str().unwrap().trim(),
        prompt.trim()
    );
    // The worker's pending row carries Python's record fields; an ordinary
    // launch keeps the plain projection. The report's own event trail is in
    // the audit log (worker_started/submitted).
    let (status, listing) = get(&router, "/api/term/list").await;
    assert_eq!(status, StatusCode::OK);
    let pending = listing["pending"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["record_id"] == record_id)
        .unwrap_or_else(|| panic!("worker row missing: {listing}"));
    assert_eq!(pending["kind"], "bug-report");
    assert_eq!(pending["title"], format!("处理 {report_id}"));
    assert_eq!(pending["report_id"], report_id);
    assert_eq!(pending["name"], worker["name"]);
    let (status, plain) = post(
        &router,
        "/api/term/create",
        json!({"source":"claude","cwd":fixture.repo,"request_id":"plain-launch-one"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{plain}");
    let (status, listing) = get(&router, "/api/term/list").await;
    assert_eq!(status, StatusCode::OK);
    let ordinary = listing["pending"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["record_id"] == plain["record_id"])
        .unwrap();
    assert!(
        ordinary.get("kind").is_none() && ordinary.get("report_id").is_none(),
        "{ordinary}"
    );
    kill(
        &app,
        plain["record_id"].as_str().unwrap(),
        plain["instance_id"].as_str().unwrap(),
    )
    .await;
    let audit_text: String = fs::read_dir(&fixture.audit)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".jsonl"))
        .map(|entry| fs::read_to_string(entry.path()).unwrap())
        .collect();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = audit_text.contains("bug_report.worker_submitted");
    while !seen && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
        seen = fs::read_dir(&fixture.audit)
            .unwrap()
            .flatten()
            .any(|entry| {
                fs::read_to_string(entry.path())
                    .unwrap_or_default()
                    .contains("bug_report.worker_submitted")
            });
    }
    assert!(seen, "worker_submitted never reached the audit log");

    // A second report for the same page sees the first in its window.
    let (status, reply2) = post(
        &router,
        "/api/bug-report",
        json!({"description": "again", "page_id": "page-1", "source": "claude"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{reply2}");
    let path2 = PathBuf::from(reply2["path"].as_str().unwrap());
    let events2 = fs::read_to_string(path2.join("events.jsonl")).unwrap();
    assert!(
        events2.contains(&report_id),
        "first report's events belong to the page window"
    );
    let final2 = wait_final(&path2, Duration::from_secs(60)).await;
    assert_eq!(final2["status"], "submitted", "{final2}");
    kill(&app, &record_id, &instance_id).await;
    kill(
        &app,
        reply2["worker"]["record_id"].as_str().unwrap(),
        reply2["worker"]["instance_id"].as_str().unwrap(),
    )
    .await;
    close(app).await;
    let _ = &fixture.root;
}
