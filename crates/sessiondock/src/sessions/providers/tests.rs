use super::*;

fn rows(records: Vec<Value>) -> Vec<(Value, u64)> {
    let mut offset = 0;
    records
        .into_iter()
        .map(|record| {
            offset += record.to_string().len() as u64 + 1;
            (record, offset)
        })
        .collect()
}

fn message(kind: &str, id: &str, parent: Value, text: &str) -> Value {
    json!({"type": kind, "uuid": id, "parentUuid": parent,
           "timestamp": "2026-09-11T10:00:00.123Z", "sessionId": "synthetic",
           "message": {"role": kind, "content": text}})
}

fn project(source: &str, records: &[(Value, u64)]) -> (Value, Vec<Value>) {
    let (meta, events, error) = parse(
        source,
        Path::new("synthetic.jsonl"),
        records,
        None,
        "2026-09-11T00:00:00.000Z",
    );
    assert!(error.is_none(), "{error:?}");
    (
        meta,
        events.into_iter().map(|event| event.message).collect(),
    )
}

#[test]
fn malformed_history_base_is_not_coerced_to_an_independent_thread() {
    for base in [json!("invalid"), json!([]), json!(42), json!(false)] {
        let records = rows(vec![
            json!({"type":"session_meta","payload":{"id":"synthetic","history_base":base}}),
        ]);
        let (_, _, error) = parse("codex", Path::new("synthetic.jsonl"), &records, None, "");
        assert!(error.unwrap().contains("history_base"));
    }
}

