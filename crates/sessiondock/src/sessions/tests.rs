use super::*;

use std::io::Write;

use tempfile::TempDir;

fn claude_row(id: &str, parent: Value, role: &str, text: &str) -> Value {
    json!({"type": role, "uuid": id, "parentUuid": parent,
           "sessionId": "synthetic-session", "cwd": "/workspace/demo",
           "timestamp": "2026-09-11T10:00:00.123Z",
           "message": {"role": role, "content": [{"type": "text", "text": text}]}})
}

fn write_rows(path: &Path, rows: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = fs::File::create(path).unwrap();
    for row in rows {
        writeln!(file, "{row}").unwrap()
    }
}

fn append(path: &Path, bytes: &[u8]) {
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

fn setup() -> (TempDir, PathBuf, SessionStore, String) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("claude");
    let file = root.join("project/session.jsonl");
    write_rows(
        &file,
        &[
            claude_row("u1", Value::Null, "user", "合成用户问题"),
            claude_row("a1", json!("u1"), "assistant", "合成回答"),
        ],
    );
    let store = SessionStore::new(SessionRoots {
        claude: Some(root),
        ..Default::default()
    });
    let list = store.list(false).unwrap();
    let uid = list["sessions"][0]["uid"].as_str().unwrap().to_owned();
    (temp, file, store, uid)
}

fn continuation(batch: &Value) -> MessageQuery {
    MessageQuery {
        start: batch["end"].as_u64().unwrap(),
        head: batch["version"]["head"].as_str().unwrap().to_owned(),
        anchor: batch["anchor"].as_str().unwrap().to_owned(),
        ..Default::default()
    }
}

#[test]
fn no_roots_means_empty_not_home_discovery() {
    let store = SessionStore::new(SessionRoots::default());
    let first = store.list(false).unwrap();
    assert_eq!(first["sessions"], json!([]));
    assert_eq!(store.list(true).unwrap(), first);
    assert_eq!(
        store
            .messages("claude:missing", &MessageQuery::default())
            .unwrap_err()
            .status,
        404
    );
}

#[test]
fn configured_invalid_root_is_not_silently_an_empty_library() {
    let temp = TempDir::new().unwrap();
    let store = SessionStore::new(SessionRoots {
        claude: Some(temp.path().join("missing")),
        ..Default::default()
    });
    assert_eq!(store.list(false).unwrap_err().status, 400);
}

#[test]
fn list_and_detail_use_native_bytes_and_millisecond_timestamps() {
    let (_temp, file, store, uid) = setup();
    let list = store.list(false).unwrap();
    let batch = store.messages(&uid, &MessageQuery::default()).unwrap();
    assert_eq!(batch["reset"], true);
    assert_eq!(batch["start"], 0);
    assert_eq!(batch["end"], fs::metadata(file).unwrap().len());
    assert_eq!(list["sessions"][0]["uid"], batch["meta"]["uid"]);
    assert_eq!(batch["meta"]["cursor"]["end"], batch["end"]);
    assert_eq!(batch["messages"][0]["text"], "合成用户问题");
    assert_eq!(batch["messages"][0]["ts"], "2026-09-11T10:00:00.123Z");
    assert_eq!(batch["message_total"], 2);
    assert_eq!(batch["activity"]["state"], "working");
    assert!(
        batch["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["role"] != "status")
    );
    // Opening the session adds only the view's anchor to its row; the signed
    // document (sig, built_at) is unchanged.
    let mut after = store.list(true).unwrap();
    assert_eq!(
        after["sessions"][0]["cursor"]["anchor"],
        batch["meta"]["cursor"]["anchor"]
    );
    after["sessions"][0]["cursor"]
        .as_object_mut()
        .unwrap()
        .remove("anchor");
    assert_eq!(after, list);
}

#[test]
fn unchanged_list_reuses_summaries_and_build_timestamp() {
    let (_temp, _file, store, _uid) = setup();
    let reads = store.index().reads();
    let before = store.list(false).unwrap();
    // A forced rescan with unchanged stamps re-reads no file and republishes
    // the same document, including `built_at`.
    assert_eq!(store.list(true).unwrap(), before);
    assert_eq!(store.index().reads(), reads);
    assert_eq!(
        store.view_stats().unwrap().views,
        0,
        "the list opens nothing"
    );
}

#[test]
fn short_unicode_file_append_is_incremental_and_only_changed_file_is_reparsed() {
    let (_temp, file, store, uid) = setup();
    let initial = store.messages(&uid, &MessageQuery::default()).unwrap();
    assert!(initial["end"].as_u64().unwrap() < 4096);
    let decoded_before = store.views().unwrap().records().decoded;
    append(
        &file,
        format!(
            "{}\n",
            claude_row("a2", json!("a1"), "assistant", "追加中文🚀")
        )
        .as_bytes(),
    );
    let delta = store.messages(&uid, &continuation(&initial)).unwrap();
    assert_eq!(delta["reset"], false);
    assert_eq!(delta["start"], initial["end"]);
    assert_eq!(delta["messages"].as_array().unwrap().len(), 1);
    assert_eq!(delta["messages"][0]["text"], "追加中文🚀");
    assert_eq!(delta["end"], fs::metadata(file).unwrap().len());
    assert_eq!(delta["activity_changed"], false);
    let views = store.views().unwrap();
    assert_eq!(views.records().decoded - decoded_before, 1);
    assert_eq!(views.records().reused, 2);
}

#[test]
fn half_line_is_not_consumed_and_completion_delivers_once() {
    let (_temp, file, store, uid) = setup();
    let initial = store.messages(&uid, &MessageQuery::default()).unwrap();
    let row = format!(
        "{}\n",
        claude_row("a2", json!("a1"), "assistant", "半行安全")
    );
    let cut = row.len() / 2;
    append(&file, &row.as_bytes()[..cut]);
    let half = store.messages(&uid, &continuation(&initial)).unwrap();
    assert_eq!(half["reset"], false);
    assert_eq!(half["end"], initial["end"]);
    assert_eq!(half["messages"], json!([]));
    append(&file, &row.as_bytes()[cut..]);
    let complete = store.messages(&uid, &continuation(&half)).unwrap();
    assert_eq!(complete["reset"], false);
    assert_eq!(complete["messages"].as_array().unwrap().len(), 1);
    assert_eq!(complete["messages"][0]["text"], "半行安全");
    assert_eq!(
        store.messages(&uid, &continuation(&complete)).unwrap()["messages"],
        json!([])
    );
}

#[test]
fn complete_json_without_newline_is_still_uncommitted() {
    let (_temp, file, store, uid) = setup();
    let initial = store.messages(&uid, &MessageQuery::default()).unwrap();
    append(
        &file,
        claude_row("a2", json!("a1"), "assistant", "等待换行")
            .to_string()
            .as_bytes(),
    );
    assert_eq!(
        store.messages(&uid, &continuation(&initial)).unwrap()["end"],
        initial["end"]
    );
    append(&file, b"\n");
    assert_eq!(
        store.messages(&uid, &continuation(&initial)).unwrap()["messages"][0]["text"],
        "等待换行"
    );
}

#[test]
fn zero_byte_checkpoint_stays_stable_while_first_line_is_incomplete() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("project/session.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let row = format!(
        "{}\n",
        claude_row("u1", Value::Null, "user", "第一条完整记录")
    );
    let cut = row.len() / 2;
    fs::write(&path, &row.as_bytes()[..cut]).unwrap();
    let store = SessionStore::new(SessionRoots {
        claude: Some(temp.path().to_path_buf()),
        ..Default::default()
    });
    let uid = store.list(false).unwrap()["sessions"][0]["uid"]
        .as_str()
        .unwrap()
        .to_owned();
    let first = store.messages(&uid, &MessageQuery::default()).unwrap();
    assert_eq!(first["reset"], true);
    assert_eq!(first["end"], 0);
    let checkpoint = continuation(&first);
    let idle = store.messages(&uid, &checkpoint).unwrap();
    assert_eq!(idle["reset"], false);
    assert_eq!(idle["messages"], json!([]));
    append(&path, &row.as_bytes()[cut..]);
    let complete = store.messages(&uid, &checkpoint).unwrap();
    assert_eq!(complete["reset"], false);
    assert_eq!(complete["start"], 0);
    assert_eq!(complete["messages"][0]["text"], "第一条完整记录");
}

