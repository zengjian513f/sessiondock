//! `resolve`, `file_navigation`, the audit grouping and the SSE rewrite
//! against `test_hub.py`'s dispatch cases; no network.
use axum::http::{HeaderMap, HeaderValue};
use serde_json::{Map, Value, json};

use super::*;

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn node(nid: &str, name: &str) -> Node {
    Node {
        url: "http://127.0.0.1:1".into(),
        token: "t".repeat(32),
        id: nid.into(),
        name: name.into(),
        color: None,
        enabled: None,
    }
}

fn q(raw: &str) -> Query {
    parse_qs(raw)
}

fn body(value: Value) -> Option<Map<String, Value>> {
    match value {
        Value::Object(map) => Some(map),
        _ => panic!("object"),
    }
}

#[test]
fn query_parsing_keeps_blanks_order_and_plus() {
    let query = q("uid=claude%3Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa~x&name=&q=a+b&q=c&flag");
    assert_eq!(
        query["uid"],
        vec!["claude:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa~x".to_string()]
    );
    assert_eq!(query["name"], vec![String::new()]);
    assert_eq!(query["q"], vec!["a b".to_string(), "c".to_string()]);
    assert_eq!(query["flag"], vec![String::new()]);
    assert_eq!(
        encode_qs(&query),
        "uid=claude%3Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa~x&name=&q=a+b&q=c&flag="
    );
    assert_eq!(unquote("%E6%88%AA%E5%9B%BE.png"), "截图.png");
    assert_eq!(unquote("%zz%4"), "%zz%4");
    assert_eq!(quote("claude:same/file", ":"), "claude:same%2Ffile");
}

#[test]
fn explicit_and_display_routes() {
    assert_eq!(
        explicit_node(&format!("/api/nodes/{A}/api/live")),
        Some((A, "/api/live"))
    );
    assert_eq!(explicit_node(&format!("/api/nodes/{A}/display")), None);
    assert_eq!(explicit_node("/api/nodes/short/api/live"), None);
    assert_eq!(
        explicit_node("/api/nodes/ééééééééééééééééééééééééééééééééééé/api/x"),
        None
    );
    assert_eq!(display_route(&format!("/api/nodes/{B}/display")), Some(B));
    assert_eq!(display_route("/api/nodes/x/display"), None);
}