#[test]
fn rich_tool_fields_preserve_raw_arguments_and_report_presentation_limits() {
    let content = "x".repeat(512 * 1024 + 1);
    let records = rows(vec![
        json!({"type":"response_item","payload":{"type":"function_call","name":"Write","call_id":"large","turn_id":"turn","arguments":{"file_path":"synthetic.rs","content":content}}}),
        json!({"type":"response_item","payload":{"type":"function_call","name":"Edit","call_id":"small","turn_id":"turn","arguments":{"file_path":"synthetic.rs","old_string":"old","new_string":"new"}}}),
    ]);
    let (_, messages) = project("codex", &records);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "tool");
    assert_eq!(messages[0]["turn_id"], "turn");
    assert!(messages[0]["changes"].is_null());
    assert!(
        messages[0]["changes_unavailable_reason"]
            .as_str()
            .unwrap()
            .contains("512 KiB")
    );
    let arguments: Value = serde_json::from_str(messages[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(arguments["content"], content);
    assert_eq!(messages[1]["changes"][0]["added"], 1);
    assert_eq!(messages[1]["changes"][0]["before_complete"], false);
}

fn visible(messages: &[Value]) -> Vec<(&str, &str)> {
    messages
        .iter()
        .filter(|message| message["role"] != "status")
        .map(|message| {
            (
                message["role"].as_str().unwrap(),
                message["text"].as_str().unwrap(),
            )
        })
        .collect()
}

fn branch() -> Vec<Value> {
    vec![
        message("user", "u0", Value::Null, "共同输入"),
        message("assistant", "a0", json!("u0"), "共同回答"),
        message("user", "old-u", json!("a0"), "完成的旧输入"),
        message("assistant", "old-a", json!("old-u"), "完成的旧回答"),
        message("user", "new-u", json!("a0"), "新分支输入"),
        message("assistant", "new-a", json!("new-u"), "新分支回答"),
    ]
}

#[test]
fn claude_completed_old_branch_is_excluded_and_last_prompt_can_rewind() {
    let mut records = branch();
    let (_, messages) = project("claude", &rows(records.clone()));
    assert_eq!(
        visible(&messages),
        vec![
            ("user", "共同输入"),
            ("assistant", "共同回答"),
            ("user", "新分支输入"),
            ("assistant", "新分支回答")
        ]
    );
    records.push(json!({"type": "last-prompt", "leafUuid": "a0"}));
    let (meta, messages) = project("claude", &rows(records));
    assert_eq!(meta["_timeline_tip"], "a0");
    assert_eq!(
        visible(&messages),
        vec![("user", "共同输入"), ("assistant", "共同回答")]
    );
}

#[test]
fn claude_unanswered_sibling_survives_as_interrupted_but_completed_sibling_does_not() {
    let records = rows(vec![
        message("user", "u0", Value::Null, "共同输入"),
        message("assistant", "a0", json!("u0"), "共同回答"),
        message("user", "cancelled", json!("a0"), "快速 Esc 输入"),
        json!({"type": "attachment", "uuid": "reminder", "parentUuid": "cancelled",
               "attachment": {"type": "total_tokens_reminder"}}),
        message("user", "replacement", json!("a0"), "替代输入"),
        message("assistant", "answer", json!("replacement"), "新回答"),
    ]);
    let (_, messages) = project("claude", &records);
    let interrupted = messages
        .iter()
        .find(|message| message["text"] == "快速 Esc 输入")
        .unwrap();
    assert_eq!(interrupted["interrupted"], true);
    // Python d16c5e1: an abandoned input is still the structural start of
    // its own turn (`starts_turn = not is_interrupt`).
    assert_eq!(interrupted["turn_id"], "cancelled");
    assert_eq!(
        messages
            .iter()
            .filter(|m| m["role"] == "status")
            .map(|m| m["state"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["working", "aborted", "working"]
    );
    assert_eq!(visible(&messages).len(), 5);
}

#[test]
fn claude_declared_tip_and_confirmed_stale_boundary_are_pure_parse_options() {
    let records = rows(vec![
        message("user", "u0", Value::Null, "root"),
        message("assistant", "a0", json!("u0"), "answer"),
        message("user", "cancelled", json!("a0"), "old input"),
        message("user", "replacement", json!("a0"), "replacement"),
    ]);
    let (_, events, error) = parse_with_options(
        "claude",
        Path::new("synthetic.jsonl"),
        &records,
        None,
        "",
        ParseOptions {
            abandoned_after: records[2].1,
            ..Default::default()
        },
    );
    assert!(error.is_none());
    assert!(
        !events
            .iter()
            .any(|event| event.message["text"] == "old input")
    );
    let (meta, events, error) = parse_with_options(
        "claude",
        Path::new("synthetic.jsonl"),
        &records,
        None,
        "",
        ParseOptions {
            declared_tip: Some("a0"),
            abandoned_after: records[3].1,
            ..Default::default()
        },
    );
    assert!(error.is_none());
    assert_eq!(meta["_timeline_tip"], "a0");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.message["role"] != "status")
            .count(),
        2
    );
}

#[test]
fn claude_both_compact_shapes_reconnect_the_selected_old_branch_and_hide_protocol() {
    for boundary in [
        json!({"type": "system", "uuid": "compact", "parentUuid": null, "subtype": "compact_boundary"}),
        json!({"type": "system", "uuid": "compact", "parentUuid": null, "compactMetadata": {"trigger": "manual"}}),
    ] {
        let mut records = branch();
        records.extend([
            json!({"type": "last-prompt", "leafUuid": "a0"}), boundary,
            json!({"type": "user", "uuid": "summary", "parentUuid": "compact", "isCompactSummary": true,
                   "message": {"content": "internal summary"}}),
            message("user", "next-u", json!("summary"), "压缩后输入"),
            message("assistant", "next-a", json!("next-u"), "压缩后回答"),
        ]);
        let (_, messages) = project("claude", &rows(records));
        assert_eq!(
            visible(&messages),
            vec![
                ("user", "共同输入"),
                ("assistant", "共同回答"),
                ("event", "已压缩"),
                ("user", "压缩后输入"),
                ("assistant", "压缩后回答")
            ]
        );
        let compact = messages
            .iter()
            .find(|m| m["event_kind"] == "compact")
            .unwrap();
        assert_eq!(compact["counted"], false);
        assert!(messages.iter().any(|m| m["state"] == "idle"));
    }
}

#[test]
fn claude_compact_replayed_title_and_protocol_do_not_duplicate_bubbles() {
    let records = rows(vec![
        json!({"type": "custom-title", "customTitle": "同一标题", "sessionId": "synthetic"}),
        json!({"type": "user", "message": {"content": "/compact"}}),
        json!({"type": "custom-title", "customTitle": "同一标题", "sessionId": "synthetic"}),
        json!({"type": "system", "subtype": "compact_boundary", "uuid": "compact"}),
        json!({"type": "user", "message": {"content": "<command-name>/compact</command-name>"}}),
        json!({"type": "user", "message": {"content": "<local-command-stdout>Compacted</local-command-stdout>"}}),
        json!({"type": "attachment", "attachment": {"type": "compact_file_reference"}}),
        json!({"type": "user", "message": {"content": "讨论句中的 <command-name> 标签"}}),
    ]);
    let (_, messages) = project("claude", &records);
    assert_eq!(
        visible(&messages),
        vec![
            ("command", "/rename 同一标题"),
            ("event", "已压缩"),
            ("user", "讨论句中的 <command-name> 标签")
        ]
    );
    assert!(
        messages
            .iter()
            .find(|m| m["role"] == "command")
            .unwrap()
            .get("turn_id")
            .is_none()
    );
}

#[test]
fn claude_local_shell_uses_correlated_command_and_output_not_two_user_inputs() {
    let records = rows(vec![
        json!({"type": "system", "uuid": "outside-prefix", "parentUuid": null}),
        message(
            "user",
            "shell-in",
            json!("outside-prefix"),
            "<bash-input> printf '&lt;ok&gt; &amp; &#x4e2d;'</bash-input>",
        ),
        message(
            "user",
            "shell-out",
            json!("shell-in"),
            "<bash-stdout>&lt;ok&gt;\n</bash-stdout><bash-stderr>warning</bash-stderr>",
        ),
        message("assistant", "answer", json!("shell-out"), "完成"),
    ]);
    let (_, messages) = project("claude", &records);
    assert_eq!(
        visible(&messages),
        vec![
            ("command", "! printf '<ok> & 中'"),
            ("tool_result", "<ok>\n\nstderr:\nwarning"),
            ("assistant", "完成")
        ]
    );
    let output = messages
        .iter()
        .find(|m| m["role"] == "tool_result")
        .unwrap();
    assert_eq!(output["call_id"], "local-shell:shell-in");
    assert_eq!(output["counted"], false);
    assert_eq!(output["has_stderr"], true);
    assert_eq!(messages.iter().filter(|m| m["role"] == "status").count(), 1);
}

#[test]
fn claude_local_help_notifications_recaps_and_duration_have_semantic_fields() {
    let records = rows(vec![
        json!({"type": "system", "subtype": "local_command", "uuid": "help", "content": "<command-name>/help</command-name><command-args></command-args>"}),
        json!({"type": "system", "subtype": "local_command", "content": "<local-command-stdout>dismissed</local-command-stdout>"}),
        json!({"type": "system", "subtype": "local_command", "content": "<command-name>/rename</command-name><command-args>已记录名称</command-args>"}),
        json!({"type": "user", "message": {"content": "<task-notification>\n<status>completed</status><summary>Monitor &quot;人工任务&quot; stream ended</summary><result>第一行\n&lt;第二行&gt;</result></task-notification>"}}),
        json!({"type": "system", "subtype": "away_summary", "content": "任务收口"}),
        json!({"type": "system", "subtype": "turn_duration", "durationMs": 125000}),
    ]);
    let (_, messages) = project("claude", &records);
    assert_eq!(
        visible(&messages),
        vec![
            ("command", "/help"),
            ("event", "监控结束 · 人工任务"),
            ("event", "任务收口"),
            ("event", "")
        ]
    );
    let notification = messages.iter().find(|m| m["event_kind"] == "task").unwrap();
    assert_eq!(notification["event_status"], "completed");
    assert_eq!(notification["details"], "第一行\n<第二行>");
    assert_eq!(notification["counted"], false);
    assert_eq!(messages.iter().filter(|m| m["role"] == "status").count(), 1);
}

#[test]
fn claude_human_queue_attachment_is_visible_but_task_notification_attachment_is_not() {
    let records = rows(vec![
        json!({"type": "queue-operation", "operation": "enqueue", "content": "排队输入"}),
        json!({"type": "queue-operation", "operation": "remove", "content": "排队输入"}),
        json!({"type": "queue-operation", "operation": "dequeue"}),
        json!({"type": "attachment", "uuid": "queued", "attachment": {"type": "queued_command", "commandMode": "prompt", "origin": {"kind": "human"}, "prompt": "排队输入"}}),
        json!({"type": "attachment", "attachment": {"type": "queued_command", "commandMode": "task-notification", "prompt": "不可伪装成人类输入"}}),
    ]);
    let (_, messages) = project("claude", &records);
    assert_eq!(messages.iter().filter(|m| m["role"] == "user").count(), 1);
    assert_eq!(messages.last().unwrap()["turn_id"], "queued");
    assert!(
        messages
            .iter()
            .filter(|m| m["role"] == "queue_operation")
            .all(|m| m["counted"] == false && m["silent"] == true && m.get("turn_id").is_none())
    );
}

#[test]
fn claude_inline_sidechain_does_not_change_main_leaf_and_explicit_agent_uses_its_own_tree() {
    let mut records = vec![
        message("user", "main-u", Value::Null, "主输入"),
        message("assistant", "main-a", json!("main-u"), "主回答"),
    ];
    let mut agent = message("user", "agent-u", Value::Null, "子输入");
    agent["isSidechain"] = json!(true);
    agent["agentId"] = json!("agent-123456789");
    records.push(agent.clone());
    let (meta, messages) = project("claude", &rows(records));
    assert_eq!(meta["_timeline_tip"], "main-a");
    assert!(
        messages
            .iter()
            .any(|m| m["role"] == "user·subagent" && m["name"] == "agent-12")
    );
    let mut answer = message("assistant", "agent-a", json!("agent-u"), "子回答");
    answer["isSidechain"] = json!(true);
    let records = rows(vec![agent, answer]);
    let info = json!({"description": "审计子任务", "agentType": "review"});
    let (meta, events, error) = parse_agent(
        "claude",
        Path::new("agent.jsonl"),
        &records,
        Some(&info),
        "",
        "agent-123456789",
    );
    assert!(error.is_none(), "{error:?}");
    assert_eq!(meta["sid"], "agent-123456789");
    assert_eq!(meta["_agent_title"], "审计子任务");
    assert_eq!(meta["_agent_type"], "review");
    assert!(events.iter().all(|event| event.message["role"] != "status"));
    assert!(
        events
            .iter()
            .all(|event| event.message["turn_id"] == "agent-u")
    );
}

#[test]
fn codex_metadata_disambiguates_subagent_id_and_carries_history_topology() {
    let records = rows(vec![json!({"type": "session_meta", "payload": {
        "session_id": "parent", "id": "child", "thread_source": "subagent", "forked_from_id": "parent",
        "history_base": {"thread_id": "parent", "end_byte_offset": 120},
        "source": {"subagent": {"thread_spawn": {"parent_thread_id": "parent", "agent_path": "review/parser", "agent_role": "reviewer"}}},
    }})]);
    let (meta, _) = project("codex", &records);
    assert_eq!(meta["sid"], "child");
    assert_eq!(meta["_is_subagent"], true);
    assert_eq!(meta["_parent_thread_id"], "parent");
    assert_eq!(meta["_agent_title"], "review/parser");
    assert_eq!(meta["_agent_type"], "reviewer");
    assert_eq!(meta["history_base"]["end_byte_offset"], 120);
    let (meta, _) = project(
        "codex",
        &rows(vec![
            json!({"type": "session_meta", "payload": {"id": "fallback", "session_id": "authoritative"}}),
        ]),
    );
    assert_eq!(meta["sid"], "authoritative");
}

/// Python `_read_file` (`and not session_meta`): every session_meta after
/// the first is ignored. Old-style forks and subagent rollouts copy their
/// ancestors' metas into the child file, so the copies are counted, not fatal,
/// and the KEEP `history_base` check applies to the first meta only.
#[test]
fn codex_later_session_meta_records_are_skipped_and_counted() {
    let native = json!({"type": "session_meta", "payload": {"id": "same"}});
    let (meta, messages) = project("codex", &rows(vec![native.clone(), native]));
    assert!(messages.is_empty());
    assert_eq!(warnings(&meta), vec!["跳过重复的Codex session_meta ×1"]);
    let records = rows(vec![
        json!({"type": "session_meta", "payload": {"id": "child", "session_id": "child", "forked_from_id": "parent",
               "history_base": null, "timestamp": "2026-07-01T09:00:00Z", "cwd": "/synthetic/child"}}),
        json!({"type": "session_meta", "payload": {"id": "parent", "session_id": "parent", "forked_from_id": "grandparent",
               "history_base": null, "timestamp": "2026-06-30T09:00:00Z", "cwd": "/synthetic/parent"}}),
        json!({"type": "session_meta", "payload": {"id": "grandparent", "session_id": "grandparent",
               "history_base": "broken-but-not-ours", "cwd": "/synthetic/grandparent"}}),
        json!({"type": "session_meta", "payload": {"id": "root", "session_id": "root", "cwd": "/synthetic/root"}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "user", "turn_id": "t1", "content": "copied question"}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "assistant", "phase": "final_answer", "turn_id": "t1", "content": "copied answer"}}),
        json!({"type": "token_usage_record", "payload": {"total_tokens": 12}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "user", "turn_id": "t2", "content": "own question"}}),
    ]);
    let (meta, messages) = project("codex", &records);
    assert_eq!(
        visible(&messages),
        vec![
            ("user", "copied question"),
            ("assistant", "copied answer"),
            ("user", "own question"),
        ]
    );
    assert_eq!(meta["sid"], "child");
    assert_eq!(meta["cwd"], "/synthetic/child");
    assert_eq!(meta["forked_from_id"], "parent");
    assert_eq!(meta["history_base"], Value::Null);
    assert_eq!(meta["_is_subagent"], false);
    assert_eq!(
        warnings(&meta),
        vec![
            "跳过重复的Codex session_meta ×3",
            "跳过未知的Codex 记录类型：token_usage_record ×1",
        ]
    );
    let (native_id, declared) = crate::sessions::scope::native_identity("codex", &records);
    assert_eq!(native_id.unwrap(), "child");
    assert_eq!(declared, vec!["child"]);
}