#[test]
fn truncate_rewrite_and_forged_or_missing_anchor_reset_safely() {
    let (_temp, file, store, uid) = setup();
    let initial = store.messages(&uid, &MessageQuery::default()).unwrap();
    let mut missing = continuation(&initial);
    missing.anchor.clear();
    assert_eq!(store.messages(&uid, &missing).unwrap()["reset"], true);
    write_rows(&file, &[claude_row("u2", Value::Null, "user", "重写")]);
    let mut query = continuation(&initial);
    query.append = "1".to_owned();
    let reset = store.messages(&uid, &query).unwrap();
    assert_eq!(reset["reset"], true);
    assert_eq!(reset["messages"], json!([]));
    assert_eq!(reset["start"], reset["end"]);
    assert_eq!(reset["message_total"], 0);
    let full = store.messages(&uid, &continuation(&initial)).unwrap();
    assert_eq!(full["reset"], true);
    assert_eq!(full["messages"][0]["text"], "重写");
}

#[test]
fn same_length_middle_edit_is_detected_beyond_head_and_tail() {
    let (_temp, file, store, uid) = setup();
    let mut rows = Vec::new();
    for i in 0..60 {
        rows.push(claude_row(
            &format!("m{i}"),
            if i == 0 {
                Value::Null
            } else {
                json!(format!("m{}", i - 1))
            },
            "assistant",
            &format!("position-{i:02} {}", "x".repeat(100)),
        ));
    }
    write_rows(&file, &rows);
    let before = store.messages(&uid, &MessageQuery::default()).unwrap();
    let original = fs::read_to_string(&file).unwrap();
    let modified = original.replace("position-30", "changed--30");
    assert_eq!(original.len(), modified.len());
    assert_eq!(&original.as_bytes()[..4096], &modified.as_bytes()[..4096]);
    assert_eq!(
        &original.as_bytes()[original.len() - 512..],
        &modified.as_bytes()[modified.len() - 512..]
    );
    fs::write(&file, modified).unwrap();
    let after = store.messages(&uid, &continuation(&before)).unwrap();
    assert_eq!(after["reset"], true);
    assert!(
        after["messages"][30]["text"]
            .as_str()
            .unwrap()
            .starts_with("changed--30")
    );
}

#[test]
fn malformed_complete_record_is_noted_but_shape_errors_fail_closed_then_recover_after_repair() {
    let (_temp, file, store, uid) = setup();
    // A line that is not a JSON object is skipped
    // `_iter_records`; the row and the detail both note it.
    append(&file, b"not-json\n");
    let batch = store.messages(&uid, &MessageQuery::default()).unwrap();
    assert_eq!(batch["messages"].as_array().unwrap().len(), 2);
    assert_eq!(
        batch["meta"]["migration_warnings"],
        json!(["跳过无效的JSONL 记录 ×1"])
    );
    let list = store.list(true).unwrap();
    assert_eq!(list["sessions"][0]["supported"], true);
    assert_eq!(
        list["sessions"][0]["migration_warnings"],
        json!(["跳过无效的JSONL 记录 ×1"])
    );
    // A shape the reference adapter cannot read either still fails closed.
    append(
        &file,
        b"{\"type\":\"user\",\"uuid\":\"bad\",\"parentUuid\":\"a1\",\"sessionId\":\"synthetic-session\",\"message\":{\"role\":\"user\",\"content\":42}}\n",
    );
    assert_eq!(
        store
            .messages(&uid, &MessageQuery::default())
            .unwrap_err()
            .status,
        501
    );
    let list = store.list(true).unwrap();
    assert_eq!(list["sessions"][0]["supported"], false);
    write_rows(&file, &[claude_row("u1", Value::Null, "user", "repaired")]);
    assert_eq!(
        store.messages(&uid, &MessageQuery::default()).unwrap()["messages"][0]["text"],
        "repaired"
    );
}

#[test]
fn duplicate_native_keys_follow_python_last_key_wins() {
    let (_temp, file, store, uid) = setup();
    let initial = store.messages(&uid, &MessageQuery::default()).unwrap();
    // Escaped-equivalent keys follow json.loads and keep the last value.
    append(
        &file,
        b"{\"type\":\"user\",\"ty\\u0070e\":\"assistant\",\"uuid\":\"dup\",\"parentUuid\":\"a1\",\"sessionId\":\"synthetic-session\",\"message\":{\"role\":\"assistant\",\"content\":\"last key wins\"}}\n",
    );
    let delta = store.messages(&uid, &continuation(&initial)).unwrap();
    assert_eq!(delta["messages"][0]["text"], "last key wins");
    assert_eq!(delta["meta"]["migration_warnings"], json!([]));
    assert_eq!(delta["end"], fs::metadata(&file).unwrap().len());
    assert_eq!(store.list(true).unwrap()["sessions"][0]["supported"], true);
    write_rows(
        &file,
        &[claude_row("u1", Value::Null, "user", "repaired duplicate")],
    );
    let repaired = store.messages(&uid, &continuation(&initial)).unwrap();
    assert_eq!(repaired["reset"], true);
    assert_eq!(repaired["messages"][0]["text"], "repaired duplicate");
}

#[test]
fn branches_replace_old_history_and_agent_values_are_not_paths() {
    let (_temp, file, store, uid) = setup();
    let original = store.messages(&uid, &MessageQuery::default()).unwrap();
    append(
        &file,
        format!(
            "{}\n",
            claude_row("u2", json!("u1"), "user", "sibling branch")
        )
        .as_bytes(),
    );
    let branch = store.messages(&uid, &continuation(&original)).unwrap();
    assert_eq!(branch["reset"], true);
    assert_eq!(
        branch["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["text"].as_str())
            .collect::<Vec<_>>(),
        vec!["合成用户问题", "sibling branch"]
    );
    let query = MessageQuery {
        agent: "../../outside".to_owned(),
        ..Default::default()
    };
    assert!(matches!(
        store.messages(&uid, &query).unwrap_err().status,
        400 | 404
    ));
}

#[test]
fn last_prompt_only_append_invalidates_prior_visible_branch() {
    let (_temp, file, store, uid) = setup();
    let before = store.messages(&uid, &MessageQuery::default()).unwrap();
    let decoded_before = store.views().unwrap().records().decoded;
    append(&file, b"{\"type\":\"last-prompt\",\"leafUuid\":\"u1\"}\n");
    let after = store.messages(&uid, &continuation(&before)).unwrap();
    assert_eq!(after["reset"], true);
    assert_eq!(after["messages"].as_array().unwrap().len(), 1);
    assert_eq!(after["messages"][0]["text"], "合成用户问题");
    let idle = store.messages(&uid, &continuation(&after)).unwrap();
    assert_eq!(idle["reset"], false);
    assert_eq!(idle["messages"], json!([]));
    assert_eq!(store.views().unwrap().records().decoded - decoded_before, 1);
    let cold = SessionStore::new(store.index().roots().clone());
    assert_eq!(
        store.messages(&uid, &MessageQuery::default()).unwrap(),
        cold.messages(&uid, &MessageQuery::default()).unwrap()
    );
}

#[test]
fn cached_codex_ast_still_reprojects_an_abort_into_the_old_message() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("codex");
    let path = root.join("project/session.jsonl");
    let rows = [
        json!({"type":"session_meta","payload":{"id":"cache-abort","cwd":"/synthetic"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"commentary","turn_id":"t","content":[{"type":"output_text","text":"working"}]}}),
    ];
    write_rows(&path, &rows);
    let store = SessionStore::new(SessionRoots {
        codex: Some(root),
        ..Default::default()
    });
    let uid = uid_for("codex", &path);
    let before = store.messages(&uid, &MessageQuery::default()).unwrap();
    let decoded = store.views().unwrap().records().decoded;
    append(
        &path,
        b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"turn_aborted\",\"turn_id\":\"t\"}}\n",
    );
    let after = store.messages(&uid, &continuation(&before)).unwrap();
    assert_eq!(after["reset"], true);
    assert_eq!(after["messages"][0]["interrupted"], true);
    assert_eq!(store.views().unwrap().records().decoded - decoded, 1);
    let cold = SessionStore::new(store.index().roots().clone());
    assert_eq!(
        store.messages(&uid, &MessageQuery::default()).unwrap(),
        cold.messages(&uid, &MessageQuery::default()).unwrap()
    );
}

