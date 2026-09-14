use super::*;
use crate::runtime::{AssociationReason, NativeCatalog};
use tempfile::TempDir;

fn write(path: &Path, records: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        records
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
}
fn codex(id: &str, alias: &str) -> Value {
    json!({"type":"session_meta","payload":{"id":id,"session_id":alias}})
}
fn claude(id: &str) -> Value {
    json!({"type":"user","sessionId":id,"uuid":"synthetic-user","parentUuid":null,
        "message":{"role":"user","content":"synthetic input"}})
}

#[test]
fn catalog_uses_actual_native_ids_without_changing_display_aliases() {
    let temp = TempDir::new().unwrap();
    let codex_path = temp.path().join("codex/filename-label.jsonl");
    let claude_path = temp.path().join("claude/project/filename-label.jsonl");
    write(
        &codex_path,
        &[codex("actual-codex-thread", "display-codex-alias")],
    );
    write(&claude_path, &[claude("actual-claude-session")]);
    let store = SessionStore::new(SessionRoots {
        codex: Some(temp.path().join("codex")),
        claude: Some(temp.path().join("claude")),
        ..Default::default()
    });
    let snapshot = store.search_snapshot().unwrap();
    let before = snapshot.list.clone();
    let catalog = snapshot.native_catalog();
    for (source, path, id) in [
        ("codex", &codex_path, "actual-codex-thread"),
        ("claude", &claude_path, "actual-claude-session"),
    ] {
        let uid = uid_for(source, path);
        assert_eq!(
            catalog.verified_scope(&uid).unwrap(),
            store.native_scope(&uid, "").unwrap()
        );
        assert_eq!(catalog.verified_scope(&uid).unwrap().session_id, id);
    }
    let uid = uid_for("codex", &codex_path);
    assert_eq!(
        store.snapshot(&uid, "").unwrap().metadata()["sid"],
        "display-codex-alias"
    );
    assert_eq!(snapshot.list, before);
    assert_eq!(
        NativeCatalog::from_rows(before["sessions"].as_array().unwrap()).verified_scope(&uid),
        Err(AssociationReason::UnverifiedCatalog)
    );
}

/// A Codex file without `payload.id` stays unsupported; a Grok directory's
/// `summary.json` `info.id` is its verified identity.
#[test]
fn missing_identity_is_rejected_and_grok_summary_id_is_the_scope() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("codex/filename-guessed-id.jsonl");
    write(
        &missing,
        &[json!({"type":"session_meta","payload":{"session_id":"display-alias"}})],
    );
    let grok = temp.path().join("grok/project/synthetic-session");
    fs::create_dir_all(&grok).unwrap();
    fs::write(
        grok.join("summary.json"),
        json!({"info":{"id":"summary-native-looking-id"}}).to_string(),
    )
    .unwrap();
    let store = SessionStore::new(SessionRoots {
        codex: Some(temp.path().join("codex")),
        grok: Some(temp.path().join("grok")),
        ..Default::default()
    });
    let catalog = store.native_catalog().unwrap();
    let uid = uid_for("codex", &missing);
    assert!(store.messages(&uid, &MessageQuery::default()).is_ok());
    assert_eq!(
        catalog.verified_scope(&uid),
        Err(AssociationReason::UnsupportedNative)
    );
    let uid = uid_for("grok", &grok);
    assert!(store.messages(&uid, &MessageQuery::default()).is_ok());
    let scope = catalog.verified_scope(&uid).unwrap();
    assert_eq!(scope.session_id, "summary-native-looking-id");
    assert_eq!(store.native_scope(&uid, "").unwrap(), scope);
    assert_eq!(
        catalog.verified_scope("codex:missing"),
        Err(AssociationReason::NativeMissing)
    );
}

