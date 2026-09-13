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
    files::{FILE_TRASH_DIR, UPLOAD_DIR, WriteLimits},
    sessions::SessionRoots,
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tower::ServiceExt;

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
        self.write.join(name).to_str().unwrap().to_owned()
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
    // Write roots outside every read root or overlapping private paths fail closed.
    let mut config = fixture.config(true, WriteLimits::default(), false);
    config.file_write_roots = vec![fixture.root.join("state")];
    assert!(config.validate().is_err());
    config.file_write_roots = vec![fixture.root.join("native")];
    assert!(config.validate().is_err());
    let outside = fixture.root.join("elsewhere");
    fs::create_dir(&outside).unwrap();
    config.file_write_roots = vec![outside];
    assert!(config.validate().is_err(), "write root outside read roots");
    config.file_write_roots = vec![fixture.write.clone(), fixture.write.join("sub")];
    assert!(config.validate().is_err(), "nested write roots");
    config.file_write_roots = vec![fixture.write.clone()];
    config.file_roots = vec![];
    assert!(config.validate().is_err(), "write roots need read roots");
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
            max_jobs: 2,
            expiry: Duration::from_millis(200),
        },
        true,
    ))
    .unwrap();
    let meta = value(request(&app, "GET", "/api/meta", &[], Body::empty()).await).await;
    assert_eq!(meta["capabilities"]["files_jobs"], true);
    assert_eq!(meta["capabilities"]["files_write"]["chunk_bytes"], 8);
    assert_eq!(meta["capabilities"]["files_write"]["delete"], "trash");
    let uid = uid(&app, "parent").await;
    let other = self::uid(&app, "other").await;
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
        false
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
    let (status, body) = chunk(&app, &uid, &anchor, &job, 0, b"012345678").await;
    assert_eq!(
        (status, body["code"].as_str(), body["limit"].as_u64()),
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            Some("file_upload_chunk_too_large"),
            Some(8)
        )
    );
    let (status, body) = chunk(&app, &uid, &anchor, &job, 0, &data[..8]).await;
    assert_eq!(
        (status, body["job"]["bytes"].as_u64()),
        (StatusCode::OK, Some(8))
    );
    let (status, body) = chunk(&app, &uid, &anchor, &job, 0, &data[..8]).await;
    assert_eq!(
        (status, body["job"]["bytes"].as_u64()),
        (StatusCode::OK, Some(8)),
        "replay is idempotent"
    );
    let (status, body) = chunk(&app, &uid, &anchor, &job, 4, &data[4..12]).await;
    assert_eq!(
        (status, body["code"].as_str(), body["expected"].as_u64()),
        (StatusCode::CONFLICT, Some("file_upload_offset"), Some(8))
    );
    let (status, body) = chunk(&app, &uid, &anchor, &job, 16, b"").await;
    assert_eq!(
        (status, body["expected"].as_u64()),
        (StatusCode::CONFLICT, Some(8))
    );
    // Another session scope cannot see or continue the job.
    let (status, body) = chunk(&app, &other, &anchor, &job, 8, &data[8..]).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("file_job_unknown"))
    );
    assert_eq!(
        listing(&app, &other, &anchor, "&mode=jobs").await["jobs"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let jobs = listing(&app, &uid, &anchor, "&mode=jobs").await;
    assert_eq!(jobs["jobs"][0]["id"], job);
    assert_eq!(jobs["jobs"][0]["state"], "uploading");
    assert!(jobs["jobs"][0].get("scope").is_none());
    let (status, body) = chunk(&app, &uid, &anchor, &job, 8, &data[8..]).await;
    assert_eq!(
        (status, body["job"]["state"].as_str()),
        (StatusCode::OK, Some("completed")),
        "{body}"
    );
    assert_eq!(body["job"]["path"], fixture.w("upload.bin"));
    assert_eq!(fs::read(fixture.write.join("upload.bin")).unwrap(), data);
    assert!(
        fixture
            .write
            .join(UPLOAD_DIR)
            .read_dir()
            .unwrap()
            .next()
            .is_none()
    );
    let entries = listing(&app, &uid, &anchor, "").await;
    assert!(
        entries["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "upload.bin")
    );
    // Attachment recording with a metadata store.
    let response = request(
        &app,
        "POST",
        "/api/session/attachment",
        &[("Content-Type", "application/json")],
        Body::from(json!({"uid":uid,"ref":anchor,"job":job}).to_string()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let recorded = value(response).await;
    assert_eq!(recorded["recorded"], true);
    assert_eq!(recorded["path"], fixture.w("upload.bin"));
    assert_eq!(recorded["size"], 16);
    assert_eq!(recorded["sha256"], digest);
    assert_eq!(recorded["kind"], "file");
    let metadata: Value = serde_json::from_slice(
        &fs::read(fixture.root.join("state/session-metadata.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        metadata["sessions"][&uid]["attachments"][0]["path"],
        fixture.w("upload.bin")
    );
    let response = request(
        &app,
        "POST",
        "/api/session/attachment",
        &[("Content-Type", "application/json")],
        Body::from(json!({"uid":other,"ref":anchor,"job":job}).to_string()),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "attachment is scope bound"
    );
    // Concurrency and size limits name their values.
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
    assert_eq!(
        (status, body["code"].as_str(), body["limit"].as_u64()),
        (
            StatusCode::TOO_MANY_REQUESTS,
            Some("file_jobs_limit"),
            Some(2)
        )
    );
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
    // Expiry: idle jobs vanish with their staging bytes and answer 410.
    let two_id = two["job"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        chunk(&app, &uid, &anchor, &two_id, 0, b"12345678").await.0,
        StatusCode::OK
    );
    tokio::time::sleep(Duration::from_millis(350)).await;
    let (status, body) = chunk(&app, &uid, &anchor, &two_id, 8, b"9").await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::GONE, Some("file_job_expired"))
    );
    assert!(
        fixture
            .write
            .join(UPLOAD_DIR)
            .read_dir()
            .unwrap()
            .next()
            .is_none()
    );
    assert!(fixture.write.join("two").symlink_metadata().is_err());
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
    assert_eq!(
        (status, body["code"].as_str()),
        (
            StatusCode::BAD_REQUEST,
            Some("file_conflict_replace_unsupported")
        )
    );
    let bad = request(&app, "POST", "/api/session/files/action", &[("Content-Type", "application/json")], Body::from(json!({"uid":uid,"ref":anchor,"action":"mkdir","destination":fixture.w(""),"name":"x","extra":1}).to_string())).await;
    assert_eq!(
        bad.status(),
        StatusCode::BAD_REQUEST,
        "unknown fields are refused"
    );
}

#[tokio::test]
async fn paths_names_symlinks_and_replaced_roots_fail_closed() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let uid = uid(&app, "parent").await;
    let anchor = fixture.anchor();
    let before = snapshot(&fixture.files);
    for (body, status, code) in [
        (
            json!({"action":"mkdir","destination":fixture.w(""),"name":".."}),
            StatusCode::BAD_REQUEST,
            "file_name_invalid",
        ),
        (
            json!({"action":"mkdir","destination":fixture.w(""),"name":"a/b"}),
            StatusCode::BAD_REQUEST,
            "file_name_invalid",
        ),
        (
            json!({"action":"mkdir","destination":fixture.w(""),"name":"/etc/passwd"}),
            StatusCode::BAD_REQUEST,
            "file_name_invalid",
        ),
        (
            json!({"action":"mkdir","destination":format!("{}/..", fixture.w("sub")),"name":"x"}),
            StatusCode::BAD_REQUEST,
            "file_path_invalid",
        ),
        (
            json!({"action":"mkdir","destination":"relative/dir","name":"x"}),
            StatusCode::BAD_REQUEST,
            "file_absolute_path_required",
        ),
        (
            json!({"action":"delete","paths":["../escape"]}),
            StatusCode::BAD_REQUEST,
            "file_path_invalid",
        ),
        (
            json!({"action":"mkdir","destination":fixture.files.join("readonly").to_str().unwrap(),"name":"x"}),
            StatusCode::FORBIDDEN,
            "file_outside_write_roots",
        ),
        (
            json!({"action":"mkdir","destination":"/","name":"x"}),
            StatusCode::FORBIDDEN,
            "file_outside_anchor_root",
        ),
        (
            json!({"action":"delete","paths":[fixture.w("")]}),
            StatusCode::FORBIDDEN,
            "file_root_immutable",
        ),
        (
            json!({"action":"mkdir","destination":fixture.w(""),"name":".sessiondock-upload"}),
            StatusCode::BAD_REQUEST,
            "file_reserved_name",
        ),
    ] {
        let (got, response) = action(&app, &uid, &anchor, body.clone()).await;
        assert_eq!(
            (got, response["code"].as_str()),
            (status, Some(code)),
            "{body} → {response}"
        );
    }
    #[cfg(not(windows))]
    {
        let (status, body) = action(
            &app,
            &uid,
            &anchor,
            json!({"action":"mkdir","destination":fixture.w(""),"name":"a\\b"}),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some("file_foreign_path"))
        );
        let (status, body) = action(
            &app,
            &uid,
            &anchor,
            json!({"action":"mkdir","destination":"C:\\Users\\x","name":"a"}),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some("file_foreign_path"))
        );
    }
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), fixture.write.join("escape")).unwrap();
        let (status, body) = action(
            &app,
            &uid,
            &anchor,
            json!({"action":"mkdir","destination":fixture.w("escape"),"name":"pwned"}),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("file_symlink_forbidden"))
        );
        let (status, body) = action(
            &app,
            &uid,
            &anchor,
            json!({"action":"delete","paths":[fixture.w("escape")]}),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("file_symlink_forbidden"))
        );
        assert!(outside.path().read_dir().unwrap().next().is_none());
        fs::remove_file(fixture.write.join("escape")).unwrap();
    }
    assert_eq!(snapshot(&fixture.files), before);
    // Root replaced mid-job: the retained handle refuses, nothing lands in the
    // replacement, and the job resumes once the original root is back.
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
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("file_changed"))
    );
    assert!(fixture.write.read_dir().unwrap().next().is_none());
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
