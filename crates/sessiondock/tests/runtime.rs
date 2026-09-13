//! Only synthetic native files and loopback fake hosts; no process/home scan.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
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
    sync::Mutex,
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

struct FakeHost {
    directory: TempDir,
    record: Arc<Mutex<Value>>,
    silent: Arc<AtomicBool>,
    accepted: Arc<AtomicUsize>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl FakeHost {
    async fn new(meta: Value) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let value = json!({"name":"synthetic","host_pid":41,"pid":42,"created":10,"cols":80,"rows":24,
            "port":listener.local_addr().unwrap().port(),"token":"PRIVATE_HOST_TOKEN","meta":meta,
            "argv":["PRIVATE_ARGV"],"cwd":"/synthetic","cmd":"synthetic"});
        tokio::fs::write(
            directory.path().join("synthetic.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .await
        .unwrap();
        let record = Arc::new(Mutex::new(value));
        let silent = Arc::new(AtomicBool::new(false));
        let accepted = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let task = {
            let (record, silent, accepted, cancel) = (
                record.clone(),
                silent.clone(),
                accepted.clone(),
                cancel.clone(),
            );
            tokio::spawn(async move {
                let mut jobs = JoinSet::new();
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        result = listener.accept() => {
                            let (mut stream, _) = result.unwrap();
                            let (record, silent, accepted) = (record.clone(), silent.clone(), accepted.clone());
                            jobs.spawn(async move {
                                let mut bytes = Vec::new();
                                loop { let Ok(byte) = stream.read_u8().await else { return }; if byte == b'\n' { break } bytes.push(byte); assert!(bytes.len() < 8192); }
                                let request: Value = serde_json::from_slice(&bytes).unwrap();
                                assert_eq!(request["op"], "info"); assert_eq!(request["token"], "PRIVATE_HOST_TOKEN");
                                accepted.fetch_add(1, Ordering::SeqCst);
                                if silent.load(Ordering::SeqCst) { std::future::pending::<()>().await; }
                                let mut bytes = serde_json::to_vec(&json!({"ok":true,"info":*record.lock().await,"exited":false})).unwrap(); bytes.push(b'\n');
                                let _ = stream.write_all(&bytes).await;
                            });
                        },
                        _ = jobs.join_next(), if !jobs.is_empty() => {},
                    }
                }
                jobs.abort_all();
                while jobs.join_next().await.is_some() {}
            })
        };
        Self {
            directory,
            record,
            silent,
            accepted,
            cancel,
            task,
        }
    }

    async fn replace_instance(&self) {
        let mut record = self.record.lock().await;
        record["created"] = json!(11);
        record["pid"] = json!(43);
        record["meta"]["instance_id"] = json!("synthetic-instance-0002");
        tokio::fs::write(
            self.directory.path().join("synthetic.json"),
            serde_json::to_vec(&*record).unwrap(),
        )
        .await
        .unwrap();
    }
}

impl Drop for FakeHost {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

fn config(native: Option<PathBuf>, host: Option<PathBuf>) -> Config {
    Config {
        web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
        roots: SessionRoots {
            claude: None,
            codex: native,
            grok: None,
        },
        ptyhost_dir: host,
        // The admission test asserts exactly two probe permits and an immediate
        // refusal; batch 44 WP-A scaled read_workers with the host's cores and
        // made the pool wait 10 s. Pin both (read_workers/2 clamps to 2).
        pools: sessiondock::config::Pools {
            read_workers: 4,
            wait: std::time::Duration::ZERO,
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn get(app: &Router, path: &str) -> (StatusCode, Value, bool) {
    let request = Request::builder()
        .uri(path)
        .header("host", "127.0.0.1")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let no_store = response
        .headers()
        .get("cache-control")
        .is_some_and(|value| value == "no-store");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap(), no_store)
}

fn native_fixture() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("rollout-synthetic-codex.jsonl"),
        include_bytes!("fixtures/codex/2026/09/11/rollout-synthetic-codex.jsonl"),
    )
    .unwrap();
    directory
}

#[tokio::test]
async fn missing_explicit_host_directory_keeps_legacy_live_unknown() {
    let app = app_with_shutdown(config(None, None), CancellationToken::new()).unwrap();
    let (status, body, no_store) = get(&app, "/api/live").await;
    assert_eq!(status, StatusCode::OK);
    assert!(no_store);
    assert_eq!(body["enabled"], false);
    assert_eq!(body["known"], false);
    assert_eq!(body["partial"], true);
    assert!(body["managed"].is_null());
    assert_eq!(body["uids"], json!([]));
    assert_eq!(body["tmux_uids"], json!([]));
    assert_eq!(body["started_at"], json!({}));
}

#[tokio::test]
async fn exact_association_is_visible_only_inside_partial_managed_snapshot() {
    let native = native_fixture();
    let host = FakeHost::new(json!({"source":"codex","sid":"synthetic-codex","instance_id":"synthetic-instance-0001","secret":"PRIVATE_META"})).await;
    let app = app_with_shutdown(
        config(
            Some(native.path().into()),
            Some(host.directory.path().into()),
        ),
        CancellationToken::new(),
    )
    .unwrap();
    let (_, listing, _) = get(&app, "/api/sessions").await;
    let uid = listing["sessions"][0]["uid"].clone();
    let (status, body, no_store) = get(&app, "/api/live").await;
    assert_eq!(status, StatusCode::OK);
    assert!(no_store);
    // The managed inventory is known and partial; a synthetic PID can never
    // be verified, so the legacy running list stays empty.
    assert_eq!(body["known"], true);
    assert_eq!(body["enabled"], true);
    assert_eq!(body["partial"], true);
    assert_eq!(body["uids"], json!([]));
    assert_eq!(
        body["managed"]["sessions"][uid.as_str().unwrap()]["state"],
        "unknown"
    );
    assert_eq!(body["managed"]["unlisted"]["state"], "unknown");
    let observed = &body["managed"]["hosts"][0];
    assert_eq!(observed["association"]["status"], "matched");
    assert_eq!(observed["association"]["uid"], uid);
    assert_eq!(observed["known"], true);
    assert_eq!(observed["liveness"], "running");
    assert_eq!(observed["identity_unverified"], false);
    let encoded = body.to_string();
    for secret in [
        "PRIVATE_HOST_TOKEN",
        "PRIVATE_META",
        "PRIVATE_ARGV",
        "\"port\"",
        "\"token\"",
    ] {
        assert!(!encoded.contains(secret));
    }
    host.replace_instance().await;
    let (_, next, _) = get(&app, "/api/live?force=1").await;
    assert_eq!(
        next["managed"]["hosts"][0]["instance_id"],
        "synthetic-instance-0002"
    );
    assert_eq!(
        body["managed"]["hosts"][0]["instance_id"],
        "synthetic-instance-0001"
    );
    let (_, term, _) = get(&app, "/api/term/list").await;
    assert_eq!(term["enabled"], false);
    assert_eq!(term["sessions"], json!([]));
}

#[tokio::test]
async fn empty_metadata_is_running_but_never_associated_by_name() {
    let native = native_fixture();
    let host = FakeHost::new(json!({})).await;
    let app = app_with_shutdown(
        config(
            Some(native.path().into()),
            Some(host.directory.path().into()),
        ),
        CancellationToken::new(),
    )
    .unwrap();
    let (status, body, _) = get(&app, "/api/live").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["managed"]["hosts"][0]["association"]["reason"],
        "missing_metadata"
    );
    assert_eq!(body["managed"]["hosts"][0]["identity_unverified"], true);
    assert_eq!(body["managed"]["hosts"][0]["liveness"], "running");
}

