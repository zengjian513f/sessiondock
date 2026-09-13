//! Persisted Claude timeline pins over HTTP: a read-model "rewind" only.
//! Native files are synthetic copies, never rewritten, and no CLI exists.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use sessiondock::{config::Config, sessions::SessionRoots};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tower::ServiceExt;

fn record(kind: &str, id: &str, parent: Value, text: &str) -> Value {
    json!({"type": kind, "uuid": id, "parentUuid": parent, "sessionId": "pin-synthetic",
        "cwd": "/synthetic/rewind", "timestamp": "2026-09-12T00:00:00.000Z",
        "message": {"role": kind, "content": text}})
}

fn config(root: &Path) -> Config {
    Config {
        web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
        roots: SessionRoots {
            claude: Some(root.join("claude")),
            codex: Some(root.join("codex")),
            ..Default::default()
        },
        state_dir: Some(root.join("state")),
        ..Default::default()
    }
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("claude/project")).unwrap();
    fs::create_dir_all(root.path().join("codex")).unwrap();
    fs::create_dir(root.path().join("state")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.path().join("state"), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = root.path().join("claude/project/pin-synthetic.jsonl");
    let rows = [
        record("user", "u1", Value::Null, "Pin question one"),
        record("assistant", "a1", json!("u1"), "Pin answer one"),
        record("user", "u2", json!("a1"), "Pin question two"),
        record("assistant", "a2", json!("u2"), "Pin answer two"),
        record("user", "old-u", json!("a2"), "Pin discarded question"),
        record("assistant", "old-a", json!("old-u"), "Pin discarded answer"),
        record("user", "u3", json!("a2"), "Pin question three"),
        record("assistant", "a3", json!("u3"), "Pin answer three"),
    ];
    fs::write(
        &path,
        rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    let codex = json!({"type":"session_meta","timestamp":"2026-09-12T00:00:00Z",
        "payload":{"id":"codex-plain","cwd":"/synthetic/rewind"}});
    let text = json!({"type":"response_item","payload":{"type":"message","role":"user",
        "content":[{"type":"input_text","text":"codex synthetic question"}]}});
    fs::write(
        root.path().join("codex/rollout-codex-plain.jsonl"),
        format!("{codex}\n{text}\n"),
    )
    .unwrap();
    (root, path)
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

async fn rows(app: &Router) -> Value {
    json(call(app, "/api/sessions", None).await).await
}

fn uid(list: &Value, sid: &str) -> String {
    list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sid"] == sid)
        .unwrap_or_else(|| panic!("{sid} listed"))["uid"]
        .as_str()
        .unwrap()
        .into()
}

fn session_row<'a>(list: &'a Value, uid: &str) -> &'a Value {
    list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["uid"] == uid)
        .unwrap()
}

