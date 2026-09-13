use std::path::Path;

use serde_json::{Value, json};

use super::super::Stamp;
use super::*;

const TS: &str = "2026-09-11T10:00:00Z";
const MTIME_NS: u128 = 1_789_000_000_123_456_789;

fn stamp(size: u64) -> Stamp {
    Stamp {
        dev: 1,
        ino: 7,
        size,
        mtime_ns: MTIME_NS,
    }
}

fn encoded(rows: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for row in rows {
        bytes.extend(serde_json::to_vec(row).unwrap());
        bytes.push(b'\n');
    }
    bytes
}

/// Slice a file exactly like the index reads it.
fn regions(bytes: &[u8]) -> (Vec<u8>, Vec<u8>, u64) {
    let size = bytes.len() as u64;
    if size <= TAIL_BYTES {
        let head = bytes[..bytes.len().min(HEAD_BYTES as usize)].to_vec();
        (head, bytes.to_vec(), 0)
    } else {
        let start = size - TAIL_BYTES;
        (
            bytes[..HEAD_BYTES as usize].to_vec(),
            bytes[start as usize..].to_vec(),
            start,
        )
    }
}

fn summarize_bytes(
    source: &'static str,
    path: &Path,
    bytes: &[u8],
    sidecar: Option<&[u8]>,
) -> RowSummary {
    let (head, tail, tail_start) = regions(bytes);
    let input = Input {
        source,
        path,
        data: Some(DataFile {
            head: &head,
            tail: &tail,
            tail_start,
            stamp: stamp(bytes.len() as u64),
        }),
        sidecar: sidecar.map(|bytes| SidecarBytes::Bytes {
            bytes,
            stamp: stamp(bytes.len() as u64),
        }),
    };
    summarize(&input)
}

fn claude_row(sid: &str, kind: &str, uid: &str, parent: Value, text: &str) -> Value {
    let mut row = json!({"type": kind, "uuid": uid, "parentUuid": parent, "sessionId": sid,
        "cwd": "/synthetic/history", "timestamp": TS, "isSidechain": false});
    if kind == "user" || kind == "assistant" {
        row["message"] = json!({"role": kind, "content": text});
    }
    row
}

fn claude_path() -> &'static Path {
    Path::new("/synthetic/root/-home-zj-project/claude-session-one.jsonl")
}

// ---------------------------------------------------------------------------
// Python primitives
// ---------------------------------------------------------------------------

#[test]
fn norm_ts_matches_python_shapes_and_iso_seconds_truncates() {
    assert_eq!(
        norm_ts(&json!("2026-09-11T10:00:00Z")).as_deref(),
        Some("2026-09-11T10:00:00.000Z")
    );
    assert_eq!(
        norm_ts(&json!("2026-09-11T18:00:00.123456+08:00")).as_deref(),
        Some("2026-09-11T10:00:00.123Z")
    );
    assert_eq!(
        norm_ts(&json!("2026-09-11 10:00:00")).as_deref(),
        Some("2026-09-11T10:00:00.000Z")
    );
    assert_eq!(
        norm_ts(&json!("2026-09-11")).as_deref(),
        Some("2026-09-11T00:00:00.000Z")
    );
    assert_eq!(
        norm_ts(&json!(1_789_000_000)).as_deref(),
        Some("2026-09-10T00:26:40.000Z")
    );
    assert_eq!(
        norm_ts(&json!(1_789_000_000_500u64)).as_deref(),
        Some("2026-09-10T00:26:40.500Z")
    );
    assert_eq!(norm_ts(&json!("")), None);
    assert_eq!(norm_ts(&json!("not a date")), None);
    assert_eq!(norm_ts(&Value::Null), None);
    assert_eq!(norm_ts(&json!(0)), None);
    assert_eq!(iso_seconds(MTIME_NS), "2026-09-10T00:26:40.000Z");
}

#[test]
fn clip_and_title_from_text_follow_the_reference_rules() {
    assert_eq!(clip("  a \u{1c} b\n\nc  ", 90), "a b c");
    assert_eq!(clip(&"名".repeat(91), 90), "名".repeat(90) + "…");
    assert_eq!(
        title_from_text("claude-000 #1 alpha\nsecond line about alpha\nthird line."),
        "claude-000 #1 alpha"
    );
    assert_eq!(
        title_from_text("```python\nprint(1, 'alpha')\n```"),
        "print(1, 'alpha')"
    );
    assert_eq!(
        title_from_text("**sid** record `1` — _stem_ 名"),
        "**sid** record `1` — _stem_ 名"
    );
    assert_eq!(
        title_from_text("# heading\n- item\n* star\nab"),
        "# heading - item * star ab"
    );
    assert_eq!(title_from_text(""), "(无标题)");
    assert_eq!(
        title_from_text("<system-reminder>ignored\nmulti</system-reminder>real question"),
        "real question"
    );
    assert_eq!(
        title_from_text(
            "<command-name>/status</command-name>\n<command-args>-v</command-args>after"
        ),
        "after"
    );
    assert_eq!(title_from_text("<b>bold</b> text here"), "bold text here");
    // An opening tag without its closing tag stays text (after short-tag removal).
    assert_eq!(
        title_from_text("<system-reminder>dangling text"),
        "dangling text"
    );
    assert_eq!(
        py_splitlines("a\r\nb\rc\u{2028}d\x0be"),
        vec!["a", "b", "c", "d", "e"]
    );
    assert_eq!(py_splitlines("x\n"), vec!["x"]);
}

#[test]
fn injection_and_local_shell_envelopes_match_python() {
    assert!(is_injected("hello <SYSTEM-REMINDER> world"));
    assert!(is_injected(
        "This session is being continued from a previous conversation."
    ));
    assert!(!is_injected("plain question"));
    let far = "x".repeat(2000) + "<system-reminder>";
    assert!(
        !is_injected(&far),
        "only the first 2000 characters are searched"
    );
    assert!(is_timeline_protocol("  # agents.md instructions\nrest"));
    assert!(!is_timeline_protocol(
        "text mentioning <system-reminder> later"
    ));
    assert_eq!(
        claude_bash_input(" <bash-input> ls &amp; pwd </bash-input>\n").as_deref(),
        Some("! ls & pwd")
    );
    assert_eq!(
        claude_bash_input("<BASH-INPUT>! echo</BASH-INPUT>").as_deref(),
        Some("! echo")
    );
    assert_eq!(claude_bash_input("<bash-input>  </bash-input>"), None);
    assert_eq!(claude_bash_input("<bash-input>x</bash-input> tail"), None);
    assert!(claude_bash_output_matches(
        "<bash-stdout>out</bash-stdout>\n<bash-stderr>err</bash-stderr>"
    ));
    assert!(claude_bash_output_matches(
        "<BASH-STDOUT>\nonly\n</BASH-STDOUT>"
    ));
    assert!(!claude_bash_output_matches(
        "<bash-stdout>out</bash-stdout> and more"
    ));
    assert!(!claude_bash_output_matches("discussing <bash-stdout> tags"));
    assert!(!claude_bash_output_matches("<bash-stdout>unterminated"));
    assert!(is_codex_protocol_injection(
        "anything",
        &json!({"content_item_kinds": ["goal.internal_context"]})
    ));
    assert!(is_codex_protocol_injection(
        "<environment_context>x",
        &Value::Null
    ));
    assert!(!is_codex_protocol_injection(
        "real",
        &json!({"content_item_kinds": ["text"]})
    ));
}

