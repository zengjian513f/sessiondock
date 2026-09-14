//! Search fixtures are synthetic and exist only in temporary test directories.

use std::{fs, path::PathBuf, sync::atomic::AtomicBool, time::Duration};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sessiondock::{
    config::Config,
    search::{self, SearchError, SearchQuery},
    sessions::{SessionRoots, SessionStore},
};
use tower::ServiceExt;

fn config() -> Config {
    Config {
        web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
        ..Default::default()
    }
}

fn write(path: &std::path::Path, rows: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
}

fn user(sid: &str, id: &str, parent: Value, text: &str) -> Value {
    json!({"type":"user", "sessionId":sid, "uuid":id, "parentUuid":parent,
        "cwd":"/synthetic/search", "timestamp":"2026-09-11T10:00:00Z", "message":{"role":"user", "content":text}})
}

fn corpus(count: usize) -> (tempfile::TempDir, Config, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let claude = root.path().join("claude");
    for index in 0..count {
        let sid = format!("search-{index}");
        write(
            &claude.join("project").join(format!("{sid}.jsonl")),
            &[
                user(
                    &sid,
                    "u0",
                    Value::Null,
                    "Needle Cat cat caterpillar cat. 猫 猫猫 a.b A.B",
                ),
                json!({"type":"assistant", "uuid":"a0", "parentUuid":"u0", "sessionId":sid,
                "timestamp":"2026-09-11T10:00:01Z", "message":{"role":"assistant", "stop_reason":"end_turn", "content":"Synthetic visible answer"}}),
                json!({"type":"assistant", "uuid":"tool", "parentUuid":"a0", "sessionId":sid,
                "timestamp":"2026-09-11T10:00:02Z", "message":{"role":"assistant", "content":[{
                    "type":"tool_use", "id":"call", "name":"Read", "input":{"file_path":"RAW_ONLY_TARGET"}}]}}),
                json!({"type":"user", "uuid":"result", "parentUuid":"tool", "sessionId":sid,
                "timestamp":"2026-09-11T10:00:03Z", "message":{"role":"user", "content":[{
                    "type":"tool_result", "tool_use_id":"call", "content":"RAW_ONLY_RESULT"}]}}),
                user(&sid, "discarded", json!("a0"), "DISCARDED_BRANCH"),
                json!({"type":"assistant", "uuid":"discarded-answer", "parentUuid":"discarded", "sessionId":sid,
                "message":{"role":"assistant", "content":"DISCARDED_ANSWER"}}),
                json!({"type":"last-prompt", "leafUuid":"result"}),
                json!({"type":"system", "uuid":"compact", "parentUuid":null,
                "subtype":"compact_boundary", "timestamp":"2026-09-11T10:00:04Z"}),
                json!({"type":"user", "uuid":"summary", "parentUuid":"compact", "isCompactSummary":true,
                "message":{"role":"user", "content":"INTERNAL_COMPACT_SECRET"}}),
            ],
        );
    }
    let selected = claude.join("project/search-0.jsonl");
    let mut cfg = config();
    cfg.roots.claude = Some(claude);
    (root, cfg, selected)
}

