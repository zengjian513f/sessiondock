use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sessiondock::{config::Config, sessions::SessionRoots};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tower::ServiceExt;

fn config(root: &Path) -> Config {
    Config {
        web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
        roots: SessionRoots {
            codex: Some(root.join("native")),
            ..Default::default()
        },
        state_dir: Some(root.join("state")),
        ..Default::default()
    }
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("native")).unwrap();
    fs::create_dir(root.path().join("state")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.path().join("state"), fs::Permissions::from_mode(0o700)).unwrap();
    }
    for (name, parent) in [("parent", false), ("child", true)] {
        let mut meta = json!({"type":"session_meta","timestamp":"2026-09-12T00:00:00Z",
            "payload":{"id":name,"cwd":"/synthetic/preferences"}});
        if parent {
            meta["payload"]["forked_from_id"] = json!("parent");
            meta["payload"]["history_base"] = json!({"thread_id":"parent","end_byte_offset":0});
        }
        let text = json!({"type":"response_item","payload":{"type":"message","role":"user",
            "content":[{"type":"input_text","text":format!("{name} synthetic question")}]}});
        fs::write(
            root.path().join(format!("native/{name}.jsonl")),
            format!("{meta}\n{text}\n"),
        )
        .unwrap();
    }
    root
}

async fn call(app: &Router, uri: &str, body: Option<Value>) -> Response {
    let mut builder = Request::builder().uri(uri).header("Host", "127.0.0.1:8741");
    let body = if let Some(body) = body {
        builder = builder
            .method("POST")
            .header("Content-Type", "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn json(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn list(app: &Router) -> Value {
    json(call(app, "/api/sessions", None).await).await
}
fn uid(list: &Value, sid: &str) -> String {
    list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sid"] == sid)
        .unwrap()["uid"]
        .as_str()
        .unwrap()
        .into()
}

#[tokio::test]
async fn preferences_persist_without_changing_native_bytes_or_cursor() {
    let root = fixture();
    let native = fs::read(root.path().join("native/child.jsonl")).unwrap();
    let app = sessiondock::app(config(root.path())).unwrap();
    let initial = list(&app).await;
    let uid = uid(&initial, "child");
    let route = format!("/api/messages/{uid}");
    let before = json(call(&app, &route, None).await).await;
    let saved = call(
        &app,
        "/api/session/star",
        Some(json!({"uid":uid,"starred":true})),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    let saved = json(saved).await;
    assert_eq!(saved["starred"], true);
    let changed = list(&app).await; // Must notice metadata even inside inventory TTL.
    assert_ne!(initial["sig"], changed["sig"]);
    let after = json(call(&app, &route, None).await).await;
    assert_eq!(after["meta"]["starred"], true);
    assert_eq!(before["anchor"], after["anchor"]);
    assert_eq!(before["messages"], after["messages"]);
    assert_eq!(
        fs::read(root.path().join("native/child.jsonl")).unwrap(),
        native
    );
    drop(app);
    let restarted = sessiondock::app(config(root.path())).unwrap();
    let after = json(call(&restarted, &route, None).await).await;
    assert_eq!(after["meta"]["starred_at"], saved["starred_at"]);
    let unstar = call(
        &restarted,
        "/api/session/star",
        Some(json!({"uid":uid,"starred":false})),
    )
    .await;
    assert_eq!(json(unstar).await["starred"], false);
    assert_ne!(
        json(call(&restarted, &route, None).await).await["meta"]["starred"],
        true
    );
    let caps = json(call(&restarted, "/api/meta", None).await).await;
    assert_eq!(caps["capabilities"]["metadata"], true);
    assert_eq!(caps["capabilities"]["mutations"], false);
    assert_eq!(
        call(&restarted, "/api/session/send", Some(json!({})))
            .await
            .status(),
        StatusCode::NOT_IMPLEMENTED
    );
}

#[tokio::test]
async fn visibility_is_scoped_deduplicated_and_reports_invalid_rows() {
    let root = fixture();
    let app = sessiondock::app(config(root.path())).unwrap();
    let initial = list(&app).await;
    let parent = uid(&initial, "parent");
    let child = uid(&initial, "child");
    let response = call(
        &app,
        "/api/sessions/fork-visibility",
        Some(json!({
        "uids":[parent,parent,child,"codex:missing"],"visible":true})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(
        body["updated"],
        json!([{"uid":parent,"fork_parent_visible":true}])
    );
    assert_eq!(body["errors"].as_array().unwrap().len(), 2);
    let after = list(&app).await;
    let row = after["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["uid"] == parent)
        .unwrap();
    assert_eq!(row["fork_parent"], true);
    assert_eq!(row["fork_parent_visible"], true);
}

#[tokio::test]
async fn disabled_invalid_missing_and_stale_writes_fail_closed() {
    let root = fixture();
    let mut cfg = config(root.path());
    cfg.state_dir = None;
    let disabled = sessiondock::app(cfg).unwrap();
    assert_eq!(
        call(&disabled, "/api/session/star", Some(json!({})))
            .await
            .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(fs::read_dir(root.path().join("state")).unwrap().count(), 0);
    let app = sessiondock::app(config(root.path())).unwrap();
    assert_eq!(
        call(
            &app,
            "/api/session/star",
            Some(json!({"uid":"codex:missing","starred":true,"padding":"x".repeat(9 * 1024)}))
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        call(
            &app,
            "/api/session/star",
            Some(json!({"uid":"codex:missing","starred":true}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "/api/session/star",
            Some(json!({"uid":"codex:missing","starred":"yes"}))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "/api/session/star",
            Some(json!({"uid":"codex:missing","starred":true,"_build":"old"}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert!(
        sessiondock::app(config(root.path())).is_err(),
        "second writer must fail"
    );
}

#[tokio::test]
async fn a_preference_change_is_sent_without_reset_or_native_append() {
    let root = fixture();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let app = sessiondock::app_with_shutdown(config(root.path()), shutdown.clone()).unwrap();
    let uid = uid(&list(&app).await, "child");
    let mut body = call(&app, &format!("/api/watch?uid={uid}"), None)
        .await
        .into_body();
    async fn next(body: &mut Body) -> Value {
        let bytes = tokio::time::timeout(Duration::from_secs(3), body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        serde_json::from_str(
            std::str::from_utf8(&bytes)
                .unwrap()
                .trim()
                .strip_prefix("data: ")
                .unwrap(),
        )
        .unwrap()
    }
    let first = next(&mut body).await;
    assert_eq!(
        call(
            &app,
            "/api/session/star",
            Some(json!({"uid":uid,"starred":true}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    let changed = next(&mut body).await;
    assert_eq!(changed["meta"]["starred"], true);
    assert_eq!(changed["reset"], false);
    assert_eq!(changed["messages"], json!([]));
    assert_eq!(changed["anchor"], first["anchor"]);
    shutdown.cancel();
    drop(body);
}