#[test]
fn flatten_text_keeps_text_parts_and_rejects_unreadable_shapes() {
    assert_eq!(flatten_text(&json!("plain")).unwrap(), "plain");
    assert_eq!(
        flatten_text(&json!([
            {"type": "text", "text": "a"}, "b", {"type": "thinking", "thinking": "t"},
            {"type": "tool_result", "content": "r"}, {"type": "image", "source": {}},
            {"type": "input_text", "text": "c"}, 42
        ]))
        .unwrap(),
        "a\nb\nc"
    );
    assert_eq!(flatten_text(&Value::Null).unwrap(), "");
    assert_eq!(
        flatten_text(&json!(42)).unwrap_err(),
        "原生 content 类型无效"
    );
    assert_eq!(
        flatten_text(&json!([{"type": "text", "text": 5}])).unwrap_err(),
        "原生文本块的 text 必须是字符串"
    );
    assert_eq!(
        unquote("%2Fsynthetic%2F%E4%B8%AD%E6%96%87+x"),
        "/synthetic/中文+x"
    );
}

// ---------------------------------------------------------------------------
// Head/tail regions
// ---------------------------------------------------------------------------

#[test]
fn head_counts_pieces_and_ignores_unterminated_lines() {
    let mut bytes = b"\n\n".to_vec();
    bytes.extend(encoded(&[json!({"n": 1}), json!({"n": 2})]));
    bytes.extend(b"{\"n\": 3}");
    let head = parse_head(&bytes, 3);
    assert_eq!(
        head.records
            .iter()
            .map(|r| r.value["n"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![1],
        "two blank pieces count toward the limit"
    );
    let head = parse_head(&bytes, 40);
    assert_eq!(
        head.records.len(),
        2,
        "the unterminated last line is not decoded"
    );
    assert_eq!(head.records[1].line, 3);
    assert!(head.corrupt.is_empty());
    let corrupt = parse_head(b"{bad}\n[1]\n42\n{\"ok\":1}\n", 40);
    assert_eq!(corrupt.corrupt, vec![0, 6, 10]);
    assert_eq!(corrupt.records.len(), 1);
}

#[test]
fn tail_drops_its_partial_first_line_only_when_it_starts_inside_the_file() {
    let blob = b"tial-line\n{\"a\":1}\n{\"a\":2}";
    let tail = parse_tail(blob, 100);
    assert_eq!(tail.records.len(), 1);
    assert_eq!(tail.records[0].start, 110);
    assert_eq!(tail.records[0].value, json!({"a": 1}));
    assert!(
        tail.corrupt.is_empty(),
        "the dropped partial piece is not corrupt"
    );
    let whole = parse_tail(b"{\"a\":1}\n{\"a\":2}\n", 0);
    assert_eq!(whole.records.len(), 2);
    let data = DataFile {
        head: b"{\"a\":1}\n{\"a\":2}\n{\"a\"",
        tail: b"{\"a\":1}\n{\"a\":2}\n{\"a\"",
        tail_start: 0,
        stamp: stamp(20),
    };
    assert_eq!(committed_end(&data), Some(16));
    assert_eq!(
        cursor_head(&data, 16).as_deref(),
        Some(format!("rs-m2-1:{}", &crate::sessions::hash(&data.head[..16])[..16]).as_str())
    );
}

// ---------------------------------------------------------------------------
// Claude
// ---------------------------------------------------------------------------

#[test]
fn claude_title_precedence_custom_then_latest_ai_then_generated_then_user() {
    let sid = "claude-session-one";
    let base = vec![
        json!({"type": "ai-title", "aiTitle": "Generated first", "sessionId": sid}),
        claude_row(sid, "user", "u0", Value::Null, "User question here"),
        json!({"type": "ai-title", "aiTitle": "Generated second", "sessionId": sid}),
    ];
    let summary = summarize_bytes("claude", claude_path(), &encoded(&base), None);
    assert_eq!(
        summary.title, "Generated second",
        "latest ai-title in the tail wins over the head's first"
    );
    let mut with_custom = base.clone();
    with_custom.push(json!({"type": "custom-title", "customTitle": " Custom  Title  kept raw ", "sessionId": sid}));
    with_custom.push(json!({"type": "ai-title", "aiTitle": "Generated third"}));
    let summary = summarize_bytes("claude", claude_path(), &encoded(&with_custom), None);
    assert_eq!(
        summary.title, " Custom  Title  kept raw ",
        "custom titles are neither clipped nor normalized"
    );
    let only_user = encoded(&[claude_row(
        sid,
        "user",
        "u0",
        Value::Null,
        "First line\nsecond",
    )]);
    let summary = summarize_bytes("claude", claude_path(), &only_user, None);
    assert_eq!(summary.title, "First line");
    assert_eq!(summary.sid, sid);
    assert_eq!(summary.cwd, "/synthetic/history");
    assert_eq!(summary.created, "2026-09-11T10:00:00.000Z");
    assert_eq!(summary.updated, "2026-09-11T10:00:00.000Z");
    assert_eq!(summary.native_id.as_deref().ok(), Some(sid));
    assert_eq!(summary.declared_ids, vec![sid.to_owned()]);
    assert!(summary.supported());
    assert_eq!(summary.committed, Some(only_user.len() as u64));
    let nothing = encoded(&[json!({"type": "mode", "mode": "default"})]);
    let summary = summarize_bytes("claude", claude_path(), &nothing, None);
    assert_eq!(
        summary.title, "claude-s",
        "stem[:8] when nothing else is available"
    );
    assert_eq!(summary.sid, "claude-session-one");
    assert_eq!(
        summary.cwd, "/home/zj/project",
        "project directory name fallback"
    );
    assert_eq!(
        summary.created, "2026-09-10T00:26:40.000Z",
        "mtime truncated to seconds"
    );
    assert_eq!(summary.updated, summary.created);
    assert_eq!(summary.warnings, vec!["跳过未知的Claude 记录类型：mode ×1"]);
    assert!(summary.native_id.is_err());
}

#[test]
fn claude_first_user_skips_injected_and_shell_output_but_keeps_bash_input() {
    let sid = "claude-session-one";
    let rows = encoded(&[
        claude_row(
            sid,
            "user",
            "u0",
            Value::Null,
            "<system-reminder>injected</system-reminder>",
        ),
        claude_row(
            sid,
            "user",
            "u1",
            json!("u0"),
            "<bash-stdout>out</bash-stdout>",
        ),
        claude_row(sid, "user", "u2", json!("u1"), "   "),
        json!({"type": "user", "uuid": "side", "sessionId": sid, "isSidechain": true,
               "message": {"role": "user", "content": "sidechain text"}}),
        claude_row(
            sid,
            "user",
            "u3",
            json!("u2"),
            "<bash-input>ls -la</bash-input>",
        ),
        claude_row(sid, "user", "u4", json!("u3"), "Later question"),
    ]);
    let summary = summarize_bytes("claude", claude_path(), &rows, None);
    assert_eq!(summary.title, "! ls -la");
    let rows = encoded(&[
        claude_row(
            sid,
            "user",
            "u0",
            Value::Null,
            "<system-reminder>injected</system-reminder>",
        ),
        claude_row(sid, "user", "u1", json!("u0"), "Real question"),
    ]);
    assert_eq!(
        summarize_bytes("claude", claude_path(), &rows, None).title,
        "Real question"
    );
}

#[test]
fn claude_tail_custom_title_and_cwd_majority_beyond_the_head() {
    let sid = "claude-session-one";
    let pad = "T".repeat(2048);
    let mut rows = Vec::new();
    let mut size = 0;
    let mut n = 0;
    while size < 700 * 1024 {
        let mut row = claude_row(
            sid,
            if n % 2 == 0 { "user" } else { "assistant" },
            &format!("t{n}"),
            Value::Null,
            &pad,
        );
        row.as_object_mut().unwrap().remove("cwd");
        size += serde_json::to_vec(&row).unwrap().len() + 1;
        rows.push(row);
        n += 1;
    }
    for cwd in [
        "/edge/minority",
        "/edge/majority",
        "/edge/majority",
        "/edge/majority",
    ] {
        let mut row = claude_row(sid, "user", "w", Value::Null, "cwd row");
        row["cwd"] = json!(cwd);
        row["timestamp"] = json!("2026-09-11T11:00:00Z");
        rows.push(row);
    }
    rows.push(json!({"type": "custom-title", "customTitle": "Edge tail title", "sessionId": sid}));
    let bytes = encoded(&rows);
    assert!(bytes.len() as u64 > TAIL_BYTES + HEAD_BYTES);
    let summary = summarize_bytes("claude", claude_path(), &bytes, None);
    assert_eq!(summary.title, "Edge tail title");
    assert_eq!(summary.cwd, "/edge/majority");
    assert_eq!(summary.updated, "2026-09-11T11:00:00.000Z");
    assert_eq!(summary.created, "2026-09-11T10:00:00.000Z");
    assert!(summary.supported(), "{:?}", summary.unsupported);
    assert_eq!(summary.warnings, Vec::<String>::new());
    assert_eq!(summary.committed, Some(bytes.len() as u64));
    // Ties keep the first cwd seen in the tail, like Python's dict order.
    let mut tie = Vec::new();
    for i in 0..40 {
        let mut row = claude_row(sid, "user", &format!("h{i}"), Value::Null, "head");
        row["cwd"] = json!("");
        tie.push(row);
    }
    for cwd in ["/first", "/second", "/second", "/first"] {
        let mut row = claude_row(sid, "user", "w", Value::Null, "cwd row");
        row["cwd"] = json!(cwd);
        tie.push(row);
    }
    assert_eq!(
        summarize_bytes("claude", claude_path(), &encoded(&tie), None).cwd,
        "/first"
    );
}

#[test]
fn claude_first_record_over_head_bytes_uses_head_limit_rules() {
    let sid = "claude-session-one";
    let huge = claude_row(
        sid,
        "user",
        "big",
        Value::Null,
        &("HugeHeadTitle ".to_owned() + &"X".repeat(96 * 1024)),
    );
    let mut rows = vec![huge];
    rows.push(claude_row(
        sid,
        "assistant",
        "a0",
        json!("big"),
        "tiny reply",
    ));
    let bytes = encoded(&rows);
    let summary = summarize_bytes("claude", claude_path(), &bytes, None);
    assert_eq!(
        summary.sid, "claude-session-one",
        "no head record: file stem"
    );
    assert_eq!(summary.title, "claude-s");
    assert_eq!(
        summary.cwd, "/synthetic/history",
        "tail records still count cwd"
    );
    assert_eq!(
        summary.created, "2026-09-10T00:26:40.000Z",
        "created falls back to mtime"
    );
    assert_eq!(
        summary.updated, "2026-09-11T10:00:00.000Z",
        "updated from the tail"
    );
    assert_eq!(
        summary.native_id.as_deref().ok(),
        Some(sid),
        "the tail (whole file) still declares the id"
    );
    assert!(summary.supported());
}

#[test]
fn claude_partial_tail_line_and_unterminated_append_are_not_errors() {
    let sid = "claude-session-one";
    let mut rows = Vec::new();
    let pad = "P".repeat(4096);
    for i in 0..200 {
        rows.push(claude_row(sid, "user", &format!("u{i}"), Value::Null, &pad));
    }
    let mut bytes = encoded(&rows);
    assert!(bytes.len() as u64 > TAIL_BYTES);
    bytes.extend(b"{\"type\":\"user\",\"in-progress\":true");
    let summary = summarize_bytes("claude", claude_path(), &bytes, None);
    assert!(summary.supported(), "{:?}", summary.unsupported);
    assert_eq!(summary.committed, Some(encoded(&rows).len() as u64));
}

#[test]
fn claude_corrupt_lines_are_notes_but_shape_errors_are_hard_failures_with_no_notes() {
    let sid = "claude-session-one";
    let mut bytes = encoded(&[
        json!({"type": "mode"}),
        claude_row(sid, "user", "u0", Value::Null, "ok"),
    ]);
    bytes.extend(b"{not json}\n");
    bytes.extend(b"[1, 2]\n");
    let summary = summarize_bytes("claude", claude_path(), &bytes, None);
    // Batch 35: skipped like Python `_head_lines`; counted after the kinds.
    assert!(summary.supported(), "{:?}", summary.unsupported);
    assert_eq!(
        summary.warnings,
        [
            "跳过未知的Claude 记录类型：mode ×1",
            "跳过无效的JSONL 记录 ×2"
        ]
    );
    assert_eq!(summary.migration_warnings(), summary.warnings);
    assert_eq!(summary.title, "ok");
    let scalar = encoded(&[
        claude_row(sid, "user", "u0", Value::Null, "ok"),
        json!({"type": "user", "uuid": "u1", "sessionId": sid, "message": {"role": "user", "content": 42}}),
    ]);
    let summary = summarize_bytes("claude", claude_path(), &scalar, None);
    assert_eq!(
        summary.unsupported.as_deref(),
        Some("原生 content 类型无效")
    );
    assert_eq!(summary.title, "ok", "fields are still derived");
}

#[test]
fn claude_unknown_kinds_are_counted_once_per_line_in_first_seen_order() {
    let sid = "claude-session-one";
    let rows = encoded(&[
        json!({"type": "mode", "mode": "default", "sessionId": sid}),
        json!({"type": "permission-mode", "sessionId": sid}),
        claude_row(sid, "user", "u0", Value::Null, "current CLI prompt"),
        json!({"type": "attachment", "uuid": "a1", "parentUuid": "u0", "sessionId": sid,
               "attachment": {"type": "environment"}}),
        json!({"type": "attachment", "uuid": "a2", "parentUuid": "a1", "sessionId": sid,
               "attachment": {"type": "hook_success"}}),
        json!({"type": "attachment", "uuid": "a3", "parentUuid": "a2", "sessionId": sid,
               "attachment": {"type": "queued_command", "commandMode": "prompt"}}),
        json!({"type": "atis-latch", "atis": {"latched": true}, "sessionId": sid}),
        claude_row(sid, "assistant", "c-a0", json!("a2"), "current CLI reply"),
        json!({"type": "atis-latch", "atis": {"latched": true}, "sessionId": sid}),
        json!({"type": "file-history-delta", "sessionId": sid}),
        json!({"type": "cost-state", "sessionId": sid}),
        json!({"type": "last-prompt", "leafUuid": "c-a0"}),
        json!({"type": "queue-operation", "operation": "popAll", "sessionId": sid}),
    ]);
    let summary = summarize_bytes("claude", claude_path(), &rows, None);
    assert!(summary.supported());
    assert_eq!(
        summary.warnings,
        vec![
            "跳过未知的Claude 记录类型：mode ×1",
            "跳过未知的Claude 记录类型：permission-mode ×1",
            "跳过未知的Claude attachment 类型：environment ×1",
            "跳过未知的Claude attachment 类型：hook_success ×1",
            "跳过未知的Claude 记录类型：atis-latch ×2",
            "跳过未知的Claude 记录类型：file-history-delta ×1",
            "跳过未知的Claude 记录类型：cost-state ×1",
        ]
    );
    assert_eq!(summary.title, "current CLI prompt");
}

#[test]
fn claude_sidecar_agent_uses_meta_json_labels_and_first_eight_lines() {
    let owner = "claude-session-one";
    let path = Path::new(
        "/synthetic/root/-home-zj-project/claude-session-one/subagents/agent-claude-agent-one.jsonl",
    );
    let mut rows = Vec::new();
    for i in 0..9 {
        rows.push(json!({"type": "mode", "sessionId": owner, "n": i}));
    }
    let mut first = claude_row(owner, "user", "au", Value::Null, "agent question");
    first["isSidechain"] = json!(true);
    first["agentId"] = json!("claude-agent-one");
    first["timestamp"] = json!("2026-09-11T12:00:00Z");
    rows.push(first);
    rows.push(claude_row(
        owner,
        "assistant",
        "aa",
        json!("au"),
        "agent answer",
    ));
    let bytes = encoded(&rows);
    let meta = br#"{"description": "Synthetic Claude child", "agentType": "reviewer"}"#;
    let summary = summarize_bytes("claude", path, &bytes, Some(meta));
    let agent = summary.agent.as_ref().expect("agent meta");
    assert_eq!(agent.id, "claude-agent-one");
    assert_eq!(agent.title, "Synthetic Claude child");
    assert_eq!(agent.kind, "reviewer");
    assert_eq!(summary.sid, "claude-agent-one");
    assert_eq!(summary.title, "Synthetic Claude child");
    assert_eq!(
        summary.created, "2026-09-10T00:26:40.000Z",
        "no timestamp in the first 8 pieces: meta.json mtime"
    );
    assert_eq!(summary.updated, "2026-09-11T10:00:00.000Z");
    assert_eq!(summary.native_id.as_deref().ok(), Some(owner));
    assert_eq!(summary.cwd, "/synthetic/history");
    let summary = summarize_bytes("claude", path, &bytes, None);
    assert_eq!(summary.agent.as_ref().unwrap().title, "子代理 claude-a");
    assert_eq!(summary.agent.as_ref().unwrap().kind, "subagent");
    assert_eq!(
        summary.created, summary.updated,
        "no sidecar: created falls back to updated"
    );
    let summary = summarize_bytes("claude", path, &bytes, Some(b"{not json"));
    assert_eq!(
        summary.unsupported.as_deref(),
        Some("子代理元数据 (meta.json) 不是完整有效的 JSON 对象")
    );
    assert_eq!(
        claude::owner_path(path).unwrap(),
        Path::new("/synthetic/root/-home-zj-project/claude-session-one.jsonl")
    );
    assert_eq!(claude::agent_id(claude_path()), None);
}

// ---------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------

fn codex_path() -> &'static Path {
    Path::new("/synthetic/root/2026/09/11/rollout-2026-09-11T09-00-00-codex-session-one.jsonl")
}

fn codex_row(kind: &str, payload: Value) -> Value {
    json!({"type": kind, "payload": payload, "timestamp": TS})
}

fn codex_message(role: &str, text: &str) -> Value {
    codex_row(
        "response_item",
        json!({"type": "message", "role": role,
        "content": [{"type": if role == "user" { "input_text" } else { "output_text" }, "text": text}]}),
    )
}

#[test]
fn codex_main_rollout_fields_follow_raw_meta() {
    let rows = encoded(&[
        codex_row(
            "session_meta",
            json!({"id": "codex-one", "session_id": "codex-one",
            "timestamp": "2026-09-11T09:00:00Z", "cwd": "/synthetic/codex", "forked_from_id": null}),
        ),
        codex_row("turn_context", json!({"model": ""})),
        codex_row("turn_context", json!({"model": "synthetic-model"})),
        codex_row(
            "event_msg",
            json!({"type": "task_started", "turn_id": "t1"}),
        ),
        codex_message(
            "user",
            "<environment_context>injected</environment_context>",
        ),
        codex_message("user", "Codex question\nsecond line"),
        codex_row("token_usage_record", json!({"turn_id": "t1"})),
        codex_row("event_msg", json!({"type": "item_completed"})),
        codex_row("response_item", json!({"type": "agent_message"})),
        codex_message("assistant", "Codex answer"),
    ]);
    let summary = summarize_bytes("codex", codex_path(), &rows, None);
    assert_eq!(summary.sid, "codex-one");
    assert_eq!(summary.title, "Codex question");
    assert_eq!(summary.cwd, "/synthetic/codex");
    assert_eq!(summary.model, json!("synthetic-model"));
    assert_eq!(summary.branch, Value::Null);
    assert_eq!(summary.created, "2026-09-11T09:00:00.000Z");
    assert_eq!(
        summary.updated, "2026-09-10T00:26:40.000Z",
        "main rollouts: file mtime"
    );
    let codex = summary.codex.as_ref().unwrap();
    assert!(codex.has_meta);
    assert_eq!(codex.forked_from_id, "");
    assert_eq!(codex.history_base, Value::Null);
    assert_eq!(summary.agent, None);
    assert_eq!(summary.native_id.as_deref().ok(), Some("codex-one"));
    assert_eq!(
        summary.warnings,
        vec![
            "跳过未知的Codex 记录类型：token_usage_record ×1",
            "跳过未知的Codex event_msg：item_completed ×1",
            "跳过未知的Codex response_item：agent_message ×1",
        ]
    );
    let empty = encoded(&[codex_row("turn_context", json!({}))]);
    let summary = summarize_bytes("codex", codex_path(), &empty, None);
    assert_eq!(summary.sid, "rollout-2026-09-11T09-00-00-codex-session-one");
    assert_eq!(summary.title, "(无标题) 2026-09-11T09-00");
    assert_eq!(summary.cwd, "(未知)");
    assert_eq!(summary.created, "2026-09-10T00:26:40.000Z");
    assert!(!summary.codex.as_ref().unwrap().has_meta);
    assert!(summary.native_id.is_err());
}

#[test]
fn codex_head_reads_120_pieces_and_subagents_take_the_tail_timestamp() {
    let mut rows = Vec::new();
    for i in 0..100 {
        rows.push(codex_row(
            "event_msg",
            json!({"type": "token_count", "n": i}),
        ));
    }
    rows.push(codex_row("session_meta", json!({"id": "agent-one", "session_id": "codex-parent",
        "timestamp": "2026-09-11T09:30:00Z", "cwd": "/synthetic/agent", "thread_source": "subagent",
        "parent_thread_id": "codex-parent", "forked_from_id": "codex-parent",
        "source": {"subagent": {"thread_spawn": {"agent_path": "worker/one", "agent_role": "explorer"}}}})));
    let mut last = codex_message("assistant", "agent answer");
    last["timestamp"] = json!("2026-09-11T09:45:00Z");
    rows.push(last);
    let summary = summarize_bytes("codex", codex_path(), &encoded(&rows), None);
    assert_eq!(
        summary.sid, "agent-one",
        "subagents are keyed by id, not the parent session_id"
    );
    let agent = summary.agent.as_ref().expect("subagent");
    assert_eq!(agent.title, "worker/one");
    assert_eq!(agent.kind, "explorer");
    assert_eq!(
        summary.codex.as_ref().unwrap().parent_thread_id,
        "codex-parent"
    );
    assert_eq!(summary.updated, "2026-09-11T09:45:00.000Z");
    assert_eq!(summary.created, "2026-09-11T09:30:00.000Z");
    assert_eq!(summary.title, "(无标题) 2026-09-11T09-00");
    assert_eq!(summary.native_id.as_deref().ok(), Some("agent-one"));
}

#[test]
fn codex_duplicate_meta_bad_history_base_and_internal_context_rules() {
    // Python `_raw_meta` (`and not meta`): a later session_meta is ignored;
    // the row counts it and stays supported.
    let duplicate = encoded(&[
        codex_row(
            "session_meta",
            json!({"id": "codex-one", "session_id": "codex-one"}),
        ),
        codex_message("user", "q"),
        codex_row(
            "session_meta",
            json!({"id": "codex-one", "session_id": "codex-one"}),
        ),
        codex_row("token_usage_record", json!({})),
    ]);
    let summary = summarize_bytes("codex", codex_path(), &duplicate, None);
    assert_eq!(summary.unsupported, None);
    assert_eq!(
        summary.warnings,
        vec![
            "跳过重复的Codex session_meta ×1",
            "跳过未知的Codex 记录类型：token_usage_record ×1",
        ],
        "the duplicate-meta line leads, like the view's Skipped order"
    );
    let bad_base = encoded(&[codex_row(
        "session_meta",
        json!({"id": "x", "history_base": "nope"}),
    )]);
    let summary = summarize_bytes("codex", codex_path(), &bad_base, None);
    assert_eq!(
        summary.unsupported.as_deref(),
        Some("Codex history_base 必须是对象或 null；拒绝忽略损坏的继承身份")
    );
    assert_eq!(summary.codex.as_ref().unwrap().history_base, Value::Null);
    // The KEEP check applies to the first meta only: a copied ancestor meta
    // with a broken history_base is skipped like any other later meta.
    let later_bad_base = encoded(&[
        codex_row("session_meta", json!({"id": "x", "session_id": "x"})),
        codex_row("session_meta", json!({"id": "y", "history_base": "nope"})),
    ]);
    let summary = summarize_bytes("codex", codex_path(), &later_bad_base, None);
    assert_eq!(summary.unsupported, None);
    assert_eq!(summary.warnings, vec!["跳过重复的Codex session_meta ×1"]);
    assert_eq!(summary.native_id.as_deref().ok(), Some("x"));
    assert_eq!(summary.declared_ids, vec!["x"]);
    let internal = encoded(&[
        codex_row("session_meta", json!({"id": "x", "session_id": "x"})),
        codex_row(
            "response_item",
            json!({"type": "message", "role": "user",
            "content": [{"type": "input_text", "text": "hidden context"}],
            "internal_chat_message_metadata_passthrough": {"content_item_kinds": ["goal.internal_context"]}}),
        ),
        codex_message(
            "user",
            "<turn_aborted>Turn aborted</turn_aborted>\nVisible question",
        ),
    ]);
    let summary = summarize_bytes("codex", codex_path(), &internal, None);
    assert_eq!(
        summary.title, "Turn aborted",
        "Python does not strip the abort prefix for titles"
    );
    let fork = encoded(&[codex_row(
        "session_meta",
        json!({"id": "child", "session_id": "child",
        "forked_from_id": "parent", "history_base": {"thread_id": "parent", "end_byte_offset": 12}}),
    )]);
    let summary = summarize_bytes("codex", codex_path(), &fork, None);
    let codex = summary.codex.as_ref().unwrap();
    assert_eq!(codex.forked_from_id, "parent");
    assert_eq!(
        codex.history_base,
        json!({"thread_id": "parent", "end_byte_offset": 12})
    );
}

/// Old-style fork (2026-07/08): line 0 is the own meta (`forked_from_id`
/// = parent, `history_base` null), then one copied `session_meta` per
/// ancestor, then the copied history. Python takes the first meta everywhere
/// and reads the file alone; the copied ids are not this file's identity.
#[test]
fn codex_legacy_fork_identity_is_the_first_meta_and_copied_metas_are_counted() {
    let rows = encoded(&[
        codex_row(
            "session_meta",
            json!({"id": "child", "session_id": "child", "forked_from_id": "parent",
            "history_base": null, "timestamp": "2026-07-01T09:00:00Z", "cwd": "/synthetic/child"}),
        ),
        codex_row(
            "session_meta",
            json!({"id": "parent", "session_id": "parent", "forked_from_id": "grandparent",
            "history_base": null, "timestamp": "2026-06-30T09:00:00Z", "cwd": "/synthetic/parent"}),
        ),
        codex_row(
            "session_meta",
            json!({"id": "grandparent", "session_id": "grandparent", "forked_from_id": "root",
            "history_base": {"thread_id": "root", "end_byte_offset": 99}, "cwd": "/synthetic/grandparent"}),
        ),
        codex_row(
            "session_meta",
            json!({"id": "root", "session_id": "root", "timestamp": "2026-06-01T09:00:00Z", "cwd": "/synthetic/root"}),
        ),
        codex_message("user", "copied ancestor question"),
        codex_message("assistant", "copied ancestor answer"),
        codex_message("user", "own question"),
    ]);
    let summary = summarize_bytes("codex", codex_path(), &rows, None);
    assert_eq!(summary.unsupported, None);
    assert!(summary.supported());
    assert_eq!(summary.sid, "child");
    assert_eq!(summary.created, "2026-07-01T09:00:00.000Z");
    assert_eq!(summary.cwd, "/synthetic/child");
    assert_eq!(summary.title, "copied ancestor question");
    let codex = summary.codex.as_ref().unwrap();
    assert!(codex.has_meta);
    assert_eq!(codex.forked_from_id, "parent");
    assert_eq!(codex.history_base, Value::Null);
    assert_eq!(summary.agent, None);
    assert_eq!(summary.warnings, vec!["跳过重复的Codex session_meta ×3"]);
    assert_eq!(summary.migration_warnings(), summary.warnings);
    assert_eq!(summary.native_id.as_deref().ok(), Some("child"));
    assert_eq!(
        summary.declared_ids,
        vec!["child"],
        "copied ancestor ids must not map the parents' SIDs to this file"
    );
}

/// Subagent rollout with the parent's meta copied on line 1: the agent's
/// own record (line 0) is the identity; `session_id` names the parent.
#[test]
fn codex_subagent_rollout_with_copied_parent_meta_keeps_its_own_identity() {
    let rows = encoded(&[
        codex_row(
            "session_meta",
            json!({"id": "agent-one", "session_id": "codex-parent", "thread_source": "subagent",
            "forked_from_id": "codex-parent", "parent_thread_id": "codex-parent",
            "history_base": null, "timestamp": "2026-08-01T09:30:00Z", "cwd": "/synthetic/agent",
            "source": {"subagent": {"thread_spawn": {"agent_path": "worker/one", "agent_role": "explorer"}}}}),
        ),
        codex_row(
            "session_meta",
            json!({"id": "codex-parent", "session_id": "codex-parent",
            "timestamp": "2026-08-01T09:00:00Z", "cwd": "/synthetic/parent"}),
        ),
        codex_message("user", "agent task"),
    ]);
    let summary = summarize_bytes("codex", codex_path(), &rows, None);
    assert_eq!(summary.unsupported, None);
    assert_eq!(summary.sid, "agent-one");
    let agent = summary.agent.as_ref().expect("subagent");
    assert_eq!(agent.id, "agent-one");
    assert_eq!(agent.title, "worker/one");
    assert_eq!(agent.kind, "explorer");
    assert_eq!(
        summary.codex.as_ref().unwrap().parent_thread_id,
        "codex-parent"
    );
    assert_eq!(summary.created, "2026-08-01T09:30:00.000Z");
    assert_eq!(summary.cwd, "/synthetic/agent");
    assert_eq!(summary.warnings, vec!["跳过重复的Codex session_meta ×1"]);
    assert_eq!(summary.native_id.as_deref().ok(), Some("agent-one"));
    assert_eq!(summary.declared_ids, vec!["agent-one"]);
}

// ---------------------------------------------------------------------------
// Grok
// ---------------------------------------------------------------------------

fn grok_path() -> &'static Path {
    Path::new("/synthetic/root/%2Fsynthetic%2F%E4%B8%AD%E6%96%87/grok-session-one")
}

