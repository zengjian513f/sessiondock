//! Three-state `/api/live` through the HTTP router. Loopback fake hosts name a
//! real test-owned `sleep` child so Linux process identity can be verified;
//! nothing scans processes or touches native homes. The opt-in test at the end
//! runs one isolated real ptyhost with a fixed free shell.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sessiondock::{app_with_shutdown, config::Config, sessions::SessionRoots};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const FIXTURE: &[u8] = include_bytes!("fixtures/codex/2026/09/11/rollout-synthetic-codex.jsonl");

struct Sleeper(Child);
impl Sleeper {
    fn spawn() -> Self {
        Self(
            Command::new("sleep")
                .arg("60")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
    fn pid(&self) -> u32 {
        self.0.id()
    }
    fn end(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Drop for Sleeper {
    fn drop(&mut self) {
        self.end();
    }
}

struct FakeHost {
    exited: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl FakeHost {
    async fn new(directory: &Path, name: &str, meta: Value, pid: u32) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let record = json!({"name":name,"host_pid":std::process::id(),"pid":pid,"created":created,
            "cols":80,"rows":24,"port":listener.local_addr().unwrap().port(),"token":"PRIVATE_HOST_TOKEN",
            "meta":meta,"argv":["PRIVATE_ARGV"],"cwd":"/synthetic","cmd":"synthetic"});
        tokio::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .await
        .unwrap();
        let exited = Arc::new(AtomicBool::new(false));
        let task = {
            let exited = exited.clone();
            tokio::spawn(async move {
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let (record, exited) = (record.clone(), exited.clone());
                    tokio::spawn(async move {
                        let mut bytes = Vec::new();
                        loop {
                            let Ok(byte) = stream.read_u8().await else {
                                return;
                            };
                            if byte == b'\n' {
                                break;
                            }
                            bytes.push(byte);
                            assert!(bytes.len() < 8192);
                        }
                        let request: Value = serde_json::from_slice(&bytes).unwrap();
                        assert_eq!(request["op"], "info");
                        let mut reply = serde_json::to_vec(&json!({"ok":true,"info":record,
                            "exited":exited.load(Ordering::SeqCst),"capabilities":{"instance_guard":1}})).unwrap();
                        reply.push(b'\n');
                        let _ = stream.write_all(&reply).await;
                    });
                }
            })
        };
        Self { exited, task }
    }
}

impl Drop for FakeHost {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn native_fixture() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("rollout-synthetic-codex.jsonl"),
        FIXTURE,
    )
    .unwrap();
    // A second, unrelated native session with no managed instance at all.
    let other = String::from_utf8(FIXTURE.to_vec())
        .unwrap()
        .replace("synthetic-codex", "synthetic-codex-other");
    std::fs::write(
        directory.path().join("rollout-synthetic-codex-other.jsonl"),
        other,
    )
    .unwrap();
    directory
}

fn config(native: PathBuf, host: PathBuf) -> Config {
    Config {
        web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
        roots: SessionRoots {
            claude: None,
            codex: Some(native),
            grok: None,
        },
        ptyhost_dir: Some(host),
        ..Default::default()
    }
}

async fn get(app: &Router, path: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .uri(path)
        .header("host", "127.0.0.1")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .map(|v| v.to_str().unwrap()),
        Some("no-store")
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

async fn uids(app: &Router) -> (String, String) {
    let (_, listing) = get(app, "/api/sessions").await;
    let mut managed = None;
    let mut other = None;
    for row in listing["sessions"].as_array().unwrap() {
        match row["sid"].as_str().unwrap() {
            "synthetic-codex" => managed = Some(row["uid"].as_str().unwrap().to_owned()),
            "synthetic-codex-other" => other = Some(row["uid"].as_str().unwrap().to_owned()),
            sid => panic!("unexpected fixture session {sid}"),
        }
    }
    (managed.unwrap(), other.unwrap())
}

fn legacy_envelope(body: &Value) {
    for key in [
        "enabled",
        "known",
        "partial",
        "unavailable_reason",
        "uids",
        "tmux_uids",
        "started_at",
        "managed",
    ] {
        assert!(body.get(key).is_some(), "{key}");
    }
    assert_eq!(body["partial"], true);
    assert_eq!(body["tmux_uids"], json!([]));
    assert!(body["uids"].is_array() && body["started_at"].is_object());
    let encoded = body.to_string();
    for private in [
        "PRIVATE_HOST_TOKEN",
        "PRIVATE_ARGV",
        "\"port\"",
        "\"token\"",
    ] {
        assert!(!encoded.contains(private), "{private}");
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn live_reports_running_exited_and_unknown_per_uid_without_stopping_unrelated_sessions() {
    let native = native_fixture();
    let hosts = tempfile::tempdir().unwrap();
    let mut child = Sleeper::spawn();
    let host = FakeHost::new(
        hosts.path(),
        "synthetic",
        json!({"source":"codex","sid":"synthetic-codex","instance_id":"synthetic-instance-0001"}),
        child.pid(),
    )
    .await;
    let app = app_with_shutdown(
        config(native.path().into(), hosts.path().into()),
        CancellationToken::new(),
    )
    .unwrap();
    let (managed_uid, other_uid) = uids(&app).await;

    // Running: reachable host, child alive, identity verified.
    let (status, body) = get(&app, "/api/live").await;
    assert_eq!(status, StatusCode::OK);
    legacy_envelope(&body);
    assert_eq!(body["enabled"], true);
    assert_eq!(body["known"], true);
    assert_eq!(body["uids"], json!([managed_uid]));
    assert!(
        body["started_at"][&managed_uid]
            .as_f64()
            .is_some_and(|at| at > 1.0e9)
    );
    let managed = &body["managed"];
    assert_eq!(managed["known"], true);
    assert_eq!(managed["partial"], true);
    assert_eq!(managed["external_detection"], "not_implemented");
    assert_eq!(managed["process_identity"], "linux_proc");
    assert_eq!(managed["cache"]["hit"], false);
    let session = &managed["sessions"][&managed_uid];
    assert_eq!(session["state"], "running");
    assert_eq!(session["evidence"], "host_info");
    assert_eq!(session["host"], "synthetic");
    assert_eq!(session["instance_id"], "synthetic-instance-0001");
    assert_eq!(session["pid"], child.pid());
    assert_eq!(session["started_at"], body["started_at"][&managed_uid]);
    assert!(managed["sessions"].get(&other_uid).is_none());
    assert_eq!(
        managed["unlisted"],
        json!({"state":"unknown","reason":"no_instance"})
    );
    let observed = &managed["hosts"][0];
    assert_eq!(observed["process"]["status"], "verified");
    assert_eq!(observed["process"]["child"]["pid"], child.pid());
    assert_eq!(observed["process"]["host"]["pid"], std::process::id());
    assert_eq!(observed["liveness"], "running");

    // Within the TTL the same snapshot is reused; force bypasses it.
    let (_, again) = get(&app, "/api/live").await;
    assert_eq!(again["managed"]["cache"]["hit"], true);
    assert_eq!(
        again["managed"]["observed_at"],
        body["managed"]["observed_at"]
    );
    let (_, forced) = get(&app, "/api/live?force=1").await;
    assert_eq!(forced["managed"]["cache"]["hit"], false);
    assert_eq!(forced["uids"], json!([managed_uid]));

    // Explicit host exit.
    host.exited.store(true, Ordering::SeqCst);
    let (_, exited) = get(&app, "/api/live?force=1").await;
    assert_eq!(exited["uids"], json!([]));
    assert_eq!(exited["started_at"], json!({}));
    let session = &exited["managed"]["sessions"][&managed_uid];
    assert_eq!(session["state"], "exited");
    assert_eq!(session["evidence"], "host_exit");
    assert_eq!(exited["managed"]["hosts"][0]["process"]["status"], "reaped");
    assert!(exited["managed"]["sessions"].get(&other_uid).is_none());

    // The host claims running again but the identified child is gone: the
    // claim is not enough, and unrelated sessions still are not listed.
    host.exited.store(false, Ordering::SeqCst);
    child.end();
    let (_, gone) = get(&app, "/api/live?force=1").await;
    assert_eq!(gone["uids"], json!([]));
    let session = &gone["managed"]["sessions"][&managed_uid];
    assert_eq!(session["state"], "unknown");
    assert_eq!(session["reason"], "identity_unverifiable");
    assert_eq!(session["identity"], "not_visible");
    assert_eq!(gone["managed"]["hosts"][0]["liveness"], "running");
    assert_eq!(
        gone["managed"]["hosts"][0]["process"]["status"],
        "unverifiable"
    );
    assert!(gone["managed"]["sessions"].get(&other_uid).is_none());

    // Record removed (host cleanup): the remembered identity proves that
    // exact instance ended; the empty inventory stops nothing else.
    drop(host);
    tokio::fs::remove_file(hosts.path().join("synthetic.json"))
        .await
        .unwrap();
    let (_, ended) = get(&app, "/api/live?force=1").await;
    assert_eq!(ended["known"], true);
    assert_eq!(ended["managed"]["hosts"], json!([]));
    let session = &ended["managed"]["sessions"][&managed_uid];
    assert_eq!(session["state"], "exited");
    assert_eq!(session["evidence"], "identity_gone");
    assert_eq!(session["pid"], child.pid());
    assert!(ended["managed"]["sessions"].get(&other_uid).is_none());
    assert_eq!(ended["uids"], json!([]));
}

#[tokio::test]
async fn live_without_host_directory_keeps_the_unknown_legacy_envelope() {
    let native = native_fixture();
    let app = app_with_shutdown(
        Config {
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            roots: SessionRoots {
                claude: None,
                codex: Some(native.path().into()),
                grok: None,
            },
            ..Default::default()
        },
        CancellationToken::new(),
    )
    .unwrap();
    let (status, body) = get(&app, "/api/live?force=1").await;
    assert_eq!(status, StatusCode::OK);
    legacy_envelope(&body);
    assert_eq!(body["enabled"], false);
    assert_eq!(body["known"], false);
    assert_eq!(body["uids"], json!([]));
    assert!(body["managed"].is_null());
}

/// Opt-in: one isolated real ptyhost running a fixed free shell, in a private
/// temporary directory. The shell is asked to exit through the host; the test
/// never signals or scans processes.
#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires SESSIONDOCK_TEST_PTYHOST_BINARY as explicit absolute built host path; runs only a free /bin/sh"]
async fn isolated_real_host_transitions_from_running_to_exited() {
    struct Reap(Child);
    impl Drop for Reap {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let binary = PathBuf::from(
        std::env::var_os("SESSIONDOCK_TEST_PTYHOST_BINARY")
            .expect("set explicit SESSIONDOCK_TEST_PTYHOST_BINARY"),
    );
    assert!(binary.is_absolute() && binary.is_file());
    let native = native_fixture();
    let hosts = tempfile::tempdir().unwrap();
    let meta = json!({"source":"codex","sid":"synthetic-codex","instance_id":"synthetic-shell-instance-0001"});
    let mut host = Reap(
        Command::new(&binary)
            .arg("--dir")
            .arg(hosts.path())
            .args(["run", "--name", "synthetic-shell", "--cwd"])
            .arg(hosts.path())
            .arg("--meta")
            .arg(meta.to_string())
            .args([
                "--",
                "/bin/sh",
                "-c",
                "while IFS= read -r line; do case \"$line\" in quit) exit 0 ;; esac; done",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let app = app_with_shutdown(
        config(native.path().into(), hosts.path().into()),
        CancellationToken::new(),
    )
    .unwrap();
    let (managed_uid, other_uid) = uids(&app).await;
    let running = timeout(Duration::from_secs(10), async {
        loop {
            let (_, body) = get(&app, "/api/live?force=1").await;
            if body["uids"] == json!([managed_uid]) {
                break body;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    let session = &running["managed"]["sessions"][&managed_uid];
    assert_eq!(session["state"], "running");
    assert_eq!(session["instance_id"], "synthetic-shell-instance-0001");
    let record: Value =
        serde_json::from_slice(&std::fs::read(hosts.path().join("synthetic-shell.json")).unwrap())
            .unwrap();
    assert_eq!(session["pid"], record["pid"]);
    assert_eq!(
        running["managed"]["hosts"][0]["process"]["host"]["pid"],
        host.0.id()
    );
    assert!(running["managed"]["sessions"].get(&other_uid).is_none());

    let client =
        ptyhost_client::HostClient::new(hosts.path(), ptyhost_client::Limits::default()).unwrap();
    client
        .request(
            "synthetic-shell",
            ptyhost_client::ControlOp::Send {
                text: "quit\n".into(),
            },
        )
        .await
        .unwrap();
    let status = timeout(Duration::from_secs(10), async {
        loop {
            if let Some(status) = host.0.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert!(status.success());
    assert!(!hosts.path().join("synthetic-shell.json").exists());
    let (_, ended) = get(&app, "/api/live?force=1").await;
    assert_eq!(ended["uids"], json!([]));
    let session = &ended["managed"]["sessions"][&managed_uid];
    assert_eq!(session["state"], "exited");
    assert_eq!(session["evidence"], "identity_gone");
    assert_eq!(session["pid"], record["pid"]);
    assert!(ended["managed"]["sessions"].get(&other_uid).is_none());
}