#[test]
fn native_list_cursor_matches_detail_and_view_cache_is_reused() {
    let (_temp, _file, store, uid) = setup();
    // Before any open the row carries the physical cursor only; the semantic
    // anchor is borrowed from the cached view once the session was opened,
    // without changing the signature (docs/history-pages.md).
    let list = store.list(false).unwrap();
    let batch = store.messages(&uid, &MessageQuery::default()).unwrap();
    let physical = &list["sessions"][0]["cursor"];
    assert_eq!(physical["end"], batch["meta"]["cursor"]["end"]);
    assert_eq!(physical["head"], batch["meta"]["cursor"]["head"]);
    assert!(physical.get("anchor").is_none());
    let opened = store.list(false).unwrap();
    assert_eq!(opened["sessions"][0]["cursor"], batch["meta"]["cursor"]);
    assert_eq!(opened["sig"], list["sig"]);
    assert_eq!(opened["built_at"], list["built_at"]);
    let cached = store.views().unwrap().cached(&uid, "").unwrap().clone();
    store.messages(&uid, &continuation(&batch)).unwrap();
    let reused = store.views().unwrap().cached(&uid, "").unwrap().clone();
    assert!(Arc::ptr_eq(&cached, &reused));
}

fn codex_meta(sid: &str, parent: Option<(&str, u64)>) -> Value {
    let mut value = json!({"type":"session_meta", "timestamp":"2026-09-11T10:00:00Z",
        "payload":{"id":sid,"cwd":"/example/project"}});
    if let Some((parent, end)) = parent {
        value["payload"]["forked_from_id"] = json!(parent);
        value["payload"]["history_base"] = json!({"thread_id":parent,"end_byte_offset":end});
    }
    value
}

fn codex_text(role: &str, text: &str) -> Value {
    json!({"type":"response_item", "timestamp":"2026-09-11T10:00:01Z",
        "payload":{"type":"message", "role":role,
            "content":[{"type":"input_text","text":text}]}})
}

fn fork_setup() -> (TempDir, PathBuf, PathBuf, SessionStore, String, u64) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("codex");
    let parent = root.join("parent.jsonl");
    let child = root.join("child.jsonl");
    write_rows(
        &parent,
        &[
            codex_meta("parent", None),
            codex_text("user", "inherited question"),
        ],
    );
    let cut = fs::metadata(&parent).unwrap().len();
    append(
        &parent,
        format!("{}\n", codex_text("assistant", "excluded parent tail")).as_bytes(),
    );
    write_rows(
        &child,
        &[
            codex_meta("child", Some(("parent", cut))),
            codex_text("assistant", "child answer"),
        ],
    );
    let store = SessionStore::new(SessionRoots {
        codex: Some(root),
        ..Default::default()
    });
    let uid = uid_for("codex", &child);
    store.list(false).unwrap();
    (temp, parent, child, store, uid, cut)
}

#[test]
fn codex_inheritance_keeps_leaf_offsets_and_ignores_parent_tail_appends() {
    let (_temp, parent, child, store, uid, _) = fork_setup();
    let before = store.messages(&uid, &MessageQuery::default()).unwrap();
    assert_eq!(
        before["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["text"].as_str())
            .collect::<Vec<_>>(),
        vec!["inherited question", "child answer"]
    );
    assert_eq!(before["end"], fs::metadata(&child).unwrap().len());
    assert_eq!(before["meta"]["root_sid"], "parent");
    assert_eq!(before["meta"]["fork_depth"], 1);
    // Unknown semantics after the fixed cutoff cannot contaminate this view;
    // the parent itself stays supported with a skip warning.
    append(&parent, b"{\"type\":\"future-unsupported\"}\n");
    let idle = store.messages(&uid, &continuation(&before)).unwrap();
    assert_eq!(idle["reset"], false);
    assert_eq!(idle["anchor"], before["anchor"]);
    assert_eq!(idle["messages"], json!([]));
    let parent_row = store.list(true).unwrap()["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sid"] == "parent")
        .cloned()
        .unwrap();
    assert_eq!(parent_row["supported"], true);
    assert_eq!(
        parent_row["migration_warnings"],
        json!(["跳过未知的Codex 记录类型：future-unsupported ×1"])
    );
    // A later session_meta in the parent tail is skipped and counted;
    // it is confined to the parent view and row.
    append(
        &parent,
        b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"parent\"}}\n",
    );
    let idle = store.messages(&uid, &continuation(&idle)).unwrap();
    assert_eq!(idle["reset"], false);
    assert_eq!(idle["messages"], json!([]));
    let parent_uid = uid_for("codex", &parent);
    let parent_view = store
        .messages(&parent_uid, &MessageQuery::default())
        .unwrap();
    assert_eq!(parent_view["meta"]["supported"], true);
    assert_eq!(
        parent_view["meta"]["migration_warnings"],
        json!([
            "跳过重复的Codex session_meta ×1",
            "跳过未知的Codex 记录类型：future-unsupported ×1",
        ])
    );
    assert_eq!(
        parent_view["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["text"].as_str())
            .collect::<Vec<_>>(),
        vec!["inherited question", "excluded parent tail"]
    );
    let parent_row = store.list(true).unwrap()["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sid"] == "parent")
        .cloned()
        .unwrap();
    assert_eq!(parent_row["supported"], true);
    assert_eq!(
        parent_row["migration_warnings"],
        parent_view["meta"]["migration_warnings"]
    );
    append(
        &child,
        format!("{}\n", codex_text("assistant", "child continuation")).as_bytes(),
    );
    let delta = store.messages(&uid, &continuation(&idle)).unwrap();
    assert_eq!(delta["reset"], false);
    assert_eq!(delta["messages"].as_array().unwrap().len(), 1);
    assert_eq!(delta["messages"][0]["text"], "child continuation");
}

#[test]
fn codex_parent_prefix_rewrite_resets_without_changing_leaf_bytes() {
    let (_temp, parent, child, store, uid, _) = fork_setup();
    let before = store.messages(&uid, &MessageQuery::default()).unwrap();
    let child_before = fs::read(&child).unwrap();
    let old = fs::read_to_string(&parent).unwrap();
    let new = old.replace("inherited question", "rewritten question");
    assert_eq!(old.len(), new.len());
    fs::write(&parent, new).unwrap();
    let after = store.messages(&uid, &continuation(&before)).unwrap();
    assert_eq!(after["reset"], true);
    assert_eq!(after["messages"][0]["text"], "rewritten question");
    assert_eq!(fs::read(&child).unwrap(), child_before);
    assert_eq!(after["end"], before["end"]);
    assert_ne!(after["anchor"], before["anchor"]);
}