#[test]
fn grok_rows_come_from_summary_json_with_python_fallbacks() {
    let summary_json = r#"{"generated_title": "Grok 摘要标题", "session_summary": "only summary",
        "info": {"id": "grok-summary", "cwd": "/synthetic/中文"},
        "created_at": "2026-09-11T08:00:00Z", "updated_at": "2026-09-11T08:30:00Z",
        "current_model_id": "grok-test", "agent_name": "grok-branch"}"#
        .as_bytes();
    let chat = encoded(&[
        json!({"type": "user", "content": "hello", "timestamp": "2026-09-11T09:00:00Z"}),
        json!({"type": "usage", "tokens": 1}),
        json!({"type": "usage", "tokens": 2}),
        json!({"type": "checkpoint"}),
    ]);
    let summary = summarize_bytes("grok", grok_path(), &chat, Some(summary_json));
    assert_eq!(summary.sid, "grok-summary");
    assert_eq!(summary.title, "Grok 摘要标题");
    assert_eq!(summary.cwd, "/synthetic/中文");
    assert_eq!(summary.created, "2026-09-11T08:00:00.000Z");
    assert_eq!(
        summary.updated, "2026-09-11T08:30:00.000Z",
        "chat records never override the summary"
    );
    assert_eq!(summary.model, json!("grok-test"));
    assert_eq!(summary.branch, json!("grok-branch"));
    assert_eq!(summary.size, chat.len() as u64 + summary_json.len() as u64);
    assert!(summary.grok.as_ref().unwrap().chat_exists);
    assert_eq!(
        summary.warnings,
        vec![
            "跳过未知的Grok 记录类型：usage ×2",
            "跳过未知的Grok 记录类型：checkpoint ×1"
        ]
    );
    // WP-E: the validated summary's `info.id` is the Grok native identity.
    assert_eq!(summary.native_id.as_deref().ok(), Some("grok-summary"));
    assert_eq!(summary.declared_ids, vec!["grok-summary".to_owned()]);

    let minimal = br#"{"last_active_at": "2026-09-11 12:00:00"}"#;
    let input = Input {
        source: "grok",
        path: grok_path(),
        data: None,
        sidecar: Some(SidecarBytes::Bytes {
            bytes: minimal,
            stamp: stamp(minimal.len() as u64),
        }),
    };
    let summary = summarize(&input);
    assert_eq!(summary.sid, "grok-session-one");
    assert_eq!(summary.title, "grok-ses");
    assert_eq!(summary.cwd, "/synthetic/中文");
    assert_eq!(
        summary.created, "2026-09-10T00:26:40.000Z",
        "summary mtime when the chat is absent"
    );
    assert_eq!(summary.updated, "2026-09-11T12:00:00.000Z");
    assert!(!summary.grok.as_ref().unwrap().chat_exists);
    assert_eq!(summary.size, minimal.len() as u64);
    assert!(summary.supported());
}