async fn get(app: &Router, route: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(route)
                .header("host", "127.0.0.1:8741")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn semantic_search_excludes_protocol_tools_discarded_branches_and_compact_summary() {
    let (_temp, cfg, _) = corpus(1);
    let app = sessiondock::app(cfg).unwrap();
    let result = json_body(get(&app, "/api/search?q=needle").await).await;
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    assert_eq!(result["results"][0]["hits"], 1);
    assert_eq!(result["scanned"], 1);
    assert_eq!(result["incomplete"], false);
    for invisible in [
        "RAW_ONLY_TARGET",
        "RAW_ONLY_RESULT",
        "DISCARDED_BRANCH",
        "DISCARDED_ANSWER",
        "INTERNAL_COMPACT_SECRET",
        "已压缩",
    ] {
        let query = SearchQuery {
            q: invisible.into(),
            ..Default::default()
        }
        .prepare()
        .unwrap();
        // Unicode query construction remains separate from HTTP URL encoding.
        let roots = SessionRoots {
            claude: Some(_temp.path().join("claude")),
            ..Default::default()
        };
        let store = SessionStore::new(roots);
        let pool = store.search_pool().unwrap();
        let data = search::execute(
            &pool.rows,
            |uid, cancelled, _| search::scan_view(store.search_view(&pool, uid), &query, cancelled),
            &query,
            &AtomicBool::new(false),
            |_| Ok(()),
            2,
        )
        .unwrap();
        assert!(
            data["results"].as_array().unwrap().is_empty(),
            "{invisible}"
        );
    }
}

#[tokio::test]
async fn literal_case_whole_word_and_regex_flags_match_visible_text() {
    let (_temp, cfg, _) = corpus(1);
    let app = sessiondock::app(cfg).unwrap();
    for (query, hits) in [
        ("q=cat", 4),
        ("q=cat&word=1", 3),
        ("q=cat&word=1&case=1", 2),
        ("q=c.t", 0),
        ("q=c.t&regex=1&word=1", 3),
        ("q=c.t&regex=1&word=1&case=1", 2),
    ] {
        let data = json_body(get(&app, &format!("/api/search?{query}")).await).await;
        if hits == 0 {
            assert!(data["results"].as_array().unwrap().is_empty())
        } else {
            assert_eq!(data["results"][0]["hits"], hits, "{query}")
        }
    }
}

#[tokio::test]
async fn source_filter_limit_and_explicit_partial_errors_are_not_fake_complete() {
    let (temp, mut cfg, _) = corpus(3);
    write(
        &temp.path().join("claude/project/unsupported.jsonl"),
        &[
            user("unsupported", "u0", Value::Null, "Needle unsupported"),
            // Scalar content is unreadable (unknown record
            // kinds are merely skipped).
            json!({"type":"user","sessionId":"unsupported","message":{"content":42}}),
        ],
    );
    let codex = temp.path().join("codex");
    write(
        &codex.join("rollout-search.jsonl"),
        &[
            json!({"type":"session_meta", "payload":{"id":"codex-search", "cwd":"/synthetic/search"}}),
            json!({"type":"response_item", "payload":{"type":"message", "role":"user", "content":[{"type":"input_text", "text":"Needle Codex"}]}}),
        ],
    );
    cfg.roots.codex = Some(codex);
    let app = sessiondock::app(cfg).unwrap();
    let all = json_body(get(&app, "/api/search?q=Needle").await).await;
    assert_eq!(all["total_pool"], 5);
    assert_eq!(all["scanned"], 5);
    assert_eq!(all["results"].as_array().unwrap().len(), 4);
    assert_eq!(all["errors"].as_array().unwrap().len(), 1);
    assert_eq!(all["errors"][0]["status"], 501);
    assert_eq!(all["partial"], true);
    assert_eq!(all["incomplete"], true);
    let codex = json_body(get(&app, "/api/search?q=Needle&source=codex").await).await;
    assert_eq!(codex["total_pool"], 1);
    assert_eq!(codex["results"][0]["source"], "codex");
    assert_eq!(codex["incomplete"], false);
    let limited = json_body(get(&app, "/api/search?q=Needle&source=claude&limit=1").await).await;
    assert_eq!(limited["results"].as_array().unwrap().len(), 1);
    assert_eq!(limited["truncated"], true);
    assert_eq!(limited["incomplete"], true);
}

#[tokio::test]
async fn invalid_regex_and_query_are_http_400_before_ndjson_headers() {
    let (_temp, cfg, _) = corpus(1);
    let app = sessiondock::app(cfg).unwrap();
    for query in ["q=%5B&regex=1"] {
        let response = get(&app, &format!("/api/search?{query}&progress=1")).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        assert!(json_body(response).await["error"].is_string());
    }
    let too_long = format!("/api/search?q={}", "x".repeat(4097));
    assert_eq!(get(&app, &too_long).await.status(), StatusCode::OK);
    for query in [
        "q=%28%3F%3Dcat%29&regex=1",
        "q=%28cat%29%5C1&regex=1",
        "q=cat&word=true",
        "q=cat&limit=0",
        "q=cat&limit=201",
        "q=cat&source=other",
    ] {
        assert_eq!(
            get(&app, &format!("/api/search?{query}")).await.status(),
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn large_search_matches_are_returned_complete() {
    let (_temp, cfg, path) = corpus(1);
    let text = "x".repeat(1024 * 1024);
    let rows: Vec<_> = (0..9)
        .map(|index| {
            user(
                "large-search",
                &format!("u{index}"),
                if index == 0 {
                    Value::Null
                } else {
                    json!(format!("u{}", index - 1))
                },
                &text,
            )
        })
        .collect();
    write(&path, &rows);
    let app = sessiondock::app(cfg).unwrap();
    let response = get(&app, "/api/search?q=%28%3Fs%29.%2B&regex=1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let data = json_body(response).await;
    assert_eq!(data["results"].as_array().unwrap().len(), 1);
    assert!(data["results"][0]["snippet"].as_str().unwrap().len() > 8 * 1024 * 1024);
    assert!(data["errors"].as_array().unwrap().is_empty());
    assert_eq!(data["partial"], false);
    assert_eq!(data["truncated"], false);
    assert_eq!(data["scanned"], 1);
}

#[tokio::test]
async fn ndjson_progress_matches_and_final_result_agree_with_json() {
    let (_temp, cfg, _) = corpus(4);
    let app = sessiondock::app(cfg).unwrap();
    let expected = json_body(get(&app, "/api/search?q=needle").await).await;
    let response = get(&app, "/api/search?q=needle&progress=1").await;
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/x-ndjson")
    );
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let events: Vec<Value> = std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events[0], json!({"type":"progress", "done":0, "total":4}));
    assert_eq!(events.iter().filter(|e| e["type"] == "matches").count(), 4);
    assert_eq!(events.last().unwrap()["type"], "result");
    assert_eq!(events.last().unwrap()["data"], expected);
    let done: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "progress")
        .map(|e| e["done"].as_u64().unwrap())
        .collect();
    assert_eq!(done, vec![0, 1, 2, 3, 4]);
}

#[tokio::test]
async fn empty_query_never_touches_missing_native_source() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = config();
    cfg.roots.claude = Some(temp.path().join("absent"));
    let app = sessiondock::app(cfg).unwrap();
    let response = get(&app, "/api/search?q=%20%20").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["total_pool"], 0);
    let response = get(&app, "/api/search?q=%20%20&progress=1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["type"],
        "result"
    );
    assert_eq!(
        get(&app, "/api/search?q=cat").await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_search_starts_after_unpolled_streams_release_workers() {
    let (_temp, cfg, _) = corpus(30);
    let app = sessiondock::app(cfg).unwrap();
    let first = get(&app, "/api/search?q=needle&progress=1").await;
    let second = get(&app, "/api/search?q=needle&progress=1").await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let queued_app = app.clone();
    let mut queued =
        tokio::spawn(async move { get(&queued_app, "/api/search?q=needle&progress=1").await });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut queued)
            .await
            .is_err()
    );
    drop(first);
    drop(second);
    let response = tokio::time::timeout(Duration::from_secs(3), queued)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    drop(response);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let response = get(&app, "/api/search?q=needle").await;
            if response.status() == StatusCode::OK {
                assert_eq!(json_body(response).await["scanned"], 30);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("dropped stream did not release blocked send/worker/admission");
}

#[tokio::test]
async fn partially_read_stream_can_be_dropped_and_server_remains_usable() {
    let (_temp, cfg, _) = corpus(20);
    let app = sessiondock::app(cfg).unwrap();
    let mut body = get(&app, "/api/search?q=needle&progress=1")
        .await
        .into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&first).unwrap()["type"],
        "progress"
    );
    drop(body);
    assert_eq!(get(&app, "/api/health").await.status(), StatusCode::OK);
}