#[test]
fn fork_list_checkpoint_matches_detail_across_leaf_and_parent_changes() {
    let (_temp, parent, child, store, uid, _) = fork_setup();
    let checkpoint = || {
        let detail = store.messages(&uid, &MessageQuery::default()).unwrap();
        let list = store.list(true).unwrap();
        let row = list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["uid"] == uid)
            .unwrap();
        assert_eq!(row["cursor"], detail["meta"]["cursor"]);
        detail
    };
    let before = checkpoint();
    append(
        &parent,
        format!("{}\n", codex_text("assistant", "outside inherited prefix")).as_bytes(),
    );
    let parent_tail = checkpoint();
    assert_eq!(parent_tail["anchor"], before["anchor"]);
    append(
        &child,
        format!("{}\n", codex_text("assistant", "new leaf output")).as_bytes(),
    );
    let leaf = checkpoint();
    assert_ne!(leaf["anchor"], before["anchor"]);
    assert_eq!(
        store.messages(&uid, &continuation(&before)).unwrap()["reset"],
        false
    );
    let text = fs::read_to_string(&parent).unwrap();
    fs::write(
        &parent,
        text.replace("inherited question", "rewritten question"),
    )
    .unwrap();
    let rewritten = checkpoint();
    assert_ne!(rewritten["anchor"], leaf["anchor"]);
    assert_eq!(
        store.messages(&uid, &continuation(&leaf)).unwrap()["reset"],
        true
    );
}

#[test]
fn missing_parent_is_never_a_successful_empty_history_and_can_recover() {
    let (_temp, parent, _child, store, uid, _) = fork_setup();
    let bytes = fs::read(&parent).unwrap();
    let before = store.messages(&uid, &MessageQuery::default()).unwrap();
    fs::remove_file(&parent).unwrap();
    assert!(store.messages(&uid, &continuation(&before)).is_err());
    fs::write(&parent, bytes).unwrap();
    assert_eq!(
        store.messages(&uid, &MessageQuery::default()).unwrap()["messages"][0]["text"],
        "inherited question"
    );
}

#[test]
fn newly_created_parent_is_found_after_an_unsupported_missing_dependency() {
    let (_temp, parent, _child, store, uid, _) = fork_setup();
    let bytes = fs::read(&parent).unwrap();
    fs::remove_file(&parent).unwrap();
    store.list(true).unwrap();
    assert!(store.messages(&uid, &MessageQuery::default()).is_err());
    fs::write(&parent, bytes).unwrap();
    assert_eq!(
        store.messages(&uid, &MessageQuery::default()).unwrap()["messages"][0]["text"],
        "inherited question"
    );
}

/// A rollout carrying copied ancestor metas (the old-style fork / subagent
/// shape) declares one native id: the runtime catalog associates it by the
/// first meta instead of reporting a NativeConflict, and the copied metas
/// produce no message.
#[test]
fn codex_multi_meta_file_has_one_declared_id_in_the_runtime_catalog() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("codex");
    let path = root.join("rollout-multi.jsonl");
    write_rows(
        &path,
        &[
            codex_meta("own", None),
            codex_meta("ancestor-a", None),
            codex_meta("ancestor-b", None),
            codex_text("user", "own question"),
        ],
    );
    let store = SessionStore::new(SessionRoots {
        codex: Some(root),
        ..Default::default()
    });
    let uid = uid_for("codex", &path);
    let list = store.list(false).unwrap();
    let row = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["uid"] == uid)
        .unwrap();
    assert_eq!(row["supported"], true);
    assert_eq!(row["sid"], "own");
    assert_eq!(
        row["migration_warnings"],
        json!(["跳过重复的Codex session_meta ×2"])
    );
    let catalog = store.native_catalog().unwrap();
    let scope = catalog.verified_scope(&uid).unwrap();
    assert_eq!(scope.session_id, "own");
    assert_eq!(scope.agent_id, None);
    assert_eq!(store.native_scope(&uid, "").unwrap(), scope);
    let view = store.messages(&uid, &MessageQuery::default()).unwrap();
    assert_eq!(
        view["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["text"].as_str())
            .collect::<Vec<_>>(),
        vec!["own question"]
    );
}

#[test]
fn sse_only_reads_discover_new_ambiguous_parent_ids_on_shared_inventory_ttl() {
    let (_temp, parent, _child, store, uid, _) = fork_setup();
    let before = store.messages(&uid, &MessageQuery::default()).unwrap();
    let duplicate = parent.parent().unwrap().join("duplicate-parent.jsonl");
    fs::copy(&parent, &duplicate).unwrap();
    // Advance only the internal deadline; no wall-clock sleep or list call.
    store.index().expire();
    assert_eq!(
        store
            .messages(&uid, &continuation(&before))
            .unwrap_err()
            .status,
        409
    );
}

#[test]
fn claude_agent_is_resolved_from_inventory_membership_not_user_paths() {
    let (_temp, main, store, uid) = setup();
    let agent = main
        .parent()
        .unwrap()
        .join("session/subagents/agent-worker.jsonl");
    let mut row = claude_row("worker-u", Value::Null, "user", "agent task");
    row["isSidechain"] = json!(true);
    row["agentId"] = json!("worker");
    write_rows(&agent, &[row]);
    fs::write(
        agent.with_extension("meta.json"),
        r#"{"description":"synthetic worker title","agentType":"reviewer"}"#,
    )
    .unwrap();
    let list = store.list(true).unwrap();
    assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(list["sessions"][0]["agent_items"][0]["id"], "worker");
    assert_eq!(
        list["sessions"][0]["agent_items"][0]["title"],
        "synthetic worker title"
    );
    assert_eq!(list["sessions"][0]["agent_items"][0]["type"], "reviewer");
    let query = MessageQuery {
        agent: "worker".to_owned(),
        ..Default::default()
    };
    let batch = store.messages(&uid, &query).unwrap();
    assert_eq!(batch["messages"][0]["text"], "agent task");
    assert_eq!(batch["end"], fs::metadata(&agent).unwrap().len());
    let main_batch = store.messages(&uid, &MessageQuery::default()).unwrap();
    let wrong_view = MessageQuery {
        agent: "worker".to_owned(),
        ..continuation(&main_batch)
    };
    assert_eq!(store.messages(&uid, &wrong_view).unwrap()["reset"], true);
    let foreign = main.parent().unwrap().join("foreign.jsonl");
    write_rows(
        &foreign,
        &[claude_row("foreign", Value::Null, "user", "other session")],
    );
    store.list(true).unwrap();
    assert!(
        store
            .messages(&uid_for("claude", &foreign), &query)
            .is_err()
    );
}