#[tokio::test]
async fn observation_admission_is_bounded_and_shutdown_cancels_active_probes() {
    let host = FakeHost::new(json!({})).await;
    host.silent.store(true, Ordering::SeqCst);
    let cancel = CancellationToken::new();
    let app = app_with_shutdown(
        config(None, Some(host.directory.path().into())),
        cancel.clone(),
    )
    .unwrap();
    // Display polls share one in-flight probe; only one Info request reaches
    // the host however many pages poll.
    let first = {
        let app = app.clone();
        tokio::spawn(async move { get(&app, "/api/live").await })
    };
    let second = {
        let app = app.clone();
        tokio::spawn(async move { get(&app, "/api/live").await })
    };
    timeout(Duration::from_secs(1), async {
        while host.accepted.load(Ordering::SeqCst) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(host.accepted.load(Ordering::SeqCst), 1);
    // A fresh (claim-grade) observation takes the second admission permit;
    // the next fresh observer is refused explicitly rather than queued.
    let third = {
        let app = app.clone();
        tokio::spawn(async move { get(&app, "/api/term/list").await })
    };
    timeout(Duration::from_secs(1), async {
        while host.accepted.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (status, body, _) = get(&app, "/api/term/list").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "runtime_busy");
    cancel.cancel();
    for request in [first, second, third] {
        let (status, body, _) = timeout(Duration::from_secs(1), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "shutdown");
    }
}

/// Opt-in smoke test of the imported host's real immutable `--meta` protocol.
/// The only child is a fixed free shell loop, in a fresh explicit temp directory.
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires cargo build -p ptyhost; runs one isolated free POSIX shell"]
async fn isolated_free_shell_metadata_and_web_restart_preserve_instance() {
    use ptyhost_client::{ControlOp, HostClient, Limits};
    use std::process::{Child, Command, Stdio};

    struct Reap(Child);
    impl Drop for Reap {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let host_binary = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/ptyhost");
    assert!(
        host_binary.is_file(),
        "build the local ptyhost target first"
    );
    let directory = tempfile::tempdir().unwrap();
    let native = native_fixture();
    let meta = json!({"source":"codex","sid":"synthetic-codex","instance_id":"synthetic-shell-instance-0001","private":"PRIVATE_META"});
    let mut host = Reap(
        Command::new(host_binary)
            .arg("--dir")
            .arg(directory.path())
            .args(["run", "--name", "synthetic-shell", "--cwd"])
            .arg(directory.path())
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
    let client = HostClient::new(directory.path(), Limits::default()).unwrap();
    let initial = timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(info) = client.probe("synthetic-shell").await {
                break info;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(!initial.exited);
    assert_eq!(
        initial.instance_id.as_deref(),
        Some("synthetic-shell-instance-0001")
    );
    for _ in 0..2 {
        let app = app_with_shutdown(
            config(Some(native.path().into()), Some(directory.path().into())),
            CancellationToken::new(),
        )
        .unwrap();
        let (status, body, _) = get(&app, "/api/live").await;
        assert_eq!(status, StatusCode::OK);
        let observed = &body["managed"]["hosts"][0];
        assert_eq!(observed["association"]["status"], "matched");
        assert_eq!(observed["instance_id"], "synthetic-shell-instance-0001");
        assert_eq!(observed["summary"]["pid"], initial.summary.pid);
        assert!(!body.to_string().contains("PRIVATE_META"));
        drop(app);
    }
    client
        .request(
            "synthetic-shell",
            ControlOp::Send {
                text: "quit\n".into(),
            },
        )
        .await
        .unwrap();
    let status = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(status) = host.0.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(status.success());
}