#[test]
fn codex_compaction_and_rebuilt_context_preserve_only_visible_body() {
    let records = rows(vec![
        json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": "压缩前正文"}}),
        json!({"type": "compacted", "ordinal": 2, "payload": {"replacement_history": []}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "developer", "content": "<skills_instructions>hidden</skills_instructions>"}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": "# AGENTS.md instructions\n<INSTRUCTIONS>hidden</INSTRUCTIONS>"}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": "goal hidden", "internal_chat_message_metadata_passthrough": {"content_item_kinds": ["goal.internal_context"]}}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": "解释句中的 <environment_context> 标签"}}),
    ]);
    let (_, messages) = project("codex", &records);
    assert_eq!(
        visible(&messages),
        vec![
            ("user", "压缩前正文"),
            ("event", "已压缩"),
            ("user", "解释句中的 <environment_context> 标签")
        ]
    );
    assert_eq!(messages[1]["counted"], false);
    assert_eq!(messages[1]["event_id"], "compact:2");
}

#[test]
fn codex_repeated_abort_prefix_is_removed_without_hiding_later_human_text() {
    let marker = "<turn_aborted>native interruption</turn_aborted>";
    let records = rows(vec![
        json!({"type": "response_item", "payload": {"type": "message", "role": "developer", "content": marker}}),
        json!({"type": "event_msg", "payload": {"type": "turn_aborted", "turn_id": "old", "reason": "interrupted"}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": format!("{marker}\n{marker}\n保留用户正文")}}),
    ]);
    let (meta, messages) = project("codex", &records);
    assert_eq!(visible(&messages), vec![("user", "保留用户正文")]);
    assert_eq!(meta["title"], "保留用户正文");
    assert_eq!(messages[0]["state"], "aborted");
}

#[test]
fn codex_abort_only_marks_the_last_progress_for_matching_turn() {
    let records = rows(vec![
        json!({"type": "response_item", "payload": {"type": "message", "role": "assistant", "turn_id": "active", "phase": "commentary", "content": "earlier progress"}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "assistant", "turn_id": "active", "phase": "commentary", "content": "last progress"}}),
        json!({"type": "event_msg", "payload": {"type": "turn_aborted", "turn_id": "active", "reason": "stopped"}}),
    ]);
    let (_, messages) = project("codex", &records);
    assert!(messages[0].get("interrupted").is_none());
    assert_eq!(messages[1]["interrupted"], true);
    assert_eq!(messages[1]["interrupt_reason"], "stopped");
}

#[test]
fn unknown_media_is_nonfatal_but_python_unreadable_scalar_content_still_fails() {
    let (meta, _events, error) = parse(
        "claude",
        Path::new("synthetic.jsonl"),
        &rows(vec![
            json!({"type": "user", "message": {"content": [{"type": "image", "source": {"data": "synthetic"}}]}}),
        ]),
        None,
        "",
    );
    assert!(error.is_none(), "{error:?}");
    assert!(meta.is_object());
    for records in [
        vec![json!({"type": "user", "message": {"content": 42}})],
        vec![json!({"type": "user", "message": {"content": [{"type": "text", "text": 42}]}})],
    ] {
        let (meta, events, error) = parse(
            "claude",
            Path::new("synthetic.jsonl"),
            &rows(records),
            None,
            "",
        );
        assert!(error.is_some());
        assert!(events.is_empty());
        assert!(meta.is_object());
    }
}

/// Only the conversation itself: no status, queue, tool or event rows.
fn conversation(messages: &[Value]) -> Vec<(&str, &str)> {
    visible(messages)
        .into_iter()
        .filter(|(role, _)| matches!(*role, "user" | "assistant"))
        .collect()
}

fn warnings(meta: &Value) -> Vec<String> {
    meta["migration_warnings"]
        .as_array()
        .expect("supported parse reports its warnings array")
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect()
}

fn attachment(id: &str, parent: &str, kind: &str) -> Value {
    json!({"type": "attachment", "uuid": id, "parentUuid": parent, "sessionId": "synthetic",
           "timestamp": "2026-09-11T10:00:00.123Z", "isSidechain": false,
           "attachment": {"type": kind, "synthetic": true}})
}

/// Claude Code 2.1.x main-session shape (observed 2026-09-12): a `-p` one-shot
/// starts with queue operations, then the user record, a chain of attachment
/// records linked by uuid/parentUuid, an `atis-latch`, and the assistant
/// records whose parentUuid is the LAST attachment. Unknown kinds are skipped
/// like the reference adapter; the session stays supported with counted
/// warnings and the lineage still resolves through the attachment chain.
#[test]
fn claude_current_cli_record_and_attachment_kinds_are_skipped_with_counted_warnings() {
    let sid = "synthetic";
    let mut records = vec![
        json!({"type": "queue-operation", "operation": "enqueue", "content": "one-shot prompt", "sessionId": sid, "timestamp": "2026-09-11T10:00:00.000Z"}),
        json!({"type": "queue-operation", "operation": "popAll", "sessionId": sid, "timestamp": "2026-09-11T10:00:00.001Z"}),
        json!({"type": "mode", "mode": "default", "sessionId": sid}),
        json!({"type": "permission-mode", "permissionMode": "plan", "sessionId": sid}),
        json!({"type": "bridge-session", "bridgeSessionId": "synthetic-bridge", "lastSequenceNum": 3,
               "ownerAccountUuid": "synthetic-account", "ownerOrganizationUuid": "synthetic-org", "sessionId": sid}),
        json!({"type": "agent-name", "agentName": "synthetic-agent", "sessionId": sid}),
        message("user", "u0", Value::Null, "one-shot prompt"),
    ];
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
    let mut parent = "u0".to_owned();
    for (index, kind) in kinds.iter().enumerate() {
        let id = format!("at{index}");
        records.push(attachment(&id, &parent, kind));
        parent = id;
    }
    // A second environment attachment: counts aggregate per kind.
    records.push(attachment("at-env-2", &parent, "environment"));
    parent = "at-env-2".to_owned();
    records.push(json!({"type": "atis-latch", "atis": {"latched": true}, "sessionId": sid}));
    records.push(message("assistant", "a0", json!(parent), "one-shot answer"));
    records.push(json!({"type": "file-history-delta", "backup": {"synthetic": true}, "messageId": "a0",
                        "snapshotMessageId": "u0", "timestamp": "2026-09-11T10:00:01.000Z", "trackingPath": "synthetic.txt"}));
    records.push(
        json!({"type": "cost-state", "modelUsage": {"synthetic-model": {"inputTokens": 1}},
                        "totalCostUSD": 0.0001, "sessionId": sid}),
    );
    for _ in 0..3 {
        records.push(json!({"type": "atis-latch", "atis": {"latched": false}, "sessionId": sid}));
    }
    // Resumed turn: the user hangs off the assistant, attachments again sit
    // between the user and its reply.
    records.push(message("user", "u1", json!("a0"), "second prompt"));
    records.push(attachment("at-r0", "u1", "environment"));
    records.push(attachment("at-r1", "at-r0", "hook_success"));
    records.push(message("assistant", "a1", json!("at-r1"), "second answer"));
    records.push(json!({"type": "last-prompt", "leafUuid": "a1"}));
    let (meta, messages) = project("claude", &rows(records));
    assert_eq!(
        conversation(&messages),
        vec![
            ("user", "one-shot prompt"),
            ("assistant", "one-shot answer"),
            ("user", "second prompt"),
            ("assistant", "second answer")
        ]
    );
    let queued = messages
        .iter()
        .filter(|message| message["role"] == "queue_operation")
        .map(|message| message["operation"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(queued, vec!["enqueue", "popAll"]);
    // The attachment chain is the assistant's ancestry: turn identity is the
    // user's uuid, the abandoned/interrupted logic sees a responded input.
    let answers = messages
        .iter()
        .filter(|message| message["role"] == "assistant")
        .collect::<Vec<_>>();
    assert_eq!(answers[0]["turn_id"], "u0");
    assert_eq!(answers[1]["turn_id"], "u1");
    assert!(
        messages
            .iter()
            .all(|message| message.get("interrupted").is_none())
    );
    assert_eq!(meta["_timeline_tip"], "a1");
    assert_eq!(meta["title"], "one-shot prompt");
    let warnings = warnings(&meta);
    // One entry per kind in first-seen order, with a count; ≤ 32 kinds.
    assert_eq!(warnings[0], "跳过未知的Claude 记录类型：mode ×1");
    assert_eq!(warnings[1], "跳过未知的Claude 记录类型：permission-mode ×1");
    assert_eq!(warnings[2], "跳过未知的Claude 记录类型：bridge-session ×1");
    assert_eq!(warnings[3], "跳过未知的Claude 记录类型：agent-name ×1");
    assert_eq!(
        warnings[4],
        "跳过未知的Claude attachment 类型：hook_success ×2"
    );
    assert_eq!(
        warnings[5],
        "跳过未知的Claude attachment 类型：environment ×3"
    );
    assert!(warnings.contains(&"跳过未知的Claude attachment 类型：task_status ×1".to_owned()));
    assert!(warnings.contains(&"跳过未知的Claude 记录类型：atis-latch ×4".to_owned()));
    assert!(warnings.contains(&"跳过未知的Claude 记录类型：file-history-delta ×1".to_owned()));
    assert!(warnings.contains(&"跳过未知的Claude 记录类型：cost-state ×1".to_owned()));
    assert_eq!(warnings.len(), 4 + kinds.len() + 3);
    assert!(warnings.len() <= MAX_SKIPPED_KINDS);
    assert!(
        warnings
            .iter()
            .all(|warning| warning.starts_with("跳过未知的"))
    );
}

#[test]
fn claude_queued_prompt_still_renders_and_sidechain_attachments_stay_silent() {
    let mut queued = attachment("q0", "u0", "queued_command");
    queued["attachment"] = json!({"type": "queued_command", "commandMode": "prompt",
                                  "origin": {"kind": "human"}, "prompt": "queued human prompt"});
    let mut side = attachment("side", "u0", "environment");
    side["isSidechain"] = json!(true);
    side["agentId"] = json!("side-agent");
    let records = rows(vec![
        message("user", "u0", Value::Null, "prompt"),
        queued,
        side,
        attachment("known", "q0", "total_tokens_reminder"),
        message("assistant", "a0", json!("known"), "answer"),
    ]);
    let (meta, messages) = project("claude", &records);
    assert_eq!(
        visible(&messages),
        vec![
            ("user", "prompt"),
            ("user", "queued human prompt"),
            ("assistant", "answer")
        ]
    );
    assert_eq!(messages.last().unwrap()["turn_id"], "q0");
    // Known kinds never warn; the sidechain copy of an unknown kind is counted
    // once like any other skipped record.
    assert_eq!(
        warnings(&meta),
        vec!["跳过未知的Claude attachment 类型：environment ×1"]
    );
}

#[test]
fn codex_current_cli_record_event_and_item_kinds_are_skipped_with_warnings() {
    let records = rows(vec![
        json!({"type": "session_meta", "payload": {"id": "synthetic", "cwd": "/synthetic"}}),
        json!({"type": "turn_context", "payload": {"model": "synthetic-model"}}),
        json!({"type": "event_msg", "payload": {"type": "task_started", "turn_id": "t1"}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "user", "turn_id": "t1", "content": "question"}}),
        json!({"type": "token_usage_record", "payload": {"total_tokens": 12}}),
        json!({"type": "event_msg", "payload": {"type": "item_completed", "turn_id": "t1", "item": {"type": "reasoning"}}}),
        json!({"type": "response_item", "payload": {"type": "agent_message", "turn_id": "t1", "content": "duplicate of the message below"}}),
        json!({"type": "event_msg", "payload": {"type": "item_completed", "turn_id": "t1", "item": {"type": "agent_message"}}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "assistant", "phase": "final_answer", "turn_id": "t1", "content": "answer"}}),
        json!({"type": "inter_agent_communication_metadata", "payload": {"synthetic": true}}),
        json!({"type": "event_msg", "payload": {"type": "task_complete", "turn_id": "t1", "duration_ms": 5}}),
        json!({"type": "token_usage_record", "payload": {"total_tokens": 20}}),
    ]);
    let (meta, messages) = project("codex", &records);
    assert_eq!(
        visible(&messages),
        vec![("user", "question"), ("assistant", "answer")]
    );
    assert_eq!(messages.last().unwrap()["state"], "idle");
    assert_eq!(messages.last().unwrap()["turn_id"], "t1");
    assert_eq!(meta["model"], "synthetic-model");
    assert_eq!(
        warnings(&meta),
        vec![
            "跳过未知的Codex 记录类型：token_usage_record ×2",
            "跳过未知的Codex event_msg：item_completed ×2",
            "跳过未知的Codex response_item：agent_message ×1",
            "跳过未知的Codex 记录类型：inter_agent_communication_metadata ×1",
        ]
    );
    // Hard failures the reference adapter also cannot tolerate are unchanged:
    // the file's own (first) meta with a broken history_base.
    let mut broken = records
        .iter()
        .map(|(record, _)| record.clone())
        .collect::<Vec<_>>();
    broken[0] =
        json!({"type": "session_meta", "payload": {"id": "synthetic", "history_base": "bad"}});
    let (meta, events, error) = parse(
        "codex",
        Path::new("synthetic.jsonl"),
        &rows(broken),
        None,
        "",
    );
    assert!(error.is_some());
    assert!(events.is_empty());
    assert!(meta.get("migration_warnings").is_none());
}

/// Python `_EXIT_CODE`: `"exit_code": N` or `exit[ed][ with][ code| status] N`
/// with exactly one space before the number. Grok's `exit: 1` output header
/// is not a code (`exit_code: null`, `error: false`), while a Claude Bash
/// result saying `exited with code 2` is an error with that code.
#[test]
fn output_exit_code_follows_the_python_regex_so_grok_exit_headers_are_not_errors() {
    let cases: [(&str, Option<i64>); 12] = [
        ("exit: 1\nstdout: nothing", None),
        ("exit:1", None),
        ("exit  1", None),
        ("exit 1", Some(1)),
        ("Exited with code 2\n", Some(2)),
        ("process exit status -1", Some(-1)),
        ("EXIT CODE 7", Some(7)),
        ("reexit 3", None),
        ("{\"exit_code\": 4, \"output\": \"x\"}", Some(4)),
        ("\"exit_code\":\n  5", Some(5)),
        ("nothing here", None),
        (&format!("{}exit 9", "x".repeat(400)), None),
    ];
    for (text, want) in cases {
        assert_eq!(output_exit_code(text), want, "{text:?}");
    }
    let records = rows(vec![
        json!({"type": "user", "content": "run it", "prompt_index": 1}),
        json!({"type": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "name": "run_terminal_command", "arguments": "{\"command\":\"false\"}"},
            {"id": "c2", "name": "run_terminal_command", "arguments": "{\"command\":\"true\"}"}]}),
        json!({"type": "tool_result", "tool_call_id": "c1", "content": "exit: 1\nstdout:\n"}),
        json!({"type": "tool_result", "tool_call_id": "c2", "content": "exit: 0\nstdout: ok\n"}),
    ]);
    let (_, messages) = project("grok", &records);
    let results: Vec<_> = messages
        .iter()
        .filter(|m| m["role"] == "tool_result")
        .collect();
    assert_eq!(results.len(), 2);
    for result in results {
        assert_eq!(result["error"], false, "{result}");
        assert!(result.get("exit_code").is_none(), "{result}");
    }
}