#[test]
fn window_is_applied_after_semantic_parse_and_does_not_count_rename() {
    let (_temp, file, store, uid) = setup();
    let rows = (0..650)
        .map(|i| {
            claude_row(
                &format!("m{i}"),
                if i == 0 {
                    Value::Null
                } else {
                    json!(format!("m{}", i - 1))
                },
                "assistant",
                &format!("message {i}"),
            )
        })
        .collect::<Vec<_>>();
    write_rows(&file, &rows);
    let batch = store
        .messages(
            &uid,
            &MessageQuery {
                window: "1".to_owned(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(batch["messages"].as_array().unwrap().len(), 600);
    assert_eq!(batch["message_total"], 650);
    assert_eq!(
        batch["partial"],
        json!({"head": 100, "tail": 500, "omitted": 50})
    );
    assert_eq!(batch["messages"][100]["text"], "message 150");
    append(
        &file,
        b"{\"type\":\"custom-title\",\"customTitle\":\"Renamed\"}\n",
    );
    let renamed = store.messages(&uid, &continuation(&batch)).unwrap();
    assert_eq!(renamed["message_total"], 0);
    assert_eq!(renamed["messages"][0]["text"], "/rename Renamed");
}

#[test]
fn fixture_sources_parse_tools_turns_and_native_status() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let store = SessionStore::new(SessionRoots {
        claude: Some(fixtures.join("claude")),
        codex: Some(fixtures.join("codex")),
        grok: Some(fixtures.join("grok")),
    });
    let list = store.list(false).unwrap();
    let rows = list["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for row in rows {
        assert_eq!(row["supported"], true, "{:?}", row["migration_warnings"]);
        let batch = store
            .messages(row["uid"].as_str().unwrap(), &MessageQuery::default())
            .unwrap();
        assert!(
            batch["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "assistant")
        );
        assert!(
            batch["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "tool")
        );
        assert!(
            batch["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "tool_result")
        );
    }
}

#[test]
fn codex_question_result_across_cursor_keeps_call_identity() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("rollout.jsonl");
    write_rows(
        &path,
        &[
            json!({"type": "session_meta", "payload": {"id": "question", "cwd": "/workspace/demo"}}),
            json!({"type": "response_item", "payload": {"type": "function_call", "name": "request_user_input", "call_id": "q1",
              "turn_id": "turn-1", "arguments": {"questions": [{"question": "Pick one", "options": ["A", "B"]}]}}}),
        ],
    );
    let store = SessionStore::new(SessionRoots {
        codex: Some(temp.path().to_path_buf()),
        ..Default::default()
    });
    let uid = store.list(false).unwrap()["sessions"][0]["uid"]
        .as_str()
        .unwrap()
        .to_owned();
    let initial = store.messages(&uid, &MessageQuery::default()).unwrap();
    assert_eq!(initial["activity"]["state"], "waiting");
    append(
        &path,
        format!(
            "{}\n",
            json!({"type": "response_item", "payload": {"type": "function_call_output",
            "call_id": "q1", "output": {"answers": {"choice": {"answers": ["A"]}}}}})
        )
        .as_bytes(),
    );
    let delta = store.messages(&uid, &continuation(&initial)).unwrap();
    assert_eq!(delta["messages"][0]["role"], "answer");
    assert_eq!(delta["messages"][0]["text"], "A");
    assert_eq!(delta["messages"][0]["call_id"], "q1");
    assert_eq!(delta["activity"]["state"], "working");
}

#[test]
fn large_file_is_listed_and_range_open_has_no_size_quota() {
    let (temp, _file, store, uid) = setup();
    let before = store.list(false).unwrap();
    let oversized = temp.path().join("claude/project/oversized.jsonl");
    let large_size = 4 * 1024 * 1024 * 1024 + 1;
    fs::File::create(&oversized)
        .unwrap()
        .set_len(large_size)
        .unwrap();
    let list = store.list(true).unwrap();
    let rows = list["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "the list never fails on one file's size");
    let big = rows
        .iter()
        .find(|row| row["path"].as_str().unwrap().ends_with("oversized.jsonl"))
        .unwrap();
    assert_eq!(big["size"], large_size);
    let expected = stamp(&oversized).unwrap();
    assert!(
        native_input::CheckedNative::open_range(
            &temp.path().join("claude"),
            &oversized,
            &expected,
            0,
            expected.size
        )
        .is_ok()
    );
    // The other session is untouched.
    assert!(store.messages(&uid, &MessageQuery::default()).is_ok());
    fs::remove_file(&oversized).unwrap();
    let after = store.list(true).unwrap();
    assert_eq!(after["sig"], before["sig"]);
}

#[cfg(unix)]
#[test]
fn symlink_inputs_are_followed_like_python() {
    use std::os::unix::fs::symlink;
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("project")).unwrap();
    let outside = temp.path().join("outside.jsonl");
    write_rows(
        &outside,
        &[claude_row("u1", Value::Null, "user", "through link")],
    );
    symlink(outside, root.join("project/link.jsonl")).unwrap();
    let store = SessionStore::new(SessionRoots {
        claude: Some(root),
        ..Default::default()
    });
    let list = store.list(false).unwrap();
    assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
    let uid = list["sessions"][0]["uid"].as_str().unwrap();
    assert!(
        store.messages(uid, &MessageQuery::default()).unwrap()["messages"]
            .to_string()
            .contains("through link")
    );
}

#[cfg(unix)]
#[test]
fn cached_path_follows_a_replaced_parent_directory_like_python() {
    use std::os::unix::fs::symlink;
    let (temp, file, store, uid) = setup();
    let outside = temp.path().join("outside");
    write_rows(
        &outside.join("session.jsonl"),
        &[claude_row(
            "external",
            Value::Null,
            "user",
            "external replacement",
        )],
    );
    let project = file.parent().unwrap();
    fs::rename(project, temp.path().join("original-project")).unwrap();
    symlink(&outside, project).unwrap();
    let messages = store.messages(&uid, &MessageQuery::default()).unwrap();
    assert!(
        messages["messages"]
            .to_string()
            .contains("external replacement")
    );
}

#[cfg(unix)]
#[test]
fn cached_path_follows_a_replaced_final_file_like_python() {
    use std::os::unix::fs::symlink;
    let (temp, file, store, uid) = setup();
    let outside = temp.path().join("outside.jsonl");
    write_rows(
        &outside,
        &[claude_row("external", Value::Null, "user", "outside")],
    );
    fs::rename(&file, temp.path().join("original.jsonl")).unwrap();
    symlink(outside, &file).unwrap();
    assert!(
        store.messages(&uid, &MessageQuery::default()).unwrap()["messages"]
            .to_string()
            .contains("outside")
    );
}

#[test]
fn an_opened_file_must_match_the_expected_stamp_before_reading() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("session.jsonl");
    fs::write(&path, b"old\n").unwrap();
    let expected = stamp(&path).unwrap();
    fs::write(&path, b"a different file version\n").unwrap();
    assert_eq!(
        read_bounded(temp.path(), &path, &expected)
            .unwrap_err()
            .status,
        503
    );
}

#[test]
fn missing_fork_parent_is_reported_while_compact_and_unknown_media_are_readable() {
    let temp = TempDir::new().unwrap();
    let codex = temp.path().join("codex");
    let path = codex.join("rollout.jsonl");
    write_rows(
        &path,
        &[json!({"type": "session_meta", "payload": {
            "id": "fork", "history_base": {"thread_id": "parent", "end_byte_offset": 10},
        }})],
    );
    let store = SessionStore::new(SessionRoots {
        codex: Some(codex),
        ..Default::default()
    });
    let list = store.list(false).unwrap();
    assert_eq!(list["sessions"][0]["supported"], false);
    let uid = list["sessions"][0]["uid"].as_str().unwrap();
    assert_eq!(
        store
            .messages(uid, &MessageQuery::default())
            .unwrap_err()
            .status,
        501
    );

    let (_temp, file, store, uid) = setup();
    append(
        &file,
        b"{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
    );
    let compact = store.messages(&uid, &MessageQuery::default()).unwrap();
    assert!(
        compact["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["event_kind"] == "compact" && message["counted"] == false)
    );
    write_rows(
        &file,
        &[
            json!({"type": "user", "message": {"content": [{"type": "image", "source": {"type": "base64", "data": "synthetic"}}]}}),
        ],
    );
    let media = store.messages(&uid, &MessageQuery::default()).unwrap();
    assert_eq!(media["messages"], json!([]));
    assert_eq!(media["meta"]["supported"], true);
}

#[test]
fn codex_telemetry_duplicates_and_internal_context_do_not_become_user_bubbles() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("rollout.jsonl");
    write_rows(
        &path,
        &[
            json!({"type": "session_meta", "payload": {"id": "filtered"}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "internal instructions"}]}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hidden goal"}],
              "internal_chat_message_metadata_passthrough": {"content_item_kinds": ["goal.internal_context"]}}}),
            json!({"type": "event_msg", "payload": {"type": "user_message", "message": "visible question"}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "visible question"}]}}),
        ],
    );
    let store = SessionStore::new(SessionRoots {
        codex: Some(temp.path().to_path_buf()),
        ..Default::default()
    });
    let list = store.list(false).unwrap();
    assert_eq!(list["sessions"][0]["title"], "visible question");
    let batch = store
        .messages(
            list["sessions"][0]["uid"].as_str().unwrap(),
            &MessageQuery::default(),
        )
        .unwrap();
    assert_eq!(batch["message_total"], 1);
    assert_eq!(batch["messages"][0]["text"], "visible question");
}

