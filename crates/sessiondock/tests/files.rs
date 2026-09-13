//! Synthetic-only HTTP contract and transport-lifetime tests. No native homes,
//! external services, paid CLIs, or production files are consulted.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sessiondock::{config::Config, files::STREAM_CHUNK_BYTES, sessions::SessionRoots};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        for directory in ["native", "claude/project", "files/nested"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        for (name, data) in [
            ("note.txt", b"synthetic safe text".as_slice()),
            ("nested/child.txt", b"nested text"),
            ("agent.txt", b"agent only"),
            ("unmentioned.txt", b"not granted"),
            ("unsafe.html", b"<script>window.syntheticBad=1</script>"),
            ("image.png", b"synthetic PNG"),
            ("valid.pdf", b"%PDF-1.4\nsynthetic"),
            ("invalid.pdf", b"not pdf"),
            ("empty.txt", b""),
        ] {
            fs::write(root.join("files").join(name), data).unwrap();
        }
        fs::write(
            root.join("files/large.bin"),
            vec![b'x'; STREAM_CHUNK_BYTES * 3 + 9],
        )
        .unwrap();
        let cwd = root.join("files");
        let meta = json!({"type":"session_meta","timestamp":"2026-09-12T00:00:00Z","payload":{"id":"parent","session_id":"parent","cwd":cwd,"thread_source":"user"}});
        let prompt = message(
            "`note.txt` `./nested` `unsafe.html` `image.png` `valid.pdf` `invalid.pdf` `empty.txt` `large.bin`",
        );
        write_records(&root.join("native/parent.jsonl"), &[meta, prompt]);
        let agent = json!({"type":"session_meta","timestamp":"2026-09-12T00:00:00Z","payload":{"id":"worker","session_id":"parent","cwd":cwd,"thread_source":"subagent","parent_thread_id":"parent","forked_from_id":"parent","agent_path":"team/worker","agent_role":"explorer"}});
        write_records(
            &root.join("native/agent.jsonl"),
            &[agent, message("`agent.txt`")],
        );
        write_records(
            &root.join("native/unsupported.jsonl"),
            &[
                json!({"type":"session_meta","timestamp":"2026-09-12T00:00:00Z","payload":{"id":"unsupported","cwd":cwd}}),
                message("`note.txt`"),
                // A scalar `content` is a shape the Python adapter cannot read
                // either; it stays a hard failure (batch 33 only turned unknown
                // record kinds into skips, batch 35 the copied Codex metas).
                json!({"type":"response_item","timestamp":"2026-09-12T00:00:01Z","payload":{"type":"message","role":"user","content":42}}),
            ],
        );
        write_records(
            &root.join("claude/project/branch.jsonl"),
            &[
                json!({"type":"user","sessionId":"branch","uuid":"u0","parentUuid":null,"cwd":cwd,"message":{"role":"user","content":"`note.txt`"}}),
                json!({"type":"assistant","sessionId":"branch","uuid":"a0","parentUuid":"u0","message":{"role":"assistant","content":"visible answer","stop_reason":"end_turn"}}),
                json!({"type":"user","sessionId":"branch","uuid":"abandoned","parentUuid":"a0","message":{"role":"user","content":"`unmentioned.txt`"}}),
                json!({"type":"assistant","sessionId":"branch","uuid":"abandoned-answer","parentUuid":"abandoned","message":{"role":"assistant","content":"abandoned answer","stop_reason":"end_turn"}}),
                json!({"type":"last-prompt","leafUuid":"a0"}),
            ],
        );
        Self { _temp: temp, root }
    }
    fn config(&self) -> Config {
        Config {
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            roots: SessionRoots {
                codex: Some(self.root.join("native")),
                claude: Some(self.root.join("claude")),
                ..Default::default()
            },
            file_roots: vec![self.root.join("files")],
            ..Default::default()
        }
    }
    fn app(&self) -> Router {
        sessiondock::app(self.config()).unwrap()
    }
    fn file(&self, name: &str) -> PathBuf {
        self.root.join("files").join(name)
    }
}
fn message(text: &str) -> Value {
    json!({"type":"response_item","timestamp":"2026-09-12T00:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}})
}
fn write_records(path: &Path, records: &[Value]) {
    fs::write(
        path,
        records
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
}
fn encode(text: &str) -> String {
    let mut output = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            output.push(byte as char)
        } else {
            output.push_str(&format!("%{byte:02X}"))
        }
    }
    output
}
fn route(directory: bool, uid: &str, reference: &str, extra: &[(&str, &str)]) -> String {
    let mut route = format!(
        "/api/session/{}?uid={}&ref={}",
        if directory { "files" } else { "file" },
        encode(uid),
        encode(reference)
    );
    for (key, value) in extra {
        route.push_str(&format!("&{}={}", encode(key), encode(value)));
    }
    route
}
async fn request(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Body,
) -> Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:8741");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}
async fn get(app: &Router, uri: &str) -> Response {
    request(app, "GET", uri, &[], Body::empty()).await
}
async fn value(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}
async fn resolve(app: &Router, uid: &str, agent: &str, refs: &[&str]) -> Response {
    request(
        app,
        "POST",
        "/api/session/resolve-files",
        &[("Content-Type", "application/json")],
        Body::from(json!({"uid":uid,"agent":agent,"refs":refs}).to_string()),
    )
    .await
}
async fn row(app: &Router, sid: &str) -> Value {
    let listing = value(get(app, "/api/sessions").await).await;
    listing["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sid"] == sid)
        .unwrap_or_else(|| panic!("missing synthetic SID {sid}: {listing}"))
        .clone()
}
async fn uid(app: &Router, sid: &str) -> String {
    row(app, sid).await["uid"].as_str().unwrap().into()
}
fn snapshot(directory: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut result = Vec::new();
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(snapshot(&path))
        } else {
            result.push((path.clone(), fs::read(path).unwrap()))
        }
    }
    result.sort_by(|left, right| left.0.cmp(&right.0));
    result
}

