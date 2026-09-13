//! Real native-file HTTP pagination fixtures; no CLI or external service.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sessiondock::{config::Config, sessions::SessionRoots};
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use tower::ServiceExt;

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
const AGENT: &str = "worker-exact-full-agent-id";
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    original: BTreeMap<PathBuf, Vec<u8>>,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        for source in ["claude", "codex"] {
            fs::create_dir(root.join(source)).unwrap();
        }
        Self {
            _temp: temp,
            root,
            original: BTreeMap::new(),
        }
    }
    fn put(&mut self, path: PathBuf, rows: &[Value]) -> PathBuf {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let data = encoded(rows);
        fs::write(&path, &data).unwrap();
        self.original.insert(path.clone(), data);
        path
    }
    fn claude(&mut self, sid: &str, rows: &[Value]) -> PathBuf {
        self.put(
            self.root
                .join("claude/project")
                .join(format!("{sid}.jsonl")),
            rows,
        )
    }
    fn codex(&mut self, sid: &str, rows: &[Value]) -> PathBuf {
        self.put(
            self.root.join("codex/project").join(format!("{sid}.jsonl")),
            rows,
        )
    }
    fn app(&self) -> Router {
        sessiondock::app(Config {
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            roots: SessionRoots {
                claude: Some(self.root.join("claude")),
                codex: Some(self.root.join("codex")),
                grok: None,
            },
            // Batch 44 WP-A: the default page is 2000 events and pools queue
            // for up to 10 s; this suite walks 200-event pages against the
            // original 4-reader / 8-response sizing with immediate rejection
            // (`wait: ZERO`) so the eight-slot admission test stays exact.
            pools: sessiondock::config::Pools {
                history_page_events: 200,
                read_workers: 4,
                wait: std::time::Duration::ZERO,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap()
    }
    fn unchanged(&self) {
        for (path, bytes) in &self.original {
            assert_eq!(fs::read(path).unwrap(), *bytes);
        }
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
fn claude_rows(sid: &str, count: usize, parts: usize) -> Vec<Value> {
    (0..count).map(|i|json!({"type":"user","sessionId":sid,"uuid":format!("row-{i}"),"parentUuid":if i==0 {Value::Null}else{json!(format!("row-{}",i-1))},"message":{"role":"user","content":(0..parts).map(|part|json!({"type":"text","text":format!("{sid}:{i:04}:{part}")})).collect::<Vec<_>>()}})).collect()
}
fn codex_header(sid: &str) -> Value {
    json!({"type":"session_meta","payload":{"id":sid,"session_id":sid,"cwd":"/synthetic/history-pages","thread_source":"user"}})
}
fn codex_message(text: &str) -> Value {
    json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}})
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
    let token = value.as_str().expect("opaque page cursor");
    assert_eq!(token.len(), 32);
    assert!(
        token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    token.to_owned()
}
fn texts(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .map(|m| m["text"].as_str().unwrap_or("").to_owned())
        .collect()
}
fn image_count(messages: &[Value]) -> usize {
    messages
        .iter()
        .map(|m| m["media"].as_array().map_or(0, Vec::len))
        .sum()
}
fn page_uri(uid: &str, cursor: &str, agent: &str) -> String {
    format!("/api/messages/{uid}/page?cursor={cursor}&agent={agent}")
}
async fn restore(app: &Router, uid: &str, agent: &str, initial: &Value) -> Vec<Value> {
    let partial = &initial["partial"];
    let head = partial["head"].as_u64().unwrap() as usize;
    let tail = partial["tail"].as_u64().unwrap() as usize;
    let omitted = partial["omitted"].as_u64().unwrap() as usize;
    let initial_messages = initial["messages"].as_array().unwrap();
    assert_eq!(initial_messages.len(), head + tail);
    assert!(omitted > 0);
    let mut restored = initial_messages[..head].to_vec();
    let mut next = Some(token(&partial["cursor"]));
    let mut position = head;
    let stop = head + omitted;
    let mut turns = 0;
    while let Some(cursor) = next {
        turns += 1;
        assert!(turns < 100, "pagination failed to make bounded progress");
        let page = ok(app, &page_uri(uid, &cursor, agent)).await;
        for absent in ["meta", "end", "anchor", "version", "activity"] {
            assert!(
                page.get(absent).is_none(),
                "page must not replace live checkpoint: {absent}"
            );
        }
        let messages = page["messages"].as_array().unwrap();
        assert!(!messages.is_empty());
        assert!(messages.len() <= 200);
        assert!(image_count(messages) <= 128);
        assert!(serde_json::to_vec(&page).unwrap().len() <= 8 * 1024 * 1024);
        assert_eq!(page["page"]["cursor"], cursor);
        assert_eq!(page["page"]["start"], position);
        assert_eq!(page["page"]["end"], position + messages.len());
        assert_eq!(page["page"]["stop"], stop);
        position += messages.len();
        assert_eq!(page["page"]["remaining"], stop - position);
        next = if page["page"]["next"].is_null() {
            None
        } else {
            Some(token(&page["page"]["next"]))
        };
        assert_eq!(next.is_none(), position == stop);
        restored.extend_from_slice(messages);
    }
    assert_eq!(position, stop);
    restored.extend_from_slice(&initial_messages[head..]);
    assert_eq!(restored.len(), head + omitted + tail);
    restored
}

#[tokio::test]
async fn fifteen_hundred_records_restore_in_order_and_full_query_stays_full() {
    let mut f = Fixture::new();
    let rows = claude_rows("ordered", 1500, 1);
    f.claude("ordered", &rows);
    let app = f.app();
    let uid = uid(&app, "ordered").await;
    let config = ok(&app, "/api/meta").await;
    assert_eq!(config["capabilities"]["history_pages"], true);
    let full = ok(&app, &format!("/api/messages/{uid}")).await;
    assert!(full["partial"].is_null());
    assert_eq!(full["messages"].as_array().unwrap().len(), 1500);
    let initial = ok(&app, &format!("/api/messages/{uid}?window=1")).await;
    assert_eq!(initial["message_total"], 1500);
    let complete = restore(&app, &uid, "", &initial).await;
    assert_eq!(
        texts(&complete),
        texts(full["messages"].as_array().unwrap())
    );
    let first = token(&initial["partial"]["cursor"]);
    let replay = ok(&app, &page_uri(&uid, &first, "")).await;
    assert_eq!(replay["page"]["start"], initial["partial"]["head"]);
    f.unchanged();
}

#[tokio::test]
async fn multiple_claude_events_at_one_native_end_are_not_skipped_or_duplicated() {
    let mut f = Fixture::new();
    f.claude("blocks", &claude_rows("blocks", 700, 3));
    let app = f.app();
    let uid = uid(&app, "blocks").await;
    let full = ok(&app, &format!("/api/messages/{uid}")).await;
    let expected = full["messages"].as_array().unwrap();
    assert_eq!(
        expected.len(),
        2100,
        "fixture must produce multiple events per JSONL record"
    );
    let initial = ok(&app, &format!("/api/messages/{uid}?window=1")).await;
    assert_eq!(
        texts(&restore(&app, &uid, "", &initial).await),
        texts(expected)
    );
    f.unchanged();
}

#[tokio::test]
async fn inherited_zero_offset_events_page_by_logical_position_not_leaf_bytes() {
    let mut f = Fixture::new();
    let mut parent = vec![codex_header("parent")];
    parent.extend((0..1000).map(|i| codex_message(&format!("parent:{i:04}"))));
    let cut = encoded(&parent).len();
    parent.push(codex_message("excluded-parent-tail"));
    f.codex("parent", &parent);
    let mut header = codex_header("fork");
    header["payload"]["forked_from_id"] = json!("parent");
    header["payload"]["history_base"] = json!({"thread_id":"parent","end_byte_offset":cut});
    let mut child = vec![header];
    child.extend((0..500).map(|i| codex_message(&format!("child:{i:04}"))));
    f.codex("fork", &child);
    let app = f.app();
    let uid = uid(&app, "fork").await;
    let initial = ok(&app, &format!("/api/messages/{uid}?window=1")).await;
    let actual = texts(&restore(&app, &uid, "", &initial).await);
    let expected = (0..1000)
        .map(|i| format!("parent:{i:04}"))
        .chain((0..500).map(|i| format!("child:{i:04}")))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    f.unchanged();
}

#[tokio::test]
async fn two_hundred_fifty_seven_images_use_budgeted_pages_not_a_failed_window() {
    let mut f = Fixture::new();
    let mut rows = vec![codex_header("images")];
    rows.extend((0..257).map(|i|json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":format!("image:{i:04}")},{"type":"input_image","image_url":format!("data:image/png;base64,{PNG}")}]}})));
    f.codex("images", &rows);
    let app = f.app();
    let uid = uid(&app, "images").await;
    let initial = ok(&app, &format!("/api/messages/{uid}?window=1")).await;
    assert!(!initial["partial"].is_null());
    assert!(image_count(initial["messages"].as_array().unwrap()) <= 128);
    let complete = restore(&app, &uid, "", &initial).await;
    assert_eq!(complete.len(), 257);
    assert_eq!(image_count(&complete), 257);
    for (index, message) in complete.iter().enumerate() {
        assert!(
            message["text"]
                .as_str()
                .unwrap()
                .starts_with(&format!("image:{index:04}"))
        );
        assert!(
            message["media"][0]["src"]
                .as_str()
                .unwrap()
                .starts_with("/api/media/")
        );
    }
    assert!(!serde_json::to_string(&complete).unwrap().contains(PNG));
    f.unchanged();
}

#[tokio::test]
async fn appending_preserves_the_old_gap_without_advancing_its_live_checkpoint() {
    let mut f = Fixture::new();
    let mut rows = vec![codex_header("append")];
    rows.extend((0..1500).map(|i| codex_message(&format!("before:{i:04}"))));
    let path = f.codex("append", &rows);
    let app = f.app();
    let uid = uid(&app, "append").await;
    let initial = ok(&app, &format!("/api/messages/{uid}?window=1")).await;
    append(&path, &[codex_message("new-live-tail")]);
    ok(&app, "/api/sessions?force=1").await;
    let complete = restore(&app, &uid, "", &initial).await;
    assert_eq!(complete.len(), 1500);
    assert!(texts(&complete).iter().all(|text| text != "new-live-tail"));
    let delta = ok(
        &app,
        &format!(
            "/api/messages/{uid}?start={}&head={}&anchor={}&append=1",
            initial["end"],
            initial["version"]["head"].as_str().unwrap(),
            initial["anchor"].as_str().unwrap()
        ),
    )
    .await;
    assert_eq!(delta["reset"], false);
    assert_eq!(
        texts(delta["messages"].as_array().unwrap()),
        ["new-live-tail"]
    );
    assert!(delta["end"].as_u64().unwrap() > initial["end"].as_u64().unwrap());
}

#[tokio::test]
async fn claude_branch_rewind_invalidates_old_gap_with_conflict() {
    let mut f = Fixture::new();
    let path = f.claude("rewind", &claude_rows("rewind", 1500, 1));
    let app = f.app();
    let uid = uid(&app, "rewind").await;
    let initial = ok(&app, &format!("/api/messages/{uid}?window=1")).await;
    let cursor = token(&initial["partial"]["cursor"]);
    append(&path, &[json!({"type":"last-prompt","leafUuid":"row-20"})]);
    ok(&app, "/api/sessions?force=1").await;
    let response = get(&app, &page_uri(&uid, &cursor, "")).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error = body(response).await;
    assert!(error.get("messages").is_none());
}

#[tokio::test]
async fn rewritten_prefix_invalidates_even_when_file_length_is_unchanged() {
    let mut f = Fixture::new();
    let mut rows = vec![codex_header("rewrite")];
    rows.extend((0..1500).map(|i| codex_message(&format!("before:{i:04}"))));
    let path = f.codex("rewrite", &rows);
    let app = f.app();
    let uid = uid(&app, "rewrite").await;
    let initial = ok(&app, &format!("/api/messages/{uid}?window=1")).await;
    let cursor = token(&initial["partial"]["cursor"]);
    let old = fs::read(&path).unwrap();
    let mut changed = old.clone();
    let at = changed
        .windows(b"before:0900".len())
        .position(|s| s == b"before:0900")
        .unwrap();
    assert!(at > 4096);
    changed[at] = b'B';
    fs::write(&path, changed).unwrap();
    ok(&app, "/api/sessions?force=1").await;
    assert_eq!(
        get(&app, &page_uri(&uid, &cursor, "")).await.status(),
        StatusCode::CONFLICT
    );
    assert_eq!(fs::metadata(path).unwrap().len(), old.len() as u64);
}

#[tokio::test]
async fn watch_initial_and_rewrite_resets_keep_windows_then_append_advances_only_live_cursor() {
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
    fn window(value: &Value) -> String {
        assert_eq!(value["reset"], true);
        assert_eq!(value["message_total"], 1500);
        assert_eq!(value["partial"]["head"], 100);
        assert_eq!(value["partial"]["tail"], 500);
        assert_eq!(value["partial"]["omitted"], 900);
        assert_eq!(value["messages"].as_array().unwrap().len(), 600);
        token(&value["partial"]["cursor"])
    }

    let mut f = Fixture::new();
    let mut rows = vec![codex_header("watch-pages")];
    rows.extend((0..1500).map(|i| codex_message(&format!("before:{i:04}"))));
    let path = f.codex("watch-pages", &rows);
    let app = f.app();
    let uid = uid(&app, "watch-pages").await;
    // No valid byte/head/anchor cursor: ViewQuery::cursor must still request a window.
    let response = get(&app, &format!("/api/watch?uid={uid}")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut stream = response.into_body();
    let initial = packet(&mut stream).await;
    let old_gap = window(&initial);

    // Same-size committed-prefix rewrite beyond the small head fingerprint.
    // This reset uses packet()'s successor query, not ViewQuery::cursor again.
    let mut changed = fs::read(&path).unwrap();
    let at = changed
        .windows(b"before:0050".len())
        .position(|bytes| bytes == b"before:0050")
        .unwrap();
    assert!(at > 4096);
    changed[at] = b'B';
    fs::write(&path, &changed).unwrap();
    let reset = packet(&mut stream).await;
    let current_gap = window(&reset);
    assert_ne!(current_gap, old_gap);
    assert_eq!(reset["end"], initial["end"]);
    assert_ne!(reset["anchor"], initial["anchor"]);
    let expected: Vec<_> = (0..100)
        .chain(1000..1500)
        .map(|i| {
            if i == 50 {
                "Before:0050".to_owned()
            } else {
                format!("before:{i:04}")
            }
        })
        .collect();
    assert_eq!(texts(reset["messages"].as_array().unwrap()), expected);
    assert_eq!(
        get(&app, &page_uri(&uid, &old_gap, "")).await.status(),
        StatusCode::CONFLICT
    );

    let mut previous = reset;
    for text in ["live-after-rewrite-1", "live-after-rewrite-2"] {
        let new_rows = [codex_message(text)];
        append(&path, &new_rows);
        changed.extend(encoded(&new_rows));
        let delta = packet(&mut stream).await;
        assert_eq!(delta["reset"], false);
        assert_eq!(delta["start"], previous["end"]);
        assert_eq!(delta["version"]["head"], previous["version"]["head"]);
        assert!(delta["end"].as_u64().unwrap() > previous["end"].as_u64().unwrap());
        assert_eq!(texts(delta["messages"].as_array().unwrap()), [text]);
        assert!(delta.get("partial").is_none_or(Value::is_null));
        previous = delta;
    }
    // The reset's gap grant survives both appends; reading it is not a live
    // checkpoint update and must not include either newly appended message.
    let page = ok(&app, &page_uri(&uid, &current_gap, "")).await;
    assert_eq!(page["page"]["cursor"], current_gap);
    assert_eq!(page["page"]["start"], 100);
    assert_eq!(page["page"]["stop"], 1000);
    assert_eq!(
        texts(page["messages"].as_array().unwrap()),
        (100..300)
            .map(|i| format!("before:{i:04}"))
            .collect::<Vec<_>>()
    );
    drop(stream);
    assert_eq!(fs::read(path).unwrap(), changed);
}

#[tokio::test]
async fn page_tokens_are_parameter_checked_and_bound_to_owner_and_exact_agent() {
    let mut f = Fixture::new();
    f.claude("owner", &claude_rows("owner", 1500, 1));
    f.claude("other", &claude_rows("other", 1, 1));
    let mut agent_rows = claude_rows("owner", 900, 1);
    for row in &mut agent_rows {
        row["agentId"] = json!(AGENT);
        row["isSidechain"] = json!(true);
    }
    let agent_path = f.put(
        f.root.join(format!(
            "claude/project/owner/subagents/agent-{AGENT}.jsonl"
        )),
        &agent_rows,
    );
    let physical_child_uid = format!(
        "claude:{}",
        &format!(
            "{:x}",
            Sha1::digest(agent_path.to_string_lossy().as_bytes())
        )[..16]
    );
    let app = f.app();
    let owner = uid(&app, "owner").await;
    let other = uid(&app, "other").await;
    let main = ok(&app, &format!("/api/messages/{owner}?window=1")).await;
    let main_token = token(&main["partial"]["cursor"]);
    let child = ok(
        &app,
        &format!("/api/messages/{owner}?window=1&agent={AGENT}"),
    )
    .await;
    let child_token = token(&child["partial"]["cursor"]);
    for uri in [
        format!("/api/messages/{owner}/page"),
        page_uri(&owner, "short", ""),
        page_uri(&owner, &"A".repeat(32), ""),
    ] {
        assert_eq!(
            get(&app, &uri).await.status(),
            StatusCode::BAD_REQUEST,
            "{uri}"
        );
    }
    assert_eq!(
        get(&app, &page_uri(&owner, &"0".repeat(32), ""))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    for uri in [
        page_uri(&other, &main_token, ""),
        page_uri(&owner, &main_token, AGENT),
        page_uri(&owner, &child_token, ""),
        page_uri(&owner, &child_token, "worker-exact"),
        page_uri(&physical_child_uid, &child_token, AGENT),
        page_uri("claude:missing-owner", &main_token, ""),
    ] {
        let response = get(&app, &uri).await;
        assert!(
            response.status().is_client_error(),
            "cross-scope accepted: {uri}"
        );
        let error = body(response).await;
        assert!(error.get("messages").is_none());
        assert!(!error.to_string().contains("owner:0100"));
    }
    let restored = restore(&app, &owner, AGENT, &child).await;
    assert_eq!(restored.len(), 900);
    f.unchanged();
}

#[tokio::test]
async fn held_page_responses_and_retained_frames_keep_all_eight_admission_slots() {
    let mut f = Fixture::new();
    f.claude("slow-consumers", &claude_rows("slow-consumers", 1500, 1));
    let app = f.app();
    let uid = uid(&app, "slow-consumers").await;
    let initial = ok(&app, &format!("/api/messages/{uid}?window=1")).await;
    let cursor = token(&initial["partial"]["cursor"]);
    let uri = page_uri(&uid, &cursor, "");
    let mut held = Vec::new();
    for _ in 0..8 {
        let response = get(&app, &uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "application/json; charset=utf-8"
        );
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        let policy = response.headers()["cache-control"].to_str().unwrap();
        assert!(policy.contains("private") && policy.contains("no-store"));
        held.push(response);
    }
    assert_eq!(
        get(&app, &uri).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let mut response_body = held.pop().unwrap().into_body();
    let frame = response_body
        .frame()
        .await
        .unwrap()
        .unwrap()
        .into_data()
        .unwrap();
    assert!(!frame.is_empty());
    drop(response_body);
    assert_eq!(
        get(&app, &uri).await.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "retained data still owns admission after Body drop"
    );
    drop(frame);
    let recovered = ok(&app, &uri).await;
    assert_eq!(recovered["page"]["cursor"], cursor);
    assert!(!recovered["messages"].as_array().unwrap().is_empty());
    drop(held);
    let replay = ok(&app, &uri).await;
    assert_eq!(replay["page"]["start"], recovered["page"]["start"]);
    assert_eq!(replay["messages"], recovered["messages"]);
    f.unchanged();
}
