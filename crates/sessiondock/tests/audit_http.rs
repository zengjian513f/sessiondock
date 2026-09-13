//! `POST /api/audit/browser` over the real router: unconfigured 501, Python's
//! protocol limits, JSONL intake, queue saturation and graceful shutdown.
//! Only a temporary directory is ever written.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use sessiondock::{
    audit::{Gate, Limits},
    config::Config,
    prepare_app,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

fn private_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
    #[cfg(not(unix))]
    fs::create_dir(path).unwrap();
}

struct Fixture {
    _temp: tempfile::TempDir,
    audit: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let audit = temp.path().join("audit");
        private_dir(&audit);
        Self { _temp: temp, audit }
    }

    fn config(&self, limits: Limits) -> Config {
        Config {
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            audit_dir: Some(self.audit.clone()),
            audit_limits: limits,
            ..Config::default()
        }
    }

    fn segments(&self) -> Vec<PathBuf> {
        let mut files: Vec<_> = fs::read_dir(&self.audit)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                let name = path.file_name().unwrap().to_string_lossy();
                name.starts_with("browser-") && name.ends_with(".jsonl")
            })
            .collect();
        files.sort();
        files
    }

    fn lines(&self) -> Vec<Value> {
        self.segments()
            .iter()
            .flat_map(|path| {
                fs::read_to_string(path)
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str::<Value>(line).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

fn unconfigured() -> Config {
    Config {
        web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
        ..Config::default()
    }
}

fn quick_limits() -> Limits {
    Limits {
        shutdown_deadline: Duration::from_secs(3),
        ..Limits::default()
    }
}

async fn post_raw(app: &Router, body: Body, content_type: &str, length: Option<usize>) -> Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/audit/browser")
        .header("Host", "127.0.0.1:8741")
        .header("Content-Type", content_type);
    if let Some(length) = length {
        builder = builder.header("Content-Length", length);
    }
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn post(app: &Router, body: &Value) -> Response {
    let text = body.to_string();
    let length = text.len();
    post_raw(app, Body::from(text), "application/json", Some(length)).await
}

async fn get(app: &Router, uri: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("Host", "127.0.0.1:8741")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json(response).await
}

async fn json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

fn batch(names: &[&str]) -> Value {
    json!({"page_id": "page-abc", "uid": "claude:1234", "_build": "test-build", "events":
        names.iter().map(|name| json!({"event": name, "ts": "2026-09-12T10:00:00.000Z",
            "severity": "info", "data": {"n": 1, "cwd": "/private/native/path", "api_key": "x"},
            "content": {"composer": "NEVER-STORED-MESSAGE-BODY"}})).collect::<Vec<_>>()})
}

async fn wait_for(mut condition: impl AsyncFnMut() -> bool) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if condition().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(condition().await, "condition not met within five seconds");
}

#[tokio::test]
async fn unconfigured_route_stays_501_and_health_reports_disabled() {
    let app = sessiondock::app(unconfigured()).unwrap();
    let response = post(&app, &batch(&["page.loaded"])).await;
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(json(response).await["code"], "not_implemented");
    let health = get(&app, "/api/health").await;
    assert_eq!(health["audit"]["enabled"], false);
    assert_eq!(health["audit"]["accepted_events"], 0);
    let meta = get(&app, "/api/meta").await;
    assert_eq!(meta["capabilities"]["audit"], false);
}

#[tokio::test]
async fn configured_intake_writes_bounded_metadata_only_records() {
    let fixture = Fixture::new();
    let app = sessiondock::app(fixture.config(quick_limits())).unwrap();
    assert_eq!(get(&app, "/api/meta").await["capabilities"]["audit"], true);
    assert!(
        fixture.segments().is_empty(),
        "idle service creates no file"
    );
    let response = post(&app, &batch(&["page.loaded", "session.opened"])).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(
        json(response).await,
        json!({"ok": true, "accepted": 2, "skipped": 0, "dropped": false})
    );
    // Beacon-style bodies (Blob with a JSON type, or a text/plain fallback)
    // use the same contract, and invalid events are skipped, not fatal.
    let beacon = json!({"_page_id": "page-abc", "events": [
        {"event": "page.hidden", "data": {"reason": "pagehide"}},
        {"event": "bad name"}
    ]})
    .to_string();
    let length = beacon.len();
    let response = post_raw(
        &app,
        Body::from(beacon),
        "text/plain;charset=UTF-8",
        Some(length),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        json(response).await,
        json!({"ok": true, "accepted": 1, "skipped": 1, "dropped": false})
    );
    wait_for(async || fixture.lines().len() == 3).await;
    let segments = fixture.segments();
    assert_eq!(segments.len(), 1);
    let name = segments[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with("browser-20") && name.ends_with(".jsonl"),
        "{name}"
    );
    let raw = fs::read_to_string(&segments[0]).unwrap();
    assert!(!raw.contains("NEVER-STORED"));
    assert!(raw.contains("/private/native/path"));
    assert!(!raw.contains("\"content\""));
    let rows = fixture.lines();
    assert_eq!(rows[0]["event"], "browser.page.loaded");
    assert_eq!(rows[0]["page_id"], "page-abc");
    assert_eq!(rows[0]["uid"], "claude:1234");
    assert_eq!(rows[0]["source"], "claude");
    assert_eq!(rows[0]["build"], "test-build");
    assert_eq!(rows[0]["client"], "127.0.0.1");
    assert_eq!(
        rows[0]["data"],
        json!({"n": 1, "cwd": "/private/native/path", "api_key": "<redacted>"})
    );
    assert_eq!(rows[1]["event"], "browser.session.opened");
    assert_eq!(rows[2]["event"], "browser.page.hidden");
    assert_eq!(rows[2]["page_id"], "page-abc");
    assert!(
        rows.iter()
            .all(|row| row["seq"].is_u64() && row["received_at"].is_string())
    );
    let health = get(&app, "/api/health").await;
    assert_eq!(health["audit"]["enabled"], true);
    assert_eq!(health["audit"]["accepted_events"], 3);
    assert_eq!(health["audit"]["accepted_batches"], 2);
    assert_eq!(health["audit"]["rejected_events"], 1);
    assert_eq!(health["audit"]["written_events"], 3);
    assert_eq!(health["audit"]["dropped_batches"], 0);
    assert!(health["audit"]["written_bytes"].as_u64().unwrap() > 0);
    assert_eq!(
        health["audit"]["written_bytes"],
        health["audit"]["retained_bytes"]
    );
}

#[tokio::test]
async fn malformed_oversize_and_too_many_events_are_rejected_explicitly() {
    let fixture = Fixture::new();
    let limits = Limits {
        body_bytes: 2048,
        max_events: 3,
        ..quick_limits()
    };
    let app = sessiondock::app(fixture.config(limits)).unwrap();
    for (body, code) in [
        ("not json", "invalid_audit_request"),
        ("[]", "invalid_audit_request"),
        ("{}", "invalid_audit_request"),
        (r#"{"events": {}}"#, "invalid_audit_request"),
    ] {
        let response = post_raw(&app, Body::from(body), "application/json", Some(body.len())).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(json(response).await["code"], code, "{body}");
    }
    let response = post(&app, &batch(&["a", "b", "c", "d"])).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json(response).await["code"], "too_many_events");
    // Over the route limit: both with a declared length and as a chunked stream.
    let big = json!({"events": [{"event": "x", "data": {"pad": "p".repeat(4000)}}]}).to_string();
    let response = post_raw(
        &app,
        Body::from(big.clone()),
        "application/json",
        Some(big.len()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json(response).await["code"], "body_too_large");
    let chunks: Vec<Result<axum::body::Bytes, std::io::Error>> = big
        .as_bytes()
        .chunks(1000)
        .map(|chunk| Ok(axum::body::Bytes::copy_from_slice(chunk)))
        .collect();
    let stream = Body::from_stream(futures_util::stream::iter(chunks));
    let response = post_raw(&app, stream, "application/json", None).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json(response).await["code"], "body_too_large");
    // Nothing invalid reaches disk; counters explain what was refused.
    let health = get(&app, "/api/health").await;
    assert_eq!(health["audit"]["rejected_requests"], 7);
    assert_eq!(health["audit"]["accepted_events"], 0);
    assert_eq!(
        health["audit"]["queued_bytes"], 0,
        "reservations were released"
    );
    assert!(fixture.segments().is_empty());
}

#[tokio::test]
async fn queue_saturation_drops_with_counters_while_requests_stay_fast() {
    let fixture = Fixture::new();
    let gate = Arc::new(Gate::default());
    gate.hold();
    let limits = Limits {
        queue_batches: 2,
        gate: Some(gate.clone()),
        ..quick_limits()
    };
    let app = sessiondock::app(fixture.config(limits)).unwrap();
    let started = Instant::now();
    let mut dropped = 0;
    let mut dropped_events = 0;
    for round in 0..8 {
        let response = post(&app, &batch(&[&format!("held.{round}")])).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = json(response).await;
        assert_eq!(body["ok"], true);
        if body["dropped"] == true {
            dropped += 1;
            // Python drops only after parsing and therefore knows the event count.
            dropped_events += body["accepted"].as_u64().unwrap();
        } else {
            assert_eq!(body["accepted"], 1);
        }
    }
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert!((5..=6).contains(&dropped), "{dropped}");
    let health = get(&app, "/api/health").await;
    assert_eq!(health["audit"]["dropped_batches"], dropped);
    assert_eq!(health["audit"]["dropped_events"], dropped_events);
    assert_eq!(health["audit"]["accepted_batches"], 8 - dropped);
    assert_eq!(health["audit"]["written_events"], 0);
    assert!(health["audit"]["queued_batches"].as_u64().unwrap() >= 2);
    gate.release();
    wait_for(async || {
        let health = get(&app, "/api/health").await;
        health["audit"]["written_events"] == health["audit"]["accepted_events"]
            && health["audit"]["queued_batches"] == 0
    })
    .await;
    let rows = fixture.lines();
    assert_eq!(rows.len() as u64, 8 - dropped);
    assert_eq!(rows[0]["event"], "browser.held.0");
    let health = get(&app, "/api/health").await;
    assert_eq!(health["audit"]["queued_bytes"], 0);
    assert_eq!(health["audit"]["dropped_events"], dropped_events);
    // Once drained, the queue accepts again.
    let response = post(&app, &batch(&["after.drain"])).await;
    assert_eq!(json(response).await["dropped"], false);
}

#[tokio::test]
async fn graceful_shutdown_flushes_queued_batches_and_closes_admission() {
    let fixture = Fixture::new();
    let gate = Arc::new(Gate::default());
    gate.hold();
    let limits = Limits {
        gate: Some(gate.clone()),
        ..quick_limits()
    };
    let stop = CancellationToken::new();
    let prepared = prepare_app(fixture.config(limits), stop.clone())
        .await
        .unwrap();
    assert!(prepared.delivery.is_none() && prepared.lifecycle.is_none());
    let audit = prepared.audit.clone().expect("configured audit service");
    let app = prepared.router;
    for round in 0..4 {
        let response = post(&app, &batch(&[&format!("flush.{round}"), "second"])).await;
        assert_eq!(json(response).await["dropped"], false);
    }
    assert_eq!(audit.snapshot().written_events, 0);
    stop.cancel();
    let response = post(&app, &batch(&["late"])).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json(response).await["code"], "shutdown");
    gate.release();
    let started = Instant::now();
    assert!(audit.shutdown().await, "drained within the deadline");
    assert!(started.elapsed() < Duration::from_secs(3));
    let snapshot = audit.snapshot();
    assert_eq!(snapshot.written_events, 8);
    assert_eq!(snapshot.dropped_events, 0);
    assert_eq!(snapshot.queued_batches, 0);
    let rows = fixture.lines();
    assert_eq!(rows.len(), 8);
    assert_eq!(rows[7]["event"], "browser.second");
    assert_eq!(rows[6]["event"], "browser.flush.3");
}