#[test]
fn codex_missing_turn_identity_never_inherits_an_earlier_turn() {
    let temp = TempDir::new().unwrap();
    write_rows(
        &temp.path().join("rollout.jsonl"),
        &[
            json!({"type": "event_msg", "payload": {"type": "task_started", "turn_id": "old-turn"}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": "new input with no turn metadata"}}),
        ],
    );
    let store = SessionStore::new(SessionRoots {
        codex: Some(temp.path().to_path_buf()),
        ..Default::default()
    });
    let list = store.list(false).unwrap();
    let batch = store
        .messages(
            list["sessions"][0]["uid"].as_str().unwrap(),
            &MessageQuery::default(),
        )
        .unwrap();
    assert!(batch["messages"][0].get("turn_id").is_none());
    assert_eq!(batch["activity"]["turn_id"], "old-turn");
}

#[test]
fn tool_envelopes_unwrap_errors_without_unwrapping_business_json() {
    let temp = TempDir::new().unwrap();
    write_rows(
        &temp.path().join("rollout.jsonl"),
        &[
            json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "failed",
              "output": "Script completed\nOutput:\n{\"output\":\"failure details\",\"wall_time_seconds\":0.1,\"exit_code\":1}"}}),
            json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "business", "output": "{\"output\":\"business value\"}"}}),
            json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "text-error", "output": "command exited with code -1\nfailed"}}),
        ],
    );
    let store = SessionStore::new(SessionRoots {
        codex: Some(temp.path().to_path_buf()),
        ..Default::default()
    });
    let list = store.list(false).unwrap();
    let batch = store
        .messages(
            list["sessions"][0]["uid"].as_str().unwrap(),
            &MessageQuery::default(),
        )
        .unwrap();
    assert_eq!(batch["messages"][0]["text"], "failure details");
    assert_eq!(batch["messages"][0]["error"], true);
    assert_eq!(batch["messages"][0]["exit_code"], 1);
    assert_eq!(batch["messages"][0]["duration_s"], 0.1);
    assert_eq!(
        batch["messages"][1]["text"],
        "{\"output\":\"business value\"}"
    );
    assert_eq!(batch["messages"][2]["exit_code"], -1);
    assert_eq!(batch["messages"][2]["error"], true);
}

#[test]
fn grok_hidden_users_do_not_change_turn_identity_but_empty_synthetic_reason_is_visible() {
    let temp = TempDir::new().unwrap();
    let directory = temp.path().join("project/session");
    write_rows(
        &directory.join("chat_history.jsonl"),
        &[
            json!({"type": "user", "prompt_index": 1, "content": "visible first"}),
            json!({"type": "user", "prompt_index": 99, "content": "hidden user", "synthetic_reason": "internal"}),
            json!({"type": "assistant", "content": "answer first"}),
            json!({"type": "user", "prompt_index": 2, "content": "visible second", "synthetic_reason": ""}),
            json!({"type": "assistant", "content": "answer second"}),
        ],
    );
    fs::write(
        directory.join("summary.json"),
        "{\"info\":{\"id\":\"grok-turns\"}}",
    )
    .unwrap();
    let store = SessionStore::new(SessionRoots {
        grok: Some(temp.path().to_path_buf()),
        ..Default::default()
    });
    let list = store.list(false).unwrap();
    let batch = store
        .messages(
            list["sessions"][0]["uid"].as_str().unwrap(),
            &MessageQuery::default(),
        )
        .unwrap();
    assert_eq!(batch["message_total"], 4);
    assert_eq!(batch["messages"][1]["turn_id"], "prompt:1");
    assert_eq!(batch["messages"][2]["text"], "visible second");
    assert_eq!(batch["messages"][3]["turn_id"], "prompt:2");
}

#[test]
fn claude_content_shapes_preserve_turn_phase_and_ignore_empty_thinking() {
    let (_temp, file, store, uid) = setup();
    write_rows(
        &file,
        &[
            json!({"type": "user", "uuid": "u1", "parentUuid": null, "message": {"content": {"type": "text", "text": "object input"}}}),
            json!({"type": "assistant", "uuid": "a1", "parentUuid": "u1", "message": {
                "stop_reason": "end_turn", "content": [{"type": "thinking", "thinking": "  "}, "string answer"],
            }}),
        ],
    );
    let batch = store.messages(&uid, &MessageQuery::default()).unwrap();
    assert_eq!(batch["message_total"], 2);
    assert_eq!(batch["messages"][0]["turn_id"], "u1");
    assert_eq!(batch["messages"][1]["turn_id"], "u1");
    assert_eq!(batch["messages"][1]["phase"], "final");
    assert_eq!(batch["activity"]["state"], "working");
}

