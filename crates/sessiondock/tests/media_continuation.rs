//! Per-message media continuation through real HTTP/SSE boundaries.
//! Synthetic Codex/Claude records in temp dirs; no CLI, paths or remote access.
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
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use tower::ServiceExt;

/// 2x1 red and 4x1 green PNGs: alternating them makes every paged position
/// verifiable by its decoded bytes, not only by descriptor count.
const RED: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAIAAAB7QOjdAAAADUlEQVR4nGP4z8AARAAI/gH/xp559wAAAABJRU5ErkJggg==";
const GREEN: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABCAIAAAB2XpiaAAAADUlEQVR4nGNg+M8ARwAZ8wP9PUPc9AAAAABJRU5ErkJggg==";
const TEXT: &str = "MANY IMAGES";

fn png(index: usize) -> &'static str {
    if index.is_multiple_of(2) { RED } else { GREEN }
}
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        for source in ["claude", "codex"] {
            fs::create_dir(root.join(source)).unwrap();
        }
        Self { _temp: temp, root }
    }
    fn put(&self, relative: &str, rows: &[Value]) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, encoded(rows)).unwrap();
        path
    }
    fn app(&self) -> Router {
        sessiondock::app(Config {
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            roots: SessionRoots {
                claude: Some(self.root.join("claude")),
                codex: Some(self.root.join("codex")),
                grok: None,
            },
            // Keep the original worker sizing while exercising queued reads.
            pools: sessiondock::config::Pools {
                read_workers: 4,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap()
    }
}
fn encoded(rows: &[Value]) -> Vec<u8> {
    rows.iter()
        .map(|row| format!("{row}\n"))
        .collect::<String>()
        .into_bytes()
}
fn append(path: &Path, rows: &[Value]) {
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(&encoded(rows))
        .unwrap();
}
fn codex_header(sid: &str) -> Value {
    json!({"type":"session_meta","payload":{"id":sid,"session_id":sid,"cwd":"/synthetic/media-continuation","thread_source":"user"}})
}
fn codex_text(text: &str) -> Value {
    json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}})
}
fn codex_images(text: &str, count: usize) -> Value {
    let mut content = vec![json!({"type":"input_text","text":text})];
    content.extend((0..count).map(|index| {
        json!({"type":"input_image","image_url":format!("data:image/png;base64,{}", png(index))})
    }));
    json!({"type":"response_item","payload":{"type":"message","role":"user","content":content}})
}
fn claude_rows(sid: &str, count: usize) -> Vec<Value> {
    let images = (0..count)
        .map(|index| {
            json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":png(index)}})
        })
        .collect::<Vec<_>>();
    vec![
        json!({"type":"user","sessionId":sid,"uuid":"row-0","parentUuid":null,"message":{"role":"user","content":[{"type":"text","text":"capture please"}]}}),
        json!({"type":"assistant","sessionId":sid,"uuid":"row-1","parentUuid":"row-0","message":{"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"capture","input":{}}]}}),
        json!({"type":"user","sessionId":sid,"uuid":"row-2","parentUuid":"row-1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":images}]}}),
    ]
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
async fn body(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 24 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
async fn ok(app: &Router, uri: &str) -> Value {
    let response = get(app, uri).await;
    assert_eq!(response.status(), StatusCode::OK, "GET {uri}");
    body(response).await
}
async fn uid(app: &Router, sid: &str) -> String {
    let listed = ok(app, "/api/sessions?force=1").await;
    listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sid"] == sid)
        .unwrap_or_else(|| panic!("missing synthetic session {sid}"))["uid"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn token(value: &Value) -> String {
    let token = value.as_str().expect("opaque media cursor");
    assert_eq!(token.len(), 32);
    assert!(
        token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    token.to_owned()
}
fn media_uri(uid: &str, cursor: &str, agent: &str) -> String {
    format!("/api/messages/{uid}/media-page?cursor={cursor}&agent={agent}")
}
fn descriptor(item: &Value) -> String {
    assert_eq!(item["lazy"], true);
    assert!(item.get("error").is_none(), "{item}");
    let src = item["src"].as_str().unwrap();
    assert!(src.starts_with("/api/media/"));
    src.to_owned()
}
fn find<'a>(messages: &'a Value, text: &str) -> &'a Value {
    messages
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["text"] == text)
        .unwrap_or_else(|| panic!("missing message {text}"))
}
fn assert_more(message: &Value, remaining: usize, total: usize) -> String {
    assert_eq!(message["media"].as_array().unwrap().len(), 16);
    let more = &message["media_more"];
    assert_eq!(more["remaining"], remaining);
    assert_eq!(more["total"], total);
    assert_eq!(more.as_object().unwrap().len(), 3);
    token(&more["cursor"])
}
/// Follow the continuation from `first` and return every descriptor src in
/// order, checking the page contract at each step.
async fn follow(app: &Router, uid: &str, agent: &str, first: &str, total: usize) -> Vec<String> {
    let mut cursor = first.to_owned();
    let mut position = 16;
    let mut srcs = Vec::new();
    loop {
        let response = get(app, &media_uri(uid, &cursor, agent)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "application/json; charset=utf-8"
        );
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        let policy = response.headers()["cache-control"].to_str().unwrap();
        assert!(policy.contains("private") && policy.contains("no-store"));
        let page = body(response).await;
        for absent in ["messages", "meta", "end", "anchor", "version", "activity"] {
            assert!(page.get(absent).is_none(), "{absent}");
        }
        let media = page["media"].as_array().unwrap();
        assert!(!media.is_empty() && media.len() <= 16);
        assert_eq!(page["page"]["cursor"], cursor);
        assert_eq!(page["page"]["start"], position);
        assert_eq!(page["page"]["end"], position + media.len());
        assert_eq!(page["page"]["total"], total);
        position += media.len();
        assert_eq!(page["page"]["remaining"], total - position);
        srcs.extend(media.iter().map(descriptor));
        if page["page"]["next"].is_null() {
            assert_eq!(position, total);
            return srcs;
        }
        let next = token(&page["page"]["next"]);
        assert_ne!(next, cursor);
        cursor = next;
        assert!(position < total);
    }
}
async fn expect_png(app: &Router, src: &str, index: usize) {
    let response = get(app, src).await;
    assert_eq!(response.status(), StatusCode::OK, "{src}");
    assert_eq!(response.headers()["content-type"], "image/png");
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(
        &bytes[..],
        STANDARD.decode(png(index)).unwrap(),
        "image {index}"
    );
}

#[tokio::test]
async fn forty_image_codex_message_shows_sixteen_inline_and_pages_the_remainder() {
    let f = Fixture::new();
    let rows = vec![
        codex_header("many"),
        codex_text("before"),
        codex_images(TEXT, 40),
        codex_text("after"),
    ];
    let path = f.put("codex/project/many.jsonl", &rows);
    let original = fs::read(&path).unwrap();
    let app = f.app();
    let uid = uid(&app, "many").await;
    let meta = ok(&app, "/api/meta").await;
    assert_eq!(meta["capabilities"]["media_continuation"], true);
    assert_eq!(meta["capabilities"]["history_pages"], true);

    let window = ok(&app, &format!("/api/messages/{uid}?window=1")).await;
    assert!(window["partial"].is_null());
    assert_eq!(window["message_total"], 3);
    let message = find(&window["messages"], TEXT);
    let cursor = assert_more(message, 24, 40);
    for other in ["before", "after"] {
        let plain = find(&window["messages"], other);
        assert!(plain.get("media").is_none());
        assert!(plain.get("media_more").is_none());
    }
    let inline = message["media"]
        .as_array()
        .unwrap()
        .iter()
        .map(descriptor)
        .collect::<Vec<_>>();
    assert!(!serde_json::to_string(&window).unwrap().contains(RED));

    let paged = follow(&app, &uid, "", &cursor, 40).await;
    assert_eq!(paged.len(), 24);
    let mut all = inline.clone();
    all.extend(paged.iter().cloned());
    for (index, src) in all.iter().enumerate() {
        expect_png(&app, src, index).await;
    }
    // Replaying the first page is idempotent; the live cursor is untouched.
    let replay = ok(&app, &media_uri(&uid, &cursor, "")).await;
    assert_eq!(replay["page"]["start"], 16);
    assert_eq!(replay["page"]["end"], 32);
    let again = ok(&app, &format!("/api/messages/{uid}?window=1")).await;
    assert_eq!(again["end"], window["end"]);
    assert_eq!(again["anchor"], window["anchor"]);

    // Token checks mirror history pages.
    assert_eq!(
        get(&app, &media_uri(&uid, &cursor, "wrong-agent"))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for uri in [
        format!("/api/messages/{uid}/media-page"),
        media_uri(&uid, "short", ""),
        media_uri(&uid, &"A".repeat(32), ""),
    ] {
        assert_eq!(
            get(&app, &uri).await.status(),
            StatusCode::BAD_REQUEST,
            "{uri}"
        );
    }
    assert_eq!(
        get(&app, &media_uri(&uid, &"0".repeat(32), ""))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let ignored = ok(&app, &format!("{}&start=1", media_uri(&uid, &cursor, ""))).await;
    assert_eq!(ignored["page"]["start"], 16);
    assert_eq!(
        get(&app, &format!("/api/messages/{uid}/page?cursor={cursor}"))
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "a media grant is not a history page"
    );
    let search = ok(&app, "/api/search?q=MANY+IMAGES").await;
    assert_eq!(search["results"].as_array().unwrap().len(), 1);
    assert_eq!(search["results"][0]["uid"], uid);
    assert!(!search.to_string().contains("/api/media/"));

    // An ordinary append keeps the old cursor valid without moving its range.
    append(&path, &[codex_text("live-tail")]);
    ok(&app, "/api/sessions?force=1").await;
    let delta = ok(
        &app,
        &format!(
            "/api/messages/{uid}?start={}&head={}&anchor={}&append=1",
            window["end"],
            window["version"]["head"].as_str().unwrap(),
            window["anchor"].as_str().unwrap()
        ),
    )
    .await;
    assert_eq!(delta["reset"], false);
    assert_eq!(delta["messages"][0]["text"], "live-tail");
    let after_append = ok(&app, &media_uri(&uid, &cursor, "")).await;
    assert_eq!(after_append["page"]["start"], 16);
    assert_eq!(after_append["page"]["total"], 40);
    assert_eq!(
        after_append["media"].as_array().unwrap().len(),
        replay["media"].as_array().unwrap().len()
    );
    // Re-reading a page returns one stable continuation token.
    assert_eq!(after_append["page"]["next"], replay["page"]["next"]);
    let second = token(&replay["page"]["next"]);
    let last = ok(&app, &media_uri(&uid, &second, "")).await;
    assert_eq!(last["page"]["remaining"], 0);
    assert!(last["page"]["next"].is_null());

    // A same-size rewrite of the message line with its mtime restored still
    // changes the semantic checkpoint: the grant must not serve stale images.
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let mut changed = fs::read(&path).unwrap();
    let at = changed
        .windows(TEXT.len())
        .position(|bytes| bytes == TEXT.as_bytes())
        .unwrap();
    changed[at..at + TEXT.len()].copy_from_slice(b"MANY IMAGEZ");
    assert_eq!(
        changed.len(),
        original.len() + encoded(&[codex_text("live-tail")]).len()
    );
    // Windows does not expose Unix ctime through `Metadata`, so an in-place
    // same-size write with its mtime restored has no observable version
    // change. Replace the file there so the native file identity changes;
    // Unix exercises the stricter same-inode ctime case.
    #[cfg(windows)]
    fs::remove_file(&path).unwrap();
    fs::write(&path, &changed).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    ok(&app, "/api/sessions?force=1").await;
    let response = get(&app, &media_uri(&uid, &cursor, "")).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error = body(response).await;
    assert!(error.get("media").is_none());
    assert!(error.get("page").is_none());
    let reloaded = ok(&app, &format!("/api/messages/{uid}?window=1")).await;
    let fresh = assert_more(find(&reloaded["messages"], "MANY IMAGEZ"), 24, 40);
    assert_ne!(fresh, cursor);
    assert_eq!(follow(&app, &uid, "", &fresh, 40).await.len(), 24);
}

#[tokio::test]
async fn twenty_image_claude_tool_result_pages_independently_of_the_provider() {
    let f = Fixture::new();
    f.put("claude/project/tool.jsonl", &claude_rows("tool", 20));
    let app = f.app();
    let uid = uid(&app, "tool").await;
    let window = ok(&app, &format!("/api/messages/{uid}?window=1")).await;
    let message = window["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool_result")
        .expect("tool_result message");
    assert_eq!(message["text"], "[图片]");
    let cursor = assert_more(message, 4, 20);
    let paged = follow(&app, &uid, "", &cursor, 20).await;
    assert_eq!(paged.len(), 4);
    for (offset, src) in paged.iter().enumerate() {
        expect_png(&app, src, 16 + offset).await;
    }
    let page = ok(&app, &media_uri(&uid, &cursor, "")).await;
    assert!(page["page"]["next"].is_null());
    assert_eq!(page["page"]["end"], 20);
    assert_eq!(page["page"]["remaining"], 0);
}

#[tokio::test]
async fn watch_delta_and_http_delta_carry_media_more_for_an_appended_message() {
    async fn packet(body: &mut Body) -> Value {
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("synthetic native update should produce an SSE packet")
            .expect("watch must remain open")
            .unwrap()
            .into_data()
            .unwrap();
        let text = std::str::from_utf8(&frame).unwrap();
        assert!(!text.contains("event: migration-error"), "{text}");
        serde_json::from_str(text.trim().strip_prefix("data: ").unwrap()).unwrap()
    }
    let f = Fixture::new();
    let path = f.put(
        "codex/project/live.jsonl",
        &[codex_header("live"), codex_text("first")],
    );
    let app = f.app();
    let uid = uid(&app, "live").await;
    let response = get(&app, &format!("/api/watch?uid={uid}")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body();
    let initial = packet(&mut stream).await;
    assert_eq!(initial["reset"], true);
    assert_eq!(initial["messages"].as_array().unwrap().len(), 1);

    append(&path, &[codex_images(TEXT, 20)]);
    let delta = packet(&mut stream).await;
    assert_eq!(delta["reset"], false);
    assert_eq!(delta["start"], initial["end"]);
    assert_eq!(delta["messages"].as_array().unwrap().len(), 1);
    let sse_cursor = assert_more(&delta["messages"][0], 4, 20);
    let paged = follow(&app, &uid, "", &sse_cursor, 20).await;
    assert_eq!(paged.len(), 4);
    for (offset, src) in paged.iter().enumerate() {
        expect_png(&app, src, 16 + offset).await;
    }

    // The same delta over plain HTTP issues its own grant for the same event.
    let http = ok(
        &app,
        &format!(
            "/api/messages/{uid}?start={}&head={}&anchor={}&append=1",
            initial["end"],
            initial["version"]["head"].as_str().unwrap(),
            initial["anchor"].as_str().unwrap()
        ),
    )
    .await;
    assert_eq!(http["reset"], false);
    let http_cursor = assert_more(&http["messages"][0], 4, 20);
    assert_ne!(http_cursor, sse_cursor);
    let page = ok(&app, &media_uri(&uid, &http_cursor, "")).await;
    assert_eq!(page["page"]["start"], 16);
    assert_eq!(page["page"]["end"], 20);
    assert!(page["page"]["next"].is_null());
    // A later ordinary append keeps both grants valid and never re-pages them.
    append(&path, &[codex_text("second")]);
    let later = packet(&mut stream).await;
    assert_eq!(later["reset"], false);
    assert_eq!(later["messages"][0]["text"], "second");
    assert!(later["messages"][0].get("media_more").is_none());
    assert_eq!(
        get(&app, &media_uri(&uid, &sse_cursor, "")).await.status(),
        StatusCode::OK
    );
    drop(stream);
}

#[tokio::test]
async fn a_ninth_media_page_waits_until_an_earlier_body_is_released() {
    let f = Fixture::new();
    f.put(
        "codex/project/slots.jsonl",
        &[codex_header("slots"), codex_images(TEXT, 40)],
    );
    let app = f.app();
    let uid = uid(&app, "slots").await;
    let window = ok(&app, &format!("/api/messages/{uid}?window=1")).await;
    let cursor = assert_more(find(&window["messages"], TEXT), 24, 40);
    let uri = media_uri(&uid, &cursor, "");
    let mut held = Vec::new();
    for _ in 0..8 {
        let response = get(&app, &uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    let mut queued = tokio::spawn({
        let app = app.clone();
        let uri = uri.clone();
        async move { get(&app, &uri).await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut queued)
            .await
            .is_err(),
        "the ninth page waits while all response slots are retained"
    );
    drop(held.pop());
    let response = tokio::time::timeout(Duration::from_secs(2), queued)
        .await
        .expect("queued media page completes after one response is released")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let recovered = body(response).await;
    assert_eq!(recovered["page"]["cursor"], cursor);
    assert_eq!(recovered["media"].as_array().unwrap().len(), 16);
    drop(held);
}
