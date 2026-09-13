//! Deferred validation through actual HTTP/history/page/SSE boundaries.
//! All native records are synthetic; no image paths, CLI or remote services.
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
use std::{fs, path::PathBuf, time::Duration};
use tower::ServiceExt;

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAE0lEQVR4nGP8z8DAwMDAxIBMAQAUQAEF3SN5DgAAAABJRU5ErkJggg==";
const GIF: &str = "R0lGODdhAwACAIEAAOYeCgAAAAAAAAAAACwAAAAAAwACAAAIBgABCBwYEAA7";

fn image(data: &str, mime: &str) -> Value {
    json!({"type":"input_image","image_url":format!("data:{mime};base64,{data}")})
}
fn row(index: usize, image: Option<Value>) -> Value {
    let mut content = vec![json!({"type":"input_text","text":format!("lazy-readable-{index:04}")})];
    content.extend(image);
    json!({"type":"response_item","payload":{"type":"message","role":"user","content":content}})
}
struct Fixture {
    _temp: tempfile::TempDir,
    path: PathBuf,
    original: Vec<u8>,
    app: Router,
}
impl Fixture {
    fn new(rows: Vec<Value>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let native = root.join("codex");
        fs::create_dir(&native).unwrap();
        let path = native.join("lazy.jsonl");
        let mut all =
            vec![json!({"type":"session_meta","payload":{"id":"lazy","cwd":"/synthetic/lazy"}})];
        all.extend(rows);
        let original = all
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>()
            .into_bytes();
        fs::write(&path, &original).unwrap();
        let app = sessiondock::app(Config {
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            roots: SessionRoots {
                claude: None,
                codex: Some(native),
                grok: None,
            },
            // The 1500-row fixture expects a 200-event page after the
            // 600-event window (batch 44 WP-A made the default page 2000).
            pools: sessiondock::config::Pools {
                history_page_events: 200,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        Self {
            _temp: temp,
            path,
            original,
            app,
        }
    }
    async fn uid(&self) -> String {
        let listed = ok(&self.app, "/api/sessions").await;
        listed["sessions"][0]["uid"].as_str().unwrap().to_owned()
    }
    fn unchanged(&self) {
        assert_eq!(fs::read(&self.path).unwrap(), self.original);
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
async fn json_body(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
async fn ok(app: &Router, uri: &str) -> Value {
    let response = get(app, uri).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    json_body(response).await
}
fn descriptors(packet: &Value) -> Vec<&Value> {
    packet["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["media"].as_array().into_iter().flatten())
        .collect()
}
fn descriptor(item: &Value) -> &str {
    let object = item.as_object().unwrap();
    assert_eq!(
        object.len(),
        3,
        "embedded descriptor must not claim inspected metadata"
    );
    assert_eq!(item["lazy"], true);
    assert!(item["alt"].as_str().is_some_and(|alt| !alt.is_empty()));
    let src = item["src"].as_str().unwrap();
    let token = src.strip_prefix("/api/media/").unwrap();
    assert_eq!(token.len(), 32);
    assert!(
        token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    src
}

#[tokio::test]
async fn history_and_search_preserve_text_while_get_alone_rejects_bad_bytes_or_pixel_budgets() {
    let mut oversized = STANDARD.decode(GIF).unwrap();
    oversized[6..8].copy_from_slice(&8193u16.to_le_bytes());
    let oversized = STANDARD.encode(oversized);
    let cases = [
        ("####", "image/png", StatusCode::UNPROCESSABLE_ENTITY),
        ("AAAA", "image/png", StatusCode::UNPROCESSABLE_ENTITY),
        (PNG, "image/jpeg", StatusCode::UNPROCESSABLE_ENTITY),
        (
            oversized.as_str(),
            "image/gif",
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (PNG, "image/png", StatusCode::OK),
    ];
    let fixture = Fixture::new(
        cases
            .iter()
            .enumerate()
            .map(|(i, (data, mime, _))| row(i, Some(image(data, mime))))
            .collect(),
    );
    let uid = fixture.uid().await;
    let uri = format!("/api/messages/{uid}?window=1");
    let history = ok(&fixture.app, &uri).await;
    assert_eq!(history["message_total"], 5);
    let images = descriptors(&history);
    assert_eq!(images.len(), 5);
    for (i, (item, (_, _, expected))) in images.iter().zip(&cases).enumerate() {
        assert!(
            history["messages"][i]["text"]
                .as_str()
                .unwrap()
                .contains(&format!("lazy-readable-{i:04}"))
        );
        let src = descriptor(item);
        // Failure is repeatable and must not poison unrelated descriptors.
        for _ in 0..2 {
            let response = get(&fixture.app, src).await;
            assert_eq!(response.status(), *expected, "image {i}");
            if *expected == StatusCode::OK {
                assert_eq!(response.headers()["content-type"], "image/png");
                assert_eq!(
                    &to_bytes(response.into_body(), 1024).await.unwrap()[..],
                    STANDARD.decode(PNG).unwrap()
                );
            } else {
                let error = json_body(response).await;
                assert!(error.get("error").is_some());
                let serialized = error.to_string();
                for private in [PNG, "####", "AAAA", "data:image"] {
                    assert!(!serialized.contains(private));
                }
            }
        }
    }
    let again = ok(&fixture.app, &uri).await;
    assert_eq!(again["end"], history["end"]);
    assert_eq!(again["anchor"], history["anchor"]);
    assert_eq!(again["messages"], history["messages"]);
    let search = ok(&fixture.app, "/api/search?q=lazy-readable").await;
    assert_eq!(search["results"].as_array().unwrap().len(), 1);
    assert_eq!(search["results"][0]["hits"], 5);
    fixture.unchanged();
}

#[tokio::test]
async fn windows_pages_and_watch_register_invalid_images_without_materializing_them() {
    let fixture = Fixture::new(
        (0..1500)
            .map(|i| {
                row(
                    i,
                    [20, 150, 1450]
                        .contains(&i)
                        .then(|| image("AAAA", "image/png")),
                )
            })
            .collect(),
    );
    let uid = fixture.uid().await;
    let history = ok(&fixture.app, &format!("/api/messages/{uid}?window=1")).await;
    assert_eq!(history["messages"].as_array().unwrap().len(), 600);
    let images = descriptors(&history);
    assert_eq!(images.len(), 2);
    for item in images {
        descriptor(item);
    }
    let token = history["partial"]["cursor"].as_str().unwrap();
    let page = ok(
        &fixture.app,
        &format!("/api/messages/{uid}/page?cursor={token}"),
    )
    .await;
    assert_eq!(page["messages"].as_array().unwrap().len(), 200);
    let images = descriptors(&page);
    assert_eq!(images.len(), 1);
    let response = get(&fixture.app, descriptor(images[0])).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    drop(response);

    let response = get(&fixture.app, &format!("/api/watch?uid={uid}")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body();
    let frame = tokio::time::timeout(Duration::from_secs(5), stream.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_data()
        .unwrap();
    let packet: Value = serde_json::from_str(
        std::str::from_utf8(&frame)
            .unwrap()
            .trim()
            .strip_prefix("data: ")
            .expect("readable history must not become migration-error"),
    )
    .unwrap();
    assert_eq!(packet["reset"], true);
    assert_eq!(packet["messages"].as_array().unwrap().len(), 600);
    let images = descriptors(&packet);
    assert_eq!(images.len(), 2);
    for item in images {
        descriptor(item);
    }
    drop(stream);
    fixture.unchanged();
}
