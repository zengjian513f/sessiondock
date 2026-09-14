//! Synthetic authorized file media. No native homes, paid CLI, or remote I/O.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sessiondock::{config::Config, sessions::SessionRoots};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use tower::ServiceExt;

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAE0lEQVR4nGP8z8DAwMDAxIBMAQAUQAEF3SN5DgAAAABJRU5ErkJggg==";
const GREEN: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABCAIAAAB2XpiaAAAADUlEQVR4nGNk+M8ABwALDwEB+wO/9wAAAABJRU5ErkJggg==";
fn image(reference: impl Into<String>) -> Value {
    json!({"type":"image","source":{"type":"file","path":reference.into()}})
}
fn text(value: impl Into<String>) -> Value {
    json!({"type":"text","text":value.into()})
}
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    native: BTreeMap<PathBuf, Vec<u8>>,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        for dir in ["claude/project", "codex", "grok", "files/a", "files/b"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for name in [
            "structured.png",
            "uri.png",
            "markdown.png",
            "raw.png",
            "unknown.payload",
            "a/same.png",
            "b/same.png",
        ] {
            fs::write(root.join("files").join(name), STANDARD.decode(PNG).unwrap()).unwrap();
        }
        fs::write(root.join("outside.png"), STANDARD.decode(GREEN).unwrap()).unwrap();
        fs::write(
            root.join("claude/private.png"),
            STANDARD.decode(GREEN).unwrap(),
        )
        .unwrap();
        Self {
            _temp: temp,
            root,
            native: BTreeMap::new(),
        }
    }
    fn file(&self, name: &str) -> PathBuf {
        self.root.join("files").join(name)
    }
    fn write(&mut self, path: PathBuf, rows: Vec<Value>) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = rows
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>()
            .into_bytes();
        fs::write(&path, &bytes).unwrap();
        self.native.insert(path, bytes);
    }
    fn put(&mut self, source: &str, sid: &str, content: Vec<Value>) {
        let cwd = self.file("");
        match source {
            "claude"=>self.write(self.root.join("claude/project").join(format!("{sid}.jsonl")),vec![json!({"type":"user","sessionId":sid,"uuid":"u","parentUuid":null,"cwd":cwd,"message":{"role":"user","content":content}})]),
            "codex"=>self.write(self.root.join("codex").join(format!("{sid}.jsonl")),vec![json!({"type":"session_meta","payload":{"id":sid,"cwd":cwd}}),json!({"type":"response_item","payload":{"type":"message","role":"user","content":content}})]),
            _=>{
                let dir=self.root.join("grok/project").join(sid);fs::create_dir_all(&dir).unwrap();
                let bytes=json!({"info":{"id":sid,"cwd":cwd}}).to_string().into_bytes();
                fs::write(dir.join("summary.json"),&bytes).unwrap();self.native.insert(dir.join("summary.json"),bytes);
                self.write(dir.join("chat_history.jsonl"),vec![json!({"type":"user","prompt_index":1,"content":content})]);
            }
        }
    }
    fn app(&self, authorized: bool) -> Router {
        sessiondock::app(Config {
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            roots: SessionRoots {
                claude: Some(self.root.join("claude")),
                codex: Some(self.root.join("codex")),
                grok: Some(self.root.join("grok")),
            },
            file_roots: if authorized {
                vec![self.file("")]
            } else {
                vec![]
            },
            ..Default::default()
        })
        .unwrap()
    }
    fn unchanged(&self) {
        for (path, bytes) in &self.native {
            assert_eq!(&fs::read(path).unwrap(), bytes);
        }
    }
}
async fn get(app: &Router, uri: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("host", "127.0.0.1:8741")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), 8 << 20)
        .await
        .unwrap()
        .to_vec()
}
async fn value(response: Response) -> Value {
    serde_json::from_slice(&bytes(response).await).unwrap()
}
async fn uid(app: &Router, sid: &str) -> String {
    let response = value(get(app, "/api/sessions").await).await;
    response["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sid"] == sid)
        .unwrap_or_else(|| panic!("missing {sid}"))["uid"]
        .as_str()
        .unwrap()
        .into()
}
async fn messages(app: &Router, sid: &str, query: &str) -> Value {
    let response = get(
        app,
        &format!("/api/messages/{}{}", uid(app, sid).await, query),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "{sid}");
    value(response).await
}
fn media(value: &Value) -> Vec<&Value> {
    value["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| {
            row.get("media")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .collect()
}
fn no_src_error(item: &Value) {
    assert!(item.get("src").is_none(), "{item}");
    assert!(
        item["error"]["message"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "{item}"
    );
}
fn file_uri(path: &Path) -> String {
    format!("file://{}", path.to_str().unwrap())
}

#[tokio::test]
async fn every_provider_projects_files_with_or_without_configured_roots() {
    let mut fixture = Fixture::new();
    for source in ["claude", "codex", "grok"] {
        fixture.put(
            source,
            &format!("{source}-files"),
            vec![
                image(fixture.file("structured.png").to_string_lossy()),
                image(file_uri(&fixture.file("uri.png"))),
                image("unknown.payload"),
                text("File fixture text survives. ![markdown](markdown.png) ./raw.png"),
            ],
        );
    }
    for configured in [false, true] {
        let app = fixture.app(configured);
        for source in ["claude", "codex", "grok"] {
            let projected = messages(&app, &format!("{source}-files"), "").await;
            assert!(!projected.to_string().contains(PNG));
            let images = media(&projected);
            assert_eq!(images.len(), 5, "configured={configured}: {projected}");
            for image in images {
                let src = image["src"].as_str().unwrap_or_else(|| panic!("{image}"));
                assert!(src.starts_with("/api/media/") && src.len() == 43);
                assert_eq!(image["lazy"], true);
                for absent in ["mime", "width", "height"] {
                    assert!(image.get(absent).is_none());
                }
                let response = get(&app, src).await;
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(response.headers()["content-type"], "image/png");
                assert_eq!(response.headers()["x-content-type-options"], "nosniff");
                assert_eq!(response.headers()["cache-control"], "private, no-store");
                assert_eq!(bytes(response).await, STANDARD.decode(PNG).unwrap());
            }
        }
    }
    fixture.unchanged();
}

#[tokio::test]
async fn replacement_rejects_old_token_and_reprojection_returns_new_bytes_without_native_mutation()
{
    let mut fixture = Fixture::new();
    fixture.put("claude", "replace", vec![image("structured.png")]);
    let app = fixture.app(true);
    let first = messages(&app, "replace", "").await;
    let old = media(&first)[0]["src"].as_str().unwrap().to_owned();
    assert_eq!(get(&app, &old).await.status(), StatusCode::OK);
    let replacement = fixture.file("replacement.tmp");
    fs::write(&replacement, STANDARD.decode(GREEN).unwrap()).unwrap();
    fs::rename(replacement, fixture.file("structured.png")).unwrap();
    assert_eq!(get(&app, &old).await.status(), StatusCode::CONFLICT);
    let latest = messages(&app, "replace", "").await;
    let image = media(&latest)[0];
    let new = image["src"].as_str().unwrap();
    assert_ne!(old, new);
    assert_eq!(image["lazy"], true);
    assert!(image.get("width").is_none() && image.get("height").is_none());
    assert_eq!(
        bytes(get(&app, new).await).await,
        STANDARD.decode(GREEN).unwrap()
    );
    assert_eq!(get(&app, &old).await.status(), StatusCode::CONFLICT);
    fixture.unchanged();
}

#[tokio::test]
async fn cold_file_descriptor_is_version_bound_before_the_first_media_get() {
    let mut fixture = Fixture::new();
    fixture.put(
        "claude",
        "cold-replace",
        vec![
            text("Readable before the first image GET"),
            image("structured.png"),
        ],
    );
    let app = fixture.app(true);
    let history = messages(&app, "cold-replace", "").await;
    let image = media(&history)[0];
    assert_eq!(image["lazy"], true);
    assert!(image.get("width").is_none());
    let old = image["src"].as_str().unwrap().to_owned();
    // No GET has materialized this token. It must not silently bind whatever
    // happens to occupy the path when the first image request arrives.
    let replacement = fixture.file("cold-replacement.tmp");
    fs::write(&replacement, STANDARD.decode(GREEN).unwrap()).unwrap();
    fs::rename(replacement, fixture.file("structured.png")).unwrap();
    let stale = get(&app, &old).await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let error = value(stale).await;
    assert!(error.get("error").is_some());
    assert!(!error.to_string().contains(fixture.root.to_str().unwrap()));
    let latest = messages(&app, "cold-replace", "").await;
    assert!(
        latest
            .to_string()
            .contains("Readable before the first image GET")
    );
    let fresh = media(&latest)[0]["src"].as_str().unwrap();
    assert_ne!(old, fresh);
    let response = get(&app, fresh).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(bytes(response).await, STANDARD.decode(GREEN).unwrap());
    fixture.unchanged();
}

#[tokio::test]
async fn warm_file_get_reauthorizes_native_scope_without_an_intervening_history_read() {
    let mut fixture = Fixture::new();
    fixture.put("claude", "warm-revoke", vec![image("structured.png")]);
    let app = fixture.app(true);
    let initial = messages(&app, "warm-revoke", "").await;
    let old = media(&initial)[0]["src"].as_str().unwrap().to_owned();
    assert_eq!(
        bytes(get(&app, &old).await).await,
        STANDARD.decode(PNG).unwrap()
    );
    fixture.put(
        "claude",
        "warm-revoke",
        vec![text("Current native scope no longer grants an image")],
    );
    // Do not refresh /messages or /sessions first: a warm blob is not authority.
    let response = get(&app, &old).await;
    assert!(matches!(
        response.status(),
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
    let error = value(response).await;
    assert!(error.get("error").is_some());
    assert!(!error.to_string().contains(PNG));
    fixture.unchanged();
}

#[tokio::test]
async fn selected_local_images_and_remote_urls_are_projected_with_or_without_roots() {
    let mut fixture = Fixture::new();
    fixture.put(
        "claude",
        "outside",
        vec![
            image(fixture.root.join("outside.png").to_string_lossy()),
            text("Outside text remains"),
        ],
    );
    fixture.put(
        "claude",
        "native",
        vec![
            image(fixture.root.join("claude/private.png").to_string_lossy()),
            text("Native text remains"),
        ],
    );
    fixture.put(
        "claude",
        "remote",
        vec![image("https://media.example.invalid/never.png")],
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(fixture.file("structured.png"), fixture.file("linked.png"))
            .unwrap();
        fixture.put("claude", "symlink", vec![image("linked.png")]);
    }
    let app = fixture.app(true);
    let readable = [
        ("outside", GREEN),
        ("native", GREEN),
        #[cfg(unix)]
        ("symlink", PNG),
    ];
    // Python media.register_path resolves a selected image reference through
    // symlinks and checks the actual file; configured roots do not narrow it.
    for (sid, expected) in readable {
        let projected = messages(&app, sid, "").await;
        let images = media(&projected);
        assert_eq!(images.len(), 1);
        let src = images[0]["src"]
            .as_str()
            .unwrap_or_else(|| panic!("{projected}"));
        assert!(src.starts_with("/api/media/"));
        assert!(images[0].get("error").is_none(), "{projected}");
        let response = get(&app, src).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "image/png");
        assert_eq!(bytes(response).await, STANDARD.decode(expected).unwrap());
        if sid != "symlink" {
            assert!(projected.to_string().contains("text remains"));
        }
    }
    let projected = messages(&app, "remote", "").await;
    let images = media(&projected);
    assert_eq!(images.len(), 1, "{projected}");
    assert_eq!(images[0]["src"], "https://media.example.invalid/never.png");
    assert_eq!(images[0]["external"], true);
    fixture.unchanged();
}

#[tokio::test]
async fn selected_branch_and_agent_have_separate_media_and_retired_branch_tokens_fail_reauthorization()
 {
    let mut fixture = Fixture::new();
    let cwd = fixture.file("");
    let path = fixture.root.join("claude/project/branch.jsonl");
    let prefix = vec![
        json!({"type":"user","sessionId":"branch","uuid":"u","parentUuid":null,"cwd":cwd,"message":{"role":"user","content":[image("structured.png")]}}),
        json!({"type":"assistant","sessionId":"branch","uuid":"a","parentUuid":"u","message":{"role":"assistant","content":"Current branch"}}),
    ];
    let mut rows = prefix.clone();
    rows.push(json!({"type":"user","sessionId":"branch","uuid":"discarded","parentUuid":"a","message":{"role":"user","content":[image("raw.png")]}}));
    rows.push(json!({"type":"last-prompt","leafUuid":"a"}));
    fixture.write(path.clone(), rows);
    fixture.write(path.with_extension("").join("subagents/agent-worker.jsonl"),vec![json!({"type":"user","sessionId":"branch","uuid":"agent-u","parentUuid":null,"cwd":cwd,"isSidechain":true,"agentId":"worker","message":{"role":"user","content":[image("markdown.png")]}})]);
    let app = fixture.app(true);
    let main = messages(&app, "branch", "").await;
    assert_eq!(media(&main).len(), 1);
    let old = media(&main)[0]["src"].as_str().unwrap().to_owned();
    let agent = messages(&app, "branch", "?agent=worker").await;
    assert_eq!(media(&agent).len(), 1);
    assert_ne!(media(&agent)[0]["src"], old);
    assert_eq!(get(&app, &old).await.status(), StatusCode::OK);
    // A new selected leaf removes the formerly granted path. The old opaque
    // token must consult current native scope instead of returning cached bytes.
    let mut replacement = prefix;
    replacement.push(json!({"type":"user","sessionId":"branch","uuid":"new","parentUuid":null,"cwd":cwd,"message":{"role":"user","content":"Replacement branch has no image"}}));
    replacement.push(json!({"type":"last-prompt","leafUuid":"new"}));
    fixture.write(path, replacement);
    let current = messages(&app, "branch", "").await;
    assert!(media(&current).is_empty());
    assert!(matches!(
        get(&app, &old).await.status(),
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
    ));
    fixture.unchanged();
}

#[tokio::test]
async fn a_missing_cwd_basename_stays_text_even_when_other_directories_share_the_name() {
    let mut fixture = Fixture::new();
    let cwd = fixture.file("");
    let mut rows = vec![json!({"type":"session_meta","payload":{"id":"window","cwd":cwd}})];
    for index in 0..700 {
        let content = match index {
            150 => "a/same.png".to_owned(),
            151 => "b/same.png".to_owned(),
            // A bare name in prose is text; the Markdown
            // spelling resolves only against cwd, where this file is absent.
            699 => "![shot](same.png)".to_owned(),
            _ => format!("plain synthetic row {index}"),
        };
        rows.push(json!({"type":"response_item","payload":{"type":"message","role":"user","content":[text(content)]}}));
    }
    fixture.write(fixture.root.join("codex/window.jsonl"), rows);
    let app = fixture.app(true);
    let projected = messages(&app, "window", "?window=1").await;
    assert!(projected["partial"].is_object());
    assert!(media(&projected).is_empty(), "{projected}");
    assert!(projected.to_string().contains("![shot](same.png)"));
    fixture.unchanged();
}

/// Python's `media.register_path` registers nothing for a text reference that
/// is not an image file: the text stays, no placeholder. Typed native references
/// keep their slot, and selected images outside configured roots or reached by
/// a hard link are ordinary images.
#[tokio::test]
async fn text_references_python_would_drop_project_no_placeholder_while_referenced_images_are_readable()
 {
    let mut fixture = Fixture::new();
    fixture.put(
        "codex",
        "missing",
        vec![text(
            "附件1: ./sessiondock_attachments/1/image.png\n\n![](assets/27055012/report.jpg)\n\nsee ./a (a directory, not an image)",
        )],
    );
    fixture.put(
        "codex",
        "outside",
        vec![text(format!(
            "![shot]({})",
            fixture.root.join("outside.png").to_string_lossy()
        ))],
    );
    fixture.put(
        "claude",
        "typed-missing",
        vec![image("never-written.png"), text("typed reference text")],
    );
    #[cfg(unix)]
    {
        fs::hard_link(
            fixture.file("raw.png"),
            fixture.file("linked-attachment.png"),
        )
        .unwrap();
        fixture.put(
            "codex",
            "hardlink",
            vec![text("附件1: ./linked-attachment.png")],
        );
    }
    let app = fixture.app(true);
    let projected = messages(&app, "missing", "").await;
    assert!(media(&projected).is_empty(), "{projected}");
    let row = &projected["messages"][0];
    assert!(
        row.get("media")
            .is_none_or(|m| m.as_array().is_some_and(Vec::is_empty)),
        "{row}"
    );
    assert!(
        row["text"]
            .as_str()
            .unwrap()
            .contains("sessiondock_attachments/1/image.png")
    );
    let projected = messages(&app, "outside", "").await;
    let images = media(&projected);
    assert_eq!(images.len(), 1, "{projected}");
    let src = images[0]["src"]
        .as_str()
        .unwrap_or_else(|| panic!("{projected}"));
    assert!(images[0].get("error").is_none(), "{projected}");
    let response = get(&app, src).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(bytes(response).await, STANDARD.decode(GREEN).unwrap());
    let projected = messages(&app, "typed-missing", "").await;
    let images = media(&projected);
    assert_eq!(images.len(), 1, "{projected}");
    no_src_error(images[0]);
    assert_eq!(images[0]["error"]["status"], 404);
    #[cfg(unix)]
    {
        let projected = messages(&app, "hardlink", "").await;
        let images = media(&projected);
        assert_eq!(images.len(), 1, "{projected}");
        let src = images[0]["src"].as_str().unwrap().to_owned();
        assert_eq!(images[0]["gallery"], true);
        let response = get(&app, &src).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(bytes(response).await, STANDARD.decode(PNG).unwrap());
    }
    fixture.unchanged();
}
