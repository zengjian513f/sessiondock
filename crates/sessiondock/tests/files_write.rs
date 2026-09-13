//! Synthetic-only HTTP tests for write-side file operations under explicit
//! write roots. No native homes, CLIs or production files are consulted.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use sessiondock::{
    config::Config,
    files::{FILE_TRASH_DIR, WriteLimits},
    sessions::SessionRoots,
};
use std::{
    fs,
    path::{Path, PathBuf},
};
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
    files: PathBuf,
    write: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let files = root.join("files");
        let write = files.join("w");
        for directory in ["native", "files/readonly", "files/w/sub"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        // The metadata store requires a private directory.
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join("state"))
                .unwrap();
        }
        #[cfg(not(unix))]
        fs::create_dir(root.join("state")).unwrap();
        fs::write(files.join("readonly/keep.txt"), b"read only").unwrap();
        fs::write(write.join("existing.txt"), b"existing").unwrap();
        let meta = json!({"type":"session_meta","timestamp":"2026-09-12T00:00:00Z","payload":{"id":"parent","session_id":"parent","cwd":files,"thread_source":"user"}});
        let prompt = message(&format!(
            "`{}/` `{}/` `{}/`",
            files.display(),
            write.display(),
            files.join("readonly").display()
        ));
        write_records(&root.join("native/parent.jsonl"), &[meta, prompt]);
        let other = json!({"type":"session_meta","timestamp":"2026-09-12T00:00:00Z","payload":{"id":"other","session_id":"other","cwd":files,"thread_source":"user"}});
        write_records(
            &root.join("native/other.jsonl"),
            &[other, message(&format!("`{}/`", write.display()))],
        );
        Self {
            _temp: temp,
            root,
            files,
            write,
        }
    }
    fn config(&self, write: bool, limits: WriteLimits, state: bool) -> Config {
        Config {
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            roots: SessionRoots {
                codex: Some(self.root.join("native")),
                ..Default::default()
            },
            file_roots: vec![self.files.clone()],
            file_write_roots: if write {
                vec![self.write.clone()]
            } else {
                vec![]
            },
            file_write_limits: limits,
            state_dir: state.then(|| self.root.join("state")),
            ..Default::default()
        }
    }
    fn app(&self) -> Router {
        sessiondock::app(self.config(true, WriteLimits::default(), true)).unwrap()
    }
    fn w(&self, name: &str) -> String {
        wire(&self.write.join(name))
    }
    fn anchor(&self) -> String {
        format!("{}/", self.write.display())
    }
    fn trash_dir(&self) -> PathBuf {
        self.root.join("state").join(FILE_TRASH_DIR)
    }
    fn trash_files(&self) -> Vec<PathBuf> {
        let trash = self.trash_dir();
        let mut found = vec![];
        if let Ok(items) = fs::read_dir(trash) {
            for item in items {
                for entry in fs::read_dir(item.unwrap().path()).unwrap() {
                    let path = entry.unwrap().path();
                    if path.file_name().unwrap() != "manifest.json" {
                        found.push(path);
                    }
                }
            }
        }
        found
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
async fn value(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}
async fn uid(app: &Router, sid: &str) -> String {
    let listing = value(request(app, "GET", "/api/sessions", &[], Body::empty()).await).await;
    listing["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sid"] == sid)
        .unwrap_or_else(|| panic!("missing synthetic SID {sid}: {listing}"))["uid"]
        .as_str()
        .unwrap()
        .to_owned()
}
async fn action(app: &Router, uid: &str, anchor: &str, mut body: Value) -> (StatusCode, Value) {
    body["uid"] = json!(uid);
    body["ref"] = json!(anchor);
    let response = request(
        app,
        "POST",
        "/api/session/files/action",
        &[("Content-Type", "application/json")],
        Body::from(body.to_string()),
    )
    .await;
    let status = response.status();
    (status, value(response).await)
}
async fn chunk(
    app: &Router,
    uid: &str,
    anchor: &str,
    job: &str,
    offset: u64,
    data: &[u8],
) -> (StatusCode, Value) {
    let response = request(
        app,
        "POST",
        &format!(
            "/api/session/files/upload?uid={}&ref={}&job={}&offset={offset}",
            encode(uid),
            encode(anchor),
            encode(job)
        ),
        &[("Content-Type", "application/octet-stream")],
        Body::from(data.to_vec()),
    )
    .await;
    let status = response.status();
    (status, value(response).await)
}
async fn listing(app: &Router, uid: &str, anchor: &str, extra: &str) -> Value {
    value(
        request(
            app,
            "GET",
            &format!(
                "/api/session/files?uid={}&ref={}{extra}",
                encode(uid),
                encode(anchor)
            ),
            &[],
            Body::empty(),
        )
        .await,
    )
    .await
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
async fn without_write_roots_everything_stays_501_and_read_roots_are_not_writable() {
    let fixture = Fixture::new();
    let app = sessiondock::app(fixture.config(false, WriteLimits::default(), false)).unwrap();
    let meta = value(request(&app, "GET", "/api/meta", &[], Body::empty()).await).await;
    assert_eq!(meta["capabilities"]["files_jobs"], false);
    assert_eq!(meta["capabilities"]["files_write"], false);
    let uid = uid(&app, "parent").await;
    for (path, content_type) in [
        ("/api/session/files/action", "application/json"),
        ("/api/session/files/upload", "application/octet-stream"),
        ("/api/session/attachment", "application/json"),
    ] {
        let response = request(
            &app,
            "POST",
            path,
            &[("Content-Type", content_type)],
            Body::from("{}"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED, "{path}");
        assert_eq!(value(response).await["code"], "files_jobs_disabled");
    }
    let jobs = request(
        &app,
        "GET",
        &format!(
            "/api/session/files?uid={}&ref={}&mode=jobs",
            encode(&uid),
            encode(&fixture.anchor())
        ),
        &[],
        Body::empty(),
    )
    .await;
    assert_eq!(jobs.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(value(jobs).await["code"], "file_mode_not_implemented");
    assert_eq!(
        listing(&app, &uid, &fixture.anchor(), "").await["writable"],
        false
    );
    // Configured roots enable compatibility; they do not create a directory jail.
    let mut config = fixture.config(true, WriteLimits::default(), false);
    for roots in [
        vec![fixture.root.join("state")],
        vec![fixture.root.join("native")],
        vec![fixture.write.clone(), fixture.write.join("sub")],
    ] {
        config.file_write_roots = roots;
        assert!(config.validate().is_ok());
    }
}

#[tokio::test]
async fn chunked_upload_lists_completes_records_and_is_scope_bound() {
    let fixture = Fixture::new();
    let native_before = snapshot(&fixture.root.join("native"));
    let readonly_before = snapshot(&fixture.files.join("readonly"));
    let app = sessiondock::app(fixture.config(
        true,
        WriteLimits {
            max_chunk_bytes: 8,
            max_job_bytes: 64,
        },
        true,
    ))
    .unwrap();
    let meta = value(request(&app, "GET", "/api/meta", &[], Body::empty()).await).await;
    assert_eq!(meta["capabilities"]["files_jobs"], true);
    assert_eq!(meta["capabilities"]["files_write"]["chunk_bytes"], 8);
    assert_eq!(meta["capabilities"]["files_write"]["delete"], "trash");
    let uid = uid(&app, "parent").await;
    let _other = self::uid(&app, "other").await;
    let anchor = fixture.anchor();
    assert_eq!(listing(&app, &uid, &anchor, "").await["writable"], true);
    assert_eq!(
        listing(
            &app,
            &uid,
            &format!("{}/", fixture.files.join("readonly").display()),
            ""
        )
        .await["writable"],
        true
    );
    let data = b"0123456789abcdef";
    use sha2::Digest;
    let digest = format!("{:x}", sha2::Sha256::digest(data));
    let (status, body) = action(&app, &uid, &anchor, json!({"action":"upload","destination":fixture.w(""),"name":"upload.bin","size":16,"sha256":digest,"conflict":"error"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let job = body["job"]["id"].as_str().unwrap().to_owned();
    assert_eq!(body["job"]["state"], "uploading");
    // Wrong content type and oversized chunks are explicit.
    let wrong = request(
        &app,
        "POST",
        &format!(
            "/api/session/files/upload?uid={}&ref={}&job={job}&offset=0",
            encode(&uid),
            encode(&anchor)
        ),
        &[("Content-Type", "text/plain")],
        Body::from("01234567"),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
    let (status, _) = chunk(&app, &uid, &anchor, &job, 0, b"012345678").await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, body) = action(
        &app,
        &uid,
        &anchor,
        json!({"action":"upload","destination":fixture.w(""),"name":"too-big","size":65}),
    )
    .await;
    assert_eq!(
        (status, body["limit"].as_u64()),
        (StatusCode::PAYLOAD_TOO_LARGE, Some(64))
    );
    let (_, one) = action(
        &app,
        &uid,
        &anchor,
        json!({"action":"upload","destination":fixture.w(""),"name":"one","size":9}),
    )
    .await;
    let (_, two) = action(
        &app,
        &uid,
        &anchor,
        json!({"action":"upload","destination":fixture.w(""),"name":"two","size":9}),
    )
    .await;
    let (status, body) = action(
        &app,
        &uid,
        &anchor,
        json!({"action":"upload","destination":fixture.w(""),"name":"three","size":9}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let third_id = body["job"]["id"].clone();
    let (status, body) = action(
        &app,
        &uid,
        &anchor,
        json!({"action":"cancel","job":one["job"]["id"]}),
    )
    .await;
    assert_eq!(
        (status, body["job"]["state"].as_str()),
        (StatusCode::OK, Some("cancelled"))
    );
    let two_id = two["job"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        chunk(&app, &uid, &anchor, &two_id, 0, b"12345678").await.0,
        StatusCode::OK
    );
    let (status, _) = chunk(&app, &uid, &anchor, &two_id, 8, b"9").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fs::read(fixture.write.join("two")).unwrap(), b"123456789");
    assert_eq!(
        action(
            &app,
            &uid,
            &anchor,
            json!({"action":"cancel", "job":third_id})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(snapshot(&fixture.root.join("native")), native_before);
    assert_eq!(snapshot(&fixture.files.join("readonly")), readonly_before);
}

#[tokio::test]
async fn actions_never_overwrite_delete_to_trash_and_report_partial_results() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let uid = uid(&app, "parent").await;
    let anchor = fixture.anchor();
    let (status, body) = action(
        &app,
        &uid,
        &anchor,
        json!({"action":"mkdir","destination":fixture.w(""),"name":"created"}),
    )
    .await;
    assert_eq!(
        (status, body["job"]["state"].as_str()),
        (StatusCode::OK, Some("completed")),
        "{body}"
    );
    assert!(fixture.write.join("created").is_dir());
    let (status, body) = action(
        &app,
        &uid,
        &anchor,
        json!({"action":"mkdir","destination":fixture.w(""),"name":"existing.txt"}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("file_exists"))
    );
    assert_eq!(body["job"]["state"], "failed");
    let (status, body) = action(
        &app,
        &uid,
        &anchor,
        json!({"action":"upload","destination":fixture.w(""),"name":"existing.txt","size":1}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("file_exists"))
    );
    let (status, _) = action(
        &app,
        &uid,
        &anchor,
        json!({"action":"new-file","destination":fixture.w("created"),"name":"note.txt"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = action(
        &app,
        &uid,
        &anchor,
        json!({"action":"rename","paths":[fixture.w("created/note.txt")],"name":"existing.txt"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "different directory: {body}");
    let (status, body) = action(&app, &uid, &anchor, json!({"action":"move","paths":[fixture.w("created/existing.txt")],"destination":fixture.w("")})).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("file_exists"))
    );
    assert_eq!(
        fs::read(fixture.write.join("existing.txt")).unwrap(),
        b"existing"
    );
    let (status, body) = action(&app, &uid, &anchor, json!({"action":"move","paths":[fixture.w("created/existing.txt")],"destination":fixture.w(""),"conflict":"keep"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(fixture.write.join("existing (1).txt").is_file());
    // Delete moves into the private state-directory trash, never into the
    // project tree; partial batches keep going.
    let (status, body) = action(&app, &uid, &anchor, json!({"action":"delete","paths":[fixture.w("existing (1).txt"), fixture.w("missing"), fixture.w("created")]})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["job"]["state"], "failed");
    assert_eq!(body["job"]["completed"].as_array().unwrap().len(), 2);
    assert_eq!(body["job"]["errors"][0]["path"], fixture.w("missing"));
    assert_eq!(body["job"]["errors"][0]["status"], 404);
    let trashed = fixture.trash_files();
    assert_eq!(trashed.len(), 2, "{trashed:?}");
    assert!(
        trashed
            .iter()
            .all(|path| path.starts_with(fixture.trash_dir()))
    );
    assert!(fs::read_dir(&fixture.write).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with("-trash")
    }));
    assert!(
        trashed
            .iter()
            .any(|path| path.is_dir() && path.file_name().unwrap() == "created")
    );
    assert!(
        fixture
            .write
            .join("existing (1).txt")
            .symlink_metadata()
            .is_err()
    );
    // The task panel sees these finished jobs for the same scope only.
    let jobs = listing(&app, &uid, &anchor, "&mode=jobs").await;
    assert!(jobs["jobs"].as_array().unwrap().len() >= 5);
    assert!(
        jobs["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|job| job["action"] == "delete" && job["state"] == "failed")
    );
    // Unimplemented actions and replace policy are explicit, not fake successes.
    let (status, body) = action(
        &app,
        &uid,
        &anchor,
        json!({"action":"copy","paths":[fixture.w("existing.txt")],"destination":fixture.w("sub")}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (
            StatusCode::NOT_IMPLEMENTED,
            Some("file_action_not_implemented")
        )
    );
    let (status, body) = action(
        &app,
        &uid,
        &anchor,
        json!({"action":"mkdir","destination":fixture.w(""),"name":"x","conflict":"replace"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let bad = request(&app, "POST", "/api/session/files/action", &[("Content-Type", "application/json")], Body::from(json!({"uid":uid,"ref":anchor,"action":"mkdir","destination":fixture.w(""),"name":"unknown-fields-ignored","extra":1}).to_string())).await;
    assert_eq!(
        bad.status(),
        StatusCode::OK,
        "Python ignores unrelated request fields"
    );
}

#[tokio::test]
async fn paths_names_symlinks_and_replaced_roots_fail_closed() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let uid = uid(&app, "parent").await;
    let anchor = fixture.anchor();
    for (body, code) in [
        (
            json!({"action":"mkdir","destination":fixture.w(""),"name":".."}),
            "file_name_invalid",
        ),
        (
            json!({"action":"mkdir","destination":fixture.w(""),"name":"a/b"}),
            "file_name_invalid",
        ),
        (
            json!({"action":"mkdir","destination":"relative/dir","name":"x"}),
            "file_absolute_path_required",
        ),
        (
            json!({"action":"delete","paths":["../escape"]}),
            "file_absolute_path_required",
        ),
    ] {
        let (status, body) = action(&app, &uid, &anchor, body).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some(code))
        );
    }
    for destination in [fixture.files.join("readonly"), fixture.write.join("sub/..")] {
        let (status, body) = action(
            &app,
            &uid,
            &anchor,
            json!({"action":"mkdir","destination":destination,"name":"operator-created"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), fixture.write.join("escape")).unwrap();
        let (status, body) = action(
            &app,
            &uid,
            &anchor,
            json!({"action":"mkdir","destination":fixture.w("escape"),"name":"through-link"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(outside.path().join("through-link").is_dir());
        let (status, body) = action(
            &app,
            &uid,
            &anchor,
            json!({"action":"delete","paths":[fixture.w("escape")]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(outside.path().join("through-link").is_dir());
    }
    // Windows prevents renaming an open directory handle; Unix exercises the
    // replacement race and recovery path.
    #[cfg(not(windows))]
    {
        let (status, body) = action(
            &app,
            &uid,
            &anchor,
            json!({"action":"upload","destination":fixture.w(""),"name":"resumed.bin","size":4}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let job = body["job"]["id"].as_str().unwrap().to_owned();
        assert_eq!(
            chunk(&app, &uid, &anchor, &job, 0, b"ab").await.0,
            StatusCode::OK
        );
        let parked = fixture.files.join("w-parked");
        fs::rename(&fixture.write, &parked).unwrap();
        fs::create_dir(&fixture.write).unwrap();
        let (status, body) = chunk(&app, &uid, &anchor, &job, 2, b"cd").await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::CONFLICT, Some("file_changed"))
        );
        let (status, body) = action(
            &app,
            &uid,
            &anchor,
            json!({"action":"mkdir","destination":fixture.w(""),"name":"x"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(fixture.write.join("x").is_dir());
        assert!(!fixture.write.join("resumed.bin").exists());
        fs::remove_dir(fixture.write.join("x")).unwrap();
        fs::remove_dir(&fixture.write).unwrap();
        fs::rename(&parked, &fixture.write).unwrap();
        let (status, body) = chunk(&app, &uid, &anchor, &job, 2, b"cd").await;
        assert_eq!(
            (status, body["job"]["state"].as_str()),
            (StatusCode::OK, Some("completed"))
        );
        assert_eq!(
            fs::read(fixture.write.join("resumed.bin")).unwrap(),
            b"abcd"
        );
    }
}

#[tokio::test]
async fn composer_raw_attachments_use_native_cwd_and_keep_json_files_as_bytes() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let uid = uid(&app, "parent").await;
    let native_before = snapshot(&fixture.root.join("native"));
    let payload = vec![b'x'; 20 * 1024]; // larger than the old JSON completion limit
    let uri = format!(
        "/api/session/attachment?uid={}&name={}&id=1",
        encode(&uid),
        encode("截图 a.png")
    );
    for (bytes, reused, name) in [
        (payload.as_slice(), false, "截图 a.png"),
        (payload.as_slice(), true, "截图 a.png"),
        (b"different".as_slice(), false, "截图 a__1.png"),
    ] {
        let response = request(
            &app,
            "POST",
            &uri,
            &[("Content-Type", "image/png")],
            Body::from(bytes.to_vec()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let result = value(response).await;
        let relative = Path::new("sessiondock_attachments").join("1").join(name);
        assert_eq!(result["relative_path"], relative.to_str().unwrap());
        assert_eq!(
            result["path_style"],
            if cfg!(windows) { "windows" } else { "posix" }
        );
        assert_eq!(result["kind"], "image");
        assert_eq!(result["reused"], reused);
        assert_eq!(result["recorded"], true);
        assert_eq!(fs::read(fixture.files.join(relative)).unwrap(), bytes);
    }
    let response = request(
        &app,
        "POST",
        &format!(
            "/api/session/attachment?uid={}&name=data.json",
            encode(&uid)
        ),
        &[("Content-Type", "application/json")],
        Body::from("{\"test\":true}"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let result = value(response).await;
    assert_eq!(result["attachment_id"], "2");
    assert_eq!(
        fs::read(result["path"].as_str().unwrap()).unwrap(),
        b"{\"test\":true}"
    );
    let metadata: Value = serde_json::from_slice(
        &fs::read(fixture.root.join("state/session-metadata.json")).unwrap(),
    )
    .unwrap();
    assert!(
        metadata["sessions"][&uid]["attachments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["path"] == result["path"])
    );
    assert_eq!(snapshot(&fixture.root.join("native")), native_before);
}

#[tokio::test]
async fn composer_raw_attachments_reject_invalid_scope_path_size_and_disabled_writes() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let uid = uid(&app, "parent").await;
    let before = snapshot(&fixture.files);
    for (query, body, status) in [
        (
            format!("uid={uid}&name=a.txt&id=../outside"),
            "x",
            StatusCode::BAD_REQUEST,
        ),
        (
            "uid=codex:unknown&name=a.txt".into(),
            "x",
            StatusCode::NOT_FOUND,
        ),
        ("uid=&name=a.txt".into(), "x", StatusCode::BAD_REQUEST),
        (format!("uid={uid}&name=a.txt"), "", StatusCode::BAD_REQUEST),
    ] {
        let response = request(
            &app,
            "POST",
            &format!("/api/session/attachment?{query}"),
            &[("Content-Type", "text/plain")],
            Body::from(body),
        )
        .await;
        let actual = response.status();
        assert_eq!(actual, status, "{}", value(response).await);
    }
    let uri = format!("/api/session/attachment?uid={uid}&name=a.txt");
    let response = request(
        &app,
        "POST",
        &uri,
        &[
            ("Content-Type", "text/plain"),
            ("Content-Length", "536870913"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let disabled = sessiondock::app(fixture.config(false, WriteLimits::default(), false)).unwrap();
    let response = request(
        &disabled,
        "POST",
        &uri,
        &[("Content-Type", "text/plain")],
        Body::from("x"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(snapshot(&fixture.files), before);
    assert!(!fixture.files.join("sessiondock_attachments").exists());
    let response = request(
        &app,
        "POST",
        &format!("/api/session/attachment?uid={uid}&name=a.txt&cwd=/tmp"),
        &[("Content-Type", "text/plain")],
        Body::from("x"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let result = value(response).await;
    assert!(
        Path::new(result["path"].as_str().unwrap())
            .canonicalize()
            .unwrap()
            .starts_with(&fixture.files)
    );
}