/// The pool freezes the candidate list; every candidate's body is streamed
/// from its current file when its turn comes (docs/read-model.md "搜索"), so
/// a rewrite after admission changes the matches but not the pool size.
#[test]
fn frozen_pool_streams_current_files_and_callback_cancellation() {
    let (_temp, cfg, path) = corpus(3);
    let store = SessionStore::new(cfg.roots);
    let pool = store.search_pool().unwrap();
    let query = SearchQuery {
        q: "Needle".into(),
        ..Default::default()
    }
    .prepare()
    .unwrap();
    let run = |pool: &sessiondock::sessions::SearchPool,
               cancelled: bool,
               emit: &mut dyn FnMut(Value) -> Result<(), SearchError>| {
        search::execute(
            &pool.rows,
            |uid, cancelled, _| search::scan_view(store.search_view(pool, uid), &query, cancelled),
            &query,
            &AtomicBool::new(cancelled),
            &mut *emit,
            1,
        )
    };
    let data = run(&pool, false, &mut |_| Ok(())).unwrap();
    assert_eq!(data["results"].as_array().unwrap().len(), 3);
    assert_eq!(data["total_pool"], 3);
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("Needle", "Absent"),
    )
    .unwrap();
    let data = run(&pool, false, &mut |_| Ok(())).unwrap();
    assert_eq!(data["results"].as_array().unwrap().len(), 2);
    assert_eq!(data["total_pool"], 3);
    let current = store.search_pool().unwrap();
    let data = run(&current, false, &mut |_| Ok(())).unwrap();
    assert_eq!(data["results"].as_array().unwrap().len(), 2);
    let mut callbacks = 0;
    let error = run(&pool, false, &mut |_| {
        callbacks += 1;
        if callbacks == 2 {
            Err(SearchError::cancelled())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert_eq!(error.code, "search_cancelled");
    assert_eq!(callbacks, 2);
    let error = run(&pool, true, &mut |_| {
        panic!("cancelled scan emitted an event")
    })
    .unwrap_err();
    assert_eq!(error.code, "search_cancelled");
}
