//! `POST /api/session/stop` for managed instances through the HTTP router with
//! an isolated ptyhost directory. Every "CLI" is a private synthetic shell
//! script; no model binary, native CLI home or production host is involved.
//! The suite skips itself when the local ptyhost target has not been built.
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

const CODEX_SID: &str = "8f3c1d2e-4a5b-4c6d-8e7f-90a1b2c3d4e5";
const OTHER_CODEX_SID: &str = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";
const CLAUDE_SID: &str = "2c9d8e7f-6a5b-4c4d-9e3f-2a1b0c9d8e7f";
/// Exits on EOF like an idle CLI prompt: Ctrl-D ends the `read` loop.
const FAKE_EOF_EXITS: &str = r#"#!/bin/sh
printf 'FAKE_%s_ARGV' "$SESSIONDOCK_TEST_LABEL"
for arg in "$@"; do printf ' [%s]' "$arg"; done
printf '\n'
exec /bin/sh -c 'stty -echo 2>/dev/null; printf "RS_SHELL_READY\n"; while IFS= read -r line; do case "$line" in quit) exit 0 ;; *) printf "RS_UNKNOWN\n" ;; esac; done'
"#;
/// Ignores EOF (a CLI that does not react to Ctrl-D): every EOF simply loops
/// back into a blocking read. Only the host's SIGHUP ends it.
const FAKE_EOF_IGNORED: &str = r#"#!/bin/sh
printf 'FAKE_%s_ARGV' "$SESSIONDOCK_TEST_LABEL"
for arg in "$@"; do printf ' [%s]' "$arg"; done
printf '\n'
exec /bin/sh -c 'stty -echo 2>/dev/null; printf "RS_SHELL_READY\n"; while :; do IFS= read -r line || continue; case "$line" in quit) exit 0 ;; *) printf "RS_UNKNOWN\n" ;; esac; done'
"#;

struct Fixture {
    _temp: tempfile::TempDir,
    lifecycle: PathBuf,
    host: PathBuf,
    web: PathBuf,
    claude_root: PathBuf,
    codex_root: PathBuf,
    launcher: PathBuf,
    codex_uid: String,
    other_codex_uid: String,
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
fn sha1_uid(source: &str, path: &Path) -> String {
    use sha1::Digest;
    let digest = sha1::Sha1::digest(path.to_str().unwrap().as_bytes());
    format!(
        "{source}:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            .get(..16)
            .unwrap()
    )
}
fn codex_rollout(root: &Path, sid: &str, cwd: &Path) -> PathBuf {
    let rollout = root.join(format!("rollout-{sid}.jsonl"));
    let rows = [
        json!({"type":"session_meta","timestamp":"2026-09-11T10:00:00Z","payload":{"id":sid,"cwd":cwd}}),
        json!({"type":"response_item","timestamp":"2026-09-11T10:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Unchanged native history"}]}}),
    ];
    file(
        &rollout,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>()
            .as_bytes(),
        0o600,
    );
    rollout
}
fn claude_row(
    sid: &str,
    kind: &str,
    uuid: &str,
    parent: Option<&str>,
    text: &str,
    cwd: &Path,
) -> Value {
    let mut row = json!({"type": kind, "uuid": uuid, "parentUuid": parent, "sessionId": sid,
        "cwd": cwd, "timestamp": "2026-09-11T10:00:00Z", "isSidechain": false,
        "message": {"role": kind, "content": text}});
    if kind == "assistant" {
        row["message"]["stop_reason"] = json!("end_turn");
    }
    row
}

