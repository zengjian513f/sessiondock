use super::*;
use std::io::Write;
use std::time::Duration;
use tempfile::TempDir;

fn fixture(chat: Option<&[u8]>, summary: Value) -> (TempDir, PathBuf, SessionStore, String) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("grok");
    let directory = root.join("%2Fsynthetic%2FGrok+project/session-12345678");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("summary.json"), summary.to_string()).unwrap();
    if let Some(chat) = chat {
        fs::write(directory.join("chat_history.jsonl"), chat).unwrap();
    }
    let uid = uid_for("grok", &directory);
    let store = SessionStore::new(SessionRoots {
        grok: Some(root),
        ..Default::default()
    });
    (temp, directory, store, uid)
}
fn query(batch: &Value) -> MessageQuery {
    MessageQuery {
        start: batch["end"].as_u64().unwrap(),
        head: batch["version"]["head"].as_str().unwrap().into(),
        anchor: batch["anchor"].as_str().unwrap().into(),
        append: "1".into(),
        ..Default::default()
    }
}
fn message(text: &str) -> Vec<u8> {
    (json!({"type":"user","content":text,"prompt_index":1,"timestamp":"2000-01-01T00:00:00Z"})
        .to_string()
        + "\n")
        .into_bytes()
}

#[test]
fn summary_only_and_zero_byte_chat_are_visible_with_truthful_versions() {
    for chat in [None, Some(b"".as_slice())] {
        let (_temp, directory, store, uid) =
            fixture(chat, json!({"session_summary":"Summary-only title"}));
        let list = store.list(false).unwrap();
        assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
        let row = &list["sessions"][0];
        assert_eq!(row["uid"], uid);
        assert_eq!(row["path"].as_str(), Some(path_text(&directory).as_ref()));
        assert_eq!(row["title"], "Summary-only title");
        assert_eq!(row["cwd"], "/synthetic/Grok+project");
        assert_eq!(row["chat_exists"], chat.is_some());
        assert_eq!(
            row["size"],
            fs::metadata(directory.join("summary.json")).unwrap().len()
        );
        let batch = store.messages(&uid, &MessageQuery::default()).unwrap();
        assert_eq!(batch["messages"], json!([]));
        assert_eq!(batch["end"], 0);
        assert_eq!(batch["meta"]["supported"], true);
        assert_eq!(batch["version"]["size"], 0);
        assert_eq!(batch["version"]["exists"], chat.is_some());
        assert_eq!(batch["version"]["mtime"].is_null(), chat.is_none());
        let next = store.messages(&uid, &query(&batch)).unwrap();
        assert_eq!(next["reset"], false);
        assert_eq!(next["messages"], json!([]));
        assert_eq!(
            directory.join("chat_history.jsonl").exists(),
            chat.is_some()
        );
    }
}

#[test]
fn absent_created_empty_appended_removed_chat_stays_one_uid_and_validates_cursor() {
    let (_temp, directory, store, uid) =
        fixture(None, json!({"generated_title":"State transitions"}));
    let absent = store.snapshot(&uid, "").unwrap();
    let absent_batch = absent.messages(&MessageQuery::default()).unwrap();
    fs::write(directory.join("chat_history.jsonl"), b"").unwrap();
    let empty = store.snapshot(&uid, "").unwrap();
    let empty_batch = empty.messages(&query(&absent_batch)).unwrap();
    assert_ne!(absent.revision(), empty.revision());
    assert_eq!(absent.cursor(), empty.cursor());
    assert_eq!(empty_batch["reset"], false);
    assert_eq!(empty_batch["meta"]["chat_exists"], true);
    let chat = message("First real Grok input");
    fs::write(
        directory.join("chat_history.jsonl"),
        &chat[..chat.len() - 2],
    )
    .unwrap();
    let partial = store.messages(&uid, &query(&empty_batch)).unwrap();
    assert_eq!(partial["messages"], json!([]));
    assert_eq!(partial["end"], 0);
    fs::OpenOptions::new()
        .append(true)
        .open(directory.join("chat_history.jsonl"))
        .unwrap()
        .write_all(&chat[chat.len() - 2..])
        .unwrap();
    let committed = store.messages(&uid, &query(&partial)).unwrap();
    assert_eq!(committed["reset"], false);
    assert_eq!(committed["messages"][0]["text"], "First real Grok input");
    assert_eq!(committed["end"], chat.len());
    assert_eq!(committed["meta"]["uid"], uid);
    fs::remove_file(directory.join("chat_history.jsonl")).unwrap();
    let removed = store.messages(&uid, &query(&committed)).unwrap();
    assert_eq!(removed["reset"], true);
    assert_eq!(removed["end"], 0);
    assert_eq!(removed["version"]["exists"], false);
    assert_eq!(store.list(true).unwrap()["sessions"][0]["uid"], uid);
}