#[test]
fn persisted_timeline_pin_equals_pure_options_resets_cursors_and_retires_explicitly() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("claude");
    let file = root.join("project/session.jsonl");
    let rows = [
        claude_row("u1", Value::Null, "user", "第一问"),
        claude_row("a1", json!("u1"), "assistant", "第一答"),
        claude_row("u2", json!("a1"), "user", "第二问"),
        claude_row("a2", json!("u2"), "assistant", "第二答"),
        claude_row("u3", json!("a2"), "user", "第三问"),
        claude_row("a3", json!("u3"), "assistant", "第三答"),
    ];
    write_rows(&file, &rows);
    let state = temp.path().join("state");
    fs::create_dir(&state).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let metadata = Arc::new(crate::metadata::MetadataStore::open(&state).unwrap());
    let store = SessionStore::with_metadata(
        SessionRoots {
            claude: Some(root),
            ..Default::default()
        },
        Some(metadata.clone()),
    );
    let uid = store.list(false).unwrap()["sessions"][0]["uid"]
        .as_str()
        .unwrap()
        .to_owned();
    let texts = |batch: &Value| {
        batch["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["text"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let natural = store.messages(&uid, &MessageQuery::default()).unwrap();
    assert_eq!(texts(&natural).len(), 6);
    assert!(natural["meta"].get("timeline_pin").is_none());

    // Rewind the display to before 第三问: the tip is its parent a2.
    let target = store.claude_rewind_target(&uid, "u3").unwrap();
    assert_eq!(target.tip, "a2");
    assert_eq!(target.stale_end, fs::metadata(&file).unwrap().len());
    assert_eq!(
        store.claude_rewind_target(&uid, "u1").unwrap_err().status,
        409
    );
    assert_eq!(
        store
            .claude_rewind_target(&uid, "never")
            .unwrap_err()
            .status,
        404
    );
    metadata
        .set_timeline_pin(
            &uid,
            crate::metadata::TimelinePin {
                tip: target.tip.clone(),
                stale_end: target.stale_end,
                target: Some("u3".into()),
                pinned_at: Some(1.0),
            },
        )
        .unwrap();
    // A stale checkpoint from the unpinned view resets instead of diffing,
    // and the pinned view equals the pure parser options on the same records.
    let pinned = store.messages(&uid, &continuation(&natural)).unwrap();
    assert_eq!(pinned["reset"], true);
    assert_eq!(texts(&pinned), ["第一问", "第一答", "第二问", "第二答"]);
    assert_eq!(pinned["meta"]["timeline_pin"]["tip"], "a2");
    assert_eq!(pinned["meta"]["timeline_pin"]["retired"], false);
    assert_eq!(pinned["meta"]["timeline_pin"]["native_rewind"], false);
    let list = store.list(true).unwrap();
    assert_eq!(list["sessions"][0]["timeline_pin"]["target"], "u3");
    assert_eq!(list["sessions"][0]["cursor"], pinned["meta"]["cursor"]);
    let bytes = fs::read(&file).unwrap();
    let mut decoder = records::Decoder::cold();
    records::scan_records(&bytes[..], &mut decoder, None).unwrap();
    let records = decoder.finish(bytes.len()).records;
    let (_, pure, error) = providers::parse_with_options(
        "claude",
        &file,
        &records,
        None,
        "",
        providers::ParseOptions {
            declared_tip: Some("a2"),
            abandoned_after: target.stale_end,
            ..Default::default()
        },
    );
    assert!(error.is_none());
    assert_eq!(
        pure.iter()
            .filter(|event| event.message["role"] != "status")
            .map(|event| event.message.clone())
            .collect::<Vec<_>>(),
        pinned["messages"].as_array().unwrap().clone()
    );
    // The pinned checkpoint is stable while nothing changes.
    let idle = store.messages(&uid, &continuation(&pinned)).unwrap();
    assert_eq!(idle["reset"], false);
    assert_eq!(idle["messages"], json!([]));
    // Native records past the boundary retire the pin explicitly: the CLI
    // continued from a3, so it never rewound.
    let appended = [
        claude_row("u4", json!("a3"), "user", "第四问"),
        claude_row("a4", json!("u4"), "assistant", "第四答"),
    ];
    for row in &appended {
        append(&file, format!("{row}\n").as_bytes());
    }
    let retired = store.messages(&uid, &continuation(&pinned)).unwrap();
    assert_eq!(retired["reset"], true);
    assert_eq!(texts(&retired).len(), 8);
    assert_eq!(retired["meta"]["timeline_pin"]["retired"], true);
    assert_eq!(
        retired["meta"]["timeline_pin"]["retired_reason"],
        "native_advanced"
    );
    assert!(
        retired["meta"]["timeline_pin"]["retired_message"]
            .as_str()
            .unwrap()
            .contains("CLI 未回滚")
    );
    let list = store.list(true).unwrap();
    assert_eq!(
        list["sessions"][0]["timeline_pin"]["retired_reason"],
        "native_advanced"
    );
    // Clearing the retired pin is again a logical view change (reset), and the
    // native file is byte-identical to what was appended, never rewritten.
    metadata.clear_timeline_pin(&uid).unwrap();
    let cleared = store.messages(&uid, &continuation(&retired)).unwrap();
    assert_eq!(cleared["reset"], true);
    assert!(cleared["meta"].get("timeline_pin").is_none());
    assert_eq!(texts(&cleared).len(), 8);
    let mut expected = Vec::new();
    for row in rows.iter().chain(appended.iter()) {
        expected.extend_from_slice(format!("{row}\n").as_bytes());
    }
    assert_eq!(fs::read(&file).unwrap(), expected);
}

/// Over the published list: the registry
/// beside the metadata hides registered runs from the ordinary view, a
/// `debug_run` view shows exactly that run, the view is re-signed and fork
/// parents are re-derived among the visible rows. Rows carry
/// `migration_warnings` only when unsupported; the detail keeps them.
#[test]
fn list_view_applies_the_debug_run_registry_beside_the_metadata() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("claude");
    let session = |name: &str, cwd: &str| {
        write_rows(
            &root.join(format!("project/{name}.jsonl")),
            &[
                json!({"type": "user", "uuid": "u1", "parentUuid": null, "sessionId": name,
                       "cwd": cwd, "timestamp": "2026-09-11T10:00:00.123Z",
                       "message": {"role": "user", "content": format!("question in {name}")}}),
                json!({"type": "assistant", "uuid": "a1", "parentUuid": "u1", "sessionId": name,
                       "cwd": cwd, "timestamp": "2026-09-11T10:00:01.123Z",
                       "message": {"role": "assistant", "content": "answer", "stop_reason": "end_turn"}}),
                json!({"type": "future-kind", "uuid": "x1", "parentUuid": "a1", "sessionId": name,
                       "timestamp": "2026-09-11T10:00:02.123Z"}),
            ],
        );
        uid_for("claude", &root.join(format!("project/{name}.jsonl")))
    };
    let ordinary = session("ordinary", "/home/user/work");
    let by_root = session("by-root", "/tmp/monkey-run-1/claude/session-01");
    let by_sid = session("by-sid", "/home/user/elsewhere");
    let state = temp.path().join("state");
    fs::create_dir(&state).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(
        state.join(DEBUG_RUNS_FILENAME),
        json!({"version": 1, "runs": {
            "run-1": {"root": "/tmp/monkey-run-1", "created": 1.0, "sessions": []},
            "run-2": {"root": "/tmp/monkey-run-2", "created": 2.0, "sessions": [
                {"source": "claude", "cwd": "/home/user/elsewhere", "sid": "by-sid", "uid": "", "name": ""}
            ]}
        }})
        .to_string(),
    )
    .unwrap();
    let metadata = Arc::new(crate::metadata::MetadataStore::open(&state).unwrap());
    let store = SessionStore::with_metadata(
        SessionRoots {
            claude: Some(root.clone()),
            ..Default::default()
        },
        Some(metadata),
    );
    assert_eq!(
        store.debug_runs_path().unwrap(),
        state.canonicalize().unwrap().join(DEBUG_RUNS_FILENAME)
    );
    let uids = |document: &Value| {
        document["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["uid"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    // The internal list is unfiltered; the view hides both registered runs.
    assert_eq!(
        store.list(false).unwrap()["sessions"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let default = store.list_view(false, "").unwrap();
    assert_eq!(uids(&default), std::slice::from_ref(&ordinary));
    assert!(default["sig"].is_string() && default["built_at"].is_number());
    let again = store.list_view(false, "").unwrap();
    assert_eq!(
        again["sig"], default["sig"],
        "the view is re-signed deterministically"
    );
    assert_ne!(
        default["sig"],
        store.list(false).unwrap()["sig"],
        "a filtered view never shares the unfiltered signature"
    );
    let run_1 = store.list_view(false, "run-1").unwrap();
    assert_eq!(uids(&run_1), std::slice::from_ref(&by_root));
    let run_2 = store.list_view(false, "run-2").unwrap();
    assert_eq!(uids(&run_2), std::slice::from_ref(&by_sid));
    assert_ne!(run_1["sig"], run_2["sig"]);
    assert!(uids(&store.list_view(false, "unknown").unwrap()).is_empty());
    assert!(uids(&store.list_view(false, "bad id!").unwrap()).is_empty());
    // Supported rows carry no `migration_warnings`; the
    // detail `meta` keeps the non-fatal notes.
    assert!(default["sessions"][0].get("migration_warnings").is_none());
    assert_eq!(default["sessions"][0]["supported"], true);
    // Stripping must not conjure `agent_items: null` on agent-less rows.
    assert!(default["sessions"][0].get("agent_items").is_none());
    let detail = store.messages(&ordinary, &MessageQuery::default()).unwrap();
    assert_eq!(
        detail["meta"]["migration_warnings"],
        json!(["跳过未知的Claude 记录类型：future-kind ×1"])
    );
    let pool = store.search_pool_view("run-1").unwrap();
    assert_eq!(pool.rows.len(), 1);
    assert_eq!(pool.rows[0]["uid"], by_root);
    assert!(pool.rows[0].get("migration_warnings").is_none());
    assert_eq!(store.search_pool().unwrap().rows.len(), 1);
    // Removing the registry (the file, not the service) shows everything.
    fs::remove_file(state.join(DEBUG_RUNS_FILENAME)).unwrap();
    assert_eq!(uids(&store.list_view(false, "").unwrap()).len(), 3);
    assert!(uids(&store.list_view(false, "run-1").unwrap()).is_empty());
}

/// The serialized list body is a pure function of the view document and
/// the cached-view set: a hot request shares the previous buffer, every
/// change that alters the bytes turns the entry over, `force=1` re-renders
/// and the sig short-circuit still comes first.
#[test]
fn list_bytes_are_cached_per_document_and_view_revision() {
    let (_temp, file, store, uid) = setup();
    let same_buffer = |a: &Bytes, b: &Bytes| a.as_ptr() == b.as_ptr() && a.len() == b.len();
    let uncached = |force: bool, run: &str, sig: &str| {
        serde_json::to_vec(&store.list_view_unless(force, run, sig).unwrap()).unwrap()
    };
    let first = store.list_view_bytes(false, "", "").unwrap();
    assert_eq!(first.as_ref(), uncached(false, "", ""));
    let second = store.list_view_bytes(false, "", "stale-sig").unwrap();
    assert!(
        same_buffer(&first, &second),
        "a hot request with another sig is served from the cache"
    );
    let document: Value = serde_json::from_slice(&second).unwrap();
    let sig = document["sig"].as_str().unwrap().to_owned();
    assert_eq!(
        store.list_view_bytes(false, "", &sig).unwrap().as_ref(),
        serde_json::to_vec(&json!({"unchanged": true, "sig": sig})).unwrap(),
        "the sig short-circuit comes before the byte cache"
    );
    // `force=1` rescans and re-renders (a fresh buffer) — the same bytes
    // while nothing changed — and the entry is replaced by that render.
    let forced = store.list_view_bytes(true, "", "").unwrap();
    assert!(!same_buffer(&first, &forced));
    assert_eq!(forced, first);
    assert!(same_buffer(
        &forced,
        &store.list_view_bytes(false, "", "").unwrap()
    ));
    // Opening a session changes only the decorations the list borrows
    // (`cursor.anchor`) and no sig; the entry still turns over, and the
    // cached and uncached renders stay byte-identical.
    let revision = store.views_revision();
    let batch = store.messages(&uid, &MessageQuery::default()).unwrap();
    assert_ne!(
        store.views_revision(),
        revision,
        "a cached view was inserted"
    );
    let opened = store.list_view_bytes(false, "", "").unwrap();
    assert!(!same_buffer(&forced, &opened));
    assert_eq!(opened.as_ref(), uncached(false, "", ""));
    let document: Value = serde_json::from_slice(&opened).unwrap();
    assert_eq!(
        document["sessions"][0]["cursor"]["anchor"],
        batch["meta"]["cursor"]["anchor"]
    );
    assert_eq!(document["sig"], sig, "opening never changes the signature");
    assert!(same_buffer(
        &opened,
        &store.list_view_bytes(false, "", "").unwrap()
    ));
    // Re-opening an unchanged session keeps the same view snapshot: the
    // revision, and so the entry, are untouched.
    let revision = store.views_revision();
    store.messages(&uid, &MessageQuery::default()).unwrap();
    assert_eq!(store.views_revision(), revision);
    assert!(same_buffer(
        &opened,
        &store.list_view_bytes(false, "", "").unwrap()
    ));
    // An append behind the server's back: the forced rescan publishes a
    // new document (new sig) and the bytes follow it.
    append(
        &file,
        format!(
            "{}\n",
            claude_row("a2", json!("a1"), "assistant", "追加一行")
        )
        .as_bytes(),
    );
    let appended = store.list_view_bytes(true, "", &sig).unwrap();
    assert!(!same_buffer(&opened, &appended));
    let document: Value = serde_json::from_slice(&appended).unwrap();
    assert_ne!(document["sig"], sig);
    assert_eq!(
        document["sessions"][0]["size"],
        fs::metadata(&file).unwrap().len()
    );
    assert_eq!(appended.as_ref(), uncached(false, "", ""));
    assert!(same_buffer(
        &appended,
        &store.list_view_bytes(false, "", "").unwrap()
    ));
}

/// Each debug-run view keeps its own entry; the registry file changing
/// turns every view over.
#[test]
fn list_bytes_keep_one_entry_per_debug_run_view() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("claude");
    for (name, cwd) in [
        ("ordinary", "/home/user/work"),
        ("by-root", "/tmp/monkey-run-1/x"),
    ] {
        write_rows(
            &root.join(format!("project/{name}.jsonl")),
            &[
                claude_row("u1", Value::Null, "user", "q"),
                json!({"type": "user", "uuid": "u2", "parentUuid": "u1", "sessionId": name,
                       "cwd": cwd, "timestamp": "2026-09-11T10:00:01.123Z",
                       "message": {"role": "user", "content": "again"}}),
            ],
        );
    }
    let state = temp.path().join("state");
    fs::create_dir(&state).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let registry = json!({"version": 1, "runs": {
        "run-1": {"root": "/tmp/monkey-run-1", "created": 1.0, "sessions": []}
    }});
    fs::write(state.join(DEBUG_RUNS_FILENAME), registry.to_string()).unwrap();
    let metadata = Arc::new(crate::metadata::MetadataStore::open(&state).unwrap());
    let store = SessionStore::with_metadata(
        SessionRoots {
            claude: Some(root),
            ..Default::default()
        },
        Some(metadata),
    );
    let same_buffer = |a: &Bytes, b: &Bytes| a.as_ptr() == b.as_ptr() && a.len() == b.len();
    let default = store.list_view_bytes(false, "", "").unwrap();
    let run = store.list_view_bytes(false, "run-1", "").unwrap();
    assert_ne!(default, run);
    assert_eq!(
        default.as_ref(),
        serde_json::to_vec(&store.list_view_unless(false, "", "").unwrap()).unwrap()
    );
    assert_eq!(
        run.as_ref(),
        serde_json::to_vec(&store.list_view_unless(false, "run-1", "").unwrap()).unwrap()
    );
    // Alternating views hit their own entries.
    assert!(same_buffer(
        &default,
        &store.list_view_bytes(false, "", "").unwrap()
    ));
    assert!(same_buffer(
        &run,
        &store.list_view_bytes(false, "run-1", "").unwrap()
    ));
    // A rewritten registry (another stamp) re-filters both views.
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(
        state.join(DEBUG_RUNS_FILENAME),
        json!({"version": 1, "runs": {}}).to_string(),
    )
    .unwrap();
    let cleared = store.list_view_bytes(false, "", "").unwrap();
    assert!(!same_buffer(&default, &cleared));
    let document: Value = serde_json::from_slice(&cleared).unwrap();
    assert_eq!(document["sessions"].as_array().unwrap().len(), 2);
    assert!(!same_buffer(
        &run,
        &store.list_view_bytes(false, "run-1", "").unwrap()
    ));
}