impl Fixture {
    fn new(host_binary: &Path) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let lifecycle = root.join("lifecycle");
        let host = root.join("hosts");
        let work = root.join("work");
        let web = root.join("web");
        let bin = root.join("bin");
        let codex_root = root.join("codex");
        let claude_root = root.join("claude");
        let claude_project = claude_root.join("project-demo");
        for path in [
            &lifecycle,
            &host,
            &work,
            &web,
            &bin,
            &codex_root,
            &claude_project,
        ] {
            directory(path);
        }
        file(&web.join("index.html"),b"<!doctype html><meta name=\"sessiondock-mode\" content=\"local\"><title>synthetic</title>",0o600);
        file(&bin.join("fake-codex"), FAKE_EOF_EXITS.as_bytes(), 0o700);
        file(&bin.join("fake-claude"), FAKE_EOF_IGNORED.as_bytes(), 0o700);
        let codex_uid = sha1_uid("codex", &codex_rollout(&codex_root, CODEX_SID, &work));
        let other_codex_uid =
            sha1_uid("codex", &codex_rollout(&codex_root, OTHER_CODEX_SID, &work));
        file(
            &claude_project.join(format!("{CLAUDE_SID}.jsonl")),
            [
                claude_row(
                    CLAUDE_SID,
                    "user",
                    "u0",
                    None,
                    "Synthetic claude question",
                    &work,
                ),
                claude_row(
                    CLAUDE_SID,
                    "assistant",
                    "a0",
                    Some("u0"),
                    "Synthetic claude answer",
                    &work,
                ),
            ]
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>()
            .as_bytes(),
            0o600,
        );
        let launcher = root.join("launcher.json");
        let config = json!({"schema":2,"host_binary":host_binary,"host_dir":host,
        "adapters":[],
        "profiles":[
            {"id":"codex-cli-v1","source":"codex","executable":bin.join("fake-codex"),
             "args":[],"new_args":[],"resume_args":["resume","{sid}"],
             "env":{"PATH":"/usr/bin:/bin","HOME":"/synthetic/codex-home","SESSIONDOCK_TEST_LABEL":"CODEX"},
             },
            {"id":"claude-cli-v1","source":"claude","executable":bin.join("fake-claude"),
             "args":[],"new_args":["--session-id","{session_id}"],"resume_args":["--resume","{sid}"],
             "env":{"PATH":"/usr/bin:/bin","HOME":"/synthetic/claude-home","SESSIONDOCK_TEST_LABEL":"CLAUDE"},
             }
        ]});
        file(&launcher, config.to_string().as_bytes(), 0o600);
        drop(LifecycleStore::initialize(&lifecycle).unwrap());
        Self {
            _temp: temp,
            lifecycle,
            host,
            web,
            claude_root,
            codex_root,
            launcher,
            codex_uid,
            other_codex_uid,
        }
    }
    fn config(&self) -> Config {
        Config {
            web_dir: self.web.clone(),
            ptyhost_dir: Some(self.host.clone()),
            lifecycle_dir: Some(self.lifecycle.clone()),
            launcher_config: Some(self.launcher.clone()),
            roots: SessionRoots {
                claude: Some(self.claude_root.clone()),
                codex: Some(self.codex_root.clone()),
                ..Default::default()
            },
            ..Default::default()
        }
    }
    fn host_records(&self) -> usize {
        fs::read_dir(&self.host)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            })
            .count()
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
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn post(router: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    request(router, Method::POST, uri, Some(body)).await
}
async fn get(router: &Router, uri: &str) -> (StatusCode, Value) {
    request(router, Method::GET, uri, None).await
}
async fn uid_of(router: &Router, sid: &str) -> String {
    let (status, list) = get(router, "/api/sessions").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sid"] == sid)
        .unwrap_or_else(|| panic!("session {sid} listed: {list}"))["uid"]
        .as_str()
        .unwrap()
        .to_owned()
}
/// Wait for the runtime catalog to associate the resumed host with its UID.
async fn wait_listed(router: &Router, uid: &str, instance: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (status, list) = get(router, "/api/term/list").await;
        assert_eq!(status, StatusCode::OK, "{list}");
        if list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["uid"] == uid && row["instance_id"] == instance)
        {
            return;
        }
        assert!(Instant::now() < deadline, "instance never listed: {list}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
async fn wait_live_state(router: &Router, uid: &str, state: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (status, live) = get(router, "/api/live?force=1").await;
        assert_eq!(status, StatusCode::OK, "{live}");
        if live["managed"]["sessions"][uid]["state"] == state {
            return live;
        }
        assert!(Instant::now() < deadline, "live never {state}: {live}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn unconfigured_lifecycle_keeps_stop_501_and_capability_false() {
    let Some(host_binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    let fixture = Fixture::new(&host_binary);
    let mut config = fixture.config();
    config.lifecycle_dir = None;
    config.launcher_config = None;
    let shutdown = CancellationToken::new();
    let prepared = prepare_app(config, shutdown.clone()).await.unwrap();
    let (_, meta) = get(&prepared.router, "/api/meta").await;
    assert_eq!(meta["capabilities"]["session_stop"], false, "{meta}");
    assert_eq!(meta["capabilities"]["terminal"], true);
    let (status, reply) = post(
        &prepared.router,
        "/api/session/stop",
        json!({"uid":fixture.codex_uid,"request_id":"stop-unconfigured"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{reply}");
    assert_eq!(reply["code"], "not_implemented");
    shutdown.cancel();
}

#[tokio::test]
async fn managed_instances_stop_through_the_host_and_external_sessions_are_refused() {
    let Some(host_binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    let fixture = Fixture::new(&host_binary);
    let shutdown = CancellationToken::new();
    let prepared = prepare_app(fixture.config(), shutdown.clone())
        .await
        .unwrap();
    let router = prepared.router.clone();
    let lifecycle = prepared.lifecycle.clone().unwrap();
    let (_, meta) = get(&router, "/api/meta").await;
    assert_eq!(meta["capabilities"]["session_stop"], true, "{meta}");
    let claude_uid = uid_of(&router, CLAUDE_SID).await;
    assert_eq!(uid_of(&router, CODEX_SID).await, fixture.codex_uid);

    // The stringified UID is looked up first, so malformed and unknown IDs
    // are both ordinary missing sessions.
    for (body, expected, code) in [
        (
            json!({"uid":"../etc","request_id":"stop-bad-uid"}),
            StatusCode::NOT_FOUND,
            "session_missing",
        ),
        (
            json!({"uid":"codex:0000000000000000","request_id":"stop-missing"}),
            StatusCode::NOT_FOUND,
            "session_missing",
        ),
    ] {
        let (status, reply) = post(&router, "/api/session/stop", body).await;
        assert_eq!(status, expected, "{reply}");
        assert_eq!(reply["code"], code, "{reply}");
    }

    // Unknown keys and an unusable optional request id are ignored in
    // dictionary body handling.
    for body in [
        json!({"uid":fixture.other_codex_uid,"request_id":"stop-extra","force":true}),
        json!({"uid":fixture.other_codex_uid,"request_id":""}),
    ] {
        let (status, reply) = post(&router, "/api/session/stop", body).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert_eq!(reply["stopped"], false);
    }

    // An inventory session with no running process is already stopped.
    let (status, reply) = post(
        &router,
        "/api/session/stop",
        json!({"uid":fixture.other_codex_uid,"request_id":"stop-unmanaged"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["stopped"], false);
    // A bare body works the same way (no request_id needed).
    let (status, reply) = post(
        &router,
        "/api/session/stop",
        json!({"uid":fixture.other_codex_uid}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(fixture.host_records(), 0);

    // ---- Managed Codex resume (declared identity, no operator binding).
    let (status, resume) = post(
        &router,
        "/api/term/takeover",
        json!({"uid":fixture.codex_uid,"request_id":"stop-codex-takeover"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resume}");
    assert_eq!(resume["running"], true, "{resume}");
    let instance = resume["instance_id"].as_str().unwrap().to_owned();
    let name = resume["name"].as_str().unwrap().to_owned();
    wait_listed(&router, &fixture.codex_uid, &instance).await;
    let live = wait_live_state(&router, &fixture.codex_uid, "running").await;
    assert_eq!(live["uids"], json!([fixture.codex_uid]));

    // request_id is an ignored browser field.
    let started = Instant::now();
    let (status, stopped) = post(
        &router,
        "/api/session/stop",
        json!({"uid":fixture.codex_uid,"request_id":"stop-codex-1"}),
    )
    .await;
    let elapsed = started.elapsed();
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["ok"], true);
    assert_eq!(stopped["stage"], "graceful", "{stopped}");
    assert_eq!(stopped["stopped"], true);
    assert_eq!(stopped["graceful_attempts"], 1);
    assert_eq!(stopped["tmux"], false);
    assert_eq!(stopped["name"], name);
    assert_eq!(stopped["instance_id"], instance);
    assert_eq!(stopped["record_id"], resume["record_id"]);
    assert_eq!(stopped["external_detection"], "proc_scan");
    assert!(
        elapsed < Duration::from_millis(2400),
        "graceful exit should end the first EOF wait early: {elapsed:?}"
    );
    for key in ["argv", "env", "token", "port", "sock", "pid"] {
        assert!(stopped.get(key).is_none(), "{key} leaked: {stopped}");
    }
    // The host reaped the shell and cleaned its record; the run state is exited.
    let ended = wait_live_state(&router, &fixture.codex_uid, "exited").await;
    assert_eq!(ended["uids"], json!([]));
    assert_eq!(fixture.host_records(), 0);
    let (status, receipt) = get(
        &router,
        &format!(
            "/api/term/new-status?record_id={}&instance_id={instance}",
            resume["record_id"].as_str().unwrap()
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["state"], "exited", "{receipt}");
    assert_eq!(receipt["native_binding"], "unbound");

    // A repeated stop re-observes current state; it does not replay a private
    // server-side outcome keyed by an ignored field.
    let (status, replay) = post(
        &router,
        "/api/session/stop",
        json!({"uid":fixture.codex_uid,"request_id":"stop-codex-1"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["stopped"], false);
    assert_eq!(replay["tmux"], false);
    assert_eq!(replay["external_detection"], "proc_scan");
    // Reusing the ignored value for another session is not a conflict.
    let (status, other) = post(
        &router,
        "/api/session/stop",
        json!({"uid":claude_uid,"request_id":"stop-codex-1"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{other}");
    assert_eq!(other["stopped"], false);
    // A new request after the exit also observes that nothing is running.
    let (status, again) = post(
        &router,
        "/api/session/stop",
        json!({"uid":fixture.codex_uid,"request_id":"stop-codex-2"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["stopped"], false);
    assert_eq!(again["tmux"], false);

    // ---- A managed Claude resume whose shell ignores EOF: both Ctrl-D
    // attempts elapse, then the guarded host stop (the term/kill path) ends it
    // within the cancel bound. The Web process signals no PID.
    let (status, claude) = post(
        &router,
        "/api/term/takeover",
        json!({"uid":claude_uid,"request_id":"stop-claude-takeover"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claude}");
    assert_eq!(claude["running"], true, "{claude}");
    let claude_instance = claude["instance_id"].as_str().unwrap().to_owned();
    wait_listed(&router, &claude_uid, &claude_instance).await;
    let started = Instant::now();
    let (status, escalated) = post(
        &router,
        "/api/session/stop",
        json!({"uid":claude_uid,"request_id":"stop-claude-1"}),
    )
    .await;
    let elapsed = started.elapsed();
    assert_eq!(status, StatusCode::OK, "{escalated}");
    assert_eq!(escalated["stage"], "stopped", "{escalated}");
    assert_eq!(escalated["stopped"], true);
    assert_eq!(escalated["graceful_attempts"], 2);
    assert_eq!(escalated["record_id"], claude["record_id"]);
    assert!(
        elapsed >= Duration::from_millis(2400) && elapsed < Duration::from_secs(9),
        "escalation must wait both EOF rounds and finish within the cancel bound: {elapsed:?}"
    );
    let (status, receipt) = get(
        &router,
        &format!(
            "/api/term/new-status?record_id={}&instance_id={claude_instance}",
            claude["record_id"].as_str().unwrap()
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["state"], "exited", "{receipt}");
    // The guarded stop is the durable cancel path: its flag is retained.
    assert!(
        receipt["unavailable_reason"]
            .as_str()
            .unwrap()
            .contains("已退出"),
        "{receipt}"
    );
    wait_live_state(&router, &claude_uid, "exited").await;
    assert_eq!(fixture.host_records(), 0);
    // Native histories were never written.
    assert_eq!(
        fs::read_to_string(
            fixture
                .codex_root
                .join(format!("rollout-{CODEX_SID}.jsonl"))
        )
        .unwrap()
        .lines()
        .count(),
        2
    );
    lifecycle.shutdown().await.unwrap();
    shutdown.cancel();
}
