//! Synthetic embedded images only; no native homes, CLI execution, or remote I/O.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sessiondock::{config::Config, sessions::SessionRoots};
use std::{collections::BTreeMap, fs, path::PathBuf, time::Duration};
use tower::ServiceExt;

// Actual solid-color 2x3 PNG and 3x2 JPEG, also decoded by media_browser.py.
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAE0lEQVR4nGP8z8DAwMDAxIBMAQAUQAEF3SN5DgAAAABJRU5ErkJggg==";
const JPEG: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/2wBDAQkJCQwLDBgNDRgyIRwhMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjL/wAARCAACAAMDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwDxyiiiv3E8w//Z";
fn image(data: &str, mime: &str) -> Value {
    json!({"type":"image","source":{"type":"base64","media_type":mime,"data":data}})
}
fn claude(sid: &str, uuid: &str, parent: Option<&str>, role: &str, content: Value) -> Value {
    json!({"type":role,"sessionId":sid,"uuid":uuid,"parentUuid":parent,"message":{"role":role,"content":content}})
}
fn codex(kind: &str, payload: Value) -> Value {
    json!({"type":kind,"payload":payload})
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
        for source in ["claude", "codex", "grok"] {
            fs::create_dir(root.join(source)).unwrap();
        }
        let mut fixture = Self {
            _temp: temp,
            root,
            native: BTreeMap::new(),
        };
        fixture.put("claude", "claude-media", vec![
            claude("claude-media", "u", None, "user", json!([image(PNG,"image/png")])),
            claude("claude-media", "call", Some("u"), "assistant", json!([{"type":"tool_use","id":"call","name":"synthetic_image","input":{}}])),
            claude("claude-media", "result", Some("call"), "user", json!([{"type":"tool_result","tool_use_id":"call","content":[{"type":"text","text":"Claude mixed media"},image(JPEG,"image/jpeg")]}])),
        ]);
        fixture.put("codex", "codex-media", vec![
            codex("session_meta", json!({"id":"codex-media","session_id":"codex-media","cwd":"/synthetic/media"})),
            codex("response_item", json!({"type":"message","role":"user","content":[{"type":"input_image","image_url":format!("data:image/png;base64,{PNG}")}]})),
            codex("response_item", json!({"type":"function_call","name":"synthetic_image","call_id":"call","arguments":"{}"})),
            codex("response_item", json!({"type":"function_call_output","call_id":"call","output":[{"type":"text","text":"Codex mixed media"},{"type":"image","mimeType":"image/jpeg","data":JPEG}]})),
        ]);
        fixture.put("grok", "grok-media", vec![
            json!({"type":"user","prompt_index":1,"content":[{"type":"image_url","image_url":{"url":format!("data:image/png;base64,{PNG}")}}]}),
            json!({"type":"assistant","content":"","tool_calls":[{"id":"call","name":"synthetic_image","arguments":"{}"}]}),
            json!({"type":"tool_result","tool_call_id":"call","content":[{"type":"text","text":"Grok mixed media"},{"type":"image_url","image_url":format!("data:image/jpeg;base64,{JPEG}")}]}),
        ]);
        fixture
    }
    fn put(&mut self, source: &str, sid: &str, rows: Vec<Value>) {
        let directory = self.root.join(source).join("project");
        fs::create_dir_all(&directory).unwrap();
        let path = if source == "grok" {
            let directory = directory.join(sid);
            fs::create_dir(&directory).unwrap();
            let summary = directory.join("summary.json");
            let bytes = json!({"info":{"id":sid,"cwd":"/synthetic/media"}})
                .to_string()
                .into_bytes();
            fs::write(&summary, &bytes).unwrap();
            self.native.insert(summary, bytes);
            directory.join("chat_history.jsonl")
        } else {
            directory.join(format!("{sid}.jsonl"))
        };
        let bytes = rows
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>()
            .into_bytes();
        fs::write(&path, &bytes).unwrap();
        self.native.insert(path, bytes);
    }
    fn config(&self) -> Config {
        Config {
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            roots: SessionRoots {
                claude: Some(self.root.join("claude")),
                codex: Some(self.root.join("codex")),
                grok: Some(self.root.join("grok")),
            },
            ..Default::default()
        }
    }
    fn app(&self) -> Router {
        sessiondock::app(self.config()).unwrap()
    }
    fn unchanged(&self) {
        for (path, bytes) in &self.native {
            assert_eq!(&fs::read(path).unwrap(), bytes);
        }
    }
}
async fn request(app: &Router, method: &str, uri: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("host", "127.0.0.1:8741")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn get(app: &Router, uri: &str) -> Response {
    request(app, "GET", uri).await
}
async fn bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), 4 << 20)
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
        .unwrap_or_else(|| panic!("missing synthetic SID {sid}"))["uid"]
        .as_str()
        .unwrap()
        .into()
}
async fn messages(app: &Router, sid: &str) -> Value {
    let response = get(app, &format!("/api/messages/{}", uid(app, sid).await)).await;
    assert_eq!(response.status(), StatusCode::OK);
    value(response).await
}
fn media(value: &Value) -> Vec<&Value> {
    value["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| {
            message
                .get("media")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .collect()
}
fn valid_src(src: &str) -> bool {
    src.strip_prefix("/api/media/").is_some_and(|token| {
        token.len() == 32
            && token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

#[tokio::test]
async fn three_providers_project_image_only_and_mixed_tool_results_as_private_opaque_media() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let caps = value(get(&app, "/api/meta").await).await;
    assert_eq!(caps["capabilities"]["media"], true);
    assert_eq!(caps["capabilities"]["media_lazy"], true);
    assert_eq!(caps["capabilities"]["media_remote"], true);
    for source in ["claude", "codex", "grok"] {
        let projected = messages(&app, &format!("{source}-media")).await;
        assert!(!projected.to_string().contains(PNG));
        assert!(!projected.to_string().contains(JPEG));
        assert!(!projected.to_string().contains("data:image"));
        assert!(
            projected["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["text"] == "[图片]" && m["media"].is_array())
        );
        assert!(
            projected["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["text"].as_str().unwrap().contains("mixed media")
                    && m["media"].is_array())
        );
        let images = media(&projected);
        assert_eq!(images.len(), 2);
        for (image, (encoded, mime)) in images
            .iter()
            .zip([(PNG, "image/png"), (JPEG, "image/jpeg")])
        {
            let src = image["src"].as_str().unwrap();
            assert!(valid_src(src));
            assert_eq!(image["lazy"], true);
            for absent in ["mime", "width", "height"] {
                assert!(image.get(absent).is_none());
            }
            assert!(image["alt"].as_str().is_some_and(|alt| !alt.is_empty()));
            let response = get(&app, src).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["content-type"], mime);
            assert_eq!(response.headers()["x-content-type-options"], "nosniff");
            let cache = response.headers()["cache-control"].to_str().unwrap();
            assert!(cache.contains("private") && cache.contains("no-store"));
            assert_eq!(bytes(response).await, STANDARD.decode(encoded).unwrap());
        }
    }
    fixture.unchanged();
}

#[tokio::test]
async fn media_route_accepts_only_opaque_get_head_and_never_interprets_paths_or_remote_urls() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let projected = messages(&app, "claude-media").await;
    let src = media(&projected)[0]["src"].as_str().unwrap();
    let head = request(&app, "HEAD", src).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert!(bytes(head).await.is_empty());
    for method in ["POST", "PUT", "DELETE", "PATCH"] {
        assert_eq!(
            request(&app, method, src).await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    for uri in [
        "/api/media/00000000000000000000000000000000",
        "/api/media/short",
        "/api/media/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "/api/media/%2Fetc%2Fpasswd",
        "/api/media/https%3A%2F%2Fmedia.example.invalid%2Fimage.png",
    ] {
        let response = get(&app, uri).await;
        assert!(response.status().is_client_error(), "{uri}");
        let body = String::from_utf8(bytes(response).await).unwrap();
        assert!(!body.contains(PNG) && !body.contains(JPEG));
    }
    assert_eq!(
        get(&app, "/api/media/00000000000000000000000000000000")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    // An unrelated query cannot redirect a valid capability to another source.
    let response = get(
        &app,
        &format!("{src}?url=https%3A%2F%2Fmedia.example.invalid%2Fother.png&path=%2Fnever-read"),
    )
    .await;
    if response.status().is_success() {
        assert_eq!(bytes(response).await, STANDARD.decode(PNG).unwrap());
    } else {
        assert!(response.status().is_client_error());
    }
    fixture.unchanged();
}

#[tokio::test]
async fn a_queued_media_response_completes_after_a_retained_frame_is_released() {
    let fixture = Fixture::new();
    let mut config = fixture.config();
    config.pools.read_workers = 4;
    let app = sessiondock::app(config).unwrap();
    let projected = messages(&app, "claude-media").await;
    let src = media(&projected)[0]["src"].as_str().unwrap();
    let mut held = Vec::new();
    for _ in 0..8 {
        let response = get(&app, src).await;
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    let mut queued = tokio::spawn({
        let app = app.clone();
        let src = src.to_owned();
        async move { get(&app, &src).await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut queued)
            .await
            .is_err(),
        "the next response waits while all response slots are retained"
    );
    let mut body = held.pop().unwrap().into_body();
    let data = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(&data[..], STANDARD.decode(PNG).unwrap());
    drop(body);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut queued)
            .await
            .is_err(),
        "the retained frame continues to own its response slot"
    );
    drop(data);
    let consumed = tokio::time::timeout(Duration::from_secs(2), queued)
        .await
        .expect("queued media response completes after its slot is released")
        .unwrap();
    assert_eq!(consumed.status(), StatusCode::OK);
    assert_eq!(bytes(consumed).await, STANDARD.decode(PNG).unwrap());
    drop(held);
    assert_eq!(get(&app, src).await.status(), StatusCode::OK);
    fixture.unchanged();
}

#[tokio::test]
async fn evicted_tokens_return_404_and_selected_history_reprojection_restores_the_image() {
    let mut fixture = Fixture::new();
    // Descriptor admission is independent of the 256-item decoded blob cache.
    // Each legal 256-reference batch contributes to the 1024-entry registry.
    for batch in 0..4 {
        let sid = format!("cache-churn-{batch}");
        let mut rows = vec![codex(
            "session_meta",
            json!({"id":sid,"session_id":sid,"cwd":"/synthetic/media"}),
        )];
        for index in 0..256 {
            rows.push(codex(
                "response_item",
                json!({"type":"message","role":"user","content":[
                    {"type":"input_text","text":format!("synthetic image {batch}:{index}")},
                    {"type":"input_image","image_url":format!("data:image/png;base64,{PNG}")}
                ]}),
            ));
        }
        fixture.put("codex", &sid, rows);
    }
    let app = fixture.app();
    let original = messages(&app, "claude-media").await;
    let old_src = media(&original)[0]["src"].as_str().unwrap();
    assert_eq!(get(&app, old_src).await.status(), StatusCode::OK);
    for batch in 0..4 {
        let churn = messages(&app, &format!("cache-churn-{batch}")).await;
        assert_eq!(media(&churn).len(), 256);
    }
    assert_eq!(get(&app, old_src).await.status(), StatusCode::NOT_FOUND);
    let restored = messages(&app, "claude-media").await;
    let src = media(&restored)[0]["src"].as_str().unwrap();
    assert!(valid_src(src));
    let response = get(&app, src).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(bytes(response).await, STANDARD.decode(PNG).unwrap());
    fixture.unchanged();
}

#[tokio::test]
async fn decoded_bytes_local_files_and_remote_urls_are_projected() {
    let mut fixture = Fixture::new();
    let cases = [
        ("invalid-image", image("AAAA", "image/png"), StatusCode::OK),
        (
            "unsupported-svg",
            image("PHN2Zy8+", "image/svg+xml"),
            StatusCode::OK,
        ),
        (
            "remote-image",
            json!({"type":"image_url","image_url":"https://media.example.invalid/private-image.png"}),
            StatusCode::OK,
        ),
        (
            "file-image",
            json!({"type":"image","source":{"type":"file","path":fixture.root.join("private-never-read.png")}}),
            StatusCode::OK,
        ),
    ];
    fs::write(
        fixture.root.join("private-never-read.png"),
        b"PRIVATE_MUST_NOT_BE_READ",
    )
    .unwrap();
    for (sid, block, _) in &cases {
        fixture.put(
            "claude",
            sid,
            vec![claude(sid, "u", None, "user", json!([block]))],
        );
    }
    let app = fixture.app();
    for (sid, _, status) in cases {
        let response = get(&app, &format!("/api/messages/{}", uid(&app, sid).await)).await;
        assert_eq!(response.status(), status, "{sid}");
        let body = value(response).await;
        if sid == "file-image" {
            let items = media(&body);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0]["lazy"], true);
            assert_eq!(body["messages"][0]["text"], "[图片]");
            let response = get(&app, items[0]["src"].as_str().unwrap()).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(bytes(response).await, b"PRIVATE_MUST_NOT_BE_READ");
        } else if sid == "invalid-image" {
            let items = media(&body);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0]["lazy"], true);
            assert_eq!(body["messages"][0]["text"], "[图片]");
            let response = get(&app, items[0]["src"].as_str().unwrap()).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(bytes(response).await, [0, 0, 0]);
        } else if sid == "remote-image" {
            let items = media(&body);
            assert_eq!(items.len(), 1);
            assert_eq!(
                items[0]["src"],
                "https://media.example.invalid/private-image.png"
            );
            assert_eq!(items[0]["external"], true);
        } else if sid == "unsupported-svg" {
            assert!(media(&body).is_empty(), "{body}");
        } else {
            assert!(body.get("error").is_some());
        }
        for private in [
            "AAAA",
            "PHN2Zy8+",
            "PRIVATE_MUST_NOT_BE_READ",
            "private-never-read.png",
        ] {
            assert!(!body.to_string().contains(private), "{sid}: leaked input");
        }
    }
    fixture.unchanged();
}