#[test]
fn resolve_unscopes_path_query_and_body_for_one_machine() {
    let scoped = format!("claude:{A}~same-file-hash");
    let resolved = resolve(
        None,
        "GET",
        &format!("/api/messages/{}", quote(&scoped, ":")),
        q("start=0&nodes=x"),
        None,
    )
    .unwrap();
    assert_eq!(resolved.nid, A);
    assert_eq!(resolved.path, "/api/messages/claude:same-file-hash");
    assert_eq!(encode_qs(&resolved.query), "start=0");
    // Rust-only sub-routes keep their suffix.
    let page = resolve(
        None,
        "GET",
        &format!("/api/messages/{scoped}/page"),
        q(""),
        None,
    )
    .unwrap();
    assert_eq!(page.path, "/api/messages/claude:same-file-hash/page");
    let delete = resolve(
        None,
        "DELETE",
        &format!("/api/session/{scoped}"),
        q(""),
        None,
    )
    .unwrap();
    assert_eq!(
        (delete.nid.as_str(), delete.path.as_str()),
        (A, "/api/session/claude:same-file-hash")
    );
    // A POST under /api/session/ is not a session path.
    let send = resolve(
        None,
        "POST",
        "/api/session/send",
        q(""),
        body(json!({"uid": scoped, "name": format!("{A}~same-terminal"), "text": "keep exact text",
            "media": [{"src": format!("/api/nodes/{A}/api/media/{}", "d".repeat(32))}, {"src": "data:x"}]})),
    )
    .unwrap();
    let forwarded = send.body.unwrap();
    assert_eq!(forwarded["uid"], "claude:same-file-hash");
    assert_eq!(forwarded["name"], "same-terminal");
    assert_eq!(
        forwarded["media"][0]["src"],
        format!("/api/media/{}", "d".repeat(32))
    );
    assert_eq!(forwarded["media"][1]["src"], "data:x");
    assert_eq!(forwarded["text"], "keep exact text");
    // Query references: uid always, name only on terminal routes.
    let term = resolve(
        None,
        "GET",
        "/api/term/attach",
        q(&format!("name={A}~same-terminal&token=x")),
        None,
    )
    .unwrap();
    assert_eq!(encode_qs(&term.query), "name=same-terminal&token=x");
    let star = resolve(
        None,
        "POST",
        "/api/session/star",
        q(""),
        body(json!({"uid": scoped, "starred": true})),
    )
    .unwrap();
    assert_eq!(star.body.unwrap()["uid"], "claude:same-file-hash");
    let nest = resolve(
        None,
        "POST",
        "/api/session/nest",
        q(""),
        body(json!({
            "uid": scoped,
            "parent_uid": format!("codex:{A}~parent-sid"),
            "independent": false
        })),
    )
    .unwrap();
    let nest_body = nest.body.unwrap();
    assert_eq!(nest_body["uid"], "claude:same-file-hash");
    assert_eq!(nest_body["parent_uid"], "codex:parent-sid");
    assert_eq!(
        resolve(
            None,
            "POST",
            "/api/session/nest",
            q(""),
            body(json!({
                "uid": scoped,
                "parent_uid": format!("codex:{B}~parent-sid")
            })),
        )
        .unwrap_err(),
        ProxyError::Invalid(ONE_MACHINE.to_string())
    );
    let trash = resolve(
        None,
        "POST",
        "/api/trash/restore",
        q(""),
        body(json!({"id": format!("{B}~claude/same-trash")})),
    )
    .unwrap();
    assert_eq!(
        (
            trash.nid.as_str(),
            trash.body.unwrap()["id"].as_str().unwrap()
        ),
        (B, "claude/same-trash")
    );
    let report = resolve(
        None,
        "POST",
        "/api/bug-report",
        q(""),
        body(json!({"terminal_name": format!("{B}~t1")})),
    )
    .unwrap();
    assert_eq!(report.body.unwrap()["terminal_name"], "t1");
    // The problem machine's capture names its terminal the same way; the
    // nested `origin`/`captured` of a cross-machine report stay untouched.
    let capture = resolve(
        None,
        "POST",
        "/api/bug-report/capture",
        q(""),
        body(json!({"terminal_name": format!("{B}~t1"), "uid": ""})),
    )
    .unwrap();
    assert_eq!(capture.nid, B);
    assert_eq!(capture.body.unwrap()["terminal_name"], "t1");
    let remote = resolve(
        None,
        "POST",
        "/api/bug-report",
        q(""),
        body(json!({"_node": A, "uid": "", "terminal_name": "",
            "origin": {"node_id": B, "uid": format!("claude:{B}~x")},
            "captured": {"session": {"uid": format!("claude:{B}~x")}}})),
    )
    .unwrap();
    assert_eq!(remote.nid, A);
    let remote = remote.body.unwrap();
    assert_eq!(remote["origin"]["uid"], format!("claude:{B}~x"));
    assert_eq!(
        remote["captured"]["session"]["uid"],
        format!("claude:{B}~x")
    );
}

#[test]
fn resolve_rejects_mixed_missing_and_foreign_targets() {
    let a = format!("claude:{A}~same-file-hash");
    let b_name = format!("{B}~same-terminal");
    let mixed = resolve(
        None,
        "POST",
        "/api/session/rewind",
        q(""),
        body(json!({"uid": a, "name": b_name})),
    );
    assert_eq!(mixed, Err(ProxyError::Invalid(ONE_MACHINE.into())));
    let none = resolve(
        None,
        "POST",
        "/api/term/create",
        q(""),
        body(json!({"source": "claude", "cwd": "/same"})),
    );
    assert_eq!(none, Err(ProxyError::Invalid(ONE_MACHINE.into())));
    let foreign = resolve(
        None,
        "POST",
        "/api/session/send",
        q(""),
        body(
            json!({"uid": a, "media": [{"src": format!("/api/nodes/{B}/api/media/{}", "d".repeat(32))}]}),
        ),
    );
    assert_eq!(foreign, Err(ProxyError::Invalid(FOREIGN_ATTACHMENT.into())));
    let unscoped = resolve(
        None,
        "GET",
        "/api/messages/claude:same-file-hash",
        q(""),
        None,
    );
    assert_eq!(
        unscoped,
        Err(ProxyError::Invalid(
            "missing or invalid machine reference".into()
        ))
    );
    let no_source = resolve(None, "GET", "/api/session/file", q("uid=plain"), None);
    assert_eq!(
        no_source,
        Err(ProxyError::Invalid("missing session source".into()))
    );
}

