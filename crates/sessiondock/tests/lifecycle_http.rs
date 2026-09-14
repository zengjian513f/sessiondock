//! In-process startup/HTTP checks using preexisting synthetic receipts only.
//! Fixture executable files are inert data, never started. No native homes or CLI.
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
    app,
    config::Config,
    lifecycle::{
        model::{LaunchSpec, Record, Source},
        store::{self, LifecycleStore},
    },
    prepare_app,
    runtime::AssociationReason,
    sessions::{SessionRoots, SessionStore},
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

// Isolate process-wide startup fixtures from one another. Response-queue tests
// still hold several HTTP bodies concurrently within their own test.
static TEST_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

const REQUEST: &str = "synthetic-existing-http-request";
const ADAPTER: &str = "synthetic-codex-v1";

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    lifecycle: PathBuf,
    host: PathBuf,
    cwd: PathBuf,
    web: PathBuf,
    launcher: PathBuf,
}

fn directory(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn file(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
impl Fixture {
    fn new(initialize: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let lifecycle = root.join("lifecycle");
        let host = root.join("hosts");
        let cwd = root.join("work");
        let web = root.join("web");
        let binaries = root.join("executables");
        for path in [&lifecycle, &host, &cwd, &web, &binaries] {
            directory(path);
        }
        file(&web.join("index.html"),b"<!doctype html><meta name=\"sessiondock-mode\" content=\"local\"><title>synthetic</title>",0o600);
        // Not a shell script or runnable binary. These satisfy checked-file
        // allowlist validation but this suite never grants a new spawn authority.
        let host_binary = binaries.join("fake-host");
        let executable = binaries.join("fake-adapter");
        file(&host_binary, b"INERT_SYNTHETIC_HOST_FIXTURE\n", 0o700);
        file(&executable, b"INERT_SYNTHETIC_ADAPTER_FIXTURE\n", 0o700);
        let launcher = root.join("launcher.json");
        let config = json!({"host_binary":host_binary,"host_dir":host,"adapters":[{
            "id":ADAPTER,"source":"codex","executable":executable,"args":["SYNTHETIC_PRIVATE_ARG"],
            "env":{"SYNTHETIC_PRIVATE_ENV":"never-public"}}]});
        file(&launcher, config.to_string().as_bytes(), 0o600);
        if initialize {
            drop(LifecycleStore::initialize(&lifecycle).unwrap());
        }
        Self {
            _temp: temp,
            root,
            lifecycle,
            host,
            cwd,
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
            ..Default::default()
        }
    }
    fn seed(&self) -> Record {
        let mut store = LifecycleStore::open(&self.lifecycle).unwrap();
        let spec = LaunchSpec::new(Source::Codex, ADAPTER.into(), &self.cwd).unwrap();
        let created = store.create(REQUEST, &spec).unwrap();
        assert!(created.prepared.is_some());
        // Drop authority without beginning start. Reopen/replay cannot mint it.
        created.record
    }
    fn create_body(&self) -> Value {
        json!({"source":"codex","cwd":self.cwd,"request_id":REQUEST})
    }
    fn assert_no_host_records(&self) {
        assert_eq!(fs::read_dir(&self.host).unwrap().count(), 0);
    }
}

async fn request(router: &Router, method: Method, uri: &str, body: Value) -> Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("host", "localhost")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn body(response: Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}
fn status_uri(record: &Record) -> String {
    format!(
        "/api/term/new-status?record_id={}&instance_id={}",
        record.record_id(),
        record.instance_id()
    )
}
fn bind_body(record: &Record, uid: &str) -> Value {
    json!({"record_id":record.record_id(),"instance_id":record.instance_id(),
        "uid":uid,"operator_confirmed":true})
}

#[tokio::test]
async fn absent_lifecycle_configuration_keeps_create_status_cancel_and_bind_disabled() {
    let _test = TEST_GATE.acquire().await.unwrap();
    let fixture = Fixture::new(false);
    let router = app(Config {
        web_dir: fixture.web.clone(),
        ..Default::default()
    })
    .unwrap();
    for (method, uri, payload) in [
        (Method::POST, "/api/term/create", fixture.create_body()),
        (Method::GET, "/api/term/new-status", json!(null)),
        (
            Method::POST,
            "/api/term/kill",
            json!({"record_id":"x","instance_id":"y"}),
        ),
        (
            Method::POST,
            "/api/term/bind",
            json!({"record_id":"x","instance_id":"y","uid":"codex:missing","operator_confirmed":true}),
        ),
    ] {
        let response = request(&router, method, uri, payload).await;
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let meta = body(request(&router, Method::GET, "/api/meta", json!(null)).await).await;
    assert_eq!(meta["capabilities"]["terminal_create"], false);
    assert_eq!(meta["capabilities"]["terminal_pending"], false);
    assert_eq!(fs::read_dir(&fixture.lifecycle).unwrap().count(), 0);
    fixture.assert_no_host_records();
}

#[tokio::test]
async fn lifecycle_requires_async_factory_both_configuration_paths_and_explicit_host() {
    let _test = TEST_GATE.acquire().await.unwrap();
    let fixture = Fixture::new(true);
    assert!(app(fixture.config()).is_err());
    for missing in ["lifecycle", "launcher", "host"] {
        let mut config = fixture.config();
        match missing {
            "lifecycle" => config.lifecycle_dir = None,
            "launcher" => config.launcher_config = None,
            _ => config.ptyhost_dir = None,
        };
        assert!(prepare_app(config, CancellationToken::new()).await.is_err());
        drop(LifecycleStore::open(&fixture.lifecycle).unwrap());
    }
    fixture.assert_no_host_records();
}

#[tokio::test]
async fn missing_and_corrupt_ledgers_fail_while_concurrent_readers_are_allowed() {
    let _test = TEST_GATE.acquire().await.unwrap();
    let missing = Fixture::new(false);
    assert!(
        prepare_app(missing.config(), CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(fs::read_dir(&missing.lifecycle).unwrap().count(), 0);
    let fixture = Fixture::new(true);
    let concurrent = LifecycleStore::open(&fixture.lifecycle).unwrap();
    let ledger = fixture.lifecycle.join(store::LEDGER_FILENAME);
    let original = fs::read(&ledger).unwrap();
    let prepared = prepare_app(fixture.config(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(fs::read(&ledger).unwrap(), original);
    drop(concurrent);
    prepared
        .lifecycle
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    let broken = b"{\"schema\":2,\"PRIVATE_CORRUPTION\": ";
    file(&ledger, broken, 0o600);
    assert!(
        prepare_app(fixture.config(), CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(fs::read(&ledger).unwrap(), broken);
    fixture.assert_no_host_records();
}

#[tokio::test]
async fn malformed_or_mismatched_launcher_fails_and_ordinary_permissions_work() {
    let _test = TEST_GATE.acquire().await.unwrap();
    let fixture = Fixture::new(true);
    let original = fs::read(&fixture.launcher).unwrap();
    file(&fixture.launcher, b"{malformed launcher data", 0o600);
    assert!(
        prepare_app(fixture.config(), CancellationToken::new())
            .await
            .is_err()
    );
    drop(LifecycleStore::open(&fixture.lifecycle).unwrap());
    file(&fixture.launcher, &original, 0o644);
    let prepared = prepare_app(fixture.config(), CancellationToken::new())
        .await
        .unwrap();
    prepared
        .lifecycle
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    let mut wrong: Value = serde_json::from_slice(&original).unwrap();
    wrong["host_dir"] = json!(fixture.cwd);
    file(&fixture.launcher, wrong.to_string().as_bytes(), 0o600);
    assert!(
        prepare_app(fixture.config(), CancellationToken::new())
            .await
            .is_err()
    );
    drop(LifecycleStore::open(&fixture.lifecycle).unwrap());
    fixture.assert_no_host_records();
}

#[tokio::test]
async fn asset_failure_and_explicit_shutdown_stop_admission() {
    let _test = TEST_GATE.acquire().await.unwrap();
    let fixture = Fixture::new(true);
    let broken_web = fixture.root.join("empty-web");
    directory(&broken_web);
    let mut config = fixture.config();
    config.web_dir = broken_web;
    assert!(prepare_app(config, CancellationToken::new()).await.is_err());
    drop(LifecycleStore::open(&fixture.lifecycle).unwrap());
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(prepare_app(fixture.config(), cancelled).await.is_err());
    drop(LifecycleStore::open(&fixture.lifecycle).unwrap());
    let record = fixture.seed();
    let stop = CancellationToken::new();
    let prepared = prepare_app(fixture.config(), stop.clone()).await.unwrap();
    drop(LifecycleStore::open(&fixture.lifecycle).unwrap());
    stop.cancel();
    prepared
        .lifecycle
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    drop(LifecycleStore::open(&fixture.lifecycle).unwrap());
    let response = request(
        &prepared.router,
        Method::GET,
        &status_uri(&record),
        json!(null),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    fixture.assert_no_host_records();
}

#[tokio::test]
async fn create_validates_required_fields_without_a_body_quota() {
    let _test = TEST_GATE.acquire().await.unwrap();
    let fixture = Fixture::new(true);
    fixture.seed();
    let before = fs::read(fixture.lifecycle.join(store::LEDGER_FILENAME)).unwrap();
    let prepared = prepare_app(fixture.config(), CancellationToken::new())
        .await
        .unwrap();
    let router = &prepared.router;
    for (field, value) in [
        ("source", json!("unknown")),
        ("cols", json!(0)),
        ("request_id", json!("x".repeat(129))),
    ] {
        let mut payload = fixture.create_body();
        payload[field] = value;
        let response = request(router, Method::POST, "/api/term/create", payload).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "field {field}");
        let error = body(response).await.to_string();
        assert!(!error.contains("PRIVATE_"));
    }
    let mut payload = fixture.create_body();
    payload["rows"] = json!(301);
    assert_eq!(
        request(router, Method::POST, "/api/term/create", payload)
            .await
            .status(),
        StatusCode::OK
    );
    for payload in [
        json!([]),
        json!(null),
        json!({}),
        json!({"source":"codex","cwd":false,"request_id":REQUEST}),
    ] {
        assert_eq!(
            request(router, Method::POST, "/api/term/create", payload)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let response = request(
        router,
        Method::POST,
        "/api/term/create",
        json!({"padding":"x".repeat(5 * 1024 * 1024)}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_ne!(body(response).await["code"], "body_too_large");
    assert_eq!(
        fs::read(fixture.lifecycle.join(store::LEDGER_FILENAME)).unwrap(),
        before
    );
    prepared
        .lifecycle
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    fixture.assert_no_host_records();
}

#[tokio::test]
async fn receipt_replay_status_and_pending_claim_keep_native_identity_and_secrets_out() {
    let _test = TEST_GATE.acquire().await.unwrap();
    let fixture = Fixture::new(true);
    let record = fixture.seed();
    let prepared = prepare_app(fixture.config(), CancellationToken::new())
        .await
        .unwrap();
    let response = request(
        &prepared.router,
        Method::POST,
        "/api/term/create",
        fixture.create_body(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let value = body(response).await;
    assert_eq!(value["record_id"], record.record_id());
    assert_eq!(value["state"], "prepared");
    assert_eq!(value["native_binding"], "unbound");
    assert_eq!(value["running"], false);
    for key in [
        "uid",
        "sid",
        "argv",
        "env",
        "token",
        "host_token",
        "adapter_id",
    ] {
        assert!(value.get(key).is_none());
    }
    assert!(!value.to_string().contains("SYNTHETIC_PRIVATE"));
    assert_eq!(
        body(
            request(
                &prepared.router,
                Method::GET,
                &status_uri(&record),
                json!(null)
            )
            .await
        )
        .await,
        value
    );
    let good_claim = json!({"name":record.host_name(),"page":"synthetic-page","record_id":record.record_id(),
        "launch_id":record.launch_id(),"instance_id":record.instance_id()});
    for payload in [
        good_claim.clone(),
        {
            let mut value = good_claim.clone();
            value["uid"] = json!("codex:fake");
            value
        },
        {
            let mut value = good_claim.clone();
            value.as_object_mut().unwrap().remove("launch_id");
            value
        },
    ] {
        assert_eq!(
            request(&prepared.router, Method::POST, "/api/term/claim", payload)
                .await
                .status(),
            StatusCode::CONFLICT
        );
    }
    prepared
        .lifecycle
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    fixture.assert_no_host_records();
}

#[tokio::test]
async fn status_and_cancel_require_the_exact_receipt_instance_not_name_or_native_uid() {
    let _test = TEST_GATE.acquire().await.unwrap();
    let fixture = Fixture::new(true);
    let record = fixture.seed();
    let prepared = prepare_app(fixture.config(), CancellationToken::new())
        .await
        .unwrap();
    for (uri, status) in [
        ("/api/term/new-status".into(), StatusCode::BAD_REQUEST),
        (
            format!("{}&uid=codex:fake", status_uri(&record)),
            StatusCode::OK,
        ),
        (
            format!(
                "/api/term/new-status?record_id={}&instance_id={}",
                record.record_id(),
                "0".repeat(32)
            ),
            StatusCode::CONFLICT,
        ),
        (
            format!(
                "/api/term/new-status?record_id={}&instance_id={}",
                "0".repeat(32),
                record.instance_id()
            ),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            request(&prepared.router, Method::GET, &uri, json!(null))
                .await
                .status(),
            status
        );
    }
    let cancel = json!({"record_id":record.record_id(),"instance_id":record.instance_id()});
    let mut wrong = cancel.clone();
    wrong["instance_id"] = json!("0".repeat(32));
    assert_eq!(
        request(&prepared.router, Method::POST, "/api/term/kill", wrong)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let mut wrong = cancel.clone();
    wrong["name"] = json!(record.host_name());
    assert_eq!(
        request(&prepared.router, Method::POST, "/api/term/kill", wrong)
            .await
            .status(),
        StatusCode::OK
    );
    // Cancelling an unstarted receipt is a local state transition, not host kill.
    let response = request(
        &prepared.router,
        Method::POST,
        "/api/term/kill",
        cancel.clone(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await["state"], "failed");
    assert_eq!(
        request(&prepared.router, Method::POST, "/api/term/kill", cancel)
            .await
            .status(),
        StatusCode::OK
    );
    prepared
        .lifecycle
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    fixture.assert_no_host_records();
}

#[tokio::test]
async fn unconsumed_responses_queue_until_body_drop_or_consumption_releases_permits() {
    let _test = TEST_GATE.acquire().await.unwrap();
    let fixture = Fixture::new(true);
    let record = fixture.seed();
    // The response pool is sized from the read workers
    // (`Pools::responses`); pin four readers so the pool is the eight permits
    // this test counts.
    let mut config = fixture.config();
    config.pools.read_workers = 4;
    assert_eq!(config.pools.responses(), 8);
    let prepared = prepare_app(config, CancellationToken::new()).await.unwrap();
    let uri = status_uri(&record);
    let mut held = Vec::new();
    for _ in 0..8 {
        let response = request(&prepared.router, Method::GET, &uri, json!(null)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        held.push(response);
    }
    let queued_router = prepared.router.clone();
    let queued_uri = uri.clone();
    let queued = tokio::spawn(async move {
        request(&queued_router, Method::GET, &queued_uri, json!(null)).await
    });
    tokio::task::yield_now().await;
    assert!(!queued.is_finished());
    drop(held.pop());
    let queued = queued.await.unwrap();
    assert_eq!(queued.status(), StatusCode::OK);
    held.push(queued);
    let queued_router = prepared.router.clone();
    let queued_bind = bind_body(&record, "codex:missing");
    let queued = tokio::spawn(async move {
        request(&queued_router, Method::POST, "/api/term/bind", queued_bind).await
    });
    tokio::task::yield_now().await;
    assert!(!queued.is_finished());
    drop(held.pop());
    let admitted = queued.await.unwrap();
    assert_eq!(admitted.status(), StatusCode::CONFLICT);
    assert_eq!(body(admitted).await["code"], "session_error");
    let restored = request(&prepared.router, Method::GET, &uri, json!(null)).await;
    assert_eq!(restored.status(), StatusCode::OK);
    assert_eq!(body(restored).await["record_id"], record.record_id());
    let consumed = held.pop().unwrap();
    assert_eq!(body(consumed).await["state"], "prepared");
    let restored = request(&prepared.router, Method::GET, &uri, json!(null)).await;
    assert_eq!(restored.status(), StatusCode::OK);
    drop(restored);
    drop(held);
    prepared
        .lifecycle
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    fixture.assert_no_host_records();
}

#[tokio::test]
async fn bind_requires_confirmation_and_instance_but_ignores_extra_fields() {
    let _test = TEST_GATE.acquire().await.unwrap();
    let fixture = Fixture::new(true);
    let record = fixture.seed();
    let before = fs::read(fixture.lifecycle.join(store::LEDGER_FILENAME)).unwrap();
    let prepared = prepare_app(fixture.config(), CancellationToken::new())
        .await
        .unwrap();
    let valid = bind_body(&record, "codex:missing");
    for (field, value) in [
        ("operator_confirmed", json!(false)),
        ("operator_confirmed", json!("true")),
        ("operator_confirmed", json!(1)),
        ("operator_confirmed", Value::Null),
    ] {
        let mut payload = valid.clone();
        payload[field] = value;
        let response = request(&prepared.router, Method::POST, "/api/term/bind", payload).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "field {field}");
    }
    for (field, value) in [
        ("sid", json!("PRIVATE_GUESSED_SID")),
        ("source", json!("codex")),
        ("argv", json!(["PRIVATE_COMMAND"])),
        ("unknown", json!("PRIVATE_UNKNOWN")),
        ("agent", json!("child")),
    ] {
        let mut payload = valid.clone();
        payload[field] = value;
        let response = request(&prepared.router, Method::POST, "/api/term/bind", payload).await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "field {field}");
        assert!(!body(response).await.to_string().contains("PRIVATE_"));
    }
    for field in ["operator_confirmed", "record_id", "instance_id", "uid"] {
        let mut payload = valid.clone();
        payload.as_object_mut().unwrap().remove(field);
        assert_eq!(
            request(&prepared.router, Method::POST, "/api/term/bind", payload)
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "missing {field}"
        );
    }
    let mut wrong = valid;
    wrong["instance_id"] = json!("0".repeat(32));
    let response = request(&prepared.router, Method::POST, "/api/term/bind", wrong).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(body(response).await["code"], "launch_identity");
    assert_eq!(
        fs::read(fixture.lifecycle.join(store::LEDGER_FILENAME)).unwrap(),
        before
    );
    prepared
        .lifecycle
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    fixture.assert_no_host_records();
}

#[tokio::test]
async fn bind_rejects_missing_native_evidence_subagents_and_grok_without_starting_or_mutating() {
    let _test = TEST_GATE.acquire().await.unwrap();
    let fixture = Fixture::new(true);
    let record = fixture.seed();
    let codex = fixture.root.join("codex");
    let grok = fixture.root.join("grok");
    directory(&codex);
    directory(&grok);
    for (name, header) in [
        (
            "missing",
            json!({"type":"session_meta","payload":{"session_id":"display-only-id"}}),
        ),
        (
            "child",
            json!({"type":"session_meta","payload":{"id":"child-native-id","thread_source":"subagent","parent_thread_id":"absent-parent"}}),
        ),
    ] {
        file(
            &codex.join(format!("{name}.jsonl")),
            format!("{header}\n").as_bytes(),
            0o600,
        );
    }
    let project = grok.join("project");
    let session = project.join("session");
    directory(&project);
    directory(&session);
    file(
        &session.join("summary.json"),
        br#"{"info":{"id":"grok-display-id"}}"#,
        0o600,
    );
    let roots = SessionRoots {
        codex: Some(codex.clone()),
        grok: Some(grok),
        ..Default::default()
    };
    let inventory = SessionStore::new(roots.clone());
    let snapshot = inventory.search_snapshot().unwrap();
    let catalog = snapshot.native_catalog();
    let mut identities = vec![("codex:absent".to_owned(), AssociationReason::NativeMissing)];
    let mut grok_uid = None;
    for row in snapshot.list["sessions"].as_array().unwrap() {
        let uid = row["uid"].as_str().unwrap().to_owned();
        assert_ne!(
            row["sid"], "child-native-id",
            "orphan subagents are no rows"
        );
        if row["source"] == "grok" {
            // A Grok main session verifies through summary.json
            // `info.id`; binding it to this Codex receipt is a source
            // disagreement, refused before any host or ledger write.
            assert_eq!(
                catalog.verified_scope(&uid).unwrap().session_id,
                "grok-display-id"
            );
            grok_uid = Some(uid);
            continue;
        }
        assert_eq!(
            catalog.verified_scope(&uid),
            Err(AssociationReason::UnsupportedNative)
        );
        identities.push((uid, AssociationReason::UnsupportedNative));
    }
    let grok_uid = grok_uid.expect("grok row listed");
    // A subagent whose parent is not indexed is not listed, but
    // its uid (the index's path hash) stays a catalogued, bind-rejected agent.
    let child = format!(
        "codex:{}",
        &format!(
            "{:x}",
            <sha1::Sha1 as sha1::Digest>::digest(
                codex.join("child.jsonl").to_string_lossy().as_bytes()
            )
        )[..16]
    );
    assert_eq!(
        catalog.verified_scope(&child),
        Err(AssociationReason::Subagent)
    );
    identities.push((child, AssociationReason::Subagent));
    assert_eq!(identities.len(), 3);
    let before = fs::read(fixture.lifecycle.join(store::LEDGER_FILENAME)).unwrap();
    let mut config = fixture.config();
    config.roots = roots;
    let prepared = prepare_app(config, CancellationToken::new()).await.unwrap();
    for (uid, reason) in identities {
        let response = request(
            &prepared.router,
            Method::POST,
            "/api/term/bind",
            bind_body(&record, &uid),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{reason:?}");
        // This is catalog rejection, not merely a prepared receipt being unready.
        assert_eq!(body(response).await["code"], "session_error");
    }
    let response = request(
        &prepared.router,
        Method::POST,
        "/api/term/bind",
        bind_body(&record, &grok_uid),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(response).await["code"], "invalid_native_binding");
    assert_eq!(
        fs::read(fixture.lifecycle.join(store::LEDGER_FILENAME)).unwrap(),
        before
    );
    prepared
        .lifecycle
        .as_ref()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    fixture.assert_no_host_records();
}