#[test]
fn summary_edit_updates_metadata_not_body_anchor_and_search_is_frozen() {
    let chat = message("Body must not become metadata");
    let (_temp, directory, store, uid) =
        fixture(Some(&chat), json!({"generated_title":"Old title"}));
    let old = store.messages(&uid, &MessageQuery::default()).unwrap();
    let frozen = store.search_pool().unwrap();
    fs::write(directory.join("summary.json"),json!({"info":{"id":"summary-native-id","cwd":"/synthetic/new-cwd"},"generated_title":"Changed summary title","current_model_id":"synthetic-model","agent_name":"synthetic-agent","created_at":"2026-09-11T10:00:00Z","updated_at":"2026-09-11T11:00:00Z"}).to_string()).unwrap();
    let changed = store.messages(&uid, &query(&old)).unwrap();
    assert_eq!(changed["reset"], false);
    assert_eq!(changed["messages"], json!([]));
    assert_eq!(changed["anchor"], old["anchor"]);
    assert_eq!(changed["version"], old["version"]);
    assert_eq!(changed["meta"]["title"], "Changed summary title");
    assert_eq!(changed["meta"]["sid"], "summary-native-id");
    assert_eq!(changed["meta"]["created"], "2026-09-11T10:00:00.000Z");
    assert_eq!(changed["meta"]["updated"], "2026-09-11T11:00:00.000Z");
    // The frozen search pool keeps its admitted rows for the whole scan.
    assert_eq!(
        store.search_view(&frozen, &uid).unwrap().metadata()["title"],
        "Old title"
    );
    assert_eq!(
        fs::read(directory.join("chat_history.jsonl")).unwrap(),
        chat
    );
}

#[test]
fn fallback_uses_correct_file_mtime_never_native_record_timestamps() {
    for chat in [None, Some(message("Not a title"))] {
        let (_temp, directory, store, uid) = fixture(chat.as_deref(), json!({}));
        let summary_time =
            UNIX_EPOCH + Duration::from_secs(1_700_000_000) + Duration::from_millis(456);
        let chat_time =
            UNIX_EPOCH + Duration::from_secs(1_700_000_100) + Duration::from_millis(789);
        fs::File::options()
            .write(true)
            .open(directory.join("summary.json"))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(summary_time))
            .unwrap();
        if chat.is_some() {
            fs::File::options()
                .write(true)
                .open(directory.join("chat_history.jsonl"))
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(chat_time))
                .unwrap();
        }
        let result = store.messages(&uid, &MessageQuery::default()).unwrap();
        let seconds = if chat.is_some() {
            1_700_000_100
        } else {
            1_700_000_000
        };
        assert_eq!(
            result["meta"]["created"],
            timestamp(seconds * 1_000_000_000)
        );
        assert_eq!(result["meta"]["updated"], result["meta"]["created"]);
        assert_eq!(result["meta"]["title"], "session-");
    }
}

