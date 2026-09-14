use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::graph::{self, CutCheck};
use super::summary::{self, DataFile, Input, RowSummary, TAIL_BYTES};
use super::*;
use crate::sessions::{SessionRoots, SessionStore};

const TS: &str = "2026-09-11T10:00:00Z";

fn encoded(rows: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for row in rows {
        bytes.extend(serde_json::to_vec(row).unwrap());
        bytes.push(b'\n');
    }
    bytes
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn claude_row(sid: &str, kind: &str, uid: &str, parent: Value, text: &str) -> Value {
    let mut row = json!({"type": kind, "uuid": uid, "parentUuid": parent, "sessionId": sid,
        "cwd": "/synthetic/history", "timestamp": TS, "isSidechain": false});
    if kind == "user" || kind == "assistant" {
        row["message"] = json!({"role": kind, "content": text});
        if kind == "assistant" {
            row["message"]["stop_reason"] = json!("end_turn");
        }
    } else if !text.is_empty() {
        row["content"] = json!(text);
    }
    row
}

fn codex_row(kind: &str, payload: Value, ordinal: u64) -> Value {
    json!({"type": kind, "payload": payload, "ordinal": ordinal, "timestamp": TS})
}

fn codex_message(role: &str, text: &str, ordinal: u64) -> Value {
    codex_row(
        "response_item",
        json!({"type": "message", "role": role,
            "content": [{"type": if role == "user" { "input_text" } else { "output_text" }, "text": text}]}),
        ordinal,
    )
}

fn codex_meta(sid: &str, extra: Value) -> Value {
    let mut payload = json!({"id": sid, "session_id": sid, "timestamp": "2026-09-11T09:00:00Z",
        "cwd": "/synthetic/history"});
    for (key, value) in extra.as_object().into_iter().flatten() {
        payload[key] = value.clone();
    }
    codex_row("session_meta", payload, 0)
}

/// The history_parity / sessions_list_suite corpora, written the way those
/// helpers do (every source, agents, forks, orphans, cycles, bad cuts, Grok).
struct Corpus {
    root: PathBuf,
    paths: BTreeMap<String, PathBuf>,
}

impl Corpus {
    fn new(root: &Path) -> Self {
        for source in ["claude", "codex", "grok"] {
            fs::create_dir_all(root.join(source)).unwrap();
        }
        Self {
            root: root.to_path_buf(),
            paths: BTreeMap::new(),
        }
    }

    fn put(&mut self, sid: &str, source: &str, rows: &[Value]) -> PathBuf {
        let path = if source == "claude" {
            self.root
                .join("claude/project-history")
                .join(format!("{sid}.jsonl"))
        } else {
            self.root
                .join("codex/2026/09/11")
                .join(format!("rollout-{sid}.jsonl"))
        };
        write(&path, &encoded(rows));
        self.paths.insert(sid.to_owned(), path.clone());
        path
    }

    fn uid(&self, sid: &str) -> String {
        let path = &self.paths[sid];
        let source = if path.starts_with(self.root.join("claude")) {
            "claude"
        } else if path.starts_with(self.root.join("codex")) {
            "codex"
        } else {
            "grok"
        };
        uid_for(source, path)
    }

    fn roots(&self) -> SessionRoots {
        SessionRoots {
            claude: Some(self.root.join("claude")),
            codex: Some(self.root.join("codex")),
            grok: Some(self.root.join("grok")),
        }
    }
}

fn build_corpus(root: &Path) -> Corpus {
    let mut corpus = Corpus::new(root);
    for (sid, compact) in [
        ("claude-branch", None),
        ("claude-compact", Some("legacy")),
        ("claude-current-compact", Some("current")),
    ] {
        let mut rows = vec![
            claude_row(sid, "user", "u0", Value::Null, "Claude common question"),
            claude_row(
                sid,
                "assistant",
                "a0",
                json!("u0"),
                "Claude selected answer",
            ),
            claude_row(
                sid,
                "user",
                "old-u",
                json!("a0"),
                "Claude discarded completed question",
            ),
            claude_row(
                sid,
                "assistant",
                "old-a",
                json!("old-u"),
                "Claude discarded completed answer",
            ),
            json!({"type": "last-prompt", "leafUuid": "a0"}),
        ];
        if let Some(compact) = compact {
            let mut boundary = claude_row(sid, "system", "compact", Value::Null, "");
            if compact == "legacy" {
                boundary["subtype"] = json!("compact_boundary");
            } else {
                boundary["compactMetadata"] = json!({"trigger": "manual", "durationMs": 1200});
            }
            let mut summary = claude_row(
                sid,
                "user",
                "summary",
                json!("compact"),
                "INTERNAL COMPACT SUMMARY",
            );
            summary["isCompactSummary"] = json!(true);
            rows.extend([
                boundary,
                summary,
                claude_row(
                    sid,
                    "user",
                    "compact-u",
                    json!("summary"),
                    "Claude post compact question",
                ),
                claude_row(
                    sid,
                    "assistant",
                    "compact-a",
                    json!("compact-u"),
                    "Claude post compact answer",
                ),
            ]);
        }
        corpus.put(sid, "claude", &rows);
    }
    let sid = "claude-abandoned";
    corpus.put(
        sid,
        "claude",
        &[
            claude_row(sid, "user", "u0", Value::Null, "Claude abandoned common"),
            claude_row(
                sid,
                "assistant",
                "a0",
                json!("u0"),
                "Claude abandoned base answer",
            ),
            claude_row(sid, "user", "escaped", json!("a0"), "Claude fast Esc input"),
            claude_row(
                sid,
                "user",
                "replacement",
                json!("a0"),
                "Claude replacement input",
            ),
            claude_row(
                sid,
                "assistant",
                "replacement-a",
                json!("replacement"),
                "Claude replacement answer",
            ),
        ],
    );
    // Claude Code 2.1.x one-shot + resume shape (history_parity claude-cli-current).
    let sid = "claude-cli-current";
    let kinds = [
        "hook_success",
        "environment",
        "model",
        "language",
        "deferred_tools_delta",
        "agent_listing_delta",
        "mcp_instructions_delta",
        "skill_listing",
        "auto_mode",
        "instructions",
        "session_context",
        "date",
        "remote_session_change",
        "prompt_snapshot",
        "deferred_tools_record",
        "edited_text_file",
        "diagnostics",
        "hook_blocking_error",
        "file",
        "task_status",
    ];
    let attachment = |uid: &str, parent: &str, kind: &str| {
        json!({"type": "attachment", "uuid": uid, "parentUuid": parent, "sessionId": sid,
            "cwd": "/synthetic/history", "timestamp": TS, "isSidechain": false,
            "attachment": {"type": kind, "synthetic": true}})
    };
    let tail = |message: &str, snapshot: &str| {
        vec![
            json!({"type": "atis-latch", "atis": {"latched": true}, "sessionId": sid}),
            json!({"type": "file-history-delta", "backup": {"synthetic": true}, "messageId": message,
                "snapshotMessageId": snapshot, "timestamp": "2026-09-11T10:00:01Z", "trackingPath": "synthetic.txt"}),
            json!({"type": "cost-state", "modelUsage": {}, "totalCostUSD": 0.0001, "sessionId": sid}),
        ]
    };
    let mut rows = vec![
        json!({"type": "queue-operation", "operation": "enqueue", "content": "Claude one-shot prompt", "sessionId": sid, "timestamp": TS}),
        json!({"type": "queue-operation", "operation": "popAll", "sessionId": sid, "timestamp": TS}),
        json!({"type": "mode", "mode": "default", "sessionId": sid}),
        json!({"type": "permission-mode", "permissionMode": "plan", "sessionId": sid}),
        json!({"type": "bridge-session", "bridgeSessionId": "bridge", "sessionId": sid}),
        json!({"type": "agent-name", "agentName": "synthetic-agent", "sessionId": sid}),
        claude_row(sid, "user", "u0", Value::Null, "Claude one-shot prompt"),
    ];
    let mut parent = "u0".to_owned();
    for (index, kind) in kinds.iter().enumerate() {
        let uid = format!("at0-{index:02}");
        rows.push(attachment(&uid, &parent, kind));
        parent = uid;
    }
    rows.push(json!({"type": "atis-latch", "atis": {"latched": true}, "sessionId": sid}));
    rows.push(claude_row(
        sid,
        "assistant",
        "a0",
        json!(parent),
        "Claude one-shot answer",
    ));
    rows.extend(tail("a0", "u0"));
    rows.push(claude_row(
        sid,
        "user",
        "u1",
        json!("a0"),
        "Claude resumed prompt",
    ));
    let mut parent = "u1".to_owned();
    for (index, kind) in ["environment", "hook_success", "environment"]
        .iter()
        .enumerate()
    {
        let uid = format!("at1-{index:02}");
        rows.push(attachment(&uid, &parent, kind));
        parent = uid;
    }
    rows.push(claude_row(
        sid,
        "assistant",
        "a1",
        json!(parent),
        "Claude resumed answer",
    ));
    rows.extend(tail("a1", "u1"));
    corpus.put(sid, "claude", &rows);
    // sessions_list_suite's unicode main session with a sidecar agent.
    let sid = "claude-parent";
    let mut rows = vec![
        claude_row(sid, "user", "u0", Value::Null, "你好世界"),
        claude_row(sid, "assistant", "a0", json!("u0"), "收到"),
    ];
    for row in &mut rows {
        row["cwd"] = json!("/synthetic/中文项目");
        row["gitBranch"] = json!("main");
    }
    rows[1]["timestamp"] = json!("2026-09-11T12:00:01Z");
    corpus.put(sid, "claude", &rows);
    fs::write(root.join("claude/project-history/empty-zero.jsonl"), b"").unwrap();
    // Claude subagent sidecars: one under claude-branch, one orphan directory.
    for (owner, agent, sidecar) in [
        (
            "claude-branch",
            "claude-agent-one",
            Some(json!({"description": "Synthetic Claude child", "agentType": "reviewer"})),
        ),
        (
            "claude-parent",
            "claude-agent",
            Some(json!({"description": "sidecar", "agentType": "reviewer"})),
        ),
        ("claude-vanished", "claude-orphan-agent", None),
    ] {
        let path = root
            .join("claude/project-history")
            .join(owner)
            .join("subagents")
            .join(format!("agent-{agent}.jsonl"));
        let mut rows = vec![
            claude_row(
                owner,
                "user",
                "agent-u",
                Value::Null,
                "Claude agent question",
            ),
            claude_row(
                owner,
                "assistant",
                "agent-a",
                json!("agent-u"),
                "Claude agent answer",
            ),
        ];
        for row in &mut rows {
            row["isSidechain"] = json!(true);
            row["agentId"] = json!(agent);
        }
        write(&path, &encoded(&rows));
        if let Some(sidecar) = sidecar {
            fs::write(path.with_extension("meta.json"), sidecar.to_string()).unwrap();
        }
        corpus.paths.insert(agent.to_owned(), path);
    }

    let parent_rows = vec![
        codex_meta("codex-parent", json!({})),
        codex_message("user", "Codex parent prefix OLD", 1),
        codex_message("assistant", "Codex inherited answer", 2),
    ];
    let cutoff = encoded(&parent_rows).len() as u64;
    let mut with_tail = parent_rows.clone();
    with_tail.push(codex_message("user", "Codex discarded parent tail", 3));
    corpus.put("codex-parent", "codex", &with_tail);
    let child = corpus.put(
        "codex-fork",
        "codex",
        &[
            codex_meta(
                "codex-fork",
                json!({"forked_from_id": "codex-parent", "history_mode": "paginated",
                "history_base": {"thread_id": "codex-parent", "end_byte_offset": cutoff}}),
            ),
            codex_message("user", "Codex fork question", 1),
            codex_message("assistant", "Codex fork answer", 2),
        ],
    );
    let child_size = fs::metadata(&child).unwrap().len();
    corpus.put(
        "codex-grandchild",
        "codex",
        &[
            codex_meta(
                "codex-grandchild",
                json!({"forked_from_id": "codex-fork",
                "history_base": {"thread_id": "codex-fork", "end_byte_offset": child_size}}),
            ),
            codex_message("assistant", "Codex nested fork answer", 1),
        ],
    );
    corpus.put(
        "codex-badcut",
        "codex",
        &[
            codex_meta(
                "codex-badcut",
                json!({"forked_from_id": "codex-parent",
                "history_base": {"thread_id": "codex-parent", "end_byte_offset": cutoff - 7}}),
            ),
            codex_message("user", "bad cut", 1),
        ],
    );
    corpus.put(
        "codex-orphan-fork",
        "codex",
        &[
            codex_meta(
                "codex-orphan-fork",
                json!({"forked_from_id": "codex-nowhere",
                "history_base": {"thread_id": "codex-nowhere", "end_byte_offset": 10}}),
            ),
            codex_message("user", "orphan fork", 1),
        ],
    );
    corpus.put(
        "codex-no-base",
        "codex",
        &[
            codex_meta("codex-no-base", json!({"forked_from_id": "codex-parent"})),
            codex_message("user", "fork without history_base", 1),
        ],
    );
    corpus.put(
        "codex-cli-current",
        "codex",
        &[
            codex_meta("codex-cli-current", json!({})),
            codex_row("event_msg", json!({"type": "task_started", "turn_id": "t1"}), 1),
            codex_row("response_item", json!({"type": "message", "role": "user", "turn_id": "t1",
                "content": [{"type": "input_text", "text": "Codex current question"}]}), 2),
            codex_row("token_usage_record", json!({"turn_id": "t1", "total_tokens": 12}), 3),
            codex_row("event_msg", json!({"type": "item_completed", "turn_id": "t1"}), 4),
            codex_row("response_item", json!({"type": "agent_message", "turn_id": "t1"}), 5),
            codex_row("inter_agent_communication_metadata", json!({"turn_id": "t1"}), 6),
            codex_row("response_item", json!({"type": "message", "role": "assistant", "phase": "final_answer",
                "turn_id": "t1", "content": [{"type": "output_text", "text": "Codex current answer"}]}), 7),
            codex_row("event_msg", json!({"type": "task_complete", "turn_id": "t1", "duration_ms": 5}), 8),
            codex_row("token_usage_record", json!({"turn_id": "t1", "total_tokens": 20}), 9),
        ],
    );
    for (sid, owner) in [
        ("codex-agent", "codex-parent"),
        ("codex-nested-agent", "codex-agent"),
        ("codex-orphan-agent", "codex-missing"),
        ("codex-cycle-a", "codex-cycle-b"),
        ("codex-cycle-b", "codex-cycle-a"),
    ] {
        corpus.put(
            sid,
            "codex",
            &[
                codex_meta(
                    sid,
                    json!({"session_id": "codex-parent", "forked_from_id": owner,
                    "thread_source": "subagent", "parent_thread_id": owner,
                    "source": {"subagent": {"thread_spawn": {"parent_thread_id": owner,
                        "agent_path": format!("/root/{sid}"), "agent_role": "reviewer"}}}}),
                ),
                codex_message("assistant", &format!("Synthetic {sid} answer"), 1),
            ],
        );
    }
    // Two rollouts declaring the same subagent id: ambiguous.
    for suffix in ["x", "y"] {
        corpus.put(
            &format!("codex-dup-{suffix}"),
            "codex",
            &[
                codex_meta(
                    "codex-dup",
                    json!({"id": "codex-dup", "session_id": "codex-parent",
                    "thread_source": "subagent", "parent_thread_id": "codex-parent",
                    "agent_path": "dup"}),
                ),
                codex_message("assistant", "dup", 1),
            ],
        );
    }

    let grok = root.join("grok/%2Fsynthetic%2F%E4%B8%AD%E6%96%87");
    let summary_only = grok.join("grok-summary");
    fs::create_dir_all(&summary_only).unwrap();
    fs::write(
        summary_only.join("summary.json"),
        json!({"generated_title": "Grok 摘要标题", "session_summary": "only summary",
            "info": {"id": "grok-summary", "cwd": "/synthetic/中文"},
            "created_at": "2026-09-11T08:00:00Z", "updated_at": "2026-09-11T08:30:00Z",
            "current_model_id": "grok-test", "agent_name": "grok-branch"})
        .to_string(),
    )
    .unwrap();
    corpus.paths.insert("grok-summary".into(), summary_only);
    let chat = grok.join("grok-chat");
    fs::create_dir_all(&chat).unwrap();
    fs::write(
        chat.join("summary.json"),
        json!({"generated_title": "Grok advanced chat", "info": {"id": "grok-chat", "cwd": "/synthetic/grok"},
            "created_at": "2026-09-11T08:00:00Z", "last_active_at": "2026-09-11T09:30:00Z",
            "current_model_id": "synthetic-grok"})
        .to_string(),
    )
    .unwrap();
    write(
        &chat.join("chat_history.jsonl"),
        &encoded(&[
            json!({"type": "user", "content": "hello grok", "prompt_index": 0, "timestamp": "2026-09-11T08:00:01Z"}),
            json!({"type": "usage", "tokens": 1}),
            json!({"type": "assistant", "content": "hi", "timestamp": "2026-09-11T08:00:02Z"}),
            json!({"type": "usage", "tokens": 2}),
            json!({"type": "checkpoint"}),
        ]),
    );
    corpus.paths.insert("grok-chat".into(), chat);
    corpus
}

fn strip_cursor(row: &mut Value) {
    if let Some(object) = row.as_object_mut() {
        object.remove("cursor");
    }
    if let Some(items) = row.get_mut("agent_items").and_then(Value::as_array_mut) {
        for item in items {
            if let Some(object) = item.as_object_mut() {
                object.remove("cursor");
            }
        }
    }
}

fn by_uid(rows: &[Value]) -> BTreeMap<String, Value> {
    rows.iter()
        .map(|row| (row["uid"].as_str().unwrap().to_owned(), row.clone()))
        .collect()
}

/// The facade publishes exactly the index rows plus the physical cursor of
/// every supported entry (`{end, head}` from the summary; `anchor` only from
/// an opened view) — never a field the index does not know.
fn assert_facade_rows_are_index_rows_plus_cursor(facade: &[Value], rows: &[Value]) {
    let facade = by_uid(facade);
    let rows = by_uid(rows);
    assert_eq!(
        facade.keys().collect::<Vec<_>>(),
        rows.keys().collect::<Vec<_>>(),
        "same set of top-level rows"
    );
    for (uid, published) in &facade {
        let mut expected = rows[uid].clone();
        let mut actual = published.clone();
        let supported = actual["supported"] != false;
        assert_eq!(
            actual["cursor"].is_object(),
            supported && actual["cursor"]["end"].is_u64() && actual["cursor"]["head"].is_string(),
            "{uid}: supported rows carry the physical cursor"
        );
        assert!(
            actual["cursor"].get("anchor").is_none(),
            "{uid}: nothing was opened"
        );
        for item in actual["agent_items"].as_array().into_iter().flatten() {
            assert_eq!(
                item["cursor"].is_object(),
                item["supported"] != false,
                "{uid}: agent item cursor"
            );
        }
        strip_cursor(&mut expected);
        strip_cursor(&mut actual);
        assert_eq!(actual, expected, "{uid}");
    }
}

#[test]
fn facade_rows_are_the_index_rows_and_the_documented_topology_holds() {
    let temp = TempDir::new().unwrap();
    let corpus = build_corpus(temp.path());
    let store = SessionStore::new(corpus.roots());
    let facade = store.list(true).unwrap();
    let index = Index::new(corpus.roots(), None);
    let snapshot = index.refresh(true).unwrap();
    assert_facade_rows_are_index_rows_plus_cursor(
        facade["sessions"].as_array().unwrap(),
        snapshot.sessions(),
    );
    assert_ne!(
        facade["sig"],
        snapshot.sig(),
        "the cursor is part of the signed rows"
    );
    let rows = by_uid(snapshot.sessions());
    // The documented topology of history_parity, from summaries.
    let sids = |uid: &str| {
        rows[uid]["agent_items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item["id"].as_str().unwrap().to_owned())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default()
    };
    assert_eq!(
        sids(&corpus.uid("codex-parent")),
        ["codex-agent", "codex-nested-agent"]
            .map(str::to_owned)
            .into()
    );
    assert!(sids(&corpus.uid("codex-fork")).is_empty());
    assert_eq!(
        sids(&corpus.uid("claude-branch")),
        ["claude-agent-one".to_owned()].into()
    );
    for (sid, depth) in [("codex-fork", 1), ("codex-grandchild", 2)] {
        let row = &rows[&corpus.uid(sid)];
        assert_eq!(row["root_sid"], "codex-parent");
        assert_eq!(row["fork_depth"], depth);
        assert_eq!(row["created"], rows[&corpus.uid("codex-parent")]["created"]);
        assert_eq!(row["supported"], true);
    }
    for (sid, message) in [
        ("codex-cycle-a", "子代理归属关系存在循环"),
        ("codex-cycle-b", "子代理归属关系存在循环"),
        ("codex-badcut", "父历史固定前缀不在完整 JSONL 行边界"),
        (
            "codex-orphan-fork",
            "父线程不在已配置索引中，请确认显式数据源包含其原生文件",
        ),
        ("codex-dup-x", "Codex 子代理 ID 在索引中存在歧义"),
    ] {
        let row = &rows[&corpus.uid(sid)];
        assert_eq!(row["supported"], false, "{sid}");
        assert_eq!(row["migration_warnings"], json!([message]), "{sid}");
    }
    // Agents whose owner is not indexed are no rows;
    // their uid still answers a typed 501 and they stay catalogued.
    for (sid, message) in [
        (
            "codex-orphan-agent",
            "父线程不在已配置索引中，请确认显式数据源包含其原生文件",
        ),
        (
            "claude-orphan-agent",
            "Claude 子代理的主会话不在已配置索引中",
        ),
    ] {
        let uid = corpus.uid(sid);
        assert!(!rows.contains_key(&uid), "{sid} is not a top-level row");
        let orphan = snapshot.candidate(&uid).expect("orphans stay candidates");
        let error = orphan.owner_error.as_ref().expect("typed owner error");
        assert_eq!(
            (error.status, error.message.as_str()),
            (501, message),
            "{sid}"
        );
        assert_eq!(orphan.owner, None);
        assert_eq!(store.snapshot(&uid, "").unwrap_err().status, 501, "{sid}");
    }
    // A legacy fork (no history_base) is self-contained: supported, and the
    // row still follows its logical parent.
    let legacy = &rows[&corpus.uid("codex-no-base")];
    assert_eq!(legacy["supported"], true);
    assert_eq!(legacy["migration_warnings"], json!([]));
    assert_eq!(legacy["root_sid"], "codex-parent");
    assert_eq!(legacy["fork_depth"], 1);
    assert_eq!(
        legacy["size"],
        fs::metadata(&corpus.paths["codex-no-base"]).unwrap().len()
    );
    let current = &rows[&corpus.uid("claude-cli-current")];
    assert_eq!(current["supported"], true);
    assert_eq!(current["migration_warnings"].as_array().unwrap().len(), 27);
    assert_eq!(current["title"], "Claude one-shot prompt");
    let grok = &rows[&corpus.uid("grok-chat")];
    assert_eq!(
        grok["migration_warnings"],
        json!([
            "跳过未知的Grok 记录类型：usage ×2",
            "跳过未知的Grok 记录类型：checkpoint ×1"
        ])
    );
    assert_eq!(rows[&corpus.uid("grok-summary")]["chat_exists"], false);
    // Sorted like today: updated descending, uid ascending.
    let sessions = snapshot.sessions();
    for pair in sessions.windows(2) {
        let (left, right) = (&pair[0], &pair[1]);
        assert!(
            left["updated"].as_str() > right["updated"].as_str()
                || (left["updated"] == right["updated"]
                    && left["uid"].as_str() < right["uid"].as_str())
        );
    }
    assert_eq!(snapshot.rows()["sig"].as_str().unwrap().len(), 40);
    assert!(snapshot.built_at() > 0.0);
    // Candidate table: every physical file, agents with their owners.
    let agent = snapshot
        .candidate(&corpus.uid("codex-nested-agent"))
        .expect("agents stay in the candidate table");
    assert_eq!(
        agent.owner.as_deref(),
        Some(corpus.uid("codex-parent").as_str())
    );
    assert_eq!(agent.agent_id.as_deref(), Some("codex-nested-agent"));
    let sidecar = snapshot.candidate(&corpus.uid("claude-agent-one")).unwrap();
    assert_eq!(
        sidecar.owner.as_deref(),
        Some(corpus.uid("claude-branch").as_str())
    );
    assert!(
        sidecar
            .summary_path
            .as_ref()
            .unwrap()
            .ends_with("agent-claude-agent-one.meta.json")
    );
    assert!(sidecar.summary_stamp.is_some());
    let main = snapshot.candidate(&corpus.uid("claude-branch")).unwrap();
    assert_eq!(main.native_id(), Some("claude-branch"));
    assert_eq!(main.committed(), Some(main.stamp.unwrap().size));
    assert!(main.cursor_head().unwrap().starts_with("rs-m2-1:"));
    assert_eq!(snapshot.candidates().count(), corpus.paths.len());
}

#[test]
fn catalog_matches_the_opened_views_native_scopes() {
    let temp = TempDir::new().unwrap();
    let corpus = build_corpus(temp.path());
    let store = SessionStore::new(corpus.roots());
    let index = Index::new(corpus.roots(), None);
    let snapshot = index.refresh(true).unwrap();
    let actual = snapshot.catalog();
    assert_eq!(
        store
            .native_catalog()
            .unwrap()
            .verified_scope(&corpus.uid("claude-branch")),
        actual.verified_scope(&corpus.uid("claude-branch")),
        "the facade catalog is the index catalog"
    );
    // Every verified scope equals what the fully streamed view proves, and
    // every open of a rejected uid fails.
    for (sid, path) in &corpus.paths {
        let uid = corpus.uid(sid);
        match actual.verified_scope(&uid) {
            Ok(scope) => assert_eq!(
                store.native_scope(&uid, "").unwrap(),
                scope,
                "{sid} ({})",
                path.display()
            ),
            Err(_) => assert!(
                store.native_scope(&uid, "").is_err(),
                "{sid} ({}) must not open with a native scope",
                path.display()
            ),
        }
    }
    assert!(actual.verified_scope(&corpus.uid("claude-branch")).is_ok());
    assert!(actual.verified_scope(&corpus.uid("codex-fork")).is_ok());
    assert!(actual.verified_scope(&corpus.uid("codex-agent")).is_err());
    // Grok main sessions verify through summary.json `info.id`.
    assert!(actual.verified_scope(&corpus.uid("grok-chat")).is_ok());
}

#[test]
fn codex_name_index_enriches_rows_and_forks_inherit_named_roots() {
    let temp = TempDir::new().unwrap();
    let corpus = build_corpus(temp.path());
    let names = temp.path().join("session_index.jsonl");
    fs::write(
        &names,
        [
            json!({"id": "codex-parent", "thread_name": "  Named   parent  ", "updated_at": "2026-09-11T11:00:00Z"}),
            json!({"id": "codex-cli-current", "thread_name": "Current without time"}),
            json!({"id": "unknown", "thread_name": "ignored"}),
        ]
        .iter()
        .map(|row| row.to_string() + "\n")
        .collect::<String>(),
    )
    .unwrap();
    let store = SessionStore::with_metadata_and_names(corpus.roots(), None, Some(names.clone()));
    let facade = store.list(true).unwrap();
    let index = Index::new(corpus.roots(), Some(names.clone()));
    let snapshot = index.refresh(true).unwrap();
    assert_facade_rows_are_index_rows_plus_cursor(
        facade["sessions"].as_array().unwrap(),
        snapshot.sessions(),
    );
    let rows = by_uid(snapshot.sessions());
    let parent = &rows[&corpus.uid("codex-parent")];
    assert_eq!(parent["title"], "Named parent");
    assert_eq!(parent["renamed_at"], "2026-09-11T11:00:00.000Z");
    assert_eq!(parent["renamed_to"], "  Named   parent  ");
    assert_eq!(
        rows[&corpus.uid("codex-fork")]["title"],
        "Named parent",
        "forks inherit the named root"
    );
    assert_eq!(
        rows[&corpus.uid("codex-grandchild")]["title"],
        "Named parent"
    );
    let current = &rows[&corpus.uid("codex-cli-current")];
    assert_eq!(current["title"], "Current without time");
    assert_eq!(current["renamed_at"], Value::Null);
    assert_eq!(current["renamed_to"], Value::Null);
    // The name index is part of the warm check: a rename is noticed within the TTL.
    let before = snapshot.sig().to_owned();
    fs::write(
        &names,
        json!({"id": "codex-parent", "thread_name": "Renamed"}).to_string() + "\n",
    )
    .unwrap();
    let renamed = index.refresh(false).unwrap();
    assert_ne!(renamed.sig(), before);
    assert_eq!(
        by_uid(renamed.sessions())[&corpus.uid("codex-parent")]["title"],
        "Renamed"
    );
    // Without a Codex root the optional names file is simply unused.
    let broken = Index::new(
        SessionRoots {
            codex: None,
            ..corpus.roots()
        },
        Some(names),
    );
    assert!(
        broken
            .refresh(true)
            .unwrap()
            .sessions()
            .iter()
            .all(|row| row["source"] != "codex")
    );
}

#[test]
fn warm_refresh_is_stat_only_and_only_changed_files_are_reread() {
    let temp = TempDir::new().unwrap();
    let corpus = build_corpus(temp.path());
    let index = Index::new(corpus.roots(), None);
    let first = index.refresh(true).unwrap();
    let files = corpus.paths.len();
    assert_eq!(index.reads(), files, "cold: every physical entry read once");
    let second = index.refresh(true).unwrap();
    assert_eq!(index.reads(), files, "warm: no summary read at all");
    assert_eq!(second.sig(), first.sig());
    // A forced walk that finds every stamp unchanged reuses
    // the snapshot instead of rebuilding rows, graph and signature.
    assert!(
        Arc::ptr_eq(&first, &second),
        "an unchanged forced walk reuses the snapshot"
    );
    // Within the TTL the previous snapshot is reused without any I/O.
    let cached = index.refresh(false).unwrap();
    assert!(Arc::ptr_eq(&second, &cached));
    // Appending to one file re-reads exactly that file and changes the sig.
    let path = &corpus.paths["claude-parent"];
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    let mut row = claude_row("claude-parent", "user", "u1", json!("a0"), "append");
    row["timestamp"] = json!("2026-09-11T13:00:00Z");
    file.write_all(&encoded(&[row])).unwrap();
    drop(file);
    let third = index.refresh(true).unwrap();
    assert_eq!(index.reads(), files + 1);
    assert_ne!(third.sig(), second.sig());
    let row = &by_uid(third.sessions())[&corpus.uid("claude-parent")];
    assert_eq!(row["updated"], "2026-09-11T13:00:00.000Z");
    assert_eq!(row["size"], fs::metadata(path).unwrap().len());
    // A deleted file disappears; nothing else is re-read.
    fs::remove_file(&corpus.paths["codex-cli-current"]).unwrap();
    let fourth = index.refresh(true).unwrap();
    assert_eq!(index.reads(), files + 1);
    assert!(fourth.candidate(&corpus.uid("codex-cli-current")).is_none());
    assert_eq!(fourth.sessions().len(), third.sessions().len() - 1);
    // A same-size rewrite with a new mtime is a new stamp.
    let path = &corpus.paths["codex-fork"];
    let bytes = fs::read(path).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    fs::write(path, &bytes).unwrap();
    index.refresh(true).unwrap();
    assert_eq!(index.reads(), files + 2);
}

/// Claude Code continues a session into a new transcript and links the
/// origin's sidecars into the continuation's `subagents/`; the `glob`
/// lists the link, so the agent belongs to both sessions. The link is the
/// identity (uid, owner by directory), the canonical in-root target is the
/// data. Regular-file aliases use ordinary path handling; dangling
/// aliases and directory targets stay skipped.
#[cfg(unix)]
#[test]
fn a_subagent_symlink_inside_a_root_is_followed_and_owned_by_the_linking_session() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let claude = root.join("claude");
    let agent = encoded(&[
        claude_row("origin", "user", "au", Value::Null, "agent input"),
        claude_row("origin", "assistant", "aa", json!("au"), "agent reply"),
    ]);
    write(
        &claude.join("proj/origin.jsonl"),
        &encoded(&[claude_row("origin", "user", "u0", Value::Null, "hello")]),
    );
    write(&claude.join("proj/origin/subagents/agent-a1.jsonl"), &agent);
    write(
        &claude.join("proj/origin/subagents/agent-a1.meta.json"),
        br#"{"agentType":"worker","description":"fixture worker"}"#,
    );
    write(
        &claude.join("proj/continued.jsonl"),
        &encoded(&[claude_row(
            "continued",
            "user",
            "u1",
            Value::Null,
            "continue",
        )]),
    );
    let linked = claude.join("proj/continued/subagents");
    fs::create_dir_all(&linked).unwrap();
    std::os::unix::fs::symlink(
        claude.join("proj/origin/subagents/agent-a1.jsonl"),
        linked.join("agent-a1.jsonl"),
    )
    .unwrap();
    write(
        &linked.join("agent-a1.meta.json"),
        br#"{"agentType":"worker","description":"fixture worker"}"#,
    );
    // External regular files and main-file aliases are followed; dangling
    // aliases and directory targets are skipped.
    let outside = root.join("outside-agent.jsonl");
    write(&outside, &agent);
    std::os::unix::fs::symlink(&outside, linked.join("agent-outside.jsonl")).unwrap();
    std::os::unix::fs::symlink(root.join("gone.jsonl"), linked.join("agent-gone.jsonl")).unwrap();
    std::os::unix::fs::symlink(
        claude.join("proj/origin/subagents"),
        linked.join("agent-dir.jsonl"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        claude.join("proj/origin.jsonl"),
        claude.join("proj/linked-main.jsonl"),
    )
    .unwrap();
    let index = Index::new(
        SessionRoots {
            claude: Some(claude.clone()),
            ..Default::default()
        },
        None,
    );
    let snapshot = index.refresh(true).unwrap();
    let mut paths: Vec<_> = snapshot
        .candidates()
        .map(|candidate| {
            candidate
                .path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        vec![
            "claude/proj/continued.jsonl",
            "claude/proj/continued/subagents/agent-a1.jsonl",
            "claude/proj/continued/subagents/agent-outside.jsonl",
            "claude/proj/linked-main.jsonl",
            "claude/proj/origin.jsonl",
            "claude/proj/origin/subagents/agent-a1.jsonl",
        ]
    );
    let rows = by_uid(snapshot.sessions());
    let continued = &rows[&uid_for("claude", &claude.join("proj/continued.jsonl"))];
    let origin = &rows[&uid_for("claude", &claude.join("proj/origin.jsonl"))];
    let origin_items = origin["agent_items"].as_array().unwrap();
    assert_eq!(origin_items.len(), 1);
    assert_eq!(origin_items[0]["id"], "a1");
    let continued_items = continued["agent_items"].as_array().unwrap();
    assert_eq!(continued_items.len(), 2);
    assert_eq!(continued_items[0]["id"], "a1");
    assert_eq!(continued_items[1]["id"], "outside");
    assert!(continued_items.iter().all(|item| item["supported"] == true));
    let via_link = snapshot
        .candidate(&uid_for("claude", &linked.join("agent-a1.jsonl")))
        .unwrap();
    assert_eq!(via_link.data, linked.join("agent-a1.jsonl"));
    assert_eq!(via_link.root, claude.canonicalize().unwrap());
    assert_eq!(
        via_link.owner.as_deref(),
        Some(continued["uid"].as_str().unwrap())
    );
    // The linked agent opens through the facade like the original one.
    let store = SessionStore::new(SessionRoots {
        claude: Some(claude.clone()),
        ..Default::default()
    });
    for uid in [
        continued["uid"].as_str().unwrap(),
        origin["uid"].as_str().unwrap(),
    ] {
        let view = store
            .messages(
                uid,
                &crate::sessions::MessageQuery {
                    agent: "a1".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let texts: Vec<_> = view["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["text"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(texts, ["agent input", "agent reply"], "{uid}");
        assert_eq!(view["meta"]["agent_id"], "a1");
    }
}

#[test]
fn discovery_follows_python_file_and_project_aliases_and_skips_unrelated_files() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let claude = root.join("claude");
    write(
        &claude.join("proj/session.jsonl"),
        &encoded(&[claude_row("session", "user", "u0", Value::Null, "hello")]),
    );
    write(&claude.join("proj/notes.txt"), b"not a session\n");
    write(
        &claude.join("proj/session/subagents/agent-a1.jsonl"),
        &encoded(&[claude_row("session", "user", "au", Value::Null, "agent")]),
    );
    write(
        &claude.join("proj/session/subagents/README.jsonl"),
        &encoded(&[json!({"x": 1})]),
    );
    write(
        &claude.join("proj/session/other/agent-nope.jsonl"),
        &encoded(&[json!({"x": 1})]),
    );
    write(
        &claude.join("too/deep/nested/session.jsonl"),
        &encoded(&[json!({"x": 1})]),
    );
    write(&claude.join("proj/empty.jsonl"), b"");
    let outside = root.join("outside.jsonl");
    write(
        &outside,
        &encoded(&[claude_row("outside", "user", "u0", Value::Null, "x")]),
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, claude.join("proj/linked.jsonl")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), claude.join("linked-project")).unwrap();
        fs::create_dir_all(root.join("elsewhere")).unwrap();
        write(
            &root.join("elsewhere/hidden.jsonl"),
            &encoded(&[json!({"x": 1})]),
        );
    }
    let codex = root.join("codex");
    write(
        &codex.join("2026/09/11/rollout-a.jsonl"),
        &encoded(&[codex_meta("a", json!({}))]),
    );
    write(
        &codex.join("flat.jsonl"),
        &encoded(&[codex_meta("flat", json!({}))]),
    );
    write(&codex.join("2026/09/11/rollout-a.json"), b"{}\n");
    let grok = root.join("grok");
    fs::create_dir_all(grok.join("cwd/no-summary")).unwrap();
    write(&grok.join("cwd/no-summary/chat_history.jsonl"), b"{}\n");
    write(&grok.join("cwd/with-summary/summary.json"), b"{}");
    write(&grok.join("cwd/with-summary/chat_history.jsonl"), b"");
    let index = Index::new(
        SessionRoots {
            claude: Some(claude.clone()),
            codex: Some(codex.clone()),
            grok: Some(grok.clone()),
        },
        None,
    );
    let snapshot = index.refresh(true).unwrap();
    let mut paths: Vec<_> = snapshot
        .candidates()
        .map(|candidate| {
            candidate
                .path
                .strip_prefix(root.canonicalize().unwrap())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    paths.sort();
    #[cfg(unix)]
    let expected = vec![
        "claude/linked-project/hidden.jsonl",
        "claude/proj/linked.jsonl",
        "claude/proj/session.jsonl",
        "claude/proj/session/subagents/agent-a1.jsonl",
        "codex/2026/09/11/rollout-a.jsonl",
        "codex/flat.jsonl",
        "grok/cwd/with-summary",
    ];
    #[cfg(not(unix))]
    let expected = vec![
        "claude/proj/session.jsonl",
        "claude/proj/session/subagents/agent-a1.jsonl",
        "codex/2026/09/11/rollout-a.jsonl",
        "codex/flat.jsonl",
        "grok/cwd/with-summary",
    ];
    assert_eq!(paths, expected);
    let grok_row = snapshot
        .candidate(&uid_for("grok", &grok.join("cwd/with-summary")))
        .unwrap();
    assert!(
        grok_row.stamp.is_some_and(|stamp| stamp.size == 0),
        "a zero-byte Grok chat exists"
    );
    #[cfg(unix)]
    assert_eq!(
        snapshot.sessions().len(),
        6,
        "the sidecar is folded into its owner"
    );
    #[cfg(not(unix))]
    assert_eq!(
        snapshot.sessions().len(),
        4,
        "the sidecar is folded into its owner"
    );
    let missing = Index::new(
        SessionRoots {
            claude: Some(root.join("missing")),
            ..Default::default()
        },
        None,
    );
    let error = missing.refresh(true).unwrap_err();
    assert_eq!(error.status, 400);
    assert_eq!(error.message, "已配置的数据源目录不存在或不可访问");
    let empty = Index::new(SessionRoots::default(), None);
    assert_eq!(empty.refresh(true).unwrap().sessions(), &[] as &[Value]);
}

/// A source whose stamp advances on every `stat`, like a file under append.
struct MovingSource {
    stamps: Vec<Stamp>,
    calls: usize,
    bytes: Vec<u8>,
}

impl StampedSource for MovingSource {
    fn stamp(&mut self) -> Option<Stamp> {
        let stamp = self.stamps[self.calls.min(self.stamps.len() - 1)];
        self.calls += 1;
        Some(stamp)
    }
    fn read_range(&mut self, start: u64, length: u64) -> std::io::Result<Vec<u8>> {
        let start = start as usize;
        let end = (start + length as usize).min(self.bytes.len());
        Ok(self.bytes[start..end].to_vec())
    }
}

fn moving(sizes: &[u64], bytes: &[u8]) -> MovingSource {
    MovingSource {
        stamps: sizes
            .iter()
            .enumerate()
            .map(|(index, size)| Stamp {
                dev: 1,
                ino: 1,
                size: *size,
                mtime_ns: index as u128,
            })
            .collect(),
        calls: 0,
        bytes: bytes.to_vec(),
    }
}

#[test]
fn a_moving_stamp_is_reread_three_times_then_published_with_the_bytes_read() {
    let bytes = vec![b'x'; 4096];
    let mut opens = 0;
    // Every attempt sees a different stamp before and after its read.
    let read = read_data_from(&mut || {
        opens += 1;
        Ok(Box::new(moving(&[1000, 2000], &bytes)) as Box<dyn StampedSource>)
    });
    assert_eq!(opens, READ_ATTEMPTS);
    let FileRead::Data {
        head,
        tail,
        tail_start,
        stamp,
    } = read
    else {
        panic!("a changing file is still published");
    };
    assert_eq!(stamp.size, 1000, "the stamp of the bytes actually read");
    assert_eq!(head.len(), 1000);
    assert_eq!(tail.len(), 1000);
    assert_eq!(tail_start, 0);
    // A stamp that settles on the second attempt stops there.
    let mut opens = 0;
    let read = read_data_from(&mut || {
        opens += 1;
        let stamps: &[u64] = if opens == 1 { &[1000, 2000] } else { &[3000] };
        Ok(Box::new(moving(stamps, &bytes)) as Box<dyn StampedSource>)
    });
    assert_eq!(opens, 2);
    assert!(matches!(read, FileRead::Data { stamp, .. } if stamp.size == 3000));
    assert!(matches!(
        read_data_from(&mut || Err(OpenError::Vanished)),
        FileRead::Vanished
    ));
    // Large files read the head and the tail separately.
    let big = vec![b'y'; (TAIL_BYTES + 100_000) as usize];
    let read = read_data_from(&mut || {
        Ok(Box::new(moving(&[big.len() as u64], &big)) as Box<dyn StampedSource>)
    });
    let FileRead::Data {
        head,
        tail,
        tail_start,
        ..
    } = read
    else {
        panic!("stable file");
    };
    assert_eq!(head.len(), summary::HEAD_BYTES as usize);
    assert_eq!(tail.len(), TAIL_BYTES as usize);
    assert_eq!(tail_start, 100_000);
}

#[test]
fn bytes_published_under_concurrent_append_always_belong_to_their_stamp() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("claude/proj/live.jsonl");
    write(
        &path,
        &encoded(&[claude_row("live", "user", "u0", Value::Null, "start")]),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let stop = stop.clone();
        let path = path.clone();
        std::thread::spawn(move || {
            let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
            let line = encoded(&[claude_row(
                "live",
                "assistant",
                "a",
                json!("u0"),
                &"z".repeat(3000),
            )]);
            while !stop.load(Ordering::Relaxed) {
                file.write_all(&line).unwrap();
            }
        })
    };
    let started = Instant::now();
    let mut reads = 0;
    let mut grew = false;
    let mut previous = 0;
    while started.elapsed() < Duration::from_millis(400) {
        match read_data(&root, &path) {
            FileRead::Data {
                head,
                tail,
                tail_start,
                stamp,
            } => {
                assert_eq!(head.len() as u64, stamp.size.min(summary::HEAD_BYTES));
                assert_eq!(tail_start + tail.len() as u64, stamp.size);
                grew |= stamp.size > previous;
                previous = stamp.size;
                reads += 1;
            }
            _ => panic!("a live file is never unreadable"),
        }
    }
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
    assert!(reads > 0 && grew);
    // The list itself never fails while the file grows.
    let index = Index::new(
        SessionRoots {
            claude: Some(root.join("claude")),
            ..Default::default()
        },
        None,
    );
    let snapshot = index.refresh(true).unwrap();
    assert_eq!(snapshot.sessions().len(), 1);
    assert_eq!(snapshot.sessions()[0]["title"], "start");
}

// ---------------------------------------------------------------------------
// Graph rules from summaries only (no I/O).
// ---------------------------------------------------------------------------

fn entry(source: &'static str, path: &str, bytes: &[u8], sidecar: Option<&[u8]>) -> CandidateRef {
    let path = PathBuf::from(path);
    let stamp = Stamp {
        dev: 1,
        ino: 1,
        size: bytes.len() as u64,
        mtime_ns: 0,
    };
    let head = &bytes[..bytes.len().min(summary::HEAD_BYTES as usize)];
    let summary = summary::summarize(&Input {
        source,
        path: &path,
        data: Some(DataFile {
            head,
            tail: bytes,
            tail_start: 0,
            stamp,
        }),
        sidecar: sidecar.map(|bytes| summary::SidecarBytes::Bytes {
            bytes,
            stamp: Stamp {
                dev: 1,
                ino: 2,
                size: bytes.len() as u64,
                mtime_ns: 0,
            },
        }),
    });
    CandidateRef {
        uid: uid_for(source, &path),
        source,
        root: PathBuf::from("/synthetic"),
        data: path.clone(),
        path,
        summary_path: None,
        stamp: Some(stamp),
        summary_stamp: None,
        agent_id: summary.agent.as_ref().map(|agent| agent.id.clone()),
        owner: None,
        owner_error: None,
        summary: Arc::new(summary),
    }
}

fn entries(list: Vec<CandidateRef>) -> BTreeMap<String, CandidateRef> {
    list.into_iter()
        .map(|entry| (entry.uid.clone(), entry))
        .collect()
}

fn agent_rollout(sid: &str, owner: &str) -> Vec<u8> {
    encoded(&[
        codex_meta(
            sid,
            json!({"session_id": "root", "forked_from_id": owner, "thread_source": "subagent",
            "parent_thread_id": owner, "agent_path": format!("worker/{sid}"), "agent_role": "explorer"}),
        ),
        codex_message("assistant", "answer", 1),
    ])
}

#[test]
fn graph_flattens_nested_codex_agents_to_their_root_and_keeps_broken_ones_visible() {
    let root = entry(
        "codex",
        "/synthetic/codex/rollout-root.jsonl",
        &encoded(&[
            codex_meta("root", json!({})),
            codex_message("user", "Root question", 1),
        ]),
        None,
    );
    let fork = entry(
        "codex",
        "/synthetic/codex/rollout-fork.jsonl",
        &encoded(&[
            codex_meta(
                "fork",
                json!({"forked_from_id": "root", "history_base": {"thread_id": "root", "end_byte_offset": 10}}),
            ),
            codex_message("user", "Fork question", 1),
        ]),
        None,
    );
    let agent = entry(
        "codex",
        "/synthetic/codex/rollout-agent.jsonl",
        &agent_rollout("agent", "fork"),
        None,
    );
    let nested = entry(
        "codex",
        "/synthetic/codex/rollout-nested.jsonl",
        &agent_rollout("nested", "agent"),
        None,
    );
    let orphan = entry(
        "codex",
        "/synthetic/codex/rollout-orphan.jsonl",
        &agent_rollout("orphan", "missing"),
        None,
    );
    let cycle_a = entry(
        "codex",
        "/synthetic/codex/rollout-cycle-a.jsonl",
        &agent_rollout("cycle-a", "cycle-b"),
        None,
    );
    let cycle_b = entry(
        "codex",
        "/synthetic/codex/rollout-cycle-b.jsonl",
        &agent_rollout("cycle-b", "cycle-a"),
        None,
    );
    let no_parent = entry(
        "codex",
        "/synthetic/codex/rollout-noparent.jsonl",
        &encoded(&[codex_meta(
            "noparent",
            json!({"session_id": "root", "thread_source": "subagent"}),
        )]),
        None,
    );
    let uids: BTreeMap<&str, String> = [
        ("root", &root),
        ("fork", &fork),
        ("agent", &agent),
        ("nested", &nested),
        ("orphan", &orphan),
        ("cycle-a", &cycle_a),
        ("cycle-b", &cycle_b),
        ("noparent", &no_parent),
    ]
    .into_iter()
    .map(|(name, entry)| (name, entry.uid.clone()))
    .collect();
    let all = entries(vec![
        root, fork, agent, nested, orphan, cycle_a, cycle_b, no_parent,
    ]);
    let mut cuts = Vec::new();
    let built = graph::build(
        &all,
        &mut |parent, cut| {
            cuts.push((parent.uid.clone(), cut));
            CutCheck::Boundary
        },
        &BTreeMap::new(),
    );
    assert_eq!(
        cuts,
        vec![(uids["root"].clone(), 10)],
        "each cut is checked once"
    );
    let rows = by_uid(&built.rows);
    let fork_row = &rows[&uids["fork"]];
    assert_eq!(
        fork_row["agents"], 2,
        "agents attached to a fork stay with the fork, not its parent"
    );
    assert_eq!(
        fork_row["agent_items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].clone())
            .collect::<Vec<_>>(),
        vec![json!("agent"), json!("nested")]
    );
    assert_eq!(fork_row["agent_items"][0]["title"], "worker/agent");
    assert_eq!(fork_row["agent_items"][0]["type"], "explorer");
    assert_eq!(fork_row["agent_items"][0]["supported"], true);
    assert!(fork_row["agent_items"][0].get("cursor").is_none());
    assert_eq!(fork_row["root_sid"], "root");
    assert_eq!(fork_row["fork_depth"], 1);
    assert_eq!(fork_row["title"], "Root question");
    assert_eq!(fork_row["size"], all[&uids["fork"]].summary.size + 10);
    assert!(rows[&uids["root"]].get("agent_items").is_none());
    assert_eq!(built.owners[&uids["agent"]], uids["fork"]);
    assert_eq!(built.owners[&uids["nested"]], uids["fork"]);
    assert!(!built.owners.contains_key(&uids["orphan"]));
    for (name, message) in [
        ("cycle-a", "子代理归属关系存在循环"),
        ("cycle-b", "子代理归属关系存在循环"),
    ] {
        let row = &rows[&uids[name]];
        assert_eq!(row["supported"], false, "{name}");
        assert_eq!(row["migration_warnings"], json!([message]), "{name}");
        assert_eq!(built.agent_errors[&uids[name]].message, message);
    }
    // An agent whose parent resolves to no indexed row is no row at all,
    // only the typed error its uid opens with.
    for (name, message) in [
        (
            "orphan",
            "父线程不在已配置索引中，请确认显式数据源包含其原生文件",
        ),
        ("noparent", "历史依赖或子代理缺少父线程 ID"),
    ] {
        assert!(!rows.contains_key(&uids[name]), "{name}");
        let error = &built.agent_errors[&uids[name]];
        assert_eq!((error.status, error.message.as_str()), (501, message));
    }
    let seeds: BTreeMap<_, _> = built
        .catalog
        .iter()
        .map(|seed| (seed.uid.clone(), seed))
        .collect();
    assert!(seeds[&uids["agent"]].subagent);
    assert_eq!(
        seeds[&uids["agent"]].scope.as_ref().unwrap_err().status,
        404
    );
    assert_eq!(
        seeds[&uids["orphan"]].scope.as_ref().unwrap_err().status,
        501
    );
    assert_eq!(
        seeds[&uids["fork"]].scope.as_ref().unwrap().session_id,
        "fork"
    );
    assert!(!seeds[&uids["root"]].subagent);
}

#[test]
fn graph_fork_validation_messages_and_cut_outcomes() {
    let root_bytes = encoded(&[codex_meta("root", json!({}))]);
    let root = entry(
        "codex",
        "/synthetic/codex/rollout-root.jsonl",
        &root_bytes,
        None,
    );
    let make = |sid: &str, extra: Value| {
        entry(
            "codex",
            &format!("/synthetic/codex/rollout-{sid}.jsonl"),
            &encoded(&[codex_meta(sid, extra)]),
            None,
        )
    };
    let cases = vec![
        (
            "badcut",
            json!({"forked_from_id": "root", "history_base": {"thread_id": "root", "end_byte_offset": 5}}),
            CutCheck::NotBoundary,
            "父历史固定前缀不在完整 JSONL 行边界",
        ),
        (
            "beyond",
            json!({"forked_from_id": "root", "history_base": {"thread_id": "root", "end_byte_offset": 5000}}),
            CutCheck::BeyondEnd,
            "父历史固定前缀超出完整原生数据范围",
        ),
        (
            "moved",
            json!({"forked_from_id": "root", "history_base": {"thread_id": "root", "end_byte_offset": 7}}),
            CutCheck::Unreadable,
            "父历史固定前缀在读取期间变化，请重试",
        ),
        (
            "nooffset",
            json!({"forked_from_id": "root", "history_base": {"thread_id": "root", "end_byte_offset": -1}}),
            CutCheck::Boundary,
            "history_base 前缀偏移必须是非负整数",
        ),
        (
            "noparent",
            json!({"history_base": {"end_byte_offset": 0}}),
            CutCheck::Boundary,
            "history_base 缺少父线程 ID",
        ),
        (
            "missing",
            json!({"forked_from_id": "gone", "history_base": {"thread_id": "gone", "end_byte_offset": 0}}),
            CutCheck::Boundary,
            "父线程不在已配置索引中，请确认显式数据源包含其原生文件",
        ),
        (
            "selfcycle",
            json!({"forked_from_id": "selfcycle", "history_base": {"thread_id": "selfcycle", "end_byte_offset": 0}}),
            CutCheck::Boundary,
            "分叉历史依赖存在循环",
        ),
    ];
    // Legal shapes — `forked_from_id` naming another
    // thread than `history_base.thread_id`, and no `history_base` at all.
    let legal = vec![
        (
            "mismatch",
            json!({"forked_from_id": "other", "history_base": {"thread_id": "root", "end_byte_offset": 0}}),
        ),
        ("nobase", json!({"forked_from_id": "root"})),
    ];
    let mut list = vec![root];
    let mut outcomes = BTreeMap::new();
    for (sid, extra, outcome, _) in &cases {
        let entry = make(sid, extra.clone());
        outcomes.insert(
            entry.summary.codex.as_ref().unwrap().history_base["end_byte_offset"].as_u64(),
            *outcome,
        );
        list.push(entry);
    }
    for (sid, extra) in &legal {
        list.push(make(sid, extra.clone()));
    }
    let all = entries(list);
    let built = graph::build(&all, &mut |_, cut| outcomes[&Some(cut)], &BTreeMap::new());
    let rows = by_uid(&built.rows);
    let row_of = |sid: &str| {
        rows[&uid_for(
            "codex",
            Path::new(&format!("/synthetic/codex/rollout-{sid}.jsonl")),
        )]
            .clone()
    };
    for (sid, _, _, message) in &cases {
        let row = row_of(sid);
        assert_eq!(row["supported"], false, "{sid}");
        assert_eq!(row["migration_warnings"], json!([message]), "{sid}");
        // The physical outcome never touches the logical lineage: a bad cut
        // still names its `forked_from_id` root (no cut check),
        // while no or an unindexed parent leaves the row undecorated.
        if row["forked_from_id"] == "root" {
            assert_eq!(row["root_sid"], "root", "{sid}");
            assert_eq!(row["fork_depth"], 1, "{sid}");
        } else {
            assert!(row.get("root_sid").is_none(), "{sid}");
        }
    }
    let mismatch = row_of("mismatch");
    assert_eq!(mismatch["supported"], true);
    assert_eq!(mismatch["migration_warnings"], json!([]));
    assert!(
        mismatch.get("root_sid").is_none(),
        "the logical parent `other` is not indexed"
    );
    let nobase = row_of("nobase");
    assert_eq!(nobase["supported"], true);
    assert_eq!(nobase["migration_warnings"], json!([]));
    assert_eq!(nobase["root_sid"], "root");
    assert_eq!(nobase["fork_depth"], 1);
    assert_eq!(
        nobase["size"],
        all[&nobase["uid"].as_str().unwrap().to_owned()]
            .summary
            .size
    );
    // A zero cut inherits nothing but still validates the chain.
    let zero = make(
        "zero",
        json!({"forked_from_id": "root", "history_base": {"thread_id": "root", "end_byte_offset": 0}}),
    );
    let zero_uid = zero.uid.clone();
    let all = entries(vec![make("root", json!({})), zero]);
    let built = graph::build(&all, &mut |_, _| CutCheck::Boundary, &BTreeMap::new());
    let row = &by_uid(&built.rows)[&zero_uid];
    assert_eq!(row["supported"], true);
    assert_eq!(row["fork_depth"], 1);
    assert_eq!(row["size"], all[&zero_uid].summary.size);
}

/// The row lineage is the walk over
/// `forked_from_id`, independent of `history_base`.
#[test]
fn graph_legacy_fork_chain_decorates_rows_along_forked_from_id() {
    let make = |sid: &str, extra: Value, text: &str| {
        entry(
            "codex",
            &format!("/synthetic/codex/rollout-{sid}.jsonl"),
            &encoded(&[codex_meta(sid, extra), codex_message("user", text, 1)]),
            None,
        )
    };
    let c = make(
        "C",
        json!({"timestamp": "2026-09-01T00:00:00Z"}),
        "C base title",
    );
    let b = make("B", json!({"forked_from_id": "C"}), "B base title");
    let a = make("A", json!({"forked_from_id": "B"}), "A base title");
    let (a_uid, a_size) = (a.uid.clone(), a.summary.size);
    let (b_uid, b_size) = (b.uid.clone(), b.summary.size);
    let c_uid = c.uid.clone();
    let all = entries(vec![a.clone(), b.clone(), c.clone()]);
    let mut cuts = Vec::new();
    let built = graph::build(
        &all,
        &mut |parent, cut| {
            cuts.push((parent.uid.clone(), cut));
            CutCheck::Boundary
        },
        &BTreeMap::new(),
    );
    assert!(
        cuts.is_empty(),
        "no history_base: nothing physical to check"
    );
    let rows = by_uid(&built.rows);
    let row = &rows[&a_uid];
    assert_eq!(row["supported"], true);
    assert_eq!(row["migration_warnings"], json!([]));
    assert_eq!(row["root_sid"], "C");
    assert_eq!(row["fork_depth"], 2);
    assert_eq!(row["created"], "2026-09-01T00:00:00.000Z");
    assert_eq!(row["title"], "C base title");
    assert_eq!(row["size"], a_size, "null history_base adds 0 per hop");
    let middle = &rows[&b_uid];
    assert_eq!(middle["root_sid"], "C");
    assert_eq!(middle["fork_depth"], 1);
    assert_eq!(middle["title"], "C base title");
    assert_eq!(middle["size"], b_size);
    assert!(rows[&c_uid].get("root_sid").is_none());
    assert_eq!(rows[&c_uid]["title"], "C base title");
    for uid in [&a_uid, &b_uid, &c_uid] {
        assert!(
            built
                .catalog
                .iter()
                .any(|seed| seed.uid == *uid && seed.scope.is_ok())
        );
    }
    // B missing from the index: the walk stops at the missing parent, so A's
    // chain is empty — own created/title/size, and never an error.
    let all = entries(vec![a.clone(), c.clone()]);
    let built = graph::build(&all, &mut |_, _| CutCheck::Boundary, &BTreeMap::new());
    let rows = by_uid(&built.rows);
    let row = &rows[&a_uid];
    assert_eq!(row["supported"], true);
    assert_eq!(row["migration_warnings"], json!([]));
    assert!(row.get("root_sid").is_none());
    assert!(row.get("fork_depth").is_none());
    assert_eq!(row["created"], a.summary.created);
    assert_eq!(row["title"], "A base title");
    assert_eq!(row["size"], a_size);
    assert_eq!(row["forked_from_id"], "B");
}

/// A rewind past the parent's own fork point —
/// `history_base` names the physical file (R) while `forked_from_id` names
/// the logical parent (Q). Open reads R at `c`; the row follows Q → R.
#[test]
fn graph_rewind_past_fork_reads_history_base_and_decorates_along_forked_from_id() {
    let make = |sid: &str, extra: Value, text: &str| {
        entry(
            "codex",
            &format!("/synthetic/codex/rollout-{sid}.jsonl"),
            &encoded(&[
                codex_meta(sid, extra),
                codex_message("user", text, 1),
                codex_message("assistant", &format!("{text} answer"), 2),
            ]),
            None,
        )
    };
    let r = make("R", json!({}), "R base title");
    let r_size = r.summary.size;
    let c2 = r_size - 40;
    let q = make(
        "Q",
        json!({"forked_from_id": "R", "history_base": {"thread_id": "R", "end_byte_offset": c2}}),
        "Q base title",
    );
    let q_size = q.summary.size;
    // `c` exceeds Q's own size: it is still clipped against Q (`min`).
    let c = q_size + 17;
    let a = make(
        "A",
        json!({"forked_from_id": "Q", "history_base": {"thread_id": "R", "end_byte_offset": c}}),
        "A base title",
    );
    let (a_uid, a_size) = (a.uid.clone(), a.summary.size);
    let (q_uid, r_uid) = (q.uid.clone(), r.uid.clone());
    let all = entries(vec![a, q, r]);
    let mut cuts = Vec::new();
    // Q's own cut is broken; A's physical chain is [(R, c)] alone, so A is
    // unaffected while Q is unsupported.
    let built = graph::build(
        &all,
        &mut |parent, cut| {
            cuts.push((parent.uid.clone(), cut));
            if cut == c2 {
                CutCheck::NotBoundary
            } else {
                CutCheck::Boundary
            }
        },
        &BTreeMap::new(),
    );
    cuts.sort();
    let mut expected = vec![(r_uid.clone(), c), (r_uid.clone(), c2)];
    expected.sort();
    assert_eq!(cuts, expected, "both cuts are validated on R, none on Q");
    let rows = by_uid(&built.rows);
    let row = &rows[&a_uid];
    assert_eq!(row["supported"], true);
    assert_eq!(row["migration_warnings"], json!([]));
    assert_eq!(row["root_sid"], "R");
    assert_eq!(row["fork_depth"], 2);
    assert_eq!(row["title"], "R base title");
    assert_eq!(row["created"], all[&r_uid].summary.created);
    assert_eq!(row["size"], a_size + c.min(q_size) + c2.min(r_size));
    assert_eq!(row["size"], a_size + q_size + c2);
    let seeds: BTreeMap<_, _> = built
        .catalog
        .iter()
        .map(|seed| (seed.uid.clone(), seed))
        .collect();
    assert_eq!(seeds[&a_uid].scope.as_ref().unwrap().session_id, "A");
    let q_row = &rows[&q_uid];
    assert_eq!(q_row["supported"], false);
    assert_eq!(
        q_row["migration_warnings"],
        json!(["父历史固定前缀不在完整 JSONL 行边界"])
    );
    assert_eq!(
        q_row["root_sid"], "R",
        "the logical lineage is not a cut check"
    );
    assert_eq!(seeds[&q_uid].scope.as_ref().unwrap_err().status, 501);
}

#[test]
fn graph_claude_sidecars_attach_by_exact_path_and_summary_failures_keep_their_reason() {
    let main = entry(
        "claude",
        "/synthetic/claude/proj/sess.jsonl",
        &encoded(&[claude_row("sess", "user", "u0", Value::Null, "Main")]),
        None,
    );
    let agent_bytes = encoded(&[claude_row("sess", "user", "au", Value::Null, "agent")]);
    let agent = entry(
        "claude",
        "/synthetic/claude/proj/sess/subagents/agent-one.jsonl",
        &agent_bytes,
        Some(br#"{"description": "Child", "agentType": "worker"}"#),
    );
    let orphan = entry(
        "claude",
        "/synthetic/claude/proj/other/subagents/agent-two.jsonl",
        &agent_bytes,
        None,
    );
    // A shape the reference adapter cannot read either (scalar `content`);
    // a corrupt line is only a note.
    let mut scalar = claude_row("broken-sess", "user", "u0", Value::Null, "Broken");
    scalar["message"]["content"] = json!(42);
    let broken = entry(
        "claude",
        "/synthetic/claude/proj/broken.jsonl",
        &encoded(&[scalar]),
        None,
    );
    let uids = [
        main.uid.clone(),
        agent.uid.clone(),
        orphan.uid.clone(),
        broken.uid.clone(),
    ];
    let all = entries(vec![main, agent, orphan, broken]);
    let built = graph::build(&all, &mut |_, _| CutCheck::Boundary, &BTreeMap::new());
    let rows = by_uid(&built.rows);
    let main_row = &rows[&uids[0]];
    assert_eq!(main_row["agents"], 1);
    let item = &main_row["agent_items"][0];
    assert_eq!(item["id"], "one");
    assert_eq!(item["title"], "Child");
    assert_eq!(item["type"], "worker");
    assert_eq!(item["cwd"], "/synthetic/history");
    assert_eq!(item["model"], Value::Null);
    assert_eq!(item["size"], agent_bytes.len() as u64);
    assert_eq!(item["migration_warnings"], json!([]));
    assert!(item["path"].as_str().unwrap().ends_with("agent-one.jsonl"));
    assert_eq!(built.owners[&uids[1]], uids[0]);
    // `proj/other/subagents/agent-two.jsonl` without `proj/other.jsonl`:
    // it is never listed; the uid keeps its typed 501.
    assert!(!rows.contains_key(&uids[2]), "orphan sidecar is no row");
    assert_eq!(
        (
            built.agent_errors[&uids[2]].status,
            built.agent_errors[&uids[2]].message.as_str()
        ),
        (501, "Claude 子代理的主会话不在已配置索引中")
    );
    assert!(!built.owners.contains_key(&uids[2]));
    let broken_row = &rows[&uids[3]];
    assert_eq!(broken_row["supported"], false);
    assert_eq!(
        broken_row["migration_warnings"],
        json!(["原生 content 类型无效"])
    );
    assert!(broken_row.get("cursor").is_none());
    let seeds: BTreeMap<_, _> = built
        .catalog
        .iter()
        .map(|seed| (seed.uid.clone(), seed))
        .collect();
    assert_eq!(seeds[&uids[3]].scope.as_ref().unwrap_err().status, 501);
    assert_eq!(seeds[&uids[0]].scope.as_ref().unwrap().session_id, "sess");
    assert_eq!(seeds[&uids[1]].declared_ids, vec!["sess".to_owned()]);
}

// ---------------------------------------------------------------------------
// Scale: run with `cargo test -p sessiondock --release --lib sessions::index::tests::benchmark -- --ignored --nocapture`.
// ---------------------------------------------------------------------------

fn generated_corpus(root: &Path, sessions: usize, bytes_per_session: usize) -> usize {
    let mut total = 0;
    let filler = "F".repeat(1800);
    for index in 0..sessions {
        let (source, path) = if index % 2 == 0 {
            let sid = format!("claude-{index:05}");
            (
                "claude",
                root.join("claude")
                    .join(format!("project-{}", index % 7))
                    .join(format!("{sid}.jsonl")),
            )
        } else {
            let sid = format!("codex-{index:05}");
            (
                "codex",
                root.join("codex/2026/09")
                    .join(format!("{:02}", index % 28 + 1))
                    .join(format!("rollout-{sid}.jsonl")),
            )
        };
        let sid = path.file_stem().unwrap().to_string_lossy().into_owned();
        let mut bytes = Vec::with_capacity(bytes_per_session + 4096);
        if source == "codex" {
            bytes.extend(encoded(&[codex_meta(&sid, json!({}))]));
        }
        let mut n = 0;
        while bytes.len() < bytes_per_session {
            let text = format!("Synthetic {sid} record {n} {filler}");
            let row = if source == "claude" {
                claude_row(
                    &sid,
                    if n % 2 == 0 { "user" } else { "assistant" },
                    &format!("r{n}"),
                    if n == 0 {
                        Value::Null
                    } else {
                        json!(format!("r{}", n - 1))
                    },
                    &text,
                )
            } else {
                codex_message(if n % 2 == 0 { "user" } else { "assistant" }, &text, n + 1)
            };
            bytes.extend(encoded(&[row]));
            n += 1;
        }
        write(&path, &bytes);
        total += bytes.len();
    }
    total
}

#[test]
#[ignore = "scale benchmark: 2000 sessions / 1 GB, prints cold and warm timings"]
fn benchmark_cold_and_warm_list_of_2000_sessions() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    for source in ["claude", "codex", "grok"] {
        fs::create_dir_all(root.join(source)).unwrap();
    }
    let sessions = 2000;
    let total = generated_corpus(root, sessions, 512 * 1024);
    let roots = SessionRoots {
        claude: Some(root.join("claude")),
        codex: Some(root.join("codex")),
        grok: Some(root.join("grok")),
    };
    let index = Index::new(roots, None);
    let cold = Instant::now();
    let snapshot = index.refresh(true).unwrap();
    let cold = cold.elapsed();
    assert_eq!(snapshot.sessions().len(), sessions);
    let warm = Instant::now();
    let again = index.refresh(true).unwrap();
    let warm = warm.elapsed();
    assert_eq!(again.sig(), snapshot.sig());
    assert_eq!(index.reads(), sessions);
    let ttl = Instant::now();
    index.refresh(false).unwrap();
    let ttl = ttl.elapsed();
    println!(
        "index benchmark: {sessions} sessions / {:.2} GB, workers {DEFAULT_WORKERS}: cold {:?}, warm (stat only) {:?}, ttl hit {:?}",
        total as f64 / 1e9,
        cold,
        warm,
        ttl
    );
    println!(
        "cold <= 1s: {}, warm <= 50ms: {}",
        cold <= Duration::from_secs(1),
        warm <= Duration::from_millis(50)
    );
}

#[test]
fn documented_row_fields_are_exactly_todays_set() {
    let temp = TempDir::new().unwrap();
    let corpus = build_corpus(temp.path());
    let index = Index::new(corpus.roots(), None);
    let snapshot = index.refresh(true).unwrap();
    let rows = by_uid(snapshot.sessions());
    let keys = |row: &Value| row.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(
        keys(&rows[&corpus.uid("claude-compact")]),
        [
            "sid",
            "title",
            "cwd",
            "created",
            "updated",
            "model",
            "branch",
            "migration_warnings",
            "uid",
            "source",
            "path",
            "size",
            "supported"
        ]
    );
    assert_eq!(
        keys(&rows[&corpus.uid("codex-fork")]),
        [
            "sid",
            "title",
            "cwd",
            "created",
            "updated",
            "model",
            "branch",
            "forked_from_id",
            "history_base",
            "migration_warnings",
            "uid",
            "source",
            "path",
            "size",
            "supported",
            "root_sid",
            "fork_depth"
        ]
    );
    assert_eq!(
        keys(&rows[&corpus.uid("grok-chat")]),
        [
            "sid",
            "title",
            "cwd",
            "created",
            "updated",
            "model",
            "branch",
            "migration_warnings",
            "uid",
            "source",
            "path",
            "size",
            "chat_exists",
            "supported"
        ]
    );
    assert_eq!(
        keys(&rows[&corpus.uid("codex-parent")]["agent_items"][0]),
        [
            "id",
            "title",
            "type",
            "active",
            "path",
            "cwd",
            "model",
            "created",
            "updated",
            "size",
            "supported",
            "migration_warnings"
        ]
    );
    let summary: &RowSummary = &snapshot
        .candidate(&corpus.uid("codex-parent"))
        .unwrap()
        .summary;
    assert_eq!(summary.migration_warnings(), Vec::<String>::new());
}

// ---------------------------------------------------------------------------
// Python oracle: the exact corpus and comparison rules of
// tests/list_rows_parity.py, applied to the index rows directly (the script
// itself needs the HTTP server). Run with
// `SESSIONDOCK_PYTHON_SOURCE=../sessiondock cargo test -p sessiondock --lib
// sessions::index::tests::python -- --ignored --nocapture`.
// ---------------------------------------------------------------------------

fn parity_value(row: &Value, key: &str) -> Value {
    match key {
        "agent_items" => {
            let mut ids: Vec<String> = row["agent_items"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item["id"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            ids.sort();
            json!(ids)
        }
        "supported" => json!(row["supported"] != false),
        _ => match &row[key] {
            Value::Null => Value::Null,
            Value::String(text) if text.is_empty() => Value::Null,
            other => other.clone(),
        },
    }
}

fn instant(value: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    let text = value.as_str()?.replace('Z', "+00:00");
    chrono::DateTime::parse_from_rfc3339(&text)
        .ok()
        .map(|parsed| parsed.to_utc())
}

#[test]
#[ignore = "Python oracle: needs python3 and SESSIONDOCK_PYTHON_SOURCE (the Python checkout)"]
fn python_list_sessions_rows_match_field_by_field() {
    let Ok(source) = std::env::var("SESSIONDOCK_PYTHON_SOURCE") else {
        eprintln!("SESSIONDOCK_PYTHON_SOURCE not set; nothing compared");
        return;
    };
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests");
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let script = r#"
import json, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import list_rows_parity as m
from fixture_gen import SOURCES, generate
from history_parity import Corpus
root, source = Path(sys.argv[2]), Path(sys.argv[3]).resolve()
generate(root, SOURCES, 3, 50, 0, 1, 1, 1)
corpus = Corpus(root)
m.edges(corpus, source)
adapters = m.bind(source, root)
rows = [row for src in SOURCES for row in (adapters[src].list_sessions() or [])]
print(json.dumps(rows, ensure_ascii=False, default=str))
"#;
    let output = std::process::Command::new("python3")
        .args(["-c", script])
        .arg(&tests)
        .arg(&root)
        .arg(&source)
        .output()
        .expect("python3 available");
    assert!(
        output.status.success(),
        "python oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let python: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    let python = by_uid(&python);
    let index = Index::new(
        SessionRoots {
            claude: Some(root.join("claude")),
            codex: Some(root.join("codex")),
            grok: Some(root.join("grok")),
        },
        None,
    );
    let snapshot = index.refresh(true).unwrap();
    let rust = by_uid(snapshot.sessions());
    let fields = [
        "uid",
        "source",
        "sid",
        "title",
        "cwd",
        "created",
        "updated",
        "size",
        "model",
        "branch",
        "forked_from_id",
        "root_sid",
        "agent_items",
        "supported",
    ];
    let (mut pass, mut delta, mut diff) = (0, 0, 0);
    let uids: BTreeSet<_> = python.keys().chain(rust.keys()).collect();
    for uid in uids {
        let (Some(expected), Some(actual)) = (python.get(uid), rust.get(uid)) else {
            println!(
                "DIFF {uid} missing python={} rust={}",
                python.contains_key(uid),
                rust.contains_key(uid)
            );
            diff += 1;
            continue;
        };
        let mut worst = "PASS";
        for field in fields {
            let (left, right) = (parity_value(expected, field), parity_value(actual, field));
            if left == right {
                continue;
            }
            let kind = if matches!(field, "created" | "updated") {
                match (instant(&expected[field]), instant(&actual[field])) {
                    (Some(a), Some(b)) if a == b => continue,
                    (Some(_), Some(_)) if field == "updated" => "DELTA",
                    _ => "DIFF",
                }
            } else {
                "DIFF"
            };
            println!("{kind} {uid} {field} python={left} rust={right}");
            if kind == "DIFF" || worst == "PASS" {
                worst = kind;
            }
        }
        match worst {
            "PASS" => pass += 1,
            "DELTA" => delta += 1,
            _ => diff += 1,
        }
        if worst == "PASS" {
            println!("PASS {uid} {}", expected["sid"]);
        }
    }
    println!("SUMMARY {pass} PASS, {delta} DELTA, {diff} DIFF");
    assert_eq!(
        diff, 0,
        "every row field must match the Python list_sessions derivation"
    );
}

// ---------------------------------------------------------------------------
// `agent_items[].active` and `continued_in` — the
// `ClaudeAgentItemTests` / Codex subagent / `continued_in` cases end to end
// through `Index::refresh`.
// ---------------------------------------------------------------------------

/// One project, one owner transcript and its
/// `subagents/agent-*.jsonl` + `.meta.json` sidecars.
struct AgentCorpus {
    root: PathBuf,
    owner: PathBuf,
    subagents: PathBuf,
}

impl AgentCorpus {
    fn new(root: &Path) -> Self {
        let project = root.join("claude").join("-tmp-project");
        let subagents = project.join("parent-session").join("subagents");
        fs::create_dir_all(&subagents).unwrap();
        for source in ["codex", "grok"] {
            fs::create_dir_all(root.join(source)).unwrap();
        }
        Self {
            root: root.to_path_buf(),
            owner: project.join("parent-session.jsonl"),
            subagents,
        }
    }

    fn roots(&self) -> SessionRoots {
        SessionRoots {
            claude: Some(self.root.join("claude")),
            codex: Some(self.root.join("codex")),
            grok: Some(self.root.join("grok")),
        }
    }

    fn parent_rows() -> Vec<Value> {
        vec![json!({
            "type": "user", "uuid": "u0", "parentUuid": null, "isSidechain": false,
            "sessionId": "parent-session", "cwd": "/tmp/project",
            "timestamp": "2026-09-12T00:00:00.000Z",
            "message": {"role": "user", "content": "主任务"},
        })]
    }

    fn write_owner(&self, extra: &[Value]) {
        let mut rows = Self::parent_rows();
        rows.extend(extra.iter().cloned());
        write(&self.owner, &encoded(&rows));
    }

    fn append_owner(&self, rows: &[Value]) {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&self.owner)
            .unwrap();
        file.write_all(&encoded(rows)).unwrap();
    }

    fn agent(&self, id: &str, rows: &[Value], tool_use_id: &str) -> PathBuf {
        let path = self.subagents.join(format!("agent-{id}.jsonl"));
        let rows: Vec<Value> = rows
            .iter()
            .map(|row| {
                let mut row = row.clone();
                row["isSidechain"] = json!(true);
                row["agentId"] = json!(id);
                row
            })
            .collect();
        write(&path, &encoded(&rows));
        write(
            &self.subagents.join(format!("agent-{id}.meta.json")),
            serde_json::to_string(&json!({
                "agentType": "general-purpose", "description": format!("任务 {id}"),
                "toolUseId": tool_use_id,
            }))
            .unwrap()
            .as_bytes(),
        );
        path
    }

    fn user(ts: &str, text: &str) -> Value {
        json!({"type": "user", "timestamp": ts, "message": {"role": "user", "content": text}})
    }

    fn assistant(ts: &str, stop_reason: Value, tool: bool) -> Value {
        let block = if tool {
            json!({"type": "tool_use", "id": "toolu_call", "name": "Bash", "input": {}})
        } else {
            json!({"type": "text", "text": "结论"})
        };
        json!({"type": "assistant", "timestamp": ts,
            "message": {"role": "assistant", "stop_reason": stop_reason, "content": [block]}})
    }

    fn tool_result(ts: &str) -> Value {
        json!({"type": "user", "timestamp": ts, "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_call", "content": "ok"}]}})
    }

    fn notice_text(agent: &str, status: &str) -> String {
        format!(
            "<task-notification>\n<task-id>{agent}</task-id>\n<status>{status}</status>\n<summary>Agent finished</summary>\n</task-notification>"
        )
    }

    fn notice(ts: &str, agent: &str, status: &str, shape: &str) -> Value {
        let text = Self::notice_text(agent, status);
        if shape == "user" {
            return json!({"type": "user", "timestamp": ts,
                "message": {"role": "user", "content": text}});
        }
        json!({"type": "attachment", "timestamp": ts,
            "attachment": {"type": "queued_command", "commandMode": "task-notification",
                           "prompt": text, "timestamp": ts}})
    }

    fn agent_result(ts: &str, agent: &str, status: &str, tool_use_id: &str) -> Value {
        json!({"type": "user", "timestamp": ts,
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": tool_use_id,
                 "content": [{"type": "text", "text": if status == "async_launched" {
                     "Async agent launched successfully" } else { "报告" }}]}]},
            "toolUseResult": {"status": status, "agentId": agent}})
    }

    /// `agent_items` of the owner row by agent id, from a forced refresh.
    fn items(&self, index: &Index) -> BTreeMap<String, Value> {
        let snapshot = index.refresh(true).unwrap();
        let owner = uid_for("claude", &self.owner);
        let row = &by_uid(snapshot.sessions())[&owner];
        row["agent_items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| (item["id"].as_str().unwrap().to_owned(), item.clone()))
            .collect()
    }
}

fn actives(items: &BTreeMap<String, Value>) -> BTreeMap<&str, bool> {
    items
        .iter()
        .map(|(id, item)| (id.as_str(), item["active"].as_bool().unwrap()))
        .collect()
}

#[test]
fn claude_agent_items_carry_start_end_and_running_state() {
    let temp = TempDir::new().unwrap();
    let corpus = AgentCorpus::new(temp.path());
    let (user, assistant, tool_result) = (
        AgentCorpus::user,
        AgentCorpus::assistant,
        AgentCorpus::tool_result,
    );
    corpus.agent(
        "done",
        &[
            user("2026-09-12T00:10:00.000Z", "继续"),
            assistant("2026-09-12T00:10:05.000Z", json!("tool_use"), true),
            tool_result("2026-09-12T00:10:06.000Z"),
            assistant("2026-09-12T00:12:00.000Z", json!("end_turn"), false),
        ],
        "toolu_xxxxxxxxxxxxxxxxxxxx",
    );
    corpus.agent(
        "streamed",
        &[
            user("2026-09-12T00:20:00.000Z", "继续"),
            assistant("2026-09-12T00:22:00.000Z", Value::Null, false),
        ],
        "toolu_xxxxxxxxxxxxxxxxxxxx",
    );
    corpus.agent(
        "resumed",
        &[
            user("2026-09-12T00:30:00.000Z", "继续"),
            assistant("2026-09-12T00:31:00.000Z", Value::Null, false),
            user("2026-09-12T00:40:00.000Z", "再来一次"),
            assistant("2026-09-12T00:41:00.000Z", json!("tool_use"), true),
        ],
        "toolu_xxxxxxxxxxxxxxxxxxxx",
    );
    corpus.agent(
        "killed",
        &[
            user("2026-09-12T00:50:00.000Z", "继续"),
            assistant("2026-09-12T00:51:00.000Z", json!("tool_use"), true),
        ],
        "toolu_xxxxxxxxxxxxxxxxxxxx",
    );
    corpus.agent(
        "fresh",
        &[user("2026-09-12T01:00:00.000Z", "继续")],
        "toolu_fresh",
    );
    corpus.agent(
        "foreground",
        &[
            user("2026-09-12T01:10:00.000Z", "继续"),
            assistant("2026-09-12T01:12:00.000Z", Value::Null, false),
        ],
        "toolu_fg",
    );
    corpus.write_owner(&[
        AgentCorpus::notice(
            "2026-09-12T00:12:00.100Z",
            "done",
            "completed",
            "attachment",
        ),
        AgentCorpus::notice("2026-09-12T00:22:00.100Z", "streamed", "completed", "user"),
        AgentCorpus::notice(
            "2026-09-12T00:31:00.100Z",
            "resumed",
            "failed",
            "attachment",
        ),
        AgentCorpus::notice("2026-09-12T00:52:00.000Z", "killed", "killed", "attachment"),
        AgentCorpus::agent_result(
            "2026-09-12T01:00:00.100Z",
            "fresh",
            "async_launched",
            "toolu_fresh",
        ),
        AgentCorpus::agent_result(
            "2026-09-12T01:12:00.200Z",
            "foreground",
            "completed",
            "toolu_fg",
        ),
    ]);
    let index = Index::new(corpus.roots(), None);
    let items = corpus.items(&index);
    assert_eq!(
        actives(&items),
        BTreeMap::from([
            ("done", false),
            ("streamed", false),
            ("resumed", true),
            ("killed", false),
            ("fresh", true),
            ("foreground", false),
        ])
    );
    assert_eq!(items["done"]["created"], "2026-09-12T00:10:00.000Z");
    assert_eq!(items["done"]["updated"], "2026-09-12T00:12:00.000Z");
    assert_eq!(items["resumed"]["created"], "2026-09-12T00:30:00.000Z");
    assert_eq!(items["resumed"]["updated"], "2026-09-12T00:41:00.000Z");
    assert_eq!(items["fresh"]["title"], "任务 fresh");
    assert_eq!(items["fresh"]["type"], "general-purpose");
    assert_eq!(
        index.stop_scans(),
        1,
        "one owner with open sidecars scanned once"
    );
    // A hot refresh with unchanged stamps reads nothing.
    let reads = index.reads();
    corpus.items(&index);
    assert_eq!(index.reads(), reads);
    assert_eq!(index.stop_scans(), 1);
    // The unopened facade row carries the same item fields plus the cursor.
    let store = SessionStore::new(corpus.roots());
    let listed = store.list(true).unwrap();
    let row = &by_uid(listed["sessions"].as_array().unwrap())[&uid_for("claude", &corpus.owner)];
    let facade: BTreeMap<String, Value> = row["agent_items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| (item["id"].as_str().unwrap().to_owned(), item.clone()))
        .collect();
    assert_eq!(actives(&facade), actives(&items));
    assert!(facade["resumed"]["cursor"].is_object());
}

#[test]
fn claude_late_notice_copies_and_refusal_do_not_flip_a_running_agent() {
    let temp = TempDir::new().unwrap();
    let corpus = AgentCorpus::new(temp.path());
    corpus.agent(
        "worker",
        &[
            AgentCorpus::user("2026-09-12T00:10:00.000Z", "继续"),
            AgentCorpus::assistant("2026-09-12T00:19:00.600Z", json!("end_turn"), false),
            AgentCorpus::user("2026-09-12T00:19:00.700Z", "追加一条任务"),
            AgentCorpus::assistant("2026-09-12T00:19:30.000Z", json!("refusal"), false),
            AgentCorpus::assistant("2026-09-12T00:20:59.000Z", json!("tool_use"), true),
        ],
        "toolu_xxxxxxxxxxxxxxxxxxxx",
    );
    let text = AgentCorpus::notice_text("worker", "completed");
    let queue = |ts: &str, operation: &str| json!({"type": "queue-operation", "operation": operation, "timestamp": ts, "content": text});
    corpus.write_owner(&[
        queue("2026-09-12T00:19:00.650Z", "enqueue"),
        queue("2026-09-12T00:21:00.400Z", "remove"),
        AgentCorpus::notice(
            "2026-09-12T00:19:00.650Z",
            "worker",
            "completed",
            "attachment",
        ),
        queue("2026-09-12T00:33:00.000Z", "enqueue"),
        AgentCorpus::notice("2026-09-12T00:35:00.000Z", "worker", "completed", "user"),
    ]);
    let index = Index::new(corpus.roots(), None);
    assert_eq!(
        actives(&corpus.items(&index)),
        BTreeMap::from([("worker", true)])
    );
    // A notice with new text (a second real stop) stops it.
    corpus.append_owner(&[AgentCorpus::notice(
        "2026-09-12T00:40:00.000Z",
        "worker",
        "failed",
        "attachment",
    )]);
    assert_eq!(
        actives(&corpus.items(&index)),
        BTreeMap::from([("worker", false)])
    );
    assert_eq!(index.stop_scans(), 2, "the appended owner was read again");
}

#[test]
fn claude_stop_notices_are_read_incrementally_and_only_from_complete_lines() {
    let temp = TempDir::new().unwrap();
    let corpus = AgentCorpus::new(temp.path());
    let agent = corpus.agent(
        "worker",
        &[
            AgentCorpus::user("2026-09-12T01:00:00.000Z", "继续"),
            AgentCorpus::assistant("2026-09-12T01:05:00.000Z", Value::Null, false),
        ],
        "toolu_xxxxxxxxxxxxxxxxxxxx",
    );
    corpus.write_owner(&[]);
    let index = Index::new(corpus.roots(), None);
    assert_eq!(
        actives(&corpus.items(&index)),
        BTreeMap::from([("worker", true)])
    );
    assert_eq!(index.stop_scans(), 1);

    let line = serde_json::to_vec(&AgentCorpus::notice(
        "2026-09-12T01:05:00.100Z",
        "worker",
        "completed",
        "attachment",
    ))
    .unwrap();
    let (first, second) = line.split_at(line.len() / 2);
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&corpus.owner)
        .unwrap();
    file.write_all(first).unwrap();
    drop(file);
    // A half line is neither a stop nor consumed.
    assert_eq!(
        actives(&corpus.items(&index)),
        BTreeMap::from([("worker", true)])
    );
    assert_eq!(index.stop_scans(), 2);
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&corpus.owner)
        .unwrap();
    file.write_all(second).unwrap();
    file.write_all(b"\n").unwrap();
    drop(file);
    assert_eq!(
        actives(&corpus.items(&index)),
        BTreeMap::from([("worker", false)])
    );
    assert_eq!(index.stop_scans(), 3);

    // A user record appended to the sidecar after the notice is a wake-up
    // (SendMessage); the owner is unchanged and not read again.
    let mut file = fs::OpenOptions::new().append(true).open(&agent).unwrap();
    let mut wake = AgentCorpus::user("2026-09-12T01:20:00.000Z", "继续");
    wake["isSidechain"] = json!(true);
    wake["agentId"] = json!("worker");
    file.write_all(&encoded(&[wake])).unwrap();
    drop(file);
    assert_eq!(
        actives(&corpus.items(&index)),
        BTreeMap::from([("worker", true)])
    );
    assert_eq!(index.stop_scans(), 3);

    // A rewrite that shortens the owner rescans from the start.
    corpus.write_owner(&[]);
    assert_eq!(
        actives(&corpus.items(&index)),
        BTreeMap::from([("worker", true)])
    );
    corpus.write_owner(&[AgentCorpus::notice(
        "2026-09-12T01:21:00.000Z",
        "worker",
        "killed",
        "attachment",
    )]);
    assert_eq!(
        actives(&corpus.items(&index)),
        BTreeMap::from([("worker", false)])
    );
}

#[test]
fn claude_owners_without_an_open_sidecar_are_never_scanned_for_stops() {
    let temp = TempDir::new().unwrap();
    let corpus = AgentCorpus::new(temp.path());
    corpus.agent(
        "done",
        &[
            AgentCorpus::user("2026-09-12T00:10:00.000Z", "继续"),
            AgentCorpus::assistant("2026-09-12T00:12:00.000Z", json!("end_turn"), false),
        ],
        "toolu_xxxxxxxxxxxxxxxxxxxx",
    );
    // No user/assistant record at all: closed as well (`closed is
    // False` never holds).
    corpus.agent(
        "empty",
        &[json!({"type": "progress", "timestamp": "2026-09-12T00:15:00.000Z"})],
        "toolu_xxxxxxxxxxxxxxxxxxxx",
    );
    corpus.write_owner(&[AgentCorpus::notice(
        "2026-09-12T00:12:00.100Z",
        "done",
        "completed",
        "attachment",
    )]);
    let index = Index::new(corpus.roots(), None);
    let items = corpus.items(&index);
    assert_eq!(
        actives(&items),
        BTreeMap::from([("done", false), ("empty", false)])
    );
    assert_eq!(
        index.stop_scans(),
        0,
        "no open turn: the owner file is not read"
    );
    assert_eq!(items["empty"]["updated"], "2026-09-12T00:15:00.000Z");
}

#[test]
fn codex_subagent_items_carry_turn_state_and_last_record_time() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let day = root.join("codex/2026/09/12");
    fs::create_dir_all(&day).unwrap();
    for source in ["claude", "grok"] {
        fs::create_dir_all(root.join(source)).unwrap();
    }
    let parent_id = "00000000-0000-0000-0000-000000000030";
    let event = |ts: &str, kind: &str, extra: Value| {
        let mut payload = json!({"type": kind, "turn_id": "t"});
        for (key, value) in extra.as_object().into_iter().flatten() {
            payload[key] = value.clone();
        }
        json!({"type": "event_msg", "timestamp": ts, "payload": payload})
    };
    let write_agent = |name: &str, agent_id: &str, rows: &[Value]| {
        let meta = json!({"type": "session_meta", "timestamp": "2026-09-12T01:00:00Z",
            "payload": {"id": agent_id, "session_id": parent_id, "parent_thread_id": parent_id,
                "thread_source": "subagent",
                "source": {"subagent": {"thread_spawn": {"parent_thread_id": parent_id,
                    "depth": 1, "agent_path": format!("/root/{name}")}}},
                "timestamp": "2026-09-12T01:00:00Z", "cwd": "/tmp/project"}});
        let mut all = vec![meta];
        all.extend(rows.iter().cloned());
        write(
            &day.join(format!("rollout-{name}-{agent_id}.jsonl")),
            &encoded(&all),
        );
    };
    write(
        &day.join(format!("rollout-parent-{parent_id}.jsonl")),
        &encoded(&[
            json!({"type": "session_meta", "timestamp": "2026-09-12T00:59:00Z",
            "payload": {"id": parent_id, "session_id": parent_id, "thread_source": "user",
                "timestamp": "2026-09-12T00:59:00Z", "cwd": "/tmp/project"}}),
        ]),
    );
    let ids: BTreeMap<&str, String> = ["running", "done", "aborted", "resumed"]
        .iter()
        .enumerate()
        .map(|(n, kind)| {
            (
                *kind,
                format!("00000000-0000-0000-0000-00000000003{}", n + 1),
            )
        })
        .collect();
    write_agent(
        "running",
        &ids["running"],
        &[
            event("2026-09-12T01:01:00Z", "task_started", json!({})),
            json!({"type": "response_item", "timestamp": "2026-09-12T01:02:00Z",
                "payload": {"type": "reasoning"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-12T01:03:00Z",
                "payload": {"type": "token_count"}}),
        ],
    );
    write_agent(
        "done",
        &ids["done"],
        &[
            event("2026-09-12T01:01:00Z", "task_started", json!({})),
            event(
                "2026-09-12T01:10:00Z",
                "task_complete",
                json!({"completed_at": 1789166200}),
            ),
        ],
    );
    write_agent(
        "aborted",
        &ids["aborted"],
        &[
            event("2026-09-12T01:01:00Z", "task_started", json!({})),
            event(
                "2026-09-12T01:05:00Z",
                "turn_aborted",
                json!({"reason": "interrupted"}),
            ),
        ],
    );
    write_agent(
        "resumed",
        &ids["resumed"],
        &[
            event("2026-09-12T01:01:00Z", "task_started", json!({})),
            event("2026-09-12T01:05:00Z", "task_complete", json!({})),
            event("2026-09-12T01:20:00Z", "task_started", json!({})),
            json!({"type": "response_item", "timestamp": "2026-09-12T01:21:00Z",
                "payload": {"type": "message"}}),
        ],
    );
    let index = Index::new(
        SessionRoots {
            claude: Some(root.join("claude")),
            codex: Some(root.join("codex")),
            grok: Some(root.join("grok")),
        },
        None,
    );
    let snapshot = index.refresh(true).unwrap();
    let rows = by_uid(snapshot.sessions());
    let parent = rows
        .values()
        .find(|row| row["sid"] == parent_id)
        .expect("parent row");
    let items: BTreeMap<String, Value> = parent["agent_items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["title"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("/root/")
                    .to_owned(),
                item.clone(),
            )
        })
        .collect();
    assert_eq!(
        actives(&items),
        BTreeMap::from([
            ("running", true),
            ("done", false),
            ("aborted", false),
            ("resumed", true),
        ])
    );
    assert_eq!(items["done"]["updated"], "2026-09-12T01:10:00.000Z");
    assert_eq!(items["running"]["updated"], "2026-09-12T01:03:00.000Z");
    assert_eq!(items["done"]["created"], "2026-09-12T01:00:00.000Z");
    assert!(rows.values().all(|row| row.get("active").is_none()));
    assert_eq!(index.stop_scans(), 0, "Codex needs no owner scan");
}