#[test]
fn duplicate_and_conflicting_files_cannot_be_filtered_into_a_unique_native_identity() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("codex");
    let good = root.join("good.jsonl");
    let conflict = root.join("conflict.jsonl");
    write(&good, &[codex("shared-real-id", "good-display")]);
    // A Codex file's identity is its FIRST session_meta only;
    // the copied second meta is not a declaration, so the
    // two files collide on `shared-real-id` and neither is unique.
    write(
        &conflict,
        &[
            codex("shared-real-id", "conflict-display"),
            codex("other-real-id", "other-display"),
        ],
    );
    let store = SessionStore::new(SessionRoots {
        codex: Some(root.clone()),
        ..Default::default()
    });
    let original = store.native_catalog().unwrap();
    let uid = uid_for("codex", &good);
    assert_eq!(
        original.verified_scope(&uid),
        Err(AssociationReason::NativeAmbiguous)
    );
    assert_eq!(
        original.verified_scope(&uid_for("codex", &conflict)),
        Err(AssociationReason::NativeAmbiguous)
    );
    fs::remove_file(&conflict).unwrap(); // synthetic fixture only
    assert_eq!(
        store
            .native_catalog()
            .unwrap()
            .verified_scope(&uid)
            .unwrap()
            .session_id,
        "shared-real-id"
    );
    assert_eq!(
        original.verified_scope(&uid),
        Err(AssociationReason::NativeAmbiguous)
    );
    write(&conflict, &[codex("shared-real-id", "different-display")]);
    assert_eq!(
        store.native_catalog().unwrap().verified_scope(&uid),
        Err(AssociationReason::NativeAmbiguous)
    );
    // Even an unsupported transcript retains its already parsed declaration.
    fs::write(
        &conflict,
        format!("{}\nnot-json\n", codex("shared-real-id", "bad-display")),
    )
    .unwrap();
    assert_eq!(
        store.native_catalog().unwrap().verified_scope(&uid),
        Err(AssociationReason::NativeAmbiguous)
    );
}

#[test]
fn agents_are_rejected_without_making_their_legitimate_parent_ambiguous() {
    let temp = TempDir::new().unwrap();
    let claude_parent = temp.path().join("claude/project/parent-file.jsonl");
    let claude_child = temp
        .path()
        .join("claude/project/parent-file/subagents/agent-exact-child.jsonl");
    write(&claude_parent, &[claude("parent-session")]);
    let mut child = claude("parent-session");
    child["agentId"] = json!("exact-child");
    child["isSidechain"] = json!(true);
    write(&claude_child, &[child]);
    let codex_parent = temp.path().join("codex/parent-file.jsonl");
    let codex_child = temp.path().join("codex/child-file.jsonl");
    write(&codex_parent, &[codex("parent-thread", "display-parent")]);
    write(
        &codex_child,
        &[
            json!({"type":"session_meta","payload":{"id":"child-thread","thread_source":"subagent","parent_thread_id":"display-parent"}}),
        ],
    );
    let store = SessionStore::new(SessionRoots {
        claude: Some(temp.path().join("claude")),
        codex: Some(temp.path().join("codex")),
        ..Default::default()
    });
    let catalog = store.native_catalog().unwrap();
    for (source, parent, child) in [
        ("claude", &claude_parent, &claude_child),
        ("codex", &codex_parent, &codex_child),
    ] {
        assert!(catalog.verified_scope(&uid_for(source, parent)).is_ok());
        assert_eq!(
            catalog.verified_scope(&uid_for(source, child)),
            Err(AssociationReason::Subagent)
        );
    }
}

#[test]
fn catalog_refreshes_provenance_while_an_existing_snapshot_stays_immutable() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("codex/file.jsonl");
    write(&path, &[codex("native-original", "display-stable")]);
    let store = SessionStore::new(SessionRoots {
        codex: Some(temp.path().join("codex")),
        ..Default::default()
    });
    let uid = uid_for("codex", &path);
    let snapshot = store.search_snapshot().unwrap();
    fs::remove_file(&path).unwrap();
    write(&path, &[codex("native-replaced", "display-stable")]);
    assert_eq!(
        store
            .native_catalog()
            .unwrap()
            .verified_scope(&uid)
            .unwrap()
            .session_id,
        "native-replaced"
    );
    assert_eq!(
        snapshot
            .native_catalog()
            .verified_scope(&uid)
            .unwrap()
            .session_id,
        "native-original"
    );
    assert_eq!(
        store.snapshot(&uid, "").unwrap().metadata()["sid"],
        "display-stable"
    );
}