#[test]
fn size_is_the_whole_session_directory_and_attachment_subtrees_are_never_parsed() {
    // Every regular file under the session directory
    // counts (updates.jsonl, tool definitions, attachments…), nothing in
    // there is parsed, including files nested more than eight levels deep.
    let chat = message("Known input");
    let (_temp, directory, store, uid) = fixture(Some(&chat), json!({}));
    let deep = (0..20).fold(directory.join("attachments"), |path, _| path.join("nested"));
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("summary.json"), b"this must never parse").unwrap();
    fs::write(directory.join("unrelated.jsonl"), b"invalid native data").unwrap();
    fs::write(directory.join("updates.jsonl"), b"0123456789").unwrap();
    fs::create_dir_all(directory.join("attachments").join("shallow")).unwrap();
    fs::write(
        directory
            .join("attachments")
            .join("shallow")
            .join("shot.png"),
        [0u8; 32],
    )
    .unwrap();
    let result = store.list(false).unwrap();
    assert_eq!(result["sessions"].as_array().unwrap().len(), 1);
    let expected =
        2 + chat.len() + b"invalid native data".len() + 10 + 32 + b"this must never parse".len();
    assert_eq!(result["sessions"][0]["size"], expected);
    assert!(result["sessions"][0].get("size_scope").is_none());
    assert_eq!(result["sessions"][0]["chat_exists"], true);
    // The opened view reports the same directory size (`meta` is the row).
    let view = store
        .messages(&uid, &crate::sessions::MessageQuery::default())
        .unwrap();
    assert_eq!(view["meta"]["size"], expected);
    // A file appended elsewhere in the directory is picked up with the next
    // summary/chat change, exactly like Python's per-stamp refresh.
    fs::write(directory.join("updates.jsonl"), b"01234567890123456789").unwrap();
    fs::write(
        directory.join("summary.json"),
        json!({"generated_title": "Grown"}).to_string(),
    )
    .unwrap();
    let grown = store.list(true).unwrap();
    assert_eq!(grown["sessions"][0]["title"], "Grown");
    assert_eq!(
        grown["sessions"][0]["size"],
        json!({"generated_title": "Grown"}).to_string().len()
            + chat.len()
            + b"invalid native data".len()
            + 20
            + 32
            + b"this must never parse".len()
    );
}

#[test]
fn bad_summary_budget_or_special_chat_are_not_successful_empty_histories() {
    let (_temp, directory, store, uid) =
        fixture(None, json!({"generated_title":"Keep old snapshot"}));
    store.list(false).unwrap();
    // The list never fails on one session's files: the row is listed as
    // unsupported with the reason, and opening it fails.
    let unsupported_row = |needle: &str| {
        let list = store.list(true).unwrap();
        let row = &list["sessions"][0];
        assert_eq!(row["uid"], uid);
        assert_eq!(row["supported"], false);
        let warning = row["migration_warnings"][0].as_str().unwrap().to_owned();
        assert!(warning.contains(needle), "{warning}");
        assert!(row.get("cursor").is_none());
    };
    for invalid in [b"{bad".as_slice(), b"null", b"[]", b"{\"info\":42}"] {
        fs::write(directory.join("summary.json"), invalid).unwrap();
        assert!(matches!(
            store.messages(&uid, &MessageQuery::default()),
            Err(SessionError { status: 503, .. })
        ));
        unsupported_row("summary.json");
    }
    fs::write(
        directory.join("summary.json"),
        serde_json::to_vec(&json!({"padding":"x".repeat(16 * 1024 * 1024 + 1)})).unwrap(),
    )
    .unwrap();
    assert!(store.messages(&uid, &MessageQuery::default()).is_ok());
    let list = store.list(true).unwrap();
    assert_eq!(list["sessions"][0]["supported"], true);
    fs::write(directory.join("summary.json"), b"{}").unwrap();
    fs::create_dir(directory.join("chat_history.jsonl")).unwrap();
    let list = store.list(true).unwrap();
    assert_eq!(list["sessions"][0]["supported"], true);
    assert_eq!(
        store.messages(&uid, &MessageQuery::default()).unwrap()["messages"],
        json!([])
    );
}

#[cfg(unix)]
#[test]
fn ordinary_chat_links_follow_python_and_os_permission_errors_remain_visible() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (_temp, directory, store, uid) = fixture(None, json!({}));
    store.list(false).unwrap();
    let target = directory.join("fixture-target");
    fs::write(&target, b"").unwrap();
    let chat = directory.join("chat_history.jsonl");
    symlink(&target, &chat).unwrap();
    assert!(store.messages(&uid, &MessageQuery::default()).is_ok());
    let linked = store.list(true).unwrap();
    assert_eq!(linked["sessions"][0]["supported"], true);
    assert_eq!(linked["sessions"][0]["chat_exists"], true);
    fs::remove_file(&chat).unwrap();
    fs::write(&chat, b"").unwrap();
    for path in [&chat, &directory.join("summary.json")] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
        let list = store.list(true).unwrap();
        assert_eq!(
            list["sessions"][0]["supported"], false,
            "unreadable native input returned an empty success: {}",
            list["sessions"][0]
        );
        assert!(store.messages(&uid, &MessageQuery::default()).is_err());
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(store.messages(&uid, &MessageQuery::default()).is_ok());
    assert_eq!(store.list(true).unwrap()["sessions"][0]["supported"], true);
}
