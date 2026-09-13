use super::*;
use std::io::Write;
use tempfile::TempDir;

fn write(path: &Path, records: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes: Vec<_> = records
        .iter()
        .flat_map(|record| {
            serde_json::to_vec(record)
                .unwrap()
                .into_iter()
                .chain(*b"\n")
        })
        .collect();
    fs::write(path, bytes).unwrap();
}

fn claude(sid: Option<&str>, agent: Option<&str>, uuid: &str) -> Value {
    let mut row = json!({"type":"user", "uuid":uuid,"parentUuid":null,
        "timestamp":"2026-09-12T00:00:00Z", "message":{"role":"user","content":"Synthetic scope message"}});
    if let Some(sid) = sid {
        row["sessionId"] = json!(sid);
    }
    if let Some(agent) = agent {
        row["agentId"] = json!(agent);
        row["isSidechain"] = json!(true);
    }
    row
}

fn fixture() -> (TempDir, SessionStore, PathBuf, PathBuf, String) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("claude");
    let parent = root.join("project/filename.with.dots.jsonl");
    let child = root.join("project/filename.with.dots/subagents/agent-full-agent-id.jsonl");
    write(
        &parent,
        &[claude(Some("real-native-session"), None, "root-message")],
    );
    write(
        &child,
        &[claude(
            Some("real-native-session"),
            Some("full-agent-id"),
            "child-message",
        )],
    );
    let uid = uid_for("claude", &parent);
    let store = SessionStore::new(SessionRoots {
        claude: Some(root),
        ..Default::default()
    });
    (temp, store, parent, child, uid)
}