#[tokio::test]
async fn legacy_routes_preserve_native_and_file_bytes_and_writes_stay_unimplemented() {
    let fixture = Fixture::new();
    let before = snapshot(&fixture.root);
    let app = fixture.app();
    let uid = uid(&app, "parent").await;
    let response = resolve(&app, &uid, "", &["note.txt", "./nested", "unmentioned.txt"]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = value(response).await;
    assert_eq!(response["targets"].as_array().unwrap().len(), 2);
    assert_eq!(response["errors"][0]["status"], 404);
    assert_eq!(response["incomplete"], true);
    let info = value(get(&app, &route(false, &uid, "note.txt", &[("mode", "info")])).await).await;
    assert_eq!(info["preview"], "text");
    assert_eq!(info["text"], "synthetic safe text");
    assert_eq!(info["kind"], "file");
    let listing = value(get(&app, &route(true, &uid, "./nested", &[])).await).await;
    assert_eq!(listing["entries"][0]["name"], "child.txt");
    assert_eq!(listing["writable"], false);
    assert_eq!(
        listing["root"],
        fixture.root.join("files").to_str().unwrap()
    );
    for endpoint in ["/api/session/files/action", "/api/session/files/upload"] {
        let response=request(&app,"POST",endpoint,&[("Content-Type","application/json")],Body::from(json!({"uid":uid,"ref":"./nested","action":"delete","paths":[fixture.file("note.txt")]}).to_string())).await;
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }
    assert_eq!(snapshot(&fixture.root), before);
}

#[tokio::test]
async fn root_navigation_scope_branch_and_unknown_native_fail_closed() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let parent = row(&app, "parent").await;
    let uid = parent["uid"].as_str().unwrap();
    let worker = parent["agent_items"][0]["id"].as_str().unwrap();
    let agent = value(resolve(&app, uid, worker, &["agent.txt", "note.txt"]).await).await;
    assert_eq!(agent["targets"][0]["ref"], "agent.txt");
    assert_eq!(agent["errors"][0]["ref"], "note.txt");
    assert_eq!(
        get(&app, &route(false, uid, "agent.txt", &[]))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        resolve(&app, uid, "../missing", &["note.txt"])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let branch = uid_for(&app, "branch").await;
    assert_eq!(
        get(&app, &route(false, &branch, "unmentioned.txt", &[]))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let unsupported = uid_for(&app, "unsupported").await;
    assert_eq!(
        resolve(&app, &unsupported, "", &["note.txt"])
            .await
            .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        get(
            &app,
            &route(
                true,
                uid,
                "./nested",
                &[("path", fixture.root.to_str().unwrap())]
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(
            &app,
            &route(
                true,
                uid,
                "note.txt",
                &[("path", fixture.file("note.txt").to_str().unwrap())]
            )
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let root = value(
        get(
            &app,
            &route(
                true,
                uid,
                "./nested",
                &[("path", fixture.file("").to_str().unwrap())],
            ),
        )
        .await,
    )
    .await;
    assert!(root["parent"].is_null());
    assert_eq!(
        get(&app, &route(false, "codex:missing", "note.txt", &[]))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
async fn uid_for(app: &Router, sid: &str) -> String {
    uid(app, sid).await
}

#[tokio::test]
async fn disabled_and_malformed_requests_and_unknown_modes_are_explicit() {
    let fixture = Fixture::new();
    let mut cfg = fixture.config();
    cfg.file_roots.clear();
    let disabled = sessiondock::app(cfg).unwrap();
    let response = resolve(&disabled, "codex:missing", "", &["note.txt"]).await;
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(value(response).await["code"], "files_disabled");
    let app = fixture.app();
    let uid = uid(&app, "parent").await;
    for body in [
        json!({"uid":uid,"refs":"note.txt"}),
        json!({"uid":uid,"refs":[null]}),
        json!({"uid":uid,"refs":vec!["x";257]}),
        json!({"uid":uid,"refs":["x".repeat(4097)]}),
        json!({"uid":uid,"refs":["note.txt"],"cwd":"/not-authority"}),
    ] {
        let response = request(
            &app,
            "POST",
            "/api/session/resolve-files",
            &[("Content-Type", "application/json")],
            Body::from(body.to_string()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let oversized = request(
        &app,
        "POST",
        "/api/session/resolve-files",
        &[("Content-Type", "application/json")],
        Body::from(" ".repeat(1100 * 1024 + 1)),
    )
    .await;
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    for extra in [
        [("offset", "-1")],
        [("hidden", "yes")],
        [("limit", "501")],
        [("path", "../")],
    ] {
        assert_eq!(
            get(&app, &route(true, &uid, "./nested", &extra))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    for mode in ["jobs", "trash", "artifact", "thumbnail", "unrecognized"] {
        let response = get(&app, &route(true, &uid, "./nested", &[("mode", mode)])).await;
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(value(response).await["code"], "file_mode_not_implemented");
    }
    let cross_site = request(
        &app,
        "POST",
        "/api/session/resolve-files",
        &[
            ("Origin", "https://example.invalid"),
            ("Content-Type", "application/json"),
        ],
        Body::from(json!({"uid":uid,"refs":["note.txt"]}).to_string()),
    )
    .await;
    assert_eq!(cross_site.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn range_head_if_range_preview_mime_and_csp_are_honest() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let uid = uid(&app, "parent").await;
    let download = route(false, &uid, "large.bin", &[("download", "1")]);
    let response = request(
        &app,
        "GET",
        &download,
        &[("Range", "bytes=2-5")],
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response.headers()["content-range"],
        format!("bytes 2-5/{}", STREAM_CHUNK_BYTES * 3 + 9)
    );
    assert_eq!(
        to_bytes(response.into_body(), 100).await.unwrap(),
        b"xxxx".as_slice()
    );
    for header in ["bytes=-0", "bytes=2-1", "bytes=0-1,3-4"] {
        let response = request(&app, "GET", &download, &[("Range", header)], Body::empty()).await;
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert!(response.headers().contains_key("content-range"));
        assert!(
            to_bytes(response.into_body(), 100)
                .await
                .unwrap()
                .is_empty()
        );
    }
    let head = request(
        &app,
        "HEAD",
        &download,
        &[("Range", "bytes=2-5")],
        Body::empty(),
    )
    .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        head.headers()["content-length"],
        (STREAM_CHUNK_BYTES * 3 + 9).to_string()
    );
    assert!(!head.headers().contains_key("content-range"));
    assert!(to_bytes(head.into_body(), 1).await.unwrap().is_empty());
    let if_range = request(
        &app,
        "GET",
        &download,
        &[("Range", "bytes=2-5"), ("If-Range", "\"old-version\"")],
        Body::empty(),
    )
    .await;
    assert_eq!(if_range.status(), StatusCode::OK);
    assert!(!if_range.headers().contains_key("content-range"));
    drop(if_range);
    let html = get(&app, &route(false, &uid, "unsafe.html", &[])).await;
    assert_eq!(html.headers()["content-type"], "text/plain; charset=utf-8");
    assert!(
        html.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    assert_eq!(html.headers()["x-content-type-options"], "nosniff");
    drop(html);
    let pdf = get(
        &app,
        &route(false, &uid, "valid.pdf", &[("mode", "preview")]),
    )
    .await;
    assert_eq!(pdf.status(), StatusCode::OK);
    assert_eq!(pdf.headers()["content-type"], "application/pdf");
    assert_eq!(
        pdf.headers()["content-security-policy"],
        "frame-ancestors 'self'"
    );
    drop(pdf);
    assert_eq!(
        get(
            &app,
            &route(false, &uid, "invalid.pdf", &[("mode", "preview")])
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(
            &app,
            &route(false, &uid, "invalid.pdf", &[("download", "1")])
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn two_unpolled_bodies_hold_capacity_drop_frees_it_without_reading() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let uid = uid(&app, "parent").await;
    let uri = route(false, &uid, "large.bin", &[("download", "1")]);
    let first = get(&app, &uri).await;
    let second = get(&app, &uri).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let busy = get(&app, &uri).await;
    assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value(busy).await["code"], "files_busy");
    drop(first);
    let accepted = get(&app, &uri).await;
    assert_eq!(accepted.status(), StatusCode::OK);
    drop(accepted);
    drop(second);
    let text = route(false, &uid, "note.txt", &[]);
    let first = get(&app, &text).await;
    let second = get(&app, &text).await;
    assert_eq!(
        get(&app, &text).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    drop(first);
    drop(second);
    assert_eq!(get(&app, &text).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn slow_reader_reads_one_chunk_at_a_time_and_detects_next_native_change() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let uid = uid(&app, "parent").await;
    let uri = route(false, &uid, "large.bin", &[("download", "1")]);
    let response = get(&app, &uri).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(first.len(), STREAM_CHUNK_BYTES);
    // A slow consumer does not cause later data to be prefetched into a queue.
    fs::OpenOptions::new()
        .write(true)
        .open(fixture.file("large.bin"))
        .unwrap()
        .write_all(b"changed after first chunk")
        .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(2), body.frame())
        .await
        .unwrap();
    assert!(
        next.unwrap().is_err(),
        "changed file must fail the body, not end successfully"
    );
    drop(body);
    let refreshed = get(&app, &uri).await;
    assert_eq!(refreshed.status(), StatusCode::OK);
    drop(refreshed);
}

#[tokio::test]
async fn changed_unpolled_file_and_truncated_transfer_are_body_errors() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let uid = uid(&app, "parent").await;
    let uri = route(false, &uid, "large.bin", &[("download", "1")]);
    let response = get(&app, &uri).await;
    fs::OpenOptions::new()
        .write(true)
        .open(fixture.file("large.bin"))
        .unwrap()
        .set_len(1)
        .unwrap();
    assert!(
        to_bytes(response.into_body(), 4 * STREAM_CHUNK_BYTES)
            .await
            .is_err()
    );
    let empty = route(false, &uid, "empty.txt", &[("download", "1")]);
    let response = get(&app, &empty).await;
    assert_eq!(response.headers()["content-length"], "0");
    assert!(to_bytes(response.into_body(), 1).await.unwrap().is_empty());
}

#[tokio::test]
async fn shutdown_stops_unpolled_or_slow_bodies_and_rejects_new_file_work() {
    let fixture = Fixture::new();
    let cancel = CancellationToken::new();
    let app = sessiondock::app_with_shutdown(fixture.config(), cancel.clone()).unwrap();
    let uid = uid(&app, "parent").await;
    let uri = route(false, &uid, "large.bin", &[("download", "1")]);
    let unpolled = get(&app, &uri).await;
    let response = get(&app, &uri).await;
    let mut body = response.into_body();
    assert_eq!(
        body.frame()
            .await
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap()
            .len(),
        STREAM_CHUNK_BYTES
    );
    cancel.cancel();
    assert!(
        to_bytes(unpolled.into_body(), 4 * STREAM_CHUNK_BYTES)
            .await
            .is_err()
    );
    assert!(body.frame().await.unwrap().is_err());
    drop(body);
    let rejected = get(&app, &uri).await;
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value(rejected).await["code"], "shutdown");
}

#[cfg(unix)]
#[tokio::test]
async fn replaced_navigation_parent_and_symlink_do_not_expose_external_data() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let app = fixture.app();
    let uid = uid(&app, "parent").await;
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("note.txt"), b"outside synthetic secret").unwrap();
    symlink(outside.path(), fixture.file("nested/link")).unwrap();
    let listing = value(get(&app, &route(true, &uid, "./nested", &[])).await).await;
    assert_eq!(listing["incomplete"], true);
    let link = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "link")
        .unwrap();
    assert_eq!(link["kind"], "unavailable");
    assert_eq!(link["symlink"], true);
    let uri = route(
        true,
        &uid,
        "./nested",
        &[
            ("path", fixture.file("nested/child.txt").to_str().unwrap()),
            ("download", "1"),
        ],
    );
    let response = get(&app, &uri).await;
    assert_eq!(response.status(), StatusCode::OK);
    fs::rename(fixture.file("nested"), fixture.file("old-nested")).unwrap();
    symlink(outside.path(), fixture.file("nested")).unwrap();
    assert!(to_bytes(response.into_body(), 1024).await.is_err());
    assert_eq!(get(&app, &uri).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        fs::read(outside.path().join("note.txt")).unwrap(),
        b"outside synthetic secret"
    );
}