#[test]
fn grok_unknown_record_kinds_are_skipped_with_warnings() {
    let records = rows(vec![
        json!({"type": "user", "content": "question", "prompt_index": 1}),
        json!({"type": "usage", "prompt_tokens": 3}),
        json!({"type": "assistant", "content": "answer"}),
        json!({"type": "usage", "completion_tokens": 4}),
        json!({"type": "checkpoint", "id": "x"}),
        json!({}),
    ]);
    let (meta, messages) = project("grok", &records);
    assert_eq!(
        visible(&messages),
        vec![("user", "question"), ("assistant", "answer")]
    );
    assert_eq!(messages[1]["turn_id"], "prompt:1");
    assert_eq!(
        warnings(&meta),
        vec![
            "跳过未知的Grok 记录类型：usage ×2",
            "跳过未知的Grok 记录类型：checkpoint ×1",
            "跳过未知的Grok 记录类型：(空) ×1",
        ]
    );
}

#[test]
fn unknown_content_blocks_are_skipped_in_all_sources_but_invalid_media_still_fails() {
    // Message content: an unknown non-image block between two text blocks
    // contributes no line (Python joins only the text parts).
    let claude = rows(vec![
        json!({"type": "user", "uuid": "u0", "message": {"content": [
        {"type": "text", "text": "before"}, {"type": "audio", "data": "x"},
        {"type": "text", "text": "after"}, 42, null, {"text": "untyped"}]}}),
    ]);
    let (meta, messages) = project("claude", &claude);
    assert_eq!(
        visible(&messages),
        vec![("user", "before"), ("user", "after")]
    );
    assert_eq!(
        warnings(&meta),
        vec![
            "跳过未知的内容块类型：(非对象元素) ×2",
            "跳过未知的内容块类型：audio ×1",
            "跳过未知的内容块类型：(空) ×1",
        ]
    );
    let codex = rows(vec![
        json!({"type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [
            {"type": "output_text", "text": "before"}, {"type": "refusal", "refusal": "x"},
            {"type": "output_text", "text": "after"}]}}),
        json!({"type": "response_item", "payload": {"type": "reasoning", "summary": [
            {"type": "summary_text", "text": "thought"}, {"type": "encrypted", "data": "x"}]}}),
    ]);
    let (meta, messages) = project("codex", &codex);
    assert_eq!(
        conversation(&messages),
        vec![("assistant", "before\nafter")]
    );
    assert_eq!(messages[1]["role"], "thinking");
    assert_eq!(messages[1]["text"], "thought");
    assert_eq!(
        warnings(&meta),
        vec![
            "跳过未知的内容块类型：refusal ×1",
            "跳过未知的内容块类型：encrypted ×1"
        ]
    );
    let grok = rows(vec![
        json!({"type": "assistant", "content": [{"type": "text", "text": "answer"}, {"type": "citation", "url": "x"}],
               "tool_calls": [{"id": "c", "name": "Read", "arguments": {"file_path": "x"}}]}),
        json!({"type": "tool_result", "tool_call_id": "c", "content": [{"type": "text", "text": "out"}, {"type": "resource", "uri": "x"}]}),
    ]);
    let (meta, messages) = project("grok", &grok);
    assert_eq!(conversation(&messages), vec![("assistant", "answer")]);
    assert_eq!(messages.last().unwrap()["role"], "tool_result");
    assert_eq!(messages.last().unwrap()["text"], "out");
    assert_eq!(warnings(&meta), vec!["跳过未知的内容块类型：resource ×1"]);
    // External image-shaped blocks follow the same non-fatal projection path.
    for source in ["claude", "codex", "grok"] {
        let bad = json!({"type": "image_url", "image_url": "https://example.invalid/x.png"});
        let records = rows(vec![match source {
            "claude" => json!({"type": "user", "uuid": "u0", "message": {"content": [bad]}}),
            "codex" => {
                json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [bad]}})
            }
            _ => json!({"type": "user", "content": [bad]}),
        }]);
        let (meta, _events, error) =
            parse(source, Path::new("synthetic.jsonl"), &records, None, "");
        assert!(error.is_none(), "{source}: {error:?}");
        assert!(meta.is_object());
    }
}

#[test]
fn skipped_kind_warnings_are_bounded_to_32_kinds_plus_one_overflow_line() {
    let mut records = vec![message("user", "u0", Value::Null, "prompt")];
    for index in 0..40 {
        records.push(json!({"type": format!("future-kind-{index}"), "sessionId": "synthetic"}));
        records.push(json!({"type": format!("future-kind-{index}"), "sessionId": "synthetic"}));
    }
    records.push(json!({"type": "x".repeat(500), "sessionId": "synthetic"}));
    let (meta, messages) = project("claude", &rows(records));
    assert_eq!(visible(&messages), vec![("user", "prompt")]);
    let listed = warnings(&meta);
    assert_eq!(listed.len(), MAX_SKIPPED_KINDS + 1);
    assert_eq!(listed[0], "跳过未知的Claude 记录类型：future-kind-0 ×2");
    assert_eq!(listed[31], "跳过未知的Claude 记录类型：future-kind-31 ×2");
    // 8 kinds × 2 records + the oversized kind = 17 records beyond the bound.
    assert_eq!(
        listed[32],
        "另有 17 条其他未知类型已跳过（超过 32 种，未逐一列出）"
    );
    assert!(listed.iter().all(|warning| warning.chars().count() < 120));
    // A hostile kind name is clipped when it is among the listed ones.
    let (meta, _) = project(
        "claude",
        &rows(vec![
            message("user", "u0", Value::Null, "prompt"),
            json!({"type": "y".repeat(500), "sessionId": "synthetic"}),
        ]),
    );
    let clipped = warnings(&meta);
    assert_eq!(clipped.len(), 1);
    assert!(clipped[0].chars().count() < 100, "{}", clipped[0]);
    assert!(clipped[0].ends_with("… ×1"));
}

