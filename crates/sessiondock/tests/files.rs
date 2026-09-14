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

fn wire(path: &Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    {
        let value = value
            .strip_prefix("\\\\?\\UNC\\")
            .map(|rest| format!("//{rest}"))
            .or_else(|| value.strip_prefix("\\\\?\\").map(str::to_owned))
            .unwrap_or_else(|| value.into_owned());
        return value.replace('\\', "/");
    }
    #[cfg(not(windows))]
    value.into_owned()
}

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
                // either; it stays a hard failure (only unknown
                // record kinds and copied Codex metas are skipped).
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
    let uid = uid_for(&app, "parent").await;
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
        wire(fixture.root.ancestors().last().unwrap())
    );
    for endpoint in ["/api/session/files/action", "/api/session/files/upload"] {
        let response=request(&app,"POST",endpoint,&[("Content-Type","application/json")],Body::from(json!({"uid":uid,"ref":"./nested","action":"delete","paths":[fixture.file("note.txt")]}).to_string())).await;
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }
    assert_eq!(snapshot(&fixture.root), before);
}

#[tokio::test]
async fn root_navigation_preserves_scope_branch_and_unknown_native_checks() {
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
        StatusCode::OK
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
    assert_eq!(root["parent"], wire(&fixture.root));
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
async fn reads_without_roots_and_malformed_requests_and_unknown_modes_are_explicit() {
    let fixture = Fixture::new();
    let mut cfg = fixture.config();
    cfg.file_roots.clear();
    let disabled = sessiondock::app(cfg).unwrap();
    let uid = uid(&disabled, "parent").await;
    let response = get(&disabled, &route(false, &uid, "note.txt", &[])).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        &to_bytes(response.into_body(), 1024).await.unwrap()[..],
        b"synthetic safe text"
    );
    let app = fixture.app();
    let uid = uid_for(&app, "parent").await;
    for body in [
        json!({"uid":uid,"refs":"note.txt"}),
        json!({"uid":uid,"refs":[null]}),
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
    for refs in [vec!["x".to_owned(); 257], vec!["x".repeat(4097)]] {
        let response = request(
            &app,
            "POST",
            "/api/session/resolve-files",
            &[("Content-Type", "application/json")],
            Body::from(json!({"uid":uid,"refs":refs}).to_string()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let oversized = request(
        &app,
        "POST",
        "/api/session/resolve-files",
        &[("Content-Type", "application/json")],
        Body::from(" ".repeat(4 * 1024 * 1024 + 1)),
    )
    .await;
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    for extra in [[("offset", "-1")], [("path", "../")]] {
        assert_eq!(
            get(&app, &route(true, &uid, "./nested", &extra))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    for mode in ["jobs", "trash", "artifact", "thumbnail"] {
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
    let uid = uid_for(&app, "parent").await;
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
async fn concurrent_download_and_text_requests_queue_and_complete() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let uid = uid_for(&app, "parent").await;
    let uri = route(false, &uid, "large.bin", &[("download", "1")]);
    let text = route(false, &uid, "note.txt", &[]);
    let mut tasks = tokio::task::JoinSet::new();
    for route in [uri, text] {
        for _ in 0..8 {
            let app = app.clone();
            let route = route.clone();
            tasks.spawn(async move {
                let response = get(&app, &route).await;
                assert_eq!(response.status(), StatusCode::OK);
                assert!(
                    !to_bytes(response.into_body(), 1024 * 1024)
                        .await
                        .unwrap()
                        .is_empty()
                );
            });
        }
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
    })
    .await
    .expect("queued file reads should complete when preceding bodies drain");
}

#[tokio::test]
async fn slow_reader_reads_one_chunk_at_a_time_and_detects_next_native_change() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let uid = uid_for(&app, "parent").await;
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
    let uid = uid_for(&app, "parent").await;
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
    let uid = uid_for(&app, "parent").await;
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
async fn symlink_navigation_is_allowed_but_old_transfer_handles_reject_replacement() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let app = fixture.app();
    let uid = uid_for(&app, "parent").await;
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("note.txt"), b"outside synthetic secret").unwrap();
    symlink(outside.path(), fixture.file("nested/link")).unwrap();
    let listing = value(get(&app, &route(true, &uid, "./nested", &[])).await).await;
    assert_eq!(listing["incomplete"], false);
    let link = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "link")
        .unwrap();
    assert_eq!(link["kind"], "directory");
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
    assert_eq!(get(&app, &uri).await.status(), StatusCode::NOT_FOUND);
    let fresh = route(
        true,
        &uid,
        "./nested",
        &[
            ("path", outside.path().join("note.txt").to_str().unwrap()),
            ("download", "1"),
        ],
    );
    let response = get(&app, &fresh).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        &to_bytes(response.into_body(), 1024).await.unwrap()[..],
        b"outside synthetic secret"
    );
    assert_eq!(
        fs::read(outside.path().join("note.txt")).unwrap(),
        b"outside synthetic secret"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn browser_info_describes_link_leaves_only_after_a_directory_grant() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let dangling = fixture.file("dangling-link");
    symlink("missing-target", &dangling).unwrap();
    let linked_text = fixture.file("nested/display-name.txt");
    symlink("child.txt", &linked_text).unwrap();
    let pdf_name = fixture.file("nested/display-name.pdf");
    symlink("child.txt", &pdf_name).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(fixture.root.join("native/parent.jsonl"))
        .unwrap()
        .write_all(format!("{}\n", message("`./dangling-link`")).as_bytes())
        .unwrap();
    let native_before = fs::read(fixture.root.join("native/parent.jsonl")).unwrap();
    let app = fixture.app();
    let parent = row(&app, "parent").await;
    let uid = parent["uid"].as_str().unwrap();
    let worker = parent["agent_items"][0]["id"].as_str().unwrap();
    let info_uri = |path: &Path| {
        route(
            true,
            uid,
            "./nested",
            &[("mode", "info"), ("path", path.to_str().unwrap())],
        )
    };

    let response = get(&app, &info_uri(&dangling)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let info = value(response).await;
    assert_eq!(info["kind"], "symlink");
    assert_eq!(info["path"], dangling.to_str().unwrap());
    assert_eq!(info["name"], "dangling-link");
    assert_eq!(info["link_target"], "missing-target");
    assert_eq!(info["size"], fs::symlink_metadata(&dangling).unwrap().len());
    assert!(info["mode"].as_str().unwrap().starts_with('l'));
    assert!(info.get("preview").is_none());
    assert!(!dangling.exists());

    // A mentioned direct reference still needs a resolvable file, exactly as
    // Python files.resolve does; the browser grant does not weaken that route.
    let direct = get(
        &app,
        &route(false, uid, "./dangling-link", &[("mode", "info")]),
    )
    .await;
    assert_eq!(direct.status(), StatusCode::NOT_FOUND);
    let missing_parent = get(&app, &info_uri(&fixture.file("missing-parent/leaf"))).await;
    assert_eq!(missing_parent.status(), StatusCode::NOT_FOUND);
    let missing_leaf = get(&app, &info_uri(&fixture.file("nested/missing-leaf"))).await;
    assert_eq!(missing_leaf.status(), StatusCode::NOT_FOUND);

    let denied_ref = get(
        &app,
        &route(
            true,
            uid,
            "./not-recorded",
            &[("mode", "info"), ("path", dangling.to_str().unwrap())],
        ),
    )
    .await;
    assert_eq!(denied_ref.status(), StatusCode::NOT_FOUND);
    let denied_agent = get(
        &app,
        &route(
            true,
            uid,
            "./nested",
            &[
                ("mode", "info"),
                ("path", dangling.to_str().unwrap()),
                ("agent", worker),
            ],
        ),
    )
    .await;
    assert_eq!(denied_agent.status(), StatusCode::NOT_FOUND);
    let other = uid_for(&app, "branch").await;
    let denied_session = get(
        &app,
        &route(
            true,
            &other,
            "./nested",
            &[("mode", "info"), ("path", dangling.to_str().unwrap())],
        ),
    )
    .await;
    assert_eq!(denied_session.status(), StatusCode::NOT_FOUND);

    let linked = value(get(&app, &info_uri(&linked_text)).await).await;
    assert_eq!(linked["kind"], "symlink");
    assert_eq!(linked["name"], "display-name.txt");
    assert_eq!(linked["link_target"], "child.txt");
    assert_eq!(linked["preview"], "text");
    assert_eq!(linked["text"], "nested text");
    assert_eq!(
        linked["size"],
        fs::symlink_metadata(&linked_text).unwrap().len()
    );
    // Python describes the link's suffix, not the target's suffix.
    let pdf = value(get(&app, &info_uri(&pdf_name)).await).await;
    assert_eq!(pdf["kind"], "symlink");
    assert_eq!(pdf["preview"], "application/pdf");
    assert!(pdf.get("text").is_none());
    let ordinary = value(get(&app, &info_uri(&fixture.file("note.txt"))).await).await;
    assert_eq!(ordinary["kind"], "file");
    assert_eq!(ordinary["preview"], "text");
    assert_eq!(ordinary["text"], "synthetic safe text");

    // A durable grant still permits this metadata request after its original
    // directory has gone away; the current native scope is checked above.
    fs::remove_dir_all(fixture.file("nested")).unwrap();
    let retained = get(&app, &info_uri(&dangling)).await;
    assert_eq!(retained.status(), StatusCode::OK);
    assert_eq!(value(retained).await["kind"], "symlink");
    assert_eq!(
        fs::read(fixture.root.join("native/parent.jsonl")).unwrap(),
        native_before
    );
}

#[tokio::test]
async fn file_queries_read_only_relevant_fields_like_python() {
    let fixture = Fixture::new();
    fs::write(fixture.file("nested/.hidden"), "hidden").unwrap();
    #[cfg(unix)]
    fs::write(fixture.file("nested/tab\tname.txt"), "tab text").unwrap();
    let mut config = fixture.config();
    config.file_write_roots = vec![fixture.file("")];
    let app = sessiondock::app(config).unwrap();
    let uid = uid_for(&app, "parent").await;
    let resolved = request(
        &app,
        "POST",
        "/api/session/resolve-files",
        &[("Content-Type", "application/json")],
        Body::from(
            json!({"uid":uid,"refs":["note.txt"],"cwd":"/not-authority","extra":true}).to_string(),
        ),
    )
    .await;
    assert_eq!(resolved.status(), StatusCode::OK);
    assert_eq!(
        value(resolved).await["targets"][0]["path"],
        wire(&fixture.file("note.txt"))
    );

    for directory in [false, true] {
        let reference = if directory { "./nested" } else { "note.txt" };
        let path = fixture.file("note.txt");
        let info = get(
            &app,
            &route(
                directory,
                &uid,
                reference,
                &[
                    ("mode", "info"),
                    ("path", path.to_str().unwrap()),
                    ("offset", "invalid"),
                    ("raw", "1"),
                    ("extra", "ignored"),
                ],
            ),
        )
        .await;
        assert_eq!(info.status(), StatusCode::OK);
        assert_eq!(value(info).await["text"], "synthetic safe text");
        let download = get(
            &app,
            &route(
                directory,
                &uid,
                reference,
                &[
                    ("mode", "info"),
                    ("path", path.to_str().unwrap()),
                    ("download", "1"),
                    ("offset", "-1"),
                ],
            ),
        )
        .await;
        assert_eq!(download.status(), StatusCode::OK);
        assert!(
            download.headers()["content-disposition"]
                .to_str()
                .unwrap()
                .starts_with("attachment;")
        );
        assert_eq!(
            to_bytes(download.into_body(), 100).await.unwrap(),
            b"synthetic safe text".as_slice()
        );
    }
    let raw = get(
        &app,
        &route(
            false,
            &uid,
            "note.txt",
            &[
                ("mode", "unrecognized"),
                ("path", "\0ignored"),
                ("offset", "invalid"),
                ("raw", "yes"),
                ("download", "yes"),
            ],
        ),
    )
    .await;
    assert_eq!(raw.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(raw.into_body(), 100).await.unwrap(),
        b"synthetic safe text".as_slice()
    );
    #[cfg(unix)]
    {
        let tab = fixture.file("nested/tab\tname.txt");
        let info = get(
            &app,
            &route(
                true,
                &uid,
                "./nested",
                &[("mode", "info"), ("path", tab.to_str().unwrap())],
            ),
        )
        .await;
        assert_eq!(info.status(), StatusCode::OK);
        assert_eq!(value(info).await["text"], "tab text");
    }
    for limit in ["501", "0", "invalid", "-1"] {
        let listing = get(
            &app,
            &route(
                true,
                &uid,
                "./nested",
                &[
                    ("mode", "unrecognized"),
                    ("raw", "1"),
                    ("hidden", "yes"),
                    ("limit", limit),
                    ("extra", "ignored"),
                ],
            ),
        )
        .await;
        assert_eq!(listing.status(), StatusCode::OK);
        let expected = if cfg!(unix) { 3 } else { 2 };
        assert_eq!(
            value(listing).await["entries"].as_array().unwrap().len(),
            expected
        );
    }
    let listing = value(
        get(
            &app,
            &route(true, &uid, "./nested", &[("hidden", "0"), ("limit", "1")]),
        )
        .await,
    )
    .await;
    assert_eq!(listing["entries"].as_array().unwrap().len(), 1);
    assert_eq!(listing["total"], if cfg!(unix) { 2 } else { 1 });
    for offset in [
        "",
        "0",
        "+0",
        "-0",
        " 0 ",
        "1_0",
        "١٠",
        "999999999999999999999999999999999999999",
    ] {
        let listing = get(&app, &route(true, &uid, "./nested", &[("offset", offset)])).await;
        assert_eq!(listing.status(), StatusCode::OK, "offset={offset:?}");
        let listing = value(listing).await;
        assert_eq!(
            listing["entries"].as_array().unwrap().len(),
            if offset.contains('1') || offset.contains('١') || offset.contains('9') {
                0
            } else {
                if cfg!(unix) { 3 } else { 2 }
            }
        );
    }
    for offset in ["-1", "invalid", "_1", "1_", "1__0", " ", "²"] {
        assert_eq!(
            get(&app, &route(true, &uid, "./nested", &[("offset", offset)]))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "offset={offset:?}"
        );
    }
    let jobs = get(
        &app,
        &route(
            true,
            &uid,
            "./nested",
            &[
                ("mode", "jobs"),
                ("path", "\0ignored"),
                ("offset", "invalid"),
                ("download", "1"),
            ],
        ),
    )
    .await;
    assert_eq!(jobs.status(), StatusCode::OK);
    assert_eq!(value(jobs).await["jobs"], json!([]));

    let upload = request(&app, "POST", "/api/session/files/action", &[("Content-Type", "application/json")], Body::from(json!({"uid":uid,"ref":"./nested","action":"upload","destination":fixture.file("nested"),"name":"query.bin","size":4}).to_string())).await;
    assert_eq!(upload.status(), StatusCode::OK);
    let upload = value(upload).await;
    let job = upload["job"]["id"].as_str().unwrap();
    let chunk = request(
        &app,
        "POST",
        &format!(
            "/api/session/files/upload?uid={}&ref={}&job={}&offset=0&extra=ignored",
            encode(&uid),
            encode("./nested"),
            encode(job)
        ),
        &[("Content-Type", "application/octet-stream")],
        Body::from("test"),
    )
    .await;
    assert_eq!(chunk.status(), StatusCode::OK);
    assert_eq!(fs::read(fixture.file("nested/query.bin")).unwrap(), b"test");
}

#[cfg(unix)]
#[tokio::test]
async fn granted_browser_paths_accept_literal_colon_directories() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.file("nested/colon:")).unwrap();
    fs::write(fixture.file("nested/colon:/child.txt"), "colon text").unwrap();
    let mut config = fixture.config();
    config.file_write_roots = vec![fixture.file("")];
    let app = sessiondock::app(config).unwrap();
    let uid = uid_for(&app, "parent").await;
    let directory = format!("{}/colon://", fixture.file("nested").display());
    let path = format!("{directory}child.txt");
    let listing = get(
        &app,
        &route(true, &uid, "./nested", &[("path", &directory)]),
    )
    .await;
    assert_eq!(listing.status(), StatusCode::OK);
    assert_eq!(value(listing).await["entries"][0]["name"], "child.txt");
    let info = get(
        &app,
        &route(true, &uid, "./nested", &[("mode", "info"), ("path", &path)]),
    )
    .await;
    assert_eq!(info.status(), StatusCode::OK);
    assert_eq!(value(info).await["text"], "colon text");
    let action = request(&app, "POST", "/api/session/files/action", &[("Content-Type", "application/json")], Body::from(json!({"uid":uid,"ref":"./nested","action":"mkdir","destination":directory,"name":"created"}).to_string())).await;
    assert_eq!(action.status(), StatusCode::OK);
    assert!(fixture.file("nested/colon:/created").is_dir());
    assert_eq!(
        get(
            &app,
            &route(
                true,
                &uid,
                "unmentioned-directory",
                &[("path", &path), ("mode", "info")]
            )
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}
