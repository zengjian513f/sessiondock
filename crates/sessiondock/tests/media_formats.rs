//! Fixed genuine encodings; never executes an image CLI or reads native homes.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sessiondock::{config::Config, sessions::SessionRoots};
use std::{collections::BTreeMap, fs, path::PathBuf};
use tower::ServiceExt;

// Encoded once from 3x2 solid RGB pixels. The matching Python fixtures are
// independently decoded in real Chromium, including both two-frame animations.
const GIF: &str = "R0lGODdhAwACAIEAAOYeCgAAAAAAAAAAACwAAAAAAwACAAAIBgABCBwYEAA7";
const GIF_ANIMATED: &str = "R0lGODlhAwACAIEAAOYeCgAAAAAAAAAAACH/C05FVFNDQVBFMi4wAwEAAAAh+QQACAAAACwAAAAAAwACAAAIBgABCBwYEAAh+QQBDAABACwAAAAAAwACAIEKHuYAAAAAAAAAAAAIBgABCBwYEAA7";
const WEBP: &str = "UklGRh4AAABXRUJQVlA4TBEAAAAvAkAAAAdQj5pXof+BiOh/AAA=";
const WEBP_ANIMATED: &str = "UklGRogAAABXRUJQVlA4WAoAAAACAAAAAgAAAQAAQU5JTQYAAAAAAAAAAABBTk1GKgAAAAAAAAAAAAIAAAEAAFAAAAJWUDhMEQAAAC8CQAAAB1CPmleh/4GI6H8AAEFOTUYqAAAAAAAAAAAAAgAAAQAAeAAAAFZQOEwRAAAALwJAAAAHUI8q1Lz/gYjofwAA";
const AVIF: &str = "AAAAIGZ0eXBhdmlmAAAAAGF2aWZtaWYxbWlhZk1BMUIAAADrbWV0YQAAAAAAAAAhaGRscgAAAAAAAAAAcGljdAAAAAAAAAAAAAAAAAAAAAAOcGl0bQAAAAAAAQAAAB5pbG9jAAAAAEQAAAEAAQAAAAEAAAETAAAAKAAAAChpaW5mAAAAAAABAAAAGmluZmUCAAAAAAEAAGF2MDFDb2xvcgAAAABqaXBycAAAAEtpcGNvAAAAFGlzcGUAAAAAAAAAAwAAAAIAAAAQcGl4aQAAAAADCAgIAAAADGF2MUOBAAwAAAAAE2NvbHJuY2x4AAEADQAGgAAAABdpcG1hAAAAAAAAAAEAAQQBAoMEAAAAMG1kYXQSAAoIGAQrRAQ0GhAyGhTHh4ZlAgggnkAAAJBLsrmsYuXxFjb2VyIt";
const BMP: &str = "Qk1OAAAAAAAAADYAAAAoAAAAAwAAAAIAAAABABgAAAAAABgAAADEDgAAxA4AAAAAAAAAAAAACh7mCh7mCh7mAAAACh7mCh7mCh7mAAAA";
const FORMATS: [(&str, &str, &str); 6] = [
    ("gif", "image/gif", GIF),
    ("gif-animated", "image/gif", GIF_ANIMATED),
    ("webp", "image/webp", WEBP),
    ("webp-animated", "image/webp", WEBP_ANIMATED),
    ("avif", "image/avif", AVIF),
    ("bmp", "image/bmp", BMP),
];