#[test]
fn claude_pin_target_resolves_to_the_parent_on_the_active_lineage_only() {
    use super::claude::{PinTargetError, resolve_pin_target};
    let records = rows(vec![
        message("user", "u0", Value::Null, "root"),
        message("assistant", "a0", json!("u0"), "answer"),
        message("user", "old-u", json!("a0"), "discarded"),
        message("assistant", "old-a", json!("old-u"), "discarded answer"),
        message("user", "u1", json!("a0"), "kept"),
        message("assistant", "a1", json!("u1"), "kept answer"),
    ]);
    assert_eq!(resolve_pin_target(&records, "u1"), Ok("a0".into()));
    assert_eq!(resolve_pin_target(&records, "a1"), Ok("u1".into()));
    assert_eq!(
        resolve_pin_target(&records, "u0"),
        Err(PinTargetError::NoParent)
    );
    assert_eq!(
        resolve_pin_target(&records, "old-u"),
        Err(PinTargetError::Inactive)
    );
    assert_eq!(
        resolve_pin_target(&records, "missing"),
        Err(PinTargetError::Unknown)
    );
    assert!(matches!(
        resolve_pin_target(
            &rows(vec![json!({"type":"summary","summary":"only"})]),
            "u0"
        ),
        Err(PinTargetError::Unsupported(_))
    ));
}

#[test]
fn claude_pin_is_applied_or_retired_with_an_explicit_reason() {
    use super::claude::{PinRetirement, apply_pin};
    let base = vec![
        message("user", "u0", Value::Null, "root"),
        message("assistant", "a0", json!("u0"), "answer"),
        message("user", "u1", json!("a0"), "second"),
        message("assistant", "a1", json!("u1"), "second answer"),
    ];
    let records = rows(base.clone());
    let boundary = records[3].1;
    // Nothing after the boundary: the pin is the declared tip and the
    // boundary hides earlier abandoned inputs, exactly the pure option pair.
    let outcome = apply_pin(&records, "a0", boundary);
    assert_eq!(outcome.declared_tip.as_deref(), Some("a0"));
    assert_eq!(outcome.abandoned_after, boundary);
    assert!(outcome.retired.is_none());
    let (meta, events, error) = parse_with_options(
        "claude",
        Path::new("synthetic.jsonl"),
        &records,
        None,
        "",
        ParseOptions {
            declared_tip: outcome.declared_tip.as_deref(),
            abandoned_after: outcome.abandoned_after,
            ..Default::default()
        },
    );
    assert!(error.is_none());
    assert_eq!(meta["_timeline_tip"], "a0");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.message["role"] != "status")
            .map(|event| event.message["text"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        vec!["root", "answer"]
    );
    let after = |extra: Vec<Value>| rows(base.iter().cloned().chain(extra).collect());
    let cases = [
        (
            vec![json!({"type": "last-prompt", "leafUuid": "a0"})],
            PinRetirement::NativeConfirmed,
        ),
        (
            vec![message(
                "user",
                "u2",
                json!("a0"),
                "sent after a real rewind",
            )],
            PinRetirement::NativeContinued,
        ),
        (
            vec![message("user", "u2", json!("a1"), "sent without rewinding")],
            PinRetirement::NativeAdvanced,
        ),
        (
            vec![message("user", "u2", json!("u0"), "rewound further back")],
            PinRetirement::NativeDiverged,
        ),
    ];
    for (extra, expected) in cases {
        let outcome = apply_pin(&after(extra), "a0", boundary);
        assert_eq!(outcome.retired, Some(expected));
        assert!(outcome.declared_tip.is_none());
        assert_eq!(outcome.abandoned_after, boundary);
    }
    let missing = apply_pin(&records, "never-written", boundary);
    assert_eq!(missing.retired, Some(PinRetirement::TipMissing));
    assert_eq!(missing.abandoned_after, 0);
    assert_ne!(
        PinRetirement::NativeAdvanced.code(),
        PinRetirement::NativeDiverged.code()
    );
    assert!(
        PinRetirement::NativeAdvanced
            .message()
            .contains("CLI 未回滚")
    );
}

// ---------------------------------------------------------------------------
// Batch 35 (WP-C): broken Claude lineage renders the reachable part with a
// warning, exactly like Python `_active_lineage` (verified against the
// reference adapter on the same synthetic records).
// ---------------------------------------------------------------------------

/// The natural lineage plus its notes, for the shapes below.
fn claude_lineage(records: &[(Value, u64)]) -> super::claude::Lineage {
    super::claude::lineage(records, ParseOptions::default())
}

fn active(lineage: &super::claude::Lineage) -> Vec<String> {
    let mut ids = lineage
        .active
        .as_ref()
        .expect("a tip was seen")
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

/// u1 → a1 → [record x lost to a torn line] → u2 (parentUuid x) → a2: Python
/// walks a2 → u2 → x, stops there, and renders u2/a2 only.
#[test]
fn claude_missing_ancestor_truncates_the_timeline_with_a_warning() {
    let records = rows(vec![
        message("user", "u1", Value::Null, "first question"),
        message("assistant", "a1", json!("u1"), "first answer"),
        message("user", "u2", json!("x"), "second question"),
        message("assistant", "a2", json!("u2"), "second answer"),
    ]);
    let lineage = claude_lineage(&records);
    assert_eq!(lineage.tip.as_deref(), Some("a2"));
    assert_eq!(active(&lineage), ["a2", "u2", "x"]);
    assert!(lineage.abandoned.is_empty());
    assert_eq!(
        lineage.warnings,
        ["Claude 祖先链在 x 处中断，之前的记录不在当前时间线"]
    );
    let (meta, messages) = project("claude", &records);
    assert_eq!(meta["_timeline_tip"], "a2");
    assert_eq!(
        visible(&messages),
        [("user", "second question"), ("assistant", "second answer")]
    );
    assert!(
        messages
            .iter()
            .all(|message| message.get("interrupted").is_none())
    );
    // A single record whose parent never existed is its own timeline.
    let records = rows(vec![message(
        "user",
        "a",
        json!("missing-root"),
        "incomplete",
    )]);
    let lineage = claude_lineage(&records);
    assert_eq!(active(&lineage), ["a", "missing-root"]);
    assert_eq!(
        lineage.warnings,
        ["Claude 祖先链在 missing-root 处中断，之前的记录不在当前时间线"]
    );
    let (_, messages) = project("claude", &records);
    assert_eq!(visible(&messages), [("user", "incomplete")]);
}

/// a.parentUuid = b, b.parentUuid = a: Python's walk adds both and stops at
/// the first repeat; every active record renders in file order.
#[test]
fn claude_lineage_cycle_is_truncated_at_the_revisited_node_with_a_warning() {
    let cycle = vec![
        message("user", "a", json!("b"), "cycle a"),
        message("assistant", "b", json!("a"), "cycle b"),
    ];
    // tip = a (declared by last-prompt): a → b → a.
    let mut declared = cycle.clone();
    declared.push(json!({"type": "last-prompt", "leafUuid": "a"}));
    let records = rows(declared);
    let lineage = claude_lineage(&records);
    assert_eq!(lineage.tip.as_deref(), Some("a"));
    assert_eq!(active(&lineage), ["a", "b"]);
    assert_eq!(lineage.warnings, ["Claude 祖先链存在循环，已在 a 处截断"]);
    let (meta, messages) = project("claude", &records);
    assert_eq!(meta["_timeline_tip"], "a");
    assert_eq!(
        visible(&messages),
        [("user", "cycle a"), ("assistant", "cycle b")]
    );
    // tip = b (last graph record): b → a → b.
    let records = rows(cycle);
    let lineage = claude_lineage(&records);
    assert_eq!(lineage.tip.as_deref(), Some("b"));
    assert_eq!(active(&lineage), ["a", "b"]);
    assert_eq!(lineage.warnings, ["Claude 祖先链存在循环，已在 b 处截断"]);
    let (_, messages) = project("claude", &records);
    assert_eq!(
        visible(&messages),
        [("user", "cycle a"), ("assistant", "cycle b")]
    );
}

/// `last-prompt` naming a leaf with no record: Python's active set is just
/// that uuid, so every graph node is hidden and only non-graph records (the
/// custom title's `/rename`) render. A later graph record moves the tip and
/// clears the note.
#[test]
fn claude_declared_leaf_without_a_record_hides_every_graph_node_with_a_warning() {
    let base = vec![
        message("user", "u1", Value::Null, "q"),
        message("assistant", "a1", json!("u1"), "ans"),
        json!({"type": "last-prompt", "leafUuid": "missing"}),
        json!({"type": "summary", "summary": "sum text", "leafUuid": "u1"}),
        json!({"type": "custom-title", "customTitle": "Custom"}),
    ];
    let records = rows(base.clone());
    let lineage = claude_lineage(&records);
    assert_eq!(lineage.tip.as_deref(), Some("missing"));
    assert_eq!(active(&lineage), ["missing"]);
    assert_eq!(lineage.warnings, ["Claude 声明的叶子 missing 不在记录中"]);
    let (meta, messages) = project("claude", &records);
    assert_eq!(meta["_timeline_tip"], "missing");
    assert_eq!(visible(&messages), [("command", "/rename Custom")]);
    assert!(conversation(&messages).is_empty());
    let mut continued = base;
    continued.push(message("user", "u2", json!("u1"), "q2"));
    let records = rows(continued);
    let lineage = claude_lineage(&records);
    assert_eq!(lineage.tip.as_deref(), Some("u2"));
    assert_eq!(active(&lineage), ["u1", "u2"]);
    assert!(lineage.warnings.is_empty());
    let (_, messages) = project("claude", &records);
    assert_eq!(conversation(&messages), [("user", "q"), ("user", "q2")]);
}

/// A broken chain never disables pins: the reachable part is still the
/// active lineage the pin target is validated against.
#[test]
fn claude_pin_target_on_a_truncated_lineage_uses_the_reachable_part() {
    use super::claude::{PinTargetError, resolve_pin_target};
    let records = rows(vec![
        message("user", "u1", Value::Null, "first question"),
        message("assistant", "a1", json!("u1"), "first answer"),
        message("user", "u2", json!("x"), "second question"),
        message("assistant", "a2", json!("u2"), "second answer"),
    ]);
    assert_eq!(resolve_pin_target(&records, "a2"), Ok("u2".to_owned()));
    assert_eq!(resolve_pin_target(&records, "u2"), Ok("x".to_owned()));
    assert_eq!(
        resolve_pin_target(&records, "a1"),
        Err(PinTargetError::Inactive)
    );
    assert_eq!(
        resolve_pin_target(&records, "x"),
        Err(PinTargetError::Unknown)
    );
}

// ---------------------------------------------------------------------------
// Batch 36 (WP-D): Claude interrupted turns stay visible — Python d16c5e1
// `_active_lineage` (interrupt_nodes / abandoned via interrupt ancestry /
// offshoot / deferred_abort) and `_read_one` (filter, `starts_turn = not
// is_interrupt`, `interrupted` texts, deferred `aborted`). Every expected
// sequence below was produced by the reference adapter on the same rows.
// ---------------------------------------------------------------------------

const INTERRUPT: &str = "[Request interrupted by user]";

fn turn_duration(id: &str, parent: &str) -> Value {
    json!({"type": "system", "subtype": "turn_duration", "uuid": id, "parentUuid": parent,
           "timestamp": "2026-09-12T12:00:00Z"})
}

/// Every event in order: statuses as their state, messages as their text.
fn timeline(messages: &[Value]) -> Vec<(&str, &str)> {
    messages
        .iter()
        .map(|message| {
            (
                message["role"].as_str().unwrap(),
                message["text"].as_str().unwrap(),
            )
        })
        .collect()
}

fn sorted(set: &std::collections::HashSet<String>) -> Vec<&str> {
    let mut ids = set.iter().map(String::as_str).collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

fn message_at<'a>(messages: &'a [Value], text: &str) -> &'a Value {
    messages
        .iter()
        .find(|message| message["text"] == text)
        .unwrap_or_else(|| panic!("no message {text:?}"))
}