#[test]
fn resolve_body_node_query_node_and_explicit_route() {
    // `_node` names the machine and is removed; `node` in the query is only
    // consumed when the body named none.
    let created = resolve(
        None,
        "POST",
        "/api/term/create",
        q(""),
        body(json!({"_node": B, "_build": "x"})),
    )
    .unwrap();
    assert_eq!(created.nid, B);
    assert_eq!(created.body.unwrap(), body(json!({"_build": "x"})).unwrap());
    let upload = resolve(
        None,
        "POST",
        "/api/session/attachment",
        q(&format!(
            "uid=bug-report&node={B}&name=%E6%88%AA%E5%9B%BE.png"
        )),
        None,
    )
    .unwrap();
    assert_eq!(upload.nid, B);
    assert_eq!(
        encode_qs(&upload.query),
        "uid=bug-report&name=%E6%88%AA%E5%9B%BE.png"
    );
    // Without ?node= the bug-report uid is not decoded and no machine is guessed.
    let guessed = resolve(
        None,
        "POST",
        "/api/session/attachment",
        q("uid=bug-report&name=x.png"),
        None,
    );
    assert_eq!(guessed, Err(ProxyError::Invalid(ONE_MACHINE.into())));
    let both = resolve(
        None,
        "POST",
        "/api/term/backend",
        q(&format!("node={A}")),
        body(json!({"_node": A, "backend": "tmux"})),
    )
    .unwrap();
    assert_eq!(encode_qs(&both.query), format!("node={A}"));
    // Explicit routes carry local references untouched.
    let explicit = resolve(
        Some(B),
        "POST",
        "/api/session/star",
        q("uid=claude:same-file-hash"),
        body(json!({"uid": "claude:same-file-hash", "starred": true})),
    )
    .unwrap();
    assert_eq!(explicit.nid, B);
    assert_eq!(explicit.body.unwrap()["uid"], "claude:same-file-hash");
    assert_eq!(encode_qs(&explicit.query), "uid=claude%3Asame-file-hash");
    let conflict = resolve(
        Some(B),
        "POST",
        "/api/term/create",
        q(""),
        body(json!({"_node": A})),
    );
    assert_eq!(conflict, Err(ProxyError::Invalid(ONE_MACHINE.into())));
}

#[test]
fn file_navigation_redirects_browsers_only() {
    let query = q("uid=claude%3Asame-file-hash&path=%2Fetc%2Fhosts");
    assert_eq!(
        file_navigation("application/json", &query, None).unwrap(),
        None
    );
    assert_eq!(
        file_navigation("text/html", &q("uid=x&raw=1"), None).unwrap(),
        None
    );
    assert_eq!(
        file_navigation("text/html", &q("uid=x&download=1"), None).unwrap(),
        None
    );
    assert_eq!(
        file_navigation("text/html", &q("uid=x&mode=text"), None).unwrap(),
        None
    );
    assert_eq!(
        file_navigation("text/html,application/xhtml+xml", &query, None)
            .unwrap()
            .unwrap(),
        "../../file.html?uid=claude%3Asame-file-hash&path=%2Fetc%2Fhosts"
    );
    assert_eq!(
        file_navigation("text/html", &query, Some(A))
            .unwrap()
            .unwrap(),
        format!("../../../../../file.html?uid=claude%3A{A}~same-file-hash&path=%2Fetc%2Fhosts")
    );
    let response = file_redirect("../../file.html?x=1");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()["location"], "../../file.html?x=1");
    assert_eq!(response.headers()["vary"], "Accept");
}

#[test]
fn page_nodes_lru_and_audit_grouping() {
    let pages = PageNodes::new();
    for index in 0..(PAGE_NODES_MAX + 3) {
        pages.remember(&format!("page-{index}"), A);
    }
    assert_eq!(pages.len(), PAGE_NODES_MAX);
    assert_eq!(pages.get("page-0"), None);
    assert_eq!(pages.get("page-3").as_deref(), Some(A));
    pages.remember("", B);
    assert_eq!(pages.get(""), None);

    let uid = format!("claude:{A}~same-file-hash");
    let batch = body(json!({"page_id": "page-audit", "uid": uid,
        "events": [{"event": "page.loaded", "uid": uid}, {"event": "batch.uid"},
                   {"event": "other", "uid": format!("codex:{B}~x")}]}))
    .unwrap();
    let groups = group_browser_audit(&batch, &pages);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].nid, A);
    assert_eq!(groups[0].body["uid"], "claude:same-file-hash");
    assert_eq!(groups[0].body["events"].as_array().unwrap().len(), 2);
    assert_eq!(groups[0].body["events"][1]["uid"], "claude:same-file-hash");
    assert_eq!(groups[0].body["page_id"], "page-audit");
    assert_eq!(groups[1].nid, B);
    assert_eq!(groups[1].body["events"][0]["uid"], "codex:x");
    // A pending uid follows the page's last machine with an empty uid.
    let pending = body(json!({"page_id": "page-audit", "uid": "pending:xxx",
        "events": [{"event": "dom.snapshot", "uid": "pending:xxx"}]}))
    .unwrap();
    let groups = group_browser_audit(&pending, &pages);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].nid, B);
    assert_eq!(groups[0].body["uid"], "");
    assert_eq!(groups[0].body["events"][0]["uid"], "");
    // An unknown page with unscoped uids is dropped.
    let unknown =
        body(json!({"page_id": "never", "events": [{"event": "x", "uid": "pending:y"}]})).unwrap();
    assert!(group_browser_audit(&unknown, &pages).is_empty());
}