#[test]
fn grok_invalid_summary_is_unsupported_but_still_a_row() {
    let summary = summarize_bytes("grok", grok_path(), b"", Some(b"{not json"));
    assert_eq!(
        summary.unsupported.as_deref(),
        Some("Grok summary.json 不是完整有效的 JSON")
    );
    assert_eq!(summary.title, "grok-ses");
    let summary = summarize_bytes("grok", grok_path(), b"", Some(br#"{"info": []}"#));
    assert_eq!(
        summary.unsupported.as_deref(),
        Some("Grok summary.json 的 info 必须是对象或 null")
    );
    // Batch 35: a corrupt chat line is a note, never a hard failure.
    let corrupt = summarize_bytes("grok", grok_path(), b"{bad\n", Some(b"{}"));
    assert!(corrupt.supported(), "{:?}", corrupt.unsupported);
    assert_eq!(corrupt.warnings, ["跳过无效的JSONL 记录 ×1"]);
}

// ---------------------------------------------------------------------------
// Batch 35 (WP-C): Claude torn / invalid lines are skipped like Python
// `_head_lines` / `_tail_lines` and counted as one non-fatal note.
// ---------------------------------------------------------------------------

/// The observed real-root shape: one complete line of 4096 NUL bytes plus a
/// record tail between two valid records (head and tail both see it in a
/// small file; the count is per physical line).
#[test]
fn claude_torn_nul_line_is_one_note_and_the_row_stays_supported() {
    let sid = "claude-session-one";
    let mut bytes = encoded(&[
        claude_row(sid, "user", "u1", Value::Null, "first question"),
        claude_row(sid, "assistant", "a1", json!("u1"), "first answer"),
    ]);
    let torn_start = bytes.len() as u64;
    bytes.extend(std::iter::repeat_n(0u8, 4096));
    bytes.extend("\"…tail\"}\n".as_bytes());
    let tail = encoded(&[
        claude_row(sid, "user", "u2", json!("x"), "second question"),
        claude_row(sid, "assistant", "a2", json!("u2"), "second answer"),
    ]);
    bytes.extend(&tail);
    let summary = summarize_bytes("claude", claude_path(), &bytes, None);
    assert!(summary.supported(), "{:?}", summary.unsupported);
    assert_eq!(summary.warnings, ["跳过无效的JSONL 记录 ×1"]);
    assert_eq!(summary.migration_warnings(), ["跳过无效的JSONL 记录 ×1"]);
    assert_eq!(summary.title, "first question");
    assert_eq!(summary.sid, sid);
    assert_eq!(summary.native_id.as_deref().ok(), Some(sid));
    assert_eq!(summary.committed, Some(bytes.len() as u64));
    // Both regions saw the same physical line once.
    let (head, tail_bytes, tail_start) = regions(&bytes);
    let records = Records::parse(
        &DataFile {
            head: &head,
            tail: &tail_bytes,
            tail_start,
            stamp: stamp(bytes.len() as u64),
        },
        CLAUDE_HEAD_LINES,
    );
    assert_eq!(records.head.corrupt, vec![torn_start]);
    assert_eq!(records.tail.corrupt, vec![torn_start]);
    assert_eq!(records.corrupt_lines(), 1);
    assert_eq!(records.all().count(), 4);
    // A torn line only in the tail of a large file counts the same way.
    let pad = "P".repeat(4096);
    let mut large = Vec::new();
    for i in 0..200 {
        large.extend(encoded(&[claude_row(
            sid,
            "user",
            &format!("u{i}"),
            Value::Null,
            &pad,
        )]));
    }
    assert!(large.len() as u64 > TAIL_BYTES);
    large.extend(std::iter::repeat_n(0u8, 4096));
    large.extend("\"…tail\"}\n".as_bytes());
    large.extend(&tail);
    let summary = summarize_bytes("claude", claude_path(), &large, None);
    assert!(summary.supported());
    assert_eq!(summary.warnings, ["跳过无效的JSONL 记录 ×1"]);
    assert_eq!(summary.committed, Some(large.len() as u64));
}

// ---------------------------------------------------------------------------
// Batch 36 (WP-C): turn state of agent files and the Claude `continued-in` sid.
// ---------------------------------------------------------------------------

fn sidecar_path() -> &'static Path {
    Path::new("/synthetic/root/-home-zj-project/claude-session-one/subagents/agent-worker.jsonl")
}

fn sidecar_assistant(ts: &str, stop_reason: Value) -> Value {
    json!({"type": "assistant", "isSidechain": true, "agentId": "worker", "timestamp": ts,
        "sessionId": "claude-session-one",
        "message": {"role": "assistant", "stop_reason": stop_reason,
                    "content": [{"type": "text", "text": "结论"}]}})
}

fn sidecar_user(ts: &str) -> Value {
    json!({"type": "user", "isSidechain": true, "agentId": "worker", "timestamp": ts,
        "sessionId": "claude-session-one", "message": {"role": "user", "content": "继续"}})
}

fn open_turn(rows: &[Value]) -> bool {
    summarize_bytes("claude", sidecar_path(), &encoded(rows), None)
        .agent
        .expect("sidecar")
        .open_turn
}

#[test]
fn claude_sidecar_turn_is_closed_only_by_an_assistant_end_turn() {
    // Python `_claude_agent_tail` / `_CLAUDE_TURN_CLOSED`.
    let user = sidecar_user("2026-09-12T00:10:00Z");
    assert!(!open_turn(&[
        user.clone(),
        sidecar_assistant("2026-09-12T00:12:00Z", json!("end_turn"))
    ]));
    for reason in [
        Value::Null,
        json!("tool_use"),
        json!("refusal"),
        json!("stop_sequence"),
    ] {
        assert!(
            open_turn(&[
                user.clone(),
                sidecar_assistant("2026-09-12T00:12:00Z", reason.clone())
            ]),
            "{reason} does not close the turn"
        );
    }
    assert!(
        open_turn(std::slice::from_ref(&user)),
        "a user record opens a turn"
    );
    assert!(open_turn(&[
        user.clone(),
        sidecar_assistant("2026-09-12T00:12:00Z", json!("end_turn")),
        sidecar_user("2026-09-12T00:20:00Z"),
    ]));
    // Only user/assistant records decide; trailing progress records are
    // passed over, and no such record at all means closed.
    assert!(!open_turn(&[
        user.clone(),
        sidecar_assistant("2026-09-12T00:12:00Z", json!("end_turn")),
        json!({"type": "progress", "timestamp": "2026-09-12T00:13:00Z"}),
    ]));
    assert!(!open_turn(&[
        json!({"type": "progress", "timestamp": "2026-09-12T00:13:00Z"})
    ]));
    let summary = summarize_bytes(
        "claude",
        sidecar_path(),
        &encoded(&[
            user,
            sidecar_assistant("2026-09-12T00:12:00Z", json!("tool_use")),
        ]),
        None,
    );
    assert_eq!(summary.updated, "2026-09-12T00:12:00.000Z");
    assert_eq!(summary.created, "2026-09-12T00:10:00.000Z");
    assert_eq!(summary.continued_in_sid, None);
}

#[test]
fn codex_subagent_turn_follows_the_latest_turn_boundary_event() {
    // Python `_codex_agent_tail` / `_CODEX_TURN_OPEN`.
    let meta = codex_row(
        "session_meta",
        json!({"id": "agent-one", "session_id": "codex-parent",
        "timestamp": "2026-09-11T09:30:00Z", "cwd": "/synthetic/agent", "thread_source": "subagent",
        "parent_thread_id": "codex-parent"}),
    );
    let event = |kind: &str| codex_row("event_msg", json!({"type": kind, "turn_id": "t"}));
    let open = |rows: &[Value]| {
        let mut all = vec![meta.clone()];
        all.extend(rows.iter().cloned());
        summarize_bytes("codex", codex_path(), &encoded(&all), None)
            .agent
            .expect("subagent")
            .open_turn
    };
    assert!(!open(&[]));
    assert!(open(&[event("task_started")]));
    assert!(open(&[event("turn_started")]));
    assert!(!open(&[event("task_started"), event("task_complete")]));
    assert!(!open(&[event("task_started"), event("turn_complete")]));
    assert!(!open(&[event("task_started"), event("turn_aborted")]));
    assert!(open(&[
        event("task_started"),
        event("task_complete"),
        event("task_started")
    ]));
    // Other event kinds are passed over, not treated as boundaries.
    assert!(open(&[
        event("task_started"),
        codex_row("response_item", json!({"type": "reasoning"})),
        event("token_count"),
    ]));
    assert!(!open(&[event("task_complete"), event("agent_message")]));
    // A main rollout has no agent meta at all.
    let main = codex_row(
        "session_meta",
        json!({"id": "codex-main", "session_id": "codex-main",
        "timestamp": "2026-09-11T09:30:00Z", "cwd": "/synthetic"}),
    );
    let summary = summarize_bytes(
        "codex",
        codex_path(),
        &encoded(&[main, event("task_started")]),
        None,
    );
    assert!(summary.agent.is_none());
    assert_eq!(summary.continued_in_sid, None);
}

#[test]
fn claude_continued_in_sid_is_the_last_tail_record_with_a_truthy_id() {
    let sid = "claude-session-one";
    let continued = |target: Value| {
        json!({"type": "continued-in", "sessionId": sid, "continuedInSessionId": target,
            "timestamp": "2026-09-11T12:00:00Z"})
    };
    let rows = vec![
        claude_row(sid, "user", "u0", Value::Null, "hello"),
        continued(json!("sid-first")),
        continued(json!("sid-last")),
        continued(json!("")),
        continued(Value::Null),
        json!({"type": "continued-in", "sessionId": sid, "timestamp": "2026-09-11T12:00:00Z"}),
    ];
    let summary = summarize_bytes("claude", claude_path(), &encoded(&rows), None);
    assert_eq!(summary.continued_in_sid.as_deref(), Some("sid-last"));
    // The projection (`providers/claude.rs`) passes `continued-in` over as
    // an unknown kind, like Python `read`; the row counts it the same way
    // so list and detail agree.
    assert_eq!(
        summary.warnings,
        ["跳过未知的Claude 记录类型：continued-in ×5"]
    );
    // A non-string id is `str()`-ed like Python.
    let summary = summarize_bytes(
        "claude",
        claude_path(),
        &encoded(&[
            claude_row(sid, "user", "u0", Value::Null, "hello"),
            continued(json!(7)),
        ]),
        None,
    );
    assert_eq!(summary.continued_in_sid.as_deref(), Some("7"));
    // Beyond the tail window it is not seen (bounded summary).
    let mut padded = vec![claude_row(sid, "user", "u0", Value::Null, "hello")];
    padded.push(continued(json!("sid-old")));
    while encoded(&padded).len() < TAIL_BYTES as usize + 4096 {
        padded.push(claude_row(sid, "user", "p", Value::Null, &"P".repeat(2048)));
    }
    let summary = summarize_bytes("claude", claude_path(), &encoded(&padded), None);
    assert_eq!(summary.continued_in_sid, None);
    let summary = summarize_bytes(
        "claude",
        claude_path(),
        &encoded(&[claude_row(sid, "user", "u0", Value::Null, "hello")]),
        None,
    );
    assert_eq!(summary.continued_in_sid, None);
}