/// Python `test_fast_escape_keeps_unanswered_input_visible` verbatim.
#[test]
fn claude_fast_escape_keeps_unanswered_input_visible() {
    let records = rows(vec![
        message("user", "u0", Value::Null, "共同开头"),
        message("assistant", "a0", json!("u0"), "共同回答"),
        message("user", "cancelled", json!("a0"), "快速 Esc 的输入"),
        json!({"type": "attachment", "uuid": "reminder", "parentUuid": "cancelled",
               "isSidechain": false, "attachment": {"type": "total_tokens_reminder"}}),
        message("user", "replacement", json!("a0"), "之后的新输入"),
        message("assistant", "answer", json!("replacement"), "新回答"),
    ]);
    let lineage = claude_lineage(&records);
    assert_eq!(sorted(&lineage.abandoned), ["cancelled"]);
    assert_eq!(sorted(&lineage.offshoot), ["reminder"]);
    assert!(lineage.deferred_abort.is_empty());
    let (_, messages) = project("claude", &records);
    let interrupted = message_at(&messages, "快速 Esc 的输入");
    assert_eq!(interrupted["interrupted"], true);
    assert!(
        interrupted["interrupt_reason"]
            .as_str()
            .unwrap()
            .contains("已中断")
    );
    assert_eq!(
        timeline(&messages),
        [
            ("status", "working"),
            ("user", "共同开头"),
            ("assistant", "共同回答"),
            ("user", "快速 Esc 的输入"),
            ("status", "aborted"),
            ("status", "working"),
            ("user", "之后的新输入"),
            ("assistant", "新回答"),
        ]
    );
    assert_eq!(messages[4]["reason"], "输入已中断，未进入当前 Claude 分支");
    assert_eq!(messages[4]["turn_id"], "cancelled");
}

/// Python `test_interrupted_sibling_with_tools_stays_visible` verbatim: the
/// assistant already replied, the user pressed Esc, the next input hangs off
/// the previous turn_duration. Rules 1, 3, 4, 5 and 6 together.
#[test]
fn claude_interrupted_sibling_with_tools_stays_visible() {
    let records = rows(vec![
        message("user", "u0", Value::Null, "共同开头"),
        message("assistant", "a0", json!("u0"), "全部搜了一遍"),
        turn_duration("t0", "a0"),
        message("user", "u-work", json!("t0"), "第一条，明显前后矛盾"),
        message("assistant", "a-work", json!("u-work"), "开始核对文档"),
        message("user", "interrupt", json!("a-work"), INTERRUPT),
        message("user", "u-replace", json!("t0"), "原来写需要授权"),
        message("assistant", "a-replace", json!("u-replace"), "已改正"),
    ]);
    let lineage = claude_lineage(&records);
    assert_eq!(
        active(&lineage),
        ["a-replace", "a0", "t0", "u-replace", "u0"]
    );
    assert_eq!(sorted(&lineage.abandoned), ["interrupt", "u-work"]);
    assert_eq!(sorted(&lineage.offshoot), ["a-work", "interrupt"]);
    assert_eq!(sorted(&lineage.deferred_abort), ["u-work"]);
    let (_, messages) = project("claude", &records);
    let interrupted = message_at(&messages, "第一条，明显前后矛盾");
    assert_eq!(interrupted["interrupted"], true);
    assert_eq!(
        interrupted["interrupt_reason"],
        "输入已中断，未进入当前 Claude 分支"
    );
    assert_eq!(interrupted["turn_id"], "u-work");
    assert_eq!(
        timeline(&messages),
        [
            ("status", "working"),
            ("user", "共同开头"),
            ("assistant", "全部搜了一遍"),
            ("status", "idle"),
            ("user", "第一条，明显前后矛盾"),
            ("assistant", "开始核对文档"),
            ("status", "aborted"),
            ("status", "working"),
            ("user", "原来写需要授权"),
            ("assistant", "已改正"),
        ]
    );
    // The aborted status is the native interrupt record's (no reason, same
    // turn), not one appended after the abandoned input.
    assert!(messages[6].get("reason").is_none());
    assert_eq!(messages[6]["turn_id"], "u-work");
    assert!(
        message_at(&messages, "开始核对文档")
            .get("interrupted")
            .is_none()
    );
    assert_eq!(message_at(&messages, "开始核对文档")["turn_id"], "u-work");
}

/// Rule 1: `interruptedMessageId` alone marks the interrupt record, whatever
/// its text; the same lineage as the bracketed text.
#[test]
fn claude_interrupt_record_by_message_id_marks_the_abandoned_turn() {
    let records = rows(vec![
        message("user", "u0", Value::Null, "root"),
        message("assistant", "a0", json!("u0"), "root answer"),
        turn_duration("t0", "a0"),
        message("user", "u1", json!("t0"), "cut short"),
        message("assistant", "a1", json!("u1"), "partial"),
        json!({"type": "user", "uuid": "stop", "parentUuid": "a1", "interruptedMessageId": "msg_1",
               "timestamp": "2026-09-12T12:00:00Z", "message": {"role": "user", "content": "stopped"}}),
        message("user", "u2", json!("t0"), "again"),
        message("assistant", "a2", json!("u2"), "final"),
    ]);
    let lineage = claude_lineage(&records);
    assert_eq!(sorted(&lineage.abandoned), ["stop", "u1"]);
    assert_eq!(sorted(&lineage.offshoot), ["a1", "stop"]);
    assert_eq!(sorted(&lineage.deferred_abort), ["u1"]);
    let (_, messages) = project("claude", &records);
    assert_eq!(
        timeline(&messages),
        [
            ("status", "working"),
            ("user", "root"),
            ("assistant", "root answer"),
            ("status", "idle"),
            ("user", "cut short"),
            ("assistant", "partial"),
            ("status", "aborted"),
            ("status", "working"),
            ("user", "again"),
            ("assistant", "final"),
        ]
    );
    assert_eq!(message_at(&messages, "cut short")["interrupted"], true);
    // Without the interrupt record the same shape is a completed old branch
    // (rule 2 unchanged): hidden.
    let mut records = records;
    records.remove(5);
    let lineage = claude_lineage(&records);
    assert!(lineage.abandoned.is_empty() && lineage.offshoot.is_empty());
    let (_, messages) = project("claude", &records);
    assert_eq!(
        conversation(&messages),
        [
            ("user", "root"),
            ("assistant", "root answer"),
            ("user", "again"),
            ("assistant", "final")
        ]
    );
}

