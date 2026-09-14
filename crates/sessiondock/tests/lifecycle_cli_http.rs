//! Real CLI creation/resume argument contract through the HTTP router with an
//! isolated ptyhost directory. Every "CLI" is a private synthetic shell script
//! that echoes its argv/environment to the PTY and then runs a free shell; no
//! model binary, native CLI home or production host is involved. The suite
//! skips itself when the local ptyhost target has not been built.
#![cfg(unix)]

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use ptyhost_client::{CaptureKind, ControlOp, ControlReply, HostClient, Limits};
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
const SETTINGS: &str = "/synthetic/bridge-settings.json";
const FAKE_CLI: &str = r#"#!/bin/sh
printf 'FAKE_%s_ARGV' "$SESSIONDOCK_TEST_LABEL"
for arg in "$@"; do printf ' [%s]' "$arg"; done
printf '\nFAKE_%s_ENV HOME=[%s] TERM=[%s] CLAUDE_CODE_SESSION_ID=[%s] CODEX_COMPANION_SESSION_ID=[%s] GROK_SESSION_ID=[%s] TMUX=[%s] MARK=[%s]\n' \
  "$SESSIONDOCK_TEST_LABEL" "$HOME" "$TERM" "$CLAUDE_CODE_SESSION_ID" "$CODEX_COMPANION_SESSION_ID" "$GROK_SESSION_ID" "$TMUX" "$SESSIONDOCK_TEST_MARK"
printf 'FAKE_%s_CWD [%s]\n' "$SESSIONDOCK_TEST_LABEL" "$(pwd -P)"
exec /bin/sh -c 'stty -echo 2>/dev/null; printf "RS_SHELL_READY\n"; while IFS= read -r line; do case "$line" in quit) exit 0 ;; *) printf "RS_UNKNOWN\n" ;; esac; done'
"#;