#[test]
fn headers_display_ip_and_sse_rewrite() {
    let mut browser = HeaderMap::new();
    browser.insert("content-type", HeaderValue::from_static("application/json"));
    browser.insert("x-sessiondock-page", HeaderValue::from_static("p1"));
    browser.insert("range", HeaderValue::from_static("bytes=0-1"));
    browser.insert(
        "user-agent",
        HeaderValue::from_static("Mozilla/5.0 (iPhone)"),
    );
    browser.insert("cookie", HeaderValue::from_static("secret"));
    browser.insert("upgrade", HeaderValue::from_static("websocket"));
    browser.insert("sec-websocket-key", HeaderValue::from_static("k"));
    let plain = upstream_headers(&browser, "10.0.0.9", false);
    assert_eq!(
        plain,
        vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("X-SessionDock-Page".to_string(), "p1".to_string()),
            ("Range".to_string(), "bytes=0-1".to_string()),
            ("User-Agent".to_string(), "Mozilla/5.0 (iPhone)".to_string()),
            ("X-Real-IP".to_string(), "10.0.0.9".to_string()),
        ]
    );
    let socket = upstream_headers(&browser, "10.0.0.9", true);
    assert!(socket.contains(&("Upgrade".to_string(), "websocket".to_string())));
    assert!(socket.contains(&("Sec-WebSocket-Key".to_string(), "k".to_string())));
    assert!(!socket.iter().any(|(name, _)| name == "Cookie"));

    let mut forwarded = HeaderMap::new();
    forwarded.insert(
        "x-forwarded-for",
        HeaderValue::from_static("203.0.113.5, 10.0.0.1"),
    );
    assert_eq!(
        display_ip(&forwarded, Some("127.0.0.1".parse().unwrap())),
        "203.0.113.5"
    );
    forwarded.insert("x-real-ip", HeaderValue::from_static("not-an-ip"));
    assert_eq!(
        display_ip(&forwarded, Some("::ffff:192.0.2.1".parse().unwrap())),
        "192.0.2.1"
    );
    assert_eq!(display_ip(&HeaderMap::new(), None), "127.0.0.1");

    let node = node(A, "NodeA");
    let line = b"data: {\"meta\": {\"uid\": \"claude:same\"}, \"messages\": []}\n".to_vec();
    let out = rewrite_sse_line(line, &node, "/api/watch").unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.starts_with("data: ") && text.ends_with('\n'));
    let value: Value = serde_json::from_str(&text[6..]).unwrap();
    assert_eq!(value["meta"]["uid"], format!("claude:{A}~same"));
    assert_eq!(value["meta"]["node_name"], "NodeA");
    assert_eq!(
        rewrite_sse_line(b": heartbeat\n".to_vec(), &node, "/api/watch").unwrap(),
        b": heartbeat\n"
    );
    assert_eq!(
        rewrite_sse_line(b"\n".to_vec(), &node, "/api/watch").unwrap(),
        b"\n"
    );
    assert!(rewrite_sse_line(b"data: not json\n".to_vec(), &node, "/api/watch").is_err());
}

#[test]
fn client_errors_map_like_python() {
    assert_eq!(ProxyError::from(ClientError::Timeout), ProxyError::Upstream);
    assert_eq!(
        ProxyError::from(ClientError::Invalid("bad status line")),
        ProxyError::Upstream
    );
    assert_eq!(
        ProxyError::from(ClientError::Invalid(NODE_RESPONSE_TOO_LARGE)),
        ProxyError::Invalid(NODE_RESPONSE_TOO_LARGE.into())
    );
    let response = json_response(StatusCode::OK, &json!({"ok": true}));
    assert_eq!(response.headers()["x-sessiondock-decoded-length"], "11");
}

#[test]
fn html_upstream_is_a_json_502_not_a_page_body() {
    assert_eq!(
        reject_html_upstream("text/html; charset=utf-8"),
        Err(ProxyError::Upstream)
    );
    assert_eq!(reject_html_upstream("text/html"), Err(ProxyError::Upstream));
    assert_eq!(reject_html_upstream("application/json"), Ok(()));
    assert_eq!(reject_html_upstream("text/event-stream"), Ok(()));
    assert_eq!(reject_html_upstream("application/octet-stream"), Ok(()));
}