/// Rule 3 + 4: the walk up from the interrupt passes tool calls and results;
/// every record of the cut-short turn renders under the abandoned input's
/// turn id, in file order, without `interrupted` on the assistant's rows.
#[test]
fn claude_interrupt_below_tool_results_keeps_the_whole_turn_visible() {
    let records = rows(vec![
        message("user", "u0", Value::Null, "root"),
        message("assistant", "a0", json!("u0"), "root answer"),
        turn_duration("t0", "a0"),
        message("user", "u1", json!("t0"), "cut short"),
        json!({"type": "assistant", "uuid": "a1", "parentUuid": "u1", "timestamp": "2026-09-12T12:00:00Z",
               "message": {"role": "assistant", "content": [
                   {"type": "text", "text": "calling"},
                   {"type": "tool_use", "id": "c1", "name": "Bash", "input": {"command": "true"}}]}}),
        json!({"type": "user", "uuid": "r1", "parentUuid": "a1", "timestamp": "2026-09-12T12:00:00Z",
               "message": {"role": "user", "content": [
                   {"type": "tool_result", "tool_use_id": "c1", "content": "ok"}]}}),
        message("assistant", "a1b", json!("r1"), "more"),
        message("user", "stop", json!("a1b"), INTERRUPT),
        message("user", "u2", json!("t0"), "again"),
        message("assistant", "a2", json!("u2"), "final"),
    ]);
    let lineage = claude_lineage(&records);
    assert_eq!(sorted(&lineage.abandoned), ["stop", "u1"]);
    assert_eq!(sorted(&lineage.offshoot), ["a1", "a1b", "r1", "stop"]);
    assert_eq!(sorted(&lineage.deferred_abort), ["u1"]);
    let (_, messages) = project("claude", &records);
    let roles = messages
        .iter()
        .map(|message| message["role"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        roles,
        [
            "status",
            "user",
            "assistant",
            "status",
            "user",
            "assistant",
            "tool",
            "tool_result",
            "assistant",
            "status",
            "status",
            "user",
            "assistant"
        ]
    );
    assert_eq!(messages[9]["state"], "aborted");
    assert!(messages[9].get("reason").is_none());
    for (index, message) in messages.iter().enumerate().skip(4).take(6) {
        assert_eq!(message["turn_id"], "u1", "{index}");
    }
    assert_eq!(message_at(&messages, "cut short")["interrupted"], true);
    assert!(
        messages[5..=8]
            .iter()
            .all(|message| message.get("interrupted").is_none())
    );
}

/// Rule 4 at two levels: inputs typed during the interrupt (below the
/// interrupt record, below an attachment) are unanswered offshoot users, so
/// abandoned themselves; each ends its own turn with `aborted` right away
/// (rule 5 does not apply: nothing native below them).
#[test]
fn claude_unanswered_inputs_below_the_interrupt_are_abandoned_at_every_level() {
    let records = rows(vec![
        message("user", "u0", Value::Null, "root"),
        message("assistant", "a0", json!("u0"), "root answer"),
        turn_duration("t0", "a0"),
        message("user", "u1", json!("t0"), "cut short"),
        message("assistant", "a1", json!("u1"), "partial"),
        message("user", "stop", json!("a1"), INTERRUPT),
        message("user", "u1b", json!("stop"), "typed during the interrupt"),
        json!({"type": "attachment", "uuid": "att", "parentUuid": "u1b", "isSidechain": false,
               "attachment": {"type": "total_tokens_reminder"}}),
        message("user", "u1c", json!("att"), "typed again"),
        message("user", "u2", json!("t0"), "again"),
        message("assistant", "a2", json!("u2"), "final"),
    ]);
    let lineage = claude_lineage(&records);
    assert_eq!(sorted(&lineage.abandoned), ["stop", "u1", "u1b", "u1c"]);
    assert_eq!(
        sorted(&lineage.offshoot),
        ["a1", "att", "stop", "u1b", "u1c"]
    );
    assert_eq!(sorted(&lineage.deferred_abort), ["u1"]);
    let (_, messages) = project("claude", &records);
    assert_eq!(
        timeline(&messages),
        [
            ("status", "working"),
            ("user", "root"),
            ("assistant", "root answer"),
            ("status", "idle"),
            ("user", "cut short"),
            ("assistant", "partial"),
            ("status", "aborted"),
            ("user", "typed during the interrupt"),
            ("status", "aborted"),
            ("user", "typed again"),
            ("status", "aborted"),
            ("status", "working"),
            ("user", "again"),
            ("assistant", "final"),
        ]
    );
    for (index, turn) in [(7, "u1b"), (8, "u1b"), (9, "u1c"), (10, "u1c")] {
        assert_eq!(messages[index]["turn_id"], turn, "{index}");
    }
    assert!(messages[6].get("reason").is_none());
    assert_eq!(messages[8]["reason"], "输入已中断，未进入当前 Claude 分支");
    assert_eq!(messages[10]["reason"], "输入已中断，未进入当前 Claude 分支");
    for text in ["typed during the interrupt", "typed again"] {
        assert_eq!(message_at(&messages, text)["interrupted"], true);
    }
}

/// Rule 5 next to rule 2: the deferred abort only concerns an abandoned input
/// with a native interrupt/response below it; a plain unanswered sibling
/// later in the same file still gets its `aborted` right after itself.
#[test]
fn claude_deferred_abort_is_per_input_not_per_session() {
    let records = rows(vec![
        message("user", "u0", Value::Null, "root"),
        message("assistant", "a0", json!("u0"), "root answer"),
        turn_duration("t0", "a0"),
        message("user", "u1", json!("t0"), "cut short"),
        message("assistant", "a1", json!("u1"), "partial"),
        message("user", "stop", json!("a1"), INTERRUPT),
        message("user", "u2", json!("t0"), "again"),
        message("assistant", "a2", json!("u2"), "final"),
        message("user", "u3", json!("a2"), "second unanswered"),
        message("user", "u4", json!("a2"), "second replacement"),
        message("assistant", "a4", json!("u4"), "second final"),
    ]);
    let lineage = claude_lineage(&records);
    assert_eq!(sorted(&lineage.abandoned), ["stop", "u1", "u3"]);
    assert_eq!(sorted(&lineage.offshoot), ["a1", "stop"]);
    assert_eq!(sorted(&lineage.deferred_abort), ["u1"]);
    let (_, messages) = project("claude", &records);
    assert_eq!(
        timeline(&messages)[10..],
        [
            ("user", "second unanswered"),
            ("status", "aborted"),
            ("status", "working"),
            ("user", "second replacement"),
            ("assistant", "second final"),
        ]
    );
    assert_eq!(messages[11]["reason"], "输入已中断，未进入当前 Claude 分支");
    assert_eq!(messages[11]["turn_id"], "u3");
}

/// Rule 3 respects the confirmed rewind boundary like rule 2: an interrupted
/// input at or before `abandoned_after` stays hidden with its whole branch.
#[test]
fn claude_interrupted_turn_before_the_stale_boundary_stays_hidden() {
    let records = rows(vec![
        message("user", "u0", Value::Null, "root"),
        message("assistant", "a0", json!("u0"), "root answer"),
        turn_duration("t0", "a0"),
        message("user", "u1", json!("t0"), "cut short"),
        message("assistant", "a1", json!("u1"), "partial"),
        message("user", "stop", json!("a1"), INTERRUPT),
        message("user", "u2", json!("t0"), "again"),
        message("assistant", "a2", json!("u2"), "final"),
    ]);
    let options = ParseOptions {
        abandoned_after: records[3].1,
        ..Default::default()
    };
    let lineage = super::claude::lineage(&records, options);
    assert!(lineage.abandoned.is_empty());
    assert!(lineage.offshoot.is_empty());
    let (_, events, error) = parse_with_options(
        "claude",
        Path::new("synthetic.jsonl"),
        &records,
        None,
        "",
        options,
    );
    assert!(error.is_none());
    let messages = events
        .into_iter()
        .map(|event| event.message)
        .collect::<Vec<_>>();
    assert_eq!(
        timeline(&messages),
        [
            ("status", "working"),
            ("user", "root"),
            ("assistant", "root answer"),
            ("status", "idle"),
            ("status", "working"),
            ("user", "again"),
            ("assistant", "final"),
        ]
    );
}

/// Rule 6 corner: the interrupt record is itself the unanswered sibling. It
/// is abandoned, renders only the native `aborted` (no turn of its own) and
/// the next input starts its turn normally.
#[test]
fn claude_interrupt_record_as_sibling_renders_only_the_native_aborted() {
    let records = rows(vec![
        message("user", "u0", Value::Null, "root"),
        message("assistant", "a0", json!("u0"), "root answer"),
        message("user", "stop", json!("a0"), INTERRUPT),
        message("user", "u1", json!("a0"), "again"),
        message("assistant", "a1", json!("u1"), "final"),
    ]);
    let lineage = claude_lineage(&records);
    assert_eq!(sorted(&lineage.abandoned), ["stop"]);
    assert!(lineage.offshoot.is_empty() && lineage.deferred_abort.is_empty());
    let (_, messages) = project("claude", &records);
    assert_eq!(
        timeline(&messages),
        [
            ("status", "working"),
            ("user", "root"),
            ("assistant", "root answer"),
            ("status", "aborted"),
            ("status", "working"),
            ("user", "again"),
            ("assistant", "final"),
        ]
    );
    assert_eq!(messages[3]["turn_id"], "u0");
    assert!(messages[3].get("reason").is_none());
    assert_eq!(messages[5]["turn_id"], "u1");
}

/// The batch-35 lineage notes survive the new rules: a missing ancestor above
/// the interrupted turn still truncates with the same warning, and the
/// interrupted turn below it is still visible.
#[test]
fn claude_interrupt_rules_keep_the_lineage_warnings() {
    let records = rows(vec![
        message("user", "u0", Value::Null, "lost root"),
        message("assistant", "a0", json!("u0"), "lost answer"),
        turn_duration("t0", "torn"),
        message("user", "u1", json!("t0"), "cut short"),
        message("assistant", "a1", json!("u1"), "partial"),
        message("user", "stop", json!("a1"), INTERRUPT),
        message("user", "u2", json!("t0"), "again"),
        message("assistant", "a2", json!("u2"), "final"),
    ]);
    let lineage = claude_lineage(&records);
    assert_eq!(active(&lineage), ["a2", "t0", "torn", "u2"]);
    assert_eq!(sorted(&lineage.abandoned), ["stop", "u1"]);
    assert_eq!(
        lineage.warnings,
        ["Claude 祖先链在 torn 处中断，之前的记录不在当前时间线"]
    );
    let (meta, messages) = project("claude", &records);
    assert_eq!(
        warnings(&meta),
        ["Claude 祖先链在 torn 处中断，之前的记录不在当前时间线"]
    );
    assert_eq!(
        timeline(&messages),
        [
            ("status", "idle"),
            ("user", "cut short"),
            ("assistant", "partial"),
            ("status", "aborted"),
            ("status", "working"),
            ("user", "again"),
            ("assistant", "final"),
        ]
    );
}

// ---------------------------------------------------------------------------
// Batch 36 (WP-G): Codex multi-part tool outputs (`custom_tool_call_output`
// whose `output` is `[header, chunk, chunk, …]`, one stringified envelope per
// streamed chunk) show every chunk's output in part order; `exit_code` is the
// last chunk's, `duration_s` the sum. The reference adapter shows one chunk
// (first or last depending on the trailing part) — a documented DELTA.
// ---------------------------------------------------------------------------

const CODEX_EXEC_HEADER: &str = "Script completed\nWall time 0.3 seconds\nOutput:\n";
const CODEX_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

fn codex_chunk(id: &str, output: &str, wall: f64, exit: Option<i64>) -> Value {
    let mut envelope = json!({"chunk_id": id, "wall_time_seconds": wall,
        "original_token_count": 3, "output": output});
    match exit {
        Some(code) => envelope["exit_code"] = json!(code),
        None => envelope["session_id"] = json!(9001),
    }
    json!({"type": "input_text", "text": envelope.to_string()})
}

fn codex_exec_result(output: Value) -> (Event, Vec<Event>) {
    let records = rows(vec![
        json!({"type":"session_meta","payload":{"id":"synthetic","cwd":"/synthetic/chunks"}}),
        json!({"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","call_id":"call-1",
               "input":"text(await tools.exec_command({cmd: \"one\"}))","turn_id":"t1"}}),
        json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call-1",
               "output":output,"turn_id":"t1"}}),
    ]);
    let (_, events, error) = parse(
        "codex",
        Path::new("synthetic.jsonl"),
        &records,
        None,
        "2026-09-12T00:00:00.000Z",
    );
    assert!(error.is_none(), "{error:?}");
    let result = events
        .iter()
        .find(|event| event.message["role"] == "tool_result")
        .cloned()
        .expect("tool_result event");
    (result, events)
}