fn texts(batch: &Value) -> Vec<String> {
    batch["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap().to_owned())
        .collect()
}

fn cursor_query(batch: &Value) -> String {
    format!(
        "start={}&head={}&anchor={}",
        batch["end"],
        batch["version"]["head"].as_str().unwrap(),
        batch["anchor"].as_str().unwrap()
    )
}

async fn rewind(app: &Router, uid: &str, target: Value) -> Response {
    call(
        app,
        "/api/session/rewind",
        Some(json!({"uid": uid, "target": target, "request_id": "synthetic-request"})),
    )
    .await
}

#[tokio::test]
async fn rewind_is_501_without_a_metadata_directory() {
    let (root, _) = fixture();
    let mut cfg = config(root.path());
    cfg.state_dir = None;
    let app = sessiondock::app(cfg).unwrap();
    let meta = json(call(&app, "/api/meta", None).await).await;
    assert_eq!(meta["capabilities"]["timeline_pin"], false);
    assert_eq!(meta["capabilities"]["metadata"], false);
    let list = rows(&app).await;
    let uid = uid(&list, "pin-synthetic");
    let response = rewind(&app, &uid, json!("u3")).await;
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(json(response).await["code"], "metadata_disabled");
    assert_eq!(fs::read_dir(root.path().join("state")).unwrap().count(), 0);
}

#[tokio::test]
async fn pin_reflects_in_history_cursors_reset_and_preferences_are_untouched() {
    let (root, path) = fixture();
    let native = fs::read(&path).unwrap();
    let app = sessiondock::app(config(root.path())).unwrap();
    assert_eq!(
        json(call(&app, "/api/meta", None).await).await["capabilities"]["timeline_pin"],
        true
    );
    let list = rows(&app).await;
    let uid = uid(&list, "pin-synthetic");
    let route = format!("/api/messages/{uid}");
    let star = json(
        call(
            &app,
            "/api/session/star",
            Some(json!({"uid": uid, "starred": true})),
        )
        .await,
    )
    .await;
    assert_eq!(star["starred"], true);
    let before = json(call(&app, &route, None).await).await;
    assert_eq!(texts(&before).len(), 6);
    assert!(before["meta"].get("timeline_pin").is_none());

    let response = rewind(&app, &uid, json!("u3")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["pinned"], true);
    assert_eq!(body["native_rewind"], false);
    assert_eq!(body["target"], "u3");
    assert_eq!(body["tip"], "a2");
    assert_eq!(body["stale_end"], native.len());
    assert_eq!(body["request_id"], "synthetic-request");
    assert_eq!(body["timeline_pin"]["retired"], false);
    assert!(
        body["metadata_revision"].as_u64().unwrap() > star["metadata_revision"].as_u64().unwrap()
    );

    // The stale checkpoint resets to the pinned view; nothing is diffed.
    let pinned = json(call(&app, &format!("{route}?{}", cursor_query(&before)), None).await).await;
    assert_eq!(pinned["reset"], true);
    assert_eq!(
        texts(&pinned),
        [
            "Pin question one",
            "Pin answer one",
            "Pin question two",
            "Pin answer two"
        ]
    );
    assert_eq!(pinned["meta"]["timeline_pin"]["tip"], "a2");
    assert_eq!(pinned["meta"]["timeline_pin"]["native_rewind"], false);
    assert_eq!(pinned["meta"]["starred"], true);
    assert_ne!(pinned["anchor"], before["anchor"]);
    let idle = json(call(&app, &format!("{route}?{}", cursor_query(&pinned)), None).await).await;
    assert_eq!(idle["reset"], false);
    assert_eq!(idle["messages"], json!([]));
    let list = rows(&app).await;
    let row = session_row(&list, &uid);
    assert_eq!(row["timeline_pin"]["target"], "u3");
    assert_eq!(row["timeline_pin"]["retired"], false);
    assert_eq!(row["starred"], true);
    assert_eq!(row["cursor"], pinned["meta"]["cursor"]);
    // Search and the input history use the same pinned logical view.
    let search = json(call(&app, "/api/search?q=Pin+answer+three", None).await).await;
    assert_eq!(search["results"].as_array().map_or(0, Vec::len), 0);
    let search = json(call(&app, "/api/search?q=Pin+answer+two", None).await).await;
    assert_eq!(search["results"].as_array().map_or(0, Vec::len), 1);

    // Re-pinning the same target is idempotent (same revision).
    let again = json(rewind(&app, &uid, json!("u3")).await).await;
    assert_eq!(again["metadata_revision"], body["metadata_revision"]);

    // Persisted: a restarted server shows the same pinned view.
    drop(app);
    let app = sessiondock::app(config(root.path())).unwrap();
    let restarted = json(call(&app, &route, None).await).await;
    assert_eq!(texts(&restarted).len(), 4);
    assert_eq!(restarted["meta"]["timeline_pin"]["target"], "u3");
    assert_eq!(restarted["anchor"], pinned["anchor"]);

    // Unpinning is again a logical change: reset back to the natural view.
    let cleared = json(rewind(&app, &uid, Value::Null).await).await;
    assert_eq!(cleared["pinned"], false);
    assert_eq!(cleared["native_rewind"], false);
    assert!(cleared["timeline_pin"].is_null());
    let natural = json(call(&app, &format!("{route}?{}", cursor_query(&pinned)), None).await).await;
    assert_eq!(natural["reset"], true);
    assert_eq!(texts(&natural).len(), 6);
    assert!(natural["meta"].get("timeline_pin").is_none());
    assert_eq!(natural["meta"]["starred"], true);
    assert_eq!(natural["anchor"], before["anchor"]);
    assert_eq!(fs::read(&path).unwrap(), native);
}

#[tokio::test]
async fn invalid_targets_and_non_claude_sessions_are_refused_without_writes() {
    let (root, path) = fixture();
    let native = fs::read(&path).unwrap();
    let app = sessiondock::app(config(root.path())).unwrap();
    let list = rows(&app).await;
    let claude = uid(&list, "pin-synthetic");
    let codex = uid(&list, "codex-plain");
    for (uid, target, status) in [
        (
            claude.as_str(),
            json!("never-written"),
            StatusCode::NOT_FOUND,
        ),
        (claude.as_str(), json!("old-u"), StatusCode::CONFLICT),
        (claude.as_str(), json!("u1"), StatusCode::CONFLICT),
        (claude.as_str(), json!(""), StatusCode::BAD_REQUEST),
        (
            claude.as_str(),
            json!("x".repeat(257)),
            StatusCode::BAD_REQUEST,
        ),
        (claude.as_str(), json!(7), StatusCode::BAD_REQUEST),
        (codex.as_str(), json!("u3"), StatusCode::BAD_REQUEST),
        ("claude:missing", json!("u3"), StatusCode::NOT_FOUND),
        ("claude:missing", Value::Null, StatusCode::NOT_FOUND),
    ] {
        let response = rewind(&app, uid, target.clone()).await;
        assert_eq!(response.status(), status, "{uid} {target}");
    }
    assert_eq!(
        call(
            &app,
            "/api/session/rewind",
            Some(json!({"uid": claude, "target": "u3", "_build": "stale"}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &app,
            "/api/session/rewind",
            Some(json!({"uid": claude, "target": "u3", "padding": "x".repeat(9 * 1024)}))
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let list = rows(&app).await;
    assert!(session_row(&list, &claude).get("timeline_pin").is_none());
    assert_eq!(
        texts(&json(call(&app, &format!("/api/messages/{claude}"), None).await).await).len(),
        6
    );
    assert_eq!(fs::read(&path).unwrap(), native);
    assert!(!root.path().join("state/session-metadata.json").exists());
}

#[tokio::test]
async fn native_records_past_the_pin_retire_it_with_an_explicit_reason() {
    let (root, path) = fixture();
    let app = sessiondock::app(config(root.path())).unwrap();
    let list = rows(&app).await;
    let uid = uid(&list, "pin-synthetic");
    let route = format!("/api/messages/{uid}");
    let before = json(call(&app, &route, None).await).await;
    let pinned_response = json(rewind(&app, &uid, json!("u3")).await).await;
    assert_eq!(pinned_response["timeline_pin"]["retired"], false);
    let pinned = json(call(&app, &format!("{route}?{}", cursor_query(&before)), None).await).await;
    assert_eq!(pinned["reset"], true);
    assert_eq!(texts(&pinned).len(), 4);

    // The CLI never rewound: it appends a new turn after a3.
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    for record in [
        record("user", "u4", json!("a3"), "Pin question four"),
        record("assistant", "a4", json!("u4"), "Pin answer four"),
    ] {
        writeln!(file, "{record}").unwrap();
    }
    drop(file);
    let retired = json(call(&app, &format!("{route}?{}", cursor_query(&pinned)), None).await).await;
    assert_eq!(retired["reset"], true);
    assert_eq!(texts(&retired).len(), 8);
    assert_eq!(retired["meta"]["timeline_pin"]["retired"], true);
    assert_eq!(
        retired["meta"]["timeline_pin"]["retired_reason"],
        "native_advanced"
    );
    assert!(
        retired["meta"]["timeline_pin"]["retired_message"]
            .as_str()
            .unwrap()
            .contains("CLI 未回滚")
    );
    let list = rows(&app).await;
    let row = session_row(&list, &uid);
    assert_eq!(row["timeline_pin"]["retired"], true);
    assert_eq!(row["timeline_pin"]["retired_reason"], "native_advanced");
    assert_eq!(row["timeline_pin"]["native_rewind"], false);
    assert_eq!(row["cursor"], retired["meta"]["cursor"]);
    // Ordinary appends after retirement are incremental again.
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        file,
        "{}",
        record("user", "u5", json!("a4"), "Pin question five")
    )
    .unwrap();
    drop(file);
    let diff = json(call(&app, &format!("{route}?{}", cursor_query(&retired)), None).await).await;
    assert_eq!(diff["reset"], false);
    assert_eq!(texts(&diff), ["Pin question five"]);
    assert_eq!(
        diff["meta"]["timeline_pin"]["retired_reason"],
        "native_advanced"
    );
    // A restarted server derives the same retirement from the same records
    // once the session is opened. Its list row knows the pin from the
    // metadata alone: the file grew past `stale_end`, so retirement is
    // reported only after the projection exists (docs/history-pages.md).
    drop(app);
    let app = sessiondock::app(config(root.path())).unwrap();
    let list = rows(&app).await;
    let cold = &session_row(&list, &uid)["timeline_pin"];
    assert_eq!(cold["target"], "u3");
    assert_eq!(cold["native_rewind"], false);
    assert!(cold.get("retired").is_none(), "{cold}");
    let reopened = json(call(&app, &route, None).await).await;
    assert_eq!(
        reopened["meta"]["timeline_pin"]["retired_reason"],
        "native_advanced"
    );
    let list = rows(&app).await;
    assert_eq!(
        session_row(&list, &uid)["timeline_pin"]["retired_reason"],
        "native_advanced"
    );
    // A new pin replaces the retired one; a rewind that the CLI actually
    // performed (new input parented at the pinned tip) reports continuation.
    let again = json(rewind(&app, &uid, json!("u4")).await).await;
    assert_eq!(again["tip"], "a3");
    assert_eq!(again["timeline_pin"]["retired"], false);
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        file,
        "{}",
        record("user", "u6", json!("a3"), "Pin question six")
    )
    .unwrap();
    drop(file);
    // A detail read restamps the changed file inside the list's 500 ms TTL.
    let view = json(call(&app, &route, None).await).await;
    assert_eq!(texts(&view).last().unwrap(), "Pin question six");
    assert!(!texts(&view).contains(&"Pin question five".to_owned()));
    assert_eq!(
        view["meta"]["timeline_pin"]["retired_reason"],
        "native_continued"
    );
    let list = rows(&app).await;
    assert_eq!(
        session_row(&list, &uid)["timeline_pin"]["retired_reason"],
        "native_continued"
    );
    assert_eq!(session_row(&list, &uid)["cursor"], view["meta"]["cursor"]);
}