fn block(source: &str, mime: &str, data: &str) -> Value {
    match source {
        "claude" => {
            json!({"type":"image","source":{"type":"base64","media_type":mime,"data":data}})
        }
        "codex" => json!({"type":"input_image","image_url":format!("data:{mime};base64,{data}")}),
        _ => json!({"type":"image_url","image_url":{"url":format!("data:{mime};base64,{data}")}}),
    }
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
        Self {
            _temp: temp,
            root,
            native: BTreeMap::new(),
        }
    }
    fn put(&mut self, source: &str, sid: &str, images: Vec<Value>) {
        let directory = self.root.join(source).join("synthetic-formats");
        fs::create_dir_all(&directory).unwrap();
        let rows = match source {
            "claude" => vec![
                json!({"type":"user","sessionId":sid,"uuid":"u","parentUuid":null,"message":{"role":"user","content":images}}),
            ],
            "codex" => vec![
                json!({"type":"session_meta","payload":{"id":sid,"cwd":"/synthetic/formats"}}),
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":images}}),
            ],
            _ => vec![json!({"type":"user","prompt_index":1,"content":images})],
        };
        let path = if source == "grok" {
            let directory = directory.join(sid);
            fs::create_dir(&directory).unwrap();
            let summary = directory.join("summary.json");
            let bytes = json!({"info":{"id":sid,"cwd":"/synthetic/formats"}})
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
    fn app(&self) -> Router {
        sessiondock::app(Config {
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            roots: SessionRoots {
                claude: Some(self.root.join("claude")),
                codex: Some(self.root.join("codex")),
                grok: Some(self.root.join("grok")),
            },
            ..Default::default()
        })
        .unwrap()
    }
    fn unchanged(&self) {
        for (path, original) in &self.native {
            assert_eq!(fs::read(path).unwrap(), *original);
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
    to_bytes(response.into_body(), 4 << 20)
        .await
        .unwrap()
        .to_vec()
}
async fn value(response: Response) -> Value {
    serde_json::from_slice(&bytes(response).await).unwrap()
}
async fn messages(app: &Router, sid: &str) -> Response {
    let inventory = value(get(app, "/api/sessions").await).await;
    let uid = inventory["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sid"] == sid)
        .unwrap()["uid"]
        .as_str()
        .unwrap();
    get(app, &format!("/api/messages/{uid}")).await
}

#[tokio::test]
async fn genuine_extended_formats_project_for_every_provider_and_serve_exact_private_bytes() {
    let mut fixture = Fixture::new();
    for source in ["claude", "codex", "grok"] {
        fixture.put(
            source,
            &format!("{source}-formats"),
            FORMATS
                .iter()
                .map(|(_, mime, data)| block(source, mime, data))
                .collect(),
        );
    }
    let app = fixture.app();
    for source in ["claude", "codex", "grok"] {
        let response = messages(&app, &format!("{source}-formats")).await;
        assert_eq!(response.status(), StatusCode::OK, "{source}");
        let projected = value(response).await;
        let serialized = projected.to_string();
        assert!(!serialized.contains("base64") && !serialized.contains("data:image"));
        let images: Vec<_> = projected["messages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|m| {
                m.get("media")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .collect();
        assert_eq!(images.len(), FORMATS.len());
        for (image, (name, mime, data)) in images.into_iter().zip(FORMATS) {
            assert!(!serialized.contains(data));
            assert_eq!(image["lazy"], true, "{name}");
            for absent in ["mime", "width", "height"] {
                assert!(image.get(absent).is_none(), "{name}: {absent}");
            }
            let src = image["src"].as_str().unwrap();
            let token = src.strip_prefix("/api/media/").unwrap();
            assert_eq!(token.len(), 32);
            assert!(
                token
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            );
            let response = get(&app, src).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["content-type"], mime);
            assert_eq!(response.headers()["x-content-type-options"], "nosniff");
            assert_eq!(response.headers()["cache-control"], "private, no-store");
            let extension = mime.strip_prefix("image/").unwrap();
            assert_eq!(
                response.headers()["content-disposition"],
                format!("inline; filename=\"image.{extension}\"")
            );
            assert_eq!(bytes(response).await, STANDARD.decode(data).unwrap());
        }
    }
    fixture.unchanged();
}

#[tokio::test]
async fn corrupt_signatures_and_truncated_containers_are_served_as_declared_bytes() {
    let mut fixture = Fixture::new();
    let mut cases = Vec::new();
    for (name, mime, data) in FORMATS {
        let original = STANDARD.decode(data).unwrap();
        let mut corrupt = original.clone();
        corrupt[..4].fill(0);
        cases.push((format!("bad-{name}"), mime, corrupt));
        cases.push((
            format!("short-{name}"),
            mime,
            original[..original.len() - 3].to_vec(),
        ));
    }
    for (name, mime, data) in &cases {
        fixture.put(
            "claude",
            name,
            vec![block("claude", mime, &STANDARD.encode(data))],
        );
    }
    let app = fixture.app();
    for (name, mime, expected) in cases {
        let response = messages(&app, &name).await;
        assert_eq!(response.status(), StatusCode::OK);
        let projected = value(response).await;
        let image = &projected["messages"][0]["media"][0];
        assert_eq!(image["lazy"], true);
        let response = get(&app, image["src"].as_str().unwrap()).await;
        assert_eq!(response.status(), StatusCode::OK, "{name}");
        assert_eq!(response.headers()["content-type"], mime);
        assert_eq!(bytes(response).await, expected);
    }
    fixture.unchanged();
}

fn gif_frames(count: usize, canvas: u16) -> Vec<u8> {
    let original = STANDARD.decode(GIF).unwrap();
    let mut result = original[..25].to_vec();
    result[6..8].copy_from_slice(&canvas.to_le_bytes());
    result[8..10].copy_from_slice(&canvas.to_le_bytes());
    for _ in 0..count {
        result.extend_from_slice(&original[25..original.len() - 1]);
    }
    result.push(b';');
    result
}
fn webp_frames(count: usize, canvas: u32) -> Vec<u8> {
    let original = STANDARD.decode(WEBP_ANIMATED).unwrap();
    // RIFF + VP8X + ANIM is 44 bytes; the first ANMF chunk is 50 bytes.
    assert_eq!(&original[44..48], b"ANMF");
    let mut result = original[..44].to_vec();
    result[24..27].copy_from_slice(&(canvas - 1).to_le_bytes()[..3]);
    result[27..30].copy_from_slice(&(canvas - 1).to_le_bytes()[..3]);
    for _ in 0..count {
        result.extend_from_slice(&original[44..94]);
    }
    let size = u32::try_from(result.len() - 8).unwrap();
    result[4..8].copy_from_slice(&size.to_le_bytes());
    result
}

#[tokio::test]
async fn large_animation_and_canvas_variants_are_served_without_parser_quotas() {
    let mut fixture = Fixture::new();
    let cases = [
        ("gif-frames", "image/gif", gif_frames(129, 3)),
        ("gif-pixels", "image/gif", gif_frames(65, 1024)),
        ("webp-frames", "image/webp", webp_frames(129, 3)),
        ("webp-pixels", "image/webp", webp_frames(65, 1024)),
        ("gif-canvas", "image/gif", gif_frames(1, 8193)),
    ];
    for (name, mime, data) in &cases {
        fixture.put(
            "claude",
            name,
            vec![block("claude", mime, &STANDARD.encode(data))],
        );
    }
    let app = fixture.app();
    for (name, mime, expected) in cases {
        let response = messages(&app, name).await;
        assert_eq!(response.status(), StatusCode::OK);
        let projected = value(response).await;
        let image = &projected["messages"][0]["media"][0];
        assert_eq!(image["lazy"], true);
        let response = get(&app, image["src"].as_str().unwrap()).await;
        assert_eq!(response.status(), StatusCode::OK, "{name}");
        assert_eq!(response.headers()["content-type"], mime);
        assert_eq!(bytes(response).await, expected);
    }
    fixture.unchanged();
}