#[test]
fn codex_multi_part_output_shows_every_chunk_with_last_exit_and_summed_duration() {
    let (result, _) = codex_exec_result(json!([
        {"type": "input_text", "text": CODEX_EXEC_HEADER},
        codex_chunk("c1", "first line\n", 1.0, Some(0)),
        codex_chunk("c2", "second line\r\n", 0.25, None),
        codex_chunk("c3", "third line\n", 0.5, Some(2)),
        {"type": "input_text", "text": ""},
    ]));
    assert_eq!(
        result.message["text"],
        "first line\nsecond line\r\nthird line\n"
    );
    assert_eq!(result.message["exit_code"], 2);
    assert_eq!(result.message["duration_s"], 1.75);
    assert_eq!(result.message["error"], true);
    assert_eq!(result.message["call_id"], "call-1");
    assert_eq!(result.message["name"], "exec");
    assert_eq!(result.message["turn_id"], "t1");
    assert!(result.media.is_empty());
}

#[test]
fn codex_multi_part_output_registers_a_trailing_image_and_keeps_the_text() {
    let (result, events) = codex_exec_result(json!([
        {"type": "input_text", "text": CODEX_EXEC_HEADER},
        codex_chunk("c1", "PASS one\n", 0.000_002, Some(0)),
        codex_chunk("c2", "PASS two\n", 0.000_004, Some(0)),
        {"type": "input_image", "image_url": format!("data:image/png;base64,{CODEX_PNG}")},
    ]));
    assert_eq!(result.message["text"], "PASS one\nPASS two\n");
    assert_eq!(result.message["exit_code"], 0);
    assert_eq!(result.message["duration_s"], 0.000_006);
    assert_eq!(result.message["error"], false);
    assert_eq!(result.media.len(), 1);
    for event in &events {
        assert!(!event.message.to_string().contains(CODEX_PNG));
    }
    // Chunks without output next to an image still yield the placeholder.
    let (only_image, _) = codex_exec_result(json!([
        {"type": "input_text", "text": CODEX_EXEC_HEADER},
        codex_chunk("c1", "", 1.0, Some(0)),
        codex_chunk("c2", "", 1.0, Some(0)),
        {"type": "input_image", "image_url": format!("data:image/png;base64,{CODEX_PNG}")},
    ]));
    assert_eq!(only_image.message["text"], "[图片]");
    assert_eq!(only_image.media.len(), 1);
}

#[test]
fn codex_single_chunk_and_wrapped_outputs_are_unchanged() {
    // One chunk followed by the script's own print: the chunk's output only,
    // exactly like the reference adapter.
    let (result, _) = codex_exec_result(json!([
        {"type": "input_text", "text": CODEX_EXEC_HEADER},
        codex_chunk("c1", "only\n", 1.0, Some(0)),
        {"type": "input_text", "text": "SESSION_ID=10756"},
    ]));
    assert_eq!(result.message["text"], "only\n");
    assert_eq!(result.message["duration_s"], 1.0);
    // `{"i","status","value":{…}}` wrappers are not chunks: raw text, no fields.
    let wrapped = json!({"i": 0, "status": "fulfilled",
        "value": {"chunk_id": "w", "wall_time_seconds": 1.0, "exit_code": 0, "output": "hidden"}});
    let (result, _) = codex_exec_result(json!([
        {"type": "input_text", "text": CODEX_EXEC_HEADER},
        {"type": "input_text", "text": wrapped.to_string()},
        {"type": "input_text", "text": wrapped.to_string()},
    ]));
    let text = result.message["text"].as_str().unwrap();
    assert!(text.starts_with(CODEX_EXEC_HEADER) && text.contains("\"status\":\"fulfilled\""));
    assert!(result.message.get("duration_s").is_none());
    // Sniffed from the raw text like any other envelope-less output (the
    // batch-21 documented superset), not taken from the wrapped envelope.
    assert_eq!(result.message["exit_code"], 0);
    assert_eq!(result.message["error"], false);
}

// ---------------------------------------------------------------- Grok (WP-K)
// Python 16cc89c `tests/test_adapters.py` GrokAdapterTests: the in-flight
// `user_query` envelope and its neighbours.

/// `test_in_flight_user_query_envelope_is_removed`: the protocol prefix a
/// message sent mid-turn or after an interrupt carries, and the trailing
/// reminder, are envelope; a tag inside ordinary text is kept verbatim.
#[test]
fn grok_in_flight_user_query_envelope_is_removed() {
    let records = rows(vec![
        json!({"type": "user", "content": [{"type": "text", "text":
            "The user sent a message while you were working:\n<user_query>\n不要显示任何续写状况。显示最新那个会话就行。\n</user_query>\nMake sure to complete any unfinished tasks from previous turns."}],
            "prompt_index": 7}),
        json!({"type": "user", "content": [{"type": "text", "text":
            "The user interrupted the previous turn:\n<user_query>\n如果有多个提交，amend合并。\n</user_query>\nMake sure to complete any unfinished tasks from previous turns."}],
            "prompt_index": 8}),
        json!({"type": "user", "content": [{"type": "text", "text":
            "请看这段 <user_query>示例</user_query> 标签怎么渲染"}], "prompt_index": 9}),
    ]);
    let (_, messages) = project("grok", &records);
    assert_eq!(
        visible(&messages),
        vec![
            ("user", "不要显示任何续写状况。显示最新那个会话就行。"),
            ("user", "如果有多个提交，amend合并。"),
            (
                "user",
                "请看这段 <user_query>示例</user_query> 标签怎么渲染"
            ),
        ]
    );
}

/// The prefix and suffix are optional and independent, case-insensitive like
/// the reference regex, image blocks may sit on either side of the prefix,
/// and only one newline is trimmed from each end of the body.
#[test]
fn grok_user_query_prefix_suffix_and_image_blocks_are_independent() {
    let cases: Vec<(&str, &str)> = vec![
        ("<user_query>\n plain \n</user_query>", " plain "),
        (
            "the user SENT a message while you were working:\n<user_query>\nx\n</user_query>",
            "x",
        ),
        (
            "<user_query>\ny\n</user_query>\nmake sure to complete any unfinished tasks from previous turns.\n",
            "y",
        ),
        (
            "<image_files>\n1. /a.png\n</image_files>\nThe user interrupted the previous turn:\n<image_files>\n2. /b.png\n</image_files>\n<user_query>\n[Image #1] z\n</user_query>\nMake sure to complete any unfinished tasks from previous turns.",
            "[Image #1] z",
        ),
        (
            "<user_query>\n\nkeep one blank\n\n</user_query>",
            "\nkeep one blank\n",
        ),
        // Anything else outside the tags is not an envelope.
        (
            "The user was away:\n<user_query>\nq\n</user_query>",
            "The user was away:\n<user_query>\nq\n</user_query>",
        ),
        (
            "<user_query>\nq\n</user_query>\nMake sure to finish.",
            "<user_query>\nq\n</user_query>\nMake sure to finish.",
        ),
    ];
    for (index, (text, expected)) in cases.iter().enumerate() {
        let records = rows(vec![
            json!({"type": "user", "content": text, "prompt_index": index + 1}),
        ]);
        let (_, messages) = project("grok", &records);
        assert_eq!(
            visible(&messages),
            vec![("user", *expected)],
            "case {index}: {text:?}"
        );
    }
}
// ---------------------------------------------------------------- end Grok (WP-K)