#[test]
fn claude_native_scope_uses_owner_identity_and_preserves_agent_display_and_cursor() {
    let (_temp, store, _parent, _child, uid) = fixture();
    let main = store.native_scope(&uid, "").unwrap();
    assert_eq!(
        main,
        NativeScope {
            source: "claude".into(),
            uid: uid.clone(),
            session_id: "real-native-session".into(),
            agent_id: None
        }
    );
    let before = store.snapshot(&uid, "full-agent-id").unwrap();
    let scope = store.native_scope(&uid, "full-agent-id").unwrap();
    assert_eq!(
        scope,
        NativeScope {
            source: "claude".into(),
            uid: uid.clone(),
            session_id: "real-native-session".into(),
            agent_id: Some("full-agent-id".into())
        }
    );
    let after = store.snapshot(&uid, "full-agent-id").unwrap();
    assert!(
        Arc::ptr_eq(&before, &after),
        "scope reads must reuse the validated immutable view"
    );
    assert_eq!(before.cursor(), after.cursor());
    assert_eq!(after.metadata()["sid"], "full-agent-id");
    assert_eq!(after.metadata()["uid"], uid);
    assert_eq!(
        after.messages(&MessageQuery::default()).unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(after.metadata().get("native_scope").is_none());
}

#[test]
fn missing_native_ids_and_filename_guesses_fail_only_scope_reads() {
    let (_temp, store, parent, child, uid) = fixture();
    write(&parent, &[claude(None, None, "root-message")]);
    assert_eq!(store.native_scope(&uid, "").unwrap_err().status, 501);
    assert_eq!(
        store
            .native_scope(&uid, "full-agent-id")
            .unwrap_err()
            .status,
        501
    );
    assert_eq!(
        store.snapshot(&uid, "").unwrap().metadata()["sid"],
        "filename.with.dots"
    );
    assert!(
        store
            .messages(
                &uid,
                &MessageQuery {
                    agent: "full-agent-id".into(),
                    ..Default::default()
                }
            )
            .is_ok()
    );
    write(
        &parent,
        &[claude(Some("real-native-session"), None, "root-message")],
    );
    write(
        &child,
        &[claude(None, Some("full-agent-id"), "child-message")],
    );
    assert_eq!(
        store
            .native_scope(&uid, "full-agent-id")
            .unwrap_err()
            .status,
        501
    );
    assert_eq!(
        store.snapshot(&uid, "full-agent-id").unwrap().metadata()["sid"],
        "full-agent-id"
    );
}

#[test]
fn conflicting_or_mismatched_native_ids_fail_without_hiding_readable_history() {
    let (_temp, store, parent, child, uid) = fixture();
    write(
        &child,
        &[claude(
            Some("different-parent-session"),
            Some("full-agent-id"),
            "child-message",
        )],
    );
    assert_eq!(
        store
            .native_scope(&uid, "full-agent-id")
            .unwrap_err()
            .status,
        409
    );
    assert!(
        store
            .messages(
                &uid,
                &MessageQuery {
                    agent: "full-agent-id".into(),
                    ..Default::default()
                }
            )
            .is_ok()
    );
    write(
        &child,
        &[
            claude(
                Some("real-native-session"),
                Some("full-agent-id"),
                "child-one",
            ),
            claude(Some("other-session"), Some("full-agent-id"), "child-two"),
        ],
    );
    assert_eq!(
        store
            .native_scope(&uid, "full-agent-id")
            .unwrap_err()
            .status,
        409
    );
    write(
        &parent,
        &[
            claude(Some("real-native-session"), None, "root-one"),
            claude(Some("other-session"), None, "root-two"),
        ],
    );
    assert_eq!(store.native_scope(&uid, "").unwrap_err().status, 409);
    assert_eq!(
        store.snapshot(&uid, "").unwrap().metadata()["sid"],
        "real-native-session"
    );
    assert!(store.list(true).is_ok());
}

#[test]
fn exact_inventory_ownership_is_required_without_agent_path_or_prefix_fallback() {
    let (_temp, store, parent, child, uid) = fixture();
    for invalid in [
        "full-agent",
        "agent-full-agent-id",
        "../full-agent-id",
        "filename.with.dots",
    ] {
        assert_eq!(store.native_scope(&uid, invalid).unwrap_err().status, 404);
    }
    let other = parent.with_file_name("other.jsonl");
    write(
        &other,
        &[claude(Some("other-native-session"), None, "other")],
    );
    assert_eq!(
        store
            .native_scope(&uid_for("claude", &other), "full-agent-id")
            .unwrap_err()
            .status,
        404
    );
    assert_eq!(
        store
            .native_scope(&uid_for("claude", &child), "")
            .unwrap_err()
            .status,
        404
    );
    assert_eq!(
        store.native_scope("claude:unknown", "").unwrap_err().status,
        404
    );
}

#[test]
fn scope_rechecks_native_provenance_and_keeps_previous_snapshot_immutable() {
    let (_temp, store, parent, child, uid) = fixture();
    let before = store.snapshot(&uid, "full-agent-id").unwrap();
    assert!(before.view.native_scope.is_ok());
    write(
        &child,
        &[claude(
            Some("next-native-session"),
            Some("full-agent-id"),
            "child-message",
        )],
    );
    assert_eq!(
        store
            .native_scope(&uid, "full-agent-id")
            .unwrap_err()
            .status,
        409
    );
    write(
        &parent,
        &[claude(Some("next-native-session"), None, "root-message")],
    );
    assert_eq!(
        store
            .native_scope(&uid, "full-agent-id")
            .unwrap()
            .session_id,
        "next-native-session"
    );
    assert_eq!(
        before.view.native_scope.as_ref().unwrap().session_id,
        "real-native-session"
    );
    let decoded = store.views().unwrap().records().decoded;
    store.native_scope(&uid, "full-agent-id").unwrap();
    assert_eq!(
        store.views().unwrap().records().decoded,
        decoded,
        "an unchanged owner is not re-streamed for the agent's scope"
    );
}

#[test]
fn incomplete_native_identity_is_not_guessed_and_string_values_follow_python() {
    let (_temp, store, parent, _child, uid) = fixture();
    let row = claude(Some("real-native-session"), None, "root-message");
    let bytes = serde_json::to_vec(&row).unwrap();
    fs::write(&parent, &bytes).unwrap();
    assert_eq!(store.native_scope(&uid, "").unwrap_err().status, 501);
    fs::OpenOptions::new()
        .append(true)
        .open(&parent)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert_eq!(
        store.native_scope(&uid, "").unwrap().session_id,
        "real-native-session"
    );
    for invalid in [json!(null), json!(42), json!("")] {
        let mut row = row.clone();
        row["sessionId"] = invalid;
        write(&parent, &[row]);
        assert_eq!(store.native_scope(&uid, "").unwrap_err().status, 501);
        assert!(store.messages(&uid, &MessageQuery::default()).is_ok());
    }
    for value in ["a\nb".to_owned(), "x".repeat(257)] {
        let mut row = row.clone();
        row["sessionId"] = json!(value);
        write(&parent, &[row]);
        assert_eq!(store.native_scope(&uid, "").unwrap().session_id, value);
    }
}

#[test]
fn codex_uses_real_session_meta_id_and_validates_children_separately() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("codex");
    let parent = root.join("root-filename.jsonl");
    let child = root.join("child-filename.jsonl");
    write(
        &parent,
        &[
            json!({"type":"session_meta","payload":{"id":"native-thread","session_id":"display-alias"}}),
        ],
    );
    let store = SessionStore::new(SessionRoots {
        codex: Some(root),
        ..Default::default()
    });
    let uid = uid_for("codex", &parent);
    let scope = store.native_scope(&uid, "").unwrap();
    assert_eq!(scope.source, "codex");
    assert_eq!(scope.uid, uid);
    assert_eq!(scope.session_id, "native-thread");
    assert_eq!(scope.agent_id, None);
    assert_eq!(
        store.snapshot(&uid, "").unwrap().metadata()["sid"],
        "display-alias"
    );
    write(
        &child,
        &[
            json!({"type":"session_meta","payload":{"id":"child-thread","thread_source":"subagent","parent_thread_id":"display-alias"}}),
        ],
    );
    let scope = store.native_scope(&uid, "child-thread").unwrap();
    assert_eq!(scope.session_id, "child-thread");
    assert_eq!(scope.agent_id, Some("child-thread".into()));
    assert_eq!(scope.uid, uid);
    write(
        &parent,
        &[json!({"type":"session_meta","payload":{"session_id":"display-alias"}})],
    );
    assert_eq!(store.native_scope(&uid, "").unwrap_err().status, 501);
    assert!(store.messages(&uid, &MessageQuery::default()).is_ok());
}

#[test]
fn grok_native_scope_is_explicitly_unsupported_without_affecting_history() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("grok");
    let path = root.join("project/session");
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("summary.json"), b"{}").unwrap();
    let store = SessionStore::new(SessionRoots {
        grok: Some(root),
        ..Default::default()
    });
    let uid = uid_for("grok", &path);
    assert_eq!(store.native_scope(&uid, "").unwrap_err().status, 501);
    assert!(store.messages(&uid, &MessageQuery::default()).is_ok());
}
