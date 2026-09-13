//! WP-E: process-evidence binding of a pending Codex launch, the finished
//! receipt's discard/archive rules, and the server-side `/proc` pairing.
//!
//! The "CLI" is a private bash script that re-executes itself under argv0
//! `codex` (so the scan counts it as a CLI main process), holds one synthetic
//! rollout open on fd 3 and runs a free read loop; the process table is the
//! real `/proc` of this machine, read-only. A second rollout in the same
//! cwd that nobody holds must never be chosen: cwd is not evidence. The
//! suite skips itself when ptyhost is unbuilt or `/bin/bash` is absent.
#![cfg(all(unix, target_os = "linux"))]

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
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

const HELD_SID: &str = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
const DECOY_SID: &str = "1b2c3d4e-5f6a-4b7c-8d8e-9f0a1b2c3d4e";
/// Re-exec under argv0 `codex`; fd 3 holds the rollout named by the profile
/// environment (`SESSIONDOCK_TEST_*` keys pass the launcher allowlist).
const FAKE_CODEX: &str = r#"#!/bin/bash
exec -a codex /bin/bash -c 'exec 3<>"$SESSIONDOCK_TEST_ROLLOUT"; printf "RS_SHELL_READY\n"; while IFS= read -r line; do case "$line" in quit) exit 0 ;; esac; done'
"#;

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
fn rollout(path: &Path, sid: &str, cwd: &Path) {
    let rows = [
        json!({"type":"session_meta","timestamp":"2026-09-13T10:00:00Z","payload":{"id":sid,"cwd":cwd}}),
        json!({"type":"response_item","timestamp":"2026-09-13T10:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"只回一个词：pong"}]}}),
    ];
    file(
        path,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>()
            .as_bytes(),
        0o600,
    );
}

async fn request(
    router: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let response = router
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

#[tokio::test]
async fn pending_codex_launch_binds_by_process_evidence_and_finished_receipts_archive() {
    let Some(host_binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    if !Path::new("/bin/bash").is_file() {
        eprintln!("SKIP: /bin/bash is required for the argv0 re-exec of the fake CLI");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (lifecycle, host, work, web, bin, codex_root, audit) = (
        root.join("lifecycle"),
        root.join("hosts"),
        root.join("work"),
        root.join("web"),
        root.join("bin"),
        root.join("codex"),
        root.join("audit"),
    );
    let codex_area = work.join("codex-area");
    for path in [
        &lifecycle,
        &host,
        &codex_area,
        &web,
        &bin,
        &codex_root,
        &audit,
    ] {
        directory(path);
    }
    file(
        &web.join("index.html"),
        b"<!doctype html><meta name=\"sessiondock-mode\" content=\"local\"><title>synthetic</title>",
        0o600,
    );
    file(&bin.join("fake-codex"), FAKE_CODEX.as_bytes(), 0o700);
    // Two rollouts in the launch cwd: the held one is the CLI's, the decoy is
    // an unrelated session in the same directory (Python would have had to
    // disambiguate it by process too; Rust never uses cwd at all).
    let held = codex_root.join(format!("rollout-{HELD_SID}.jsonl"));
    let decoy = codex_root.join(format!("rollout-{DECOY_SID}.jsonl"));
    rollout(&held, HELD_SID, &codex_area);
    rollout(&decoy, DECOY_SID, &codex_area);
    let launcher = root.join("launcher.json");
    let config = json!({"schema":2,"host_binary":host_binary,"host_dir":host,
    "adapters":[],
    "profiles":[{"id":"codex-cli-v1","source":"codex","executable":bin.join("fake-codex"),
        "args":[],"new_args":[],"resume_args":["resume","{sid}"],
        "env":{"PATH":"/usr/bin:/bin","HOME":root.join("home"),"SESSIONDOCK_TEST_ROLLOUT":held},
        }]});
    file(&launcher, config.to_string().as_bytes(), 0o600);
    drop(LifecycleStore::initialize(&lifecycle).unwrap());
    let shutdown = CancellationToken::new();
    let prepared = prepare_app(
        Config {
            web_dir: web.clone(),
            ptyhost_dir: Some(host.clone()),
            lifecycle_dir: Some(lifecycle.clone()),
            launcher_config: Some(launcher.clone()),
            audit_dir: Some(audit.clone()),
            roots: SessionRoots {
                codex: Some(codex_root.clone()),
                ..Default::default()
            },
            ..Default::default()
        },
        shutdown.clone(),
    )
    .await
    .unwrap();
    let router = prepared.router.clone();
    let service = prepared.lifecycle.clone().unwrap();

    let (status, receipt) = request(
        &router,
        Method::POST,
        "/api/term/create",
        Some(json!({"source":"codex","cwd":codex_area,"request_id":"autobind-codex-request"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["launch_kind"], "new_pending");
    assert_eq!(receipt["native_binding"], "unbound");
    assert!(receipt["started"].is_u64(), "{receipt}");
    assert_eq!(receipt["discarded"], false);
    let record_id = receipt["record_id"].as_str().unwrap().to_owned();
    let instance_id = receipt["instance_id"].as_str().unwrap().to_owned();

    // The background task ticks every 2 s; poll `term/list` until the row
    // reports the confirmed process binding (bounded).
    let deadline = Instant::now() + Duration::from_secs(20);
    let bound = loop {
        let (status, list) = request(&router, Method::GET, "/api/term/list", None).await;
        assert_eq!(status, StatusCode::OK, "{list}");
        let row = list["pending"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["record_id"] == record_id)
            .cloned()
            .expect("pending row listed");
        if row["binding"]["state"] == "confirmed" {
            break (row, list);
        }
        assert!(Instant::now() < deadline, "no automatic binding: {row}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    let (row, list) = bound;
    let expected_uid = {
        use sha1::Digest;
        let digest = sha1::Sha1::digest(held.to_str().unwrap().as_bytes());
        format!(
            "codex:{}",
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
                .get(..16)
                .unwrap()
        )
    };
    assert_eq!(row["binding"]["method"], "process", "{row}");
    assert_eq!(row["binding"]["uid"], expected_uid, "{row}");
    assert_eq!(row["binding"]["sid"], HELD_SID);
    assert!(row["binding"]["bound_at"].is_u64(), "{row}");
    let evidence = row["binding"]["evidence"].as_str().unwrap();
    assert!(
        evidence.contains("cli_pids=[") && evidence.contains("under host child pid"),
        "{evidence}"
    );
    assert!(
        !evidence.contains(DECOY_SID),
        "the unheld decoy is not evidence: {evidence}"
    );
    // The bound host now appears as the session's console row (`term/list`
    // observes hosts before it refreshes receipts, so the first list that
    // shows the confirmed receipt may predate the host's binding by a tick).
    drop(list);
    let deadline = Instant::now() + Duration::from_secs(10);
    let session_row = loop {
        let (_, list) = request(&router, Method::GET, "/api/term/list", None).await;
        if let Some(row) = list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|session| session["uid"] == expected_uid)
        {
            break row.clone();
        }
        assert!(
            Instant::now() < deadline,
            "bound host never listed as the session's console: {list}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    assert_eq!(session_row["instance_id"], instance_id);
    assert_eq!(session_row["name"], receipt["name"]);
    // Audit trail names the evidence.
    let mut audited = false;
    for _ in 0..40 {
        let rows: String = fs::read_dir(&audit)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
            .map(|entry| fs::read_to_string(entry.path()).unwrap_or_default())
            .collect();
        if rows.contains("lifecycle.autobind") && rows.contains("cli_pids=[") {
            audited = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(audited, "lifecycle.autobind audit row");
    // A running receipt cannot be discarded.
    let (status, refused) = request(
        &router,
        Method::POST,
        "/api/term/discard",
        Some(json!({"record_id":record_id,"instance_id":instance_id})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "launch_not_finished");

    // Stop it through the bound session, then discard the finished receipt.
    let (status, stopped) = request(
        &router,
        Method::POST,
        "/api/session/stop",
        Some(json!({"uid":expected_uid,"request_id":"autobind-stop"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["stopped"], true, "{stopped}");
    let (status, status_row) = request(
        &router,
        Method::GET,
        &format!("/api/term/new-status?record_id={record_id}&instance_id={instance_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(status_row["state"], "exited", "{status_row}");
    assert!(status_row["finished_at"].is_u64(), "{status_row}");
    assert_eq!(status_row["discardable"], true);
    let (_, list) = request(&router, Method::GET, "/api/term/list", None).await;
    assert!(
        list["pending"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["record_id"] == record_id),
        "a just-finished receipt stays listed for the archive window"
    );
    let (status, discarded) = request(
        &router,
        Method::POST,
        "/api/term/discard",
        Some(json!({"record_id":record_id,"instance_id":instance_id})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{discarded}");
    assert_eq!(discarded["discarded"], true);
    let (_, list) = request(&router, Method::GET, "/api/term/list", None).await;
    assert!(
        !list["pending"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["record_id"] == record_id),
        "discarded receipts leave the pending list: {list}"
    );
    // Still queryable, and idempotent.
    let (status, again) = request(
        &router,
        Method::POST,
        "/api/term/discard",
        Some(json!({"record_id":record_id,"instance_id":instance_id})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["discarded"], true);
    let (status, wrong) = request(
        &router,
        Method::POST,
        "/api/term/discard",
        Some(json!({"record_id":record_id,"instance_id":"0".repeat(32)})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{wrong}");
    shutdown.cancel();
    service.shutdown().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(6);
    while fs::read_dir(&host).unwrap().count() > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        fs::read_dir(&host).unwrap().count(),
        0,
        "host records cleaned"
    );
}