#[test]
fn claude_continued_in_resolves_to_the_uid_of_the_indexed_continuation() {
    // Unresolved, self-referencing and last-record-wins shapes.
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let project = root.join("claude").join("proj");
    fs::create_dir_all(&project).unwrap();
    for source in ["codex", "grok"] {
        fs::create_dir_all(root.join(source)).unwrap();
    }
    let user = |sid: &str, ts: &str, text: &str| {
        json!({"type": "user", "sessionId": sid, "timestamp": ts, "cwd": "/repo",
            "message": {"role": "user", "content": text}})
    };
    let continued = |sid: &str, target: &str, ts: &str| {
        json!({"type": "continued-in", "sessionId": sid, "continuedInSessionId": target,
            "timestamp": ts})
    };
    let parent = project.join("sid-parent.jsonl");
    write(
        &parent,
        &encoded(&[
            user("sid-parent", "2026-09-12T10:00:00Z", "hello"),
            continued("sid-parent", "sid-child", "2026-09-12T12:00:00Z"),
        ]),
    );
    let child = project.join("sid-child.jsonl");
    write(
        &child,
        &encoded(&[user("sid-child", "2026-09-12T12:00:01Z", "continued")]),
    );
    // Names a session that is not indexed: no field.
    let dangling = project.join("sid-dangling.jsonl");
    write(
        &dangling,
        &encoded(&[
            user("sid-dangling", "2026-09-12T10:00:00Z", "hello"),
            continued("sid-dangling", "sid-nowhere", "2026-09-12T12:00:00Z"),
        ]),
    );
    // Names itself: dropped.
    let selfish = project.join("sid-self.jsonl");
    write(
        &selfish,
        &encoded(&[
            user("sid-self", "2026-09-12T10:00:00Z", "hello"),
            continued("sid-self", "sid-self", "2026-09-12T12:00:00Z"),
        ]),
    );
    // Two records: the last one in the tail wins; an empty id is skipped.
    let twice = project.join("sid-twice.jsonl");
    write(
        &twice,
        &encoded(&[
            user("sid-twice", "2026-09-12T10:00:00Z", "hello"),
            continued("sid-twice", "sid-parent", "2026-09-12T11:00:00Z"),
            continued("sid-twice", "sid-child", "2026-09-12T12:00:00Z"),
            continued("sid-twice", "", "2026-09-12T13:00:00Z"),
        ]),
    );
    // A sidecar's tail is never a source of `continued_in`.
    let subagents = project.join("sid-parent").join("subagents");
    fs::create_dir_all(&subagents).unwrap();
    let mut sidecar_user = user("sid-parent", "2026-09-12T10:30:00Z", "agent");
    sidecar_user["isSidechain"] = json!(true);
    write(
        &subagents.join("agent-worker.jsonl"),
        &encoded(&[
            sidecar_user,
            continued("sid-parent", "sid-child", "2026-09-12T10:31:00Z"),
        ]),
    );
    let index = Index::new(
        SessionRoots {
            claude: Some(root.join("claude")),
            codex: Some(root.join("codex")),
            grok: Some(root.join("grok")),
        },
        None,
    );
    let snapshot = index.refresh(true).unwrap();
    let rows = by_uid(snapshot.sessions());
    let child_uid = uid_for("claude", &child);
    assert_eq!(rows[&uid_for("claude", &parent)]["continued_in"], child_uid);
    assert_eq!(rows[&uid_for("claude", &twice)]["continued_in"], child_uid);
    for path in [&child, &dangling, &selfish] {
        assert!(
            rows[&uid_for("claude", path)].get("continued_in").is_none(),
            "{}",
            path.display()
        );
    }
    assert!(
        rows[&uid_for("claude", &parent)]["agent_items"][0]
            .get("continued_in")
            .is_none()
    );
    assert!(
        rows.values()
            .all(|row| row.get("continued_in_sid").is_none())
    );
    // The facade publishes the same field and it is part of the signature.
    let store = SessionStore::new(SessionRoots {
        claude: Some(root.join("claude")),
        codex: Some(root.join("codex")),
        grok: Some(root.join("grok")),
    });
    let listed = store.list(true).unwrap();
    let published = by_uid(listed["sessions"].as_array().unwrap());
    assert_eq!(
        published[&uid_for("claude", &parent)]["continued_in"],
        child_uid
    );
}