struct Fixture {
    _temp: tempfile::TempDir,
    lifecycle: PathBuf,
    host: PathBuf,
    claude_area: PathBuf,
    codex_area: PathBuf,
    work: PathBuf,
    web: PathBuf,
    codex_root: PathBuf,
    launcher: PathBuf,
    codex_uid: String,
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
fn sha1_uid(path: &Path) -> String {
    // Same local UID baseline as the inventory: source:sha1(path)[:16].
    use sha1::Digest;
    let digest = sha1::Sha1::digest(path.to_str().unwrap().as_bytes());
    format!(
        "codex:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            .get(..16)
            .unwrap()
    )
}

impl Fixture {
    fn new(host_binary: &Path) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let lifecycle = root.join("lifecycle");
        let host = root.join("hosts");
        let work = root.join("work");
        let claude_area = work.join("claude-area");
        let codex_area = work.join("codex-area");
        let web = root.join("web");
        let bin = root.join("bin");
        let codex_root = root.join("codex");
        for path in [
            &lifecycle,
            &host,
            &claude_area,
            &codex_area,
            &web,
            &bin,
            &codex_root,
        ] {
            directory(path);
        }
        file(&web.join("index.html"),b"<!doctype html><meta name=\"sessiondock-mode\" content=\"local\"><title>synthetic</title>",0o600);
        for name in ["fake-claude", "fake-codex"] {
            file(&bin.join(name), FAKE_CLI.as_bytes(), 0o700);
        }
        // One synthetic Codex main session with a full UUID thread id.
        let rollout = codex_root.join(format!("rollout-{CODEX_SID}.jsonl"));
        let rows = [
            json!({"type":"session_meta","timestamp":"2026-09-11T10:00:00Z","payload":{"id":CODEX_SID,"cwd":codex_area}}),
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
        let codex_uid = sha1_uid(&rollout);
        let launcher = root.join("launcher.json");
        let config = json!({"schema":2,"host_binary":host_binary,"host_dir":host,
        "adapters":[],
        "profiles":[
            {"id":"claude-cli-v1","source":"claude","executable":bin.join("fake-claude"),
             "args":["--settings",SETTINGS],
             "new_args":["--session-id","{session_id}"],
             "resume_args":["--resume","{sid}"],
             "env":{"PATH":"/usr/bin:/bin","HOME":"/synthetic/claude-home","SESSIONDOCK_TEST_LABEL":"CLAUDE","SESSIONDOCK_TEST_MARK":"claude-profile"},
             "env_remove":["TERM"],
             },
            {"id":"codex-cli-v1","source":"codex","executable":bin.join("fake-codex"),
             "args":["--enable","default_mode_request_user_input","-c","suppress_unstable_features_warning=true"],
             "new_args":[],
             "resume_args":["resume","{sid}"],
             "env":{"PATH":"/usr/bin:/bin","HOME":"/synthetic/codex-home","TERM":"xterm-256color","SESSIONDOCK_TEST_LABEL":"CODEX","SESSIONDOCK_TEST_MARK":"codex-profile"},
             }
        ]});
        file(&launcher, config.to_string().as_bytes(), 0o600);
        drop(LifecycleStore::initialize(&lifecycle).unwrap());
        Self {
            _temp: temp,
            lifecycle,
            host,
            claude_area,
            codex_area,
            work,
            web,
            codex_root,
            launcher,
            codex_uid,
        }
    }
    fn config(&self) -> Config {
        Config {
            web_dir: self.web.clone(),
            ptyhost_dir: Some(self.host.clone()),
            lifecycle_dir: Some(self.lifecycle.clone()),
            launcher_config: Some(self.launcher.clone()),
            roots: SessionRoots {
                codex: Some(self.codex_root.clone()),
                ..Default::default()
            },
            ..Default::default()
        }
    }
    fn host_meta(&self, name: &str) -> Value {
        let record: Value =
            serde_json::from_slice(&fs::read(self.host.join(format!("{name}.json"))).unwrap())
                .unwrap();
        record["meta"].clone()
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

/// Read the PTY scrollback through the launch-guarded control path until the
/// fake CLI's echo lines are visible. Bounded; never a name-only operation.
async fn capture(
    lifecycle: &sessiondock::lifecycle::service::LifecycleService,
    client: &HostClient,
    record_id: &str,
    marker: &str,
) -> String {
    let target = lifecycle.target(record_id.into()).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let reply = client
            .request_launch(
                &target,
                ControlOp::Capture {
                    kind: CaptureKind::Scrollback,
                    styled: false,
                    join: true,
                    lines: 200,
                },
            )
            .await
            .unwrap();
        let ControlReply::Capture(capture) = reply else {
            panic!("capture reply")
        };
        if capture.text.contains(marker) {
            return capture.text;
        }
        assert!(Instant::now() < deadline, "no CLI echo: {}", capture.text);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
fn assert_receipt_shape(receipt: &Value) {
    for key in [
        "argv",
        "env",
        "token",
        "port",
        "sock",
        "executable",
        "args",
        "uid",
        "sid",
    ] {
        assert!(receipt.get(key).is_none(), "{key} leaked: {receipt}");
    }
    assert_eq!(receipt["native_binding"], "unbound");
    assert!(receipt["binding"].is_null());
}

#[tokio::test]
async fn real_cli_profiles_launch_exact_argv_declare_identity_and_stay_pending_for_codex() {
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
    let client = HostClient::new(
        &fixture.host,
        Limits {
            max_line_bytes: 64 * 1024,
            operation_timeout: Duration::from_secs(2),
            ..Default::default()
        },
    )
    .unwrap();
    let mut records = Vec::new();

    // ---- Claude new: server UUID `--session-id`, declared sid metadata.
    let body =
        json!({"source":"claude","cwd":fixture.claude_area,"request_id":"cli-claude-new-request"});
    let (status, claude) = post(&router, "/api/term/create", body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{claude}");
    assert_receipt_shape(&claude);
    assert_eq!(claude["running"], true, "{claude}");
    assert_eq!(claude["launch_kind"], "new_assigned");
    let claude_sid = claude["declared_sid"].as_str().unwrap().to_owned();
    assert_eq!(claude_sid.len(), 36);
    assert!(claude["declared_uid"].is_null());
    let (status, replay) = post(&router, "/api/term/create", body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["record_id"], claude["record_id"]);
    assert_eq!(replay["declared_sid"], claude_sid);
    let text = capture(
        &lifecycle,
        &client,
        claude["record_id"].as_str().unwrap(),
        "FAKE_CLAUDE_CWD",
    )
    .await;
    assert!(
        text.contains(&format!(
            "FAKE_CLAUDE_ARGV [--settings] [{SETTINGS}] [--session-id] [{claude_sid}]"
        )),
        "{text}"
    );
    // env_remove dropped the inherited TERM; the host then supplies
    // its own. Identity variables and TMUX are absent; profile env is present.
    assert!(text.contains("HOME=[/synthetic/claude-home]"), "{text}");
    assert!(text.contains("CLAUDE_CODE_SESSION_ID=[] CODEX_COMPANION_SESSION_ID=[] GROK_SESSION_ID=[] TMUX=[] MARK=[claude-profile]"), "{text}");
    assert!(
        text.contains(&format!(
            "FAKE_CLAUDE_CWD [{}]",
            fixture.claude_area.display()
        )),
        "{text}"
    );
    let meta = fixture.host_meta(claude["name"].as_str().unwrap());
    assert_eq!(meta["sid"], claude_sid);
    assert_eq!(meta["source"], "claude");
    assert_eq!(meta["launch_id"], claude["launch_id"]);
    assert_eq!(meta["instance_id"], claude["instance_id"]);
    assert!(meta.get("uid").is_none());
    records.push(claude.clone());

    // ---- Codex new: fixed prefix only; identity stays pending (no sid anywhere).
    let cwd_alias = fixture.work.join("codex-linked");
    std::os::unix::fs::symlink(&fixture.codex_area, &cwd_alias).unwrap();
    let (status, codex_new) = post(
        &router,
        "/api/term/create",
        json!({"source":"codex","cwd":cwd_alias.join("../codex-linked"),"request_id":"cli-codex-new-request"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{codex_new}");
    assert_eq!(codex_new["cwd"], fixture.codex_area.to_str().unwrap());
    // The same directory through its canonical spelling reuses the receipt.
    let (status, replay) = post(
        &router,
        "/api/term/create",
        json!({"source":"codex","cwd":fixture.codex_area,"request_id":"cli-codex-new-request"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["record_id"], codex_new["record_id"]);
    assert_receipt_shape(&codex_new);
    assert_eq!(codex_new["launch_kind"], "new_pending");
    assert!(codex_new["declared_sid"].is_null() && codex_new["declared_uid"].is_null());
    let text = capture(
        &lifecycle,
        &client,
        codex_new["record_id"].as_str().unwrap(),
        "FAKE_CODEX_CWD",
    )
    .await;
    assert!(text.contains("FAKE_CODEX_ARGV [--enable] [default_mode_request_user_input] [-c] [suppress_unstable_features_warning=true]\n"), "{text}");
    assert!(!text.contains("resume"), "{text}");
    let meta = fixture.host_meta(codex_new["name"].as_str().unwrap());
    assert!(
        meta.get("sid").is_none() && meta.get("uid").is_none(),
        "{meta}"
    );
    assert_eq!(meta["launch_id"], codex_new["launch_id"]);
    records.push(codex_new.clone());

    // ---- Codex resume through create: SID resolved from the frozen inventory.
    let (status, resume) = post(&router, "/api/term/create",
        json!({"source":"codex","cwd":fixture.codex_area,"request_id":"cli-codex-resume-request","resume_uid":fixture.codex_uid})).await;
    assert_eq!(status, StatusCode::OK, "{resume}");
    assert_receipt_shape(&resume);
    assert_eq!(resume["launch_kind"], "resume");
    assert_eq!(resume["declared_sid"], CODEX_SID);
    assert_eq!(resume["declared_uid"], fixture.codex_uid);
    let text = capture(
        &lifecycle,
        &client,
        resume["record_id"].as_str().unwrap(),
        "FAKE_CODEX_CWD",
    )
    .await;
    assert!(text.contains(&format!("FAKE_CODEX_ARGV [--enable] [default_mode_request_user_input] [-c] [suppress_unstable_features_warning=true] [resume] [{CODEX_SID}]\n")), "{text}");
    assert!(
        text.contains("HOME=[/synthetic/codex-home] TERM=[xterm-256color]"),
        "{text}"
    );
    let meta = fixture.host_meta(resume["name"].as_str().unwrap());
    assert_eq!(meta["sid"], CODEX_SID);
    assert_eq!(meta["uid"], fixture.codex_uid);
    records.push(resume.clone());
    // A second resume of the same native session reuses the live instance
    // instead of starting a second `resume` process.
    let (status, again) = post(&router, "/api/term/create",
        json!({"source":"codex","cwd":fixture.codex_area,"request_id":"cli-codex-resume-other","resume_uid":fixture.codex_uid})).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["record_id"], resume["record_id"]);
    let (status, takeover) = post(
        &router,
        "/api/term/takeover",
        json!({"uid":fixture.codex_uid,"request_id":"cli-codex-takeover","cols":120,"rows":32}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{takeover}");
    assert_eq!(takeover["record_id"], resume["record_id"]);
    assert_eq!(takeover["action"], "reused");
    assert_eq!(takeover["name"], resume["name"]);
    assert_eq!(
        fs::read_dir(&fixture.host)
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|x| x == "json"))
            .count(),
        3
    );
    // The runtime catalog associates the resumed host through its immutable
    // metadata, so the legacy list shows it as this session's console.
    let (status, list) = get(&router, "/api/term/list").await;
    assert_eq!(status, StatusCode::OK);
    let sessions = list["sessions"].as_array().unwrap();
    let row = sessions
        .iter()
        .find(|row| row["uid"] == fixture.codex_uid)
        .unwrap_or_else(|| panic!("{list}"));
    assert_eq!(row["name"], resume["name"]);
    assert_eq!(row["instance_id"], resume["instance_id"]);
    assert_eq!(row["sid"], CODEX_SID);
    assert!(row["origin_launch_id"].is_null());
    assert_eq!(
        list["resume_sources"],
        json!({"claude":true,"codex":true,"grok":false})
    );
    assert_eq!(
        list["sources"],
        json!({"claude":true,"codex":true,"grok":false})
    );
    assert_eq!(list["backend"], "ptyhost");
    assert_eq!(list["backends"][0]["name"], "ptyhost");
    assert_eq!(list["backends"][1]["available"], false);
    assert_eq!(list["pending"].as_array().unwrap().len(), 3);
    // Declared identities never accept a separate operator binding.
    let (status, bound) = post(&router, "/api/term/bind",
        json!({"record_id":resume["record_id"],"instance_id":resume["instance_id"],"uid":fixture.codex_uid,"operator_confirmed":true})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{bound}");
    assert_eq!(bound["code"], "launch_identity_declared");
    let (status, bound) = post(&router, "/api/term/bind",
        json!({"record_id":claude["record_id"],"instance_id":claude["instance_id"],"uid":fixture.codex_uid,"operator_confirmed":true})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{bound}");

    // Managed takeover reuses the existing host even with force, and optional
    // request IDs or unrelated JSON members do not add Python-absent gates.
    for (body, expected) in [
        (
            json!({"uid":fixture.codex_uid,"request_id":"cli-force","force":true}),
            StatusCode::OK,
        ),
        (
            json!({"uid":"codex:0000000000000000","request_id":"cli-missing"}),
            StatusCode::NOT_FOUND,
        ),
        (json!({"uid":fixture.codex_uid}), StatusCode::OK),
        (
            json!({"uid":"../etc","request_id":"cli-bad-uid"}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"uid":fixture.codex_uid,"request_id":"cli-extra","sid":CODEX_SID}),
            StatusCode::OK,
        ),
    ] {
        let (status, reply) = post(&router, "/api/term/takeover", body).await;
        assert_eq!(status, expected, "{reply}");
    }
    for (body, expected) in [
        (
            json!({"source":"claude","cwd":fixture.claude_area,"request_id":"cli-cross","resume_uid":fixture.codex_uid}),
            StatusCode::CONFLICT,
        ),
        (
            json!({"source":"codex","cwd":fixture.codex_area,"request_id":"cli-bad-uid","resume_uid":"codex:has space"}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"source":"codex","cwd":fixture.codex_area,"request_id":"cli-codex-new-request","sid":CODEX_SID}),
            StatusCode::OK,
        ),
        (
            json!({"source":"codex","cwd":fixture.codex_area,"request_id":"cli-codex-new-request","argv":["/bin/sh"]}),
            StatusCode::OK,
        ),
        (
            json!({"source":"codex","cwd":fixture.claude_area,"request_id":"cli-outside"}),
            StatusCode::OK,
        ),
        (
            json!({"source":"claude","cwd":fixture.work,"request_id":"cli-global-only"}),
            StatusCode::OK,
        ),
        (
            json!({"source":"grok","cwd":fixture.work,"request_id":"cli-grok"}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, reply) = post(&router, "/api/term/create", body).await;
        assert_eq!(status, expected, "{reply}");
    }
    assert_eq!(
        fs::read_dir(&fixture.host)
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|x| x == "json"))
            .count(),
        5
    );

    // ---- Directory completion accepts any absolute directory.
    let work = fixture.work.to_str().unwrap();
    let (status, done) = get(&router, &format!("/api/term/complete-dir?path={work}/c")).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(
        done["directories"],
        json!([
            format!("{work}/claude-area/"),
            format!("{work}/codex-area/"),
            format!("{work}/codex-linked/")
        ])
    );
    let (status, done) = get(
        &router,
        &format!("/api/term/complete-dir?path={work}/co&limit=1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["directories"], json!([format!("{work}/codex-area/")]));
    // Root and other ordinary absolute directories enumerate their real
    // children; configured work paths do not act as an authorization jail.
    let (status, done) = get(&router, "/api/term/complete-dir?path=/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        done["directories"]
            .as_array()
            .unwrap()
            .contains(&json!("/etc/"))
    );
    for path in ["/etc/", "~/", &format!("{work}/../")] {
        let (status, done) = get(&router, &format!("/api/term/complete-dir?path={path}")).await;
        assert_eq!(status, StatusCode::OK, "{done}");
        assert!(
            !done["directories"].as_array().unwrap().is_empty(),
            "{path}"
        );
    }
    for path in ["/var/synthetic/", "%2E%2E/"] {
        let (status, done) = get(&router, &format!("/api/term/complete-dir?path={path}")).await;
        assert_eq!(status, StatusCode::OK, "{done}");
        assert_eq!(done["directories"], json!([]), "{path}");
    }
    let (status, done) = get(&router, "/api/term/complete-dir?path=&unknown=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["directories"], json!([]));

    // ---- Backend selection: only ptyhost; tmux is an explicit error.
    let (status, backend) = post(&router, "/api/term/backend", json!({"backend":"ptyhost"})).await;
    assert_eq!(status, StatusCode::OK, "{backend}");
    assert_eq!(backend["backend"], "ptyhost");
    assert_eq!(backend["backends"][1]["name"], "tmux");
    let (status, backend) = post(&router, "/api/term/backend", json!({"backend":"tmux"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(backend["code"], "backend_unsupported");
    let (status, _) = post(&router, "/api/term/backend", json!({"backend":"screen"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Managed stop: the resumed fake CLI exits on Ctrl-D; the
    // later exact kill below then finishes on an already exited receipt.
    let (status, stopped) = post(
        &router,
        "/api/session/stop",
        json!({"uid":fixture.codex_uid,"request_id":"cli-stop"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["stage"], "graceful", "{stopped}");
    assert_eq!(stopped["record_id"], resume["record_id"]);

    // ---- Cleanup only the exact instances this test created.
    for record in &records {
        let (status, cancelled) = post(
            &router,
            "/api/term/kill",
            json!({"record_id":record["record_id"],"instance_id":record["instance_id"]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{cancelled}");
        assert_eq!(cancelled["state"], "exited", "{cancelled}");
    }
    lifecycle.shutdown().await.unwrap();
    shutdown.cancel();
}
