use super::*;
use crate::sessions::index::summary::norm_ts;
use crate::sessions::{MessageQuery, SessionRoots, SessionStore};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn put(path: &Path, rows: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes = rows
        .iter()
        .map(|row| row.to_string() + "\n")
        .collect::<String>();
    fs::write(path, bytes).unwrap();
}
fn index(path: &Path, sid: &str, title: &str) {
    put(
        path,
        &[json!({"id":sid,"thread_name":title,"updated_at":"2026-09-11T10:00:00Z"})],
    );
}
fn native(path: &Path, sid: &str, parent: Option<(&str, u64)>, agent: bool) {
    let mut meta = json!({"id":sid,"cwd":"/synthetic/names","timestamp":"2026-09-11T10:00:00Z"});
    if let Some((parent, cut)) = parent {
        meta["forked_from_id"] = json!(parent);
        if !agent {
            meta["history_base"] = json!({"thread_id":parent,"end_byte_offset":cut});
        }
    }
    if agent {
        meta["thread_source"] = json!("subagent");
        meta["agent_nickname"] = json!("Native helper label");
        meta["source"] = json!({"subagent":{"thread_spawn":{"parent_thread_id":parent.unwrap().0,"depth":1,"agent_nickname":"Native helper label"}}});
    }
    put(
        path,
        &[
            json!({"type":"session_meta","payload":meta,"timestamp":"2026-09-11T10:00:00Z"}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":format!("{sid} native question")}]},"timestamp":"2026-09-11T10:00:00Z"}),
        ],
    );
}
fn store(root: &Path, index_path: Option<PathBuf>) -> SessionStore {
    SessionStore::with_metadata_and_names(
        SessionRoots {
            codex: Some(root.join("sessions")),
            ..Default::default()
        },
        None,
        index_path,
    )
}
fn row(list: &Value, sid: &str) -> Value {
    list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sid"] == sid)
        .unwrap()
        .clone()
}

#[test]
fn file_order_empty_names_time_and_unicode() {
    let names = parse(b"{\"id\":\"one\",\"thread_name\":\"newest timestamp\",\"updated_at\":\"2026-09-11T12:00:00Z\"}\n{\"id\":\"one\",\"thread_name\":\"last line\",\"updated_at\":\"2026-09-11T10:00:00Z\"}\n{\"id\":\"one\",\"thread_name\":\"\"}\n{\"id\":\"bad-date\",\"thread_name\":\"visible\",\"updated_at\":\"invalid\"}").unwrap();
    assert_eq!(names["one"].title, "last line");
    assert_eq!(names["one"].updated, "2026-09-11T10:00:00.000Z");
    assert_eq!(names["bad-date"].updated, Value::Null);
    assert_eq!(
        clip(&format!(" \t{}\n ", "界".repeat(111)), 110),
        "界".repeat(110) + "…"
    );
    for (input, expected) in [
        (json!(0), None),
        (
            json!("2026-09-11T18:00:00+08:00"),
            Some("2026-09-11T10:00:00.000Z".to_owned()),
        ),
        (
            json!("2026-09-11 10:00:00.123"),
            Some("2026-09-11T10:00:00.123Z".to_owned()),
        ),
        (
            json!("2026-09-11"),
            Some("2026-09-11T00:00:00.000Z".to_owned()),
        ),
        (json!(1), Some("1970-01-01T00:00:01.000Z".to_owned())),
        (json!(1e100), None),
    ] {
        assert_eq!(norm_ts(&input), expected, "{input}");
    }
}

#[test]
fn malformed_rows_are_skipped_and_large_valid_indexes_are_readable() {
    let names = parse(
        b"{bad}\n[]\n{}\n{\"id\":4,\"thread_name\":\"x\"}\n{\"id\":\"a\",\"thread_name\":true}\n{\"id\":\"kept\",\"thread_name\":\"valid\"}\n{",
    )
    .unwrap();
    assert_eq!(names.len(), 2);
    assert_eq!(names["a"].title, "True");
    assert_eq!(names["kept"].title, "valid");
    assert!(parse(&vec![b'\n'; 50_001]).unwrap().is_empty());
    let bytes = (0..=10_000)
        .map(|id| format!("{{\"id\":\"{id}\",\"thread_name\":\"x\"}}\n"))
        .collect::<String>();
    assert_eq!(parse(bytes.as_bytes()).unwrap().len(), 10_001);
    for record in [
        json!({"id":"x".repeat(257),"thread_name":"x"}),
        json!({"id":"x","thread_name":"x".repeat(16 * 1024 + 1)}),
    ] {
        assert_eq!(parse(record.to_string().as_bytes()).unwrap().len(), 1);
    }
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("index.jsonl");
    let file = fs::File::create(&path).unwrap();
    file.set_len(4 * 1024 * 1024 + 1).unwrap();
    assert!(load(&path, None).unwrap().names.is_empty());
}

#[test]
fn explicit_only_cache_identity_failures_and_recovery() {
    let temp = TempDir::new().unwrap();
    let native_path = temp.path().join("sessions/root.jsonl");
    native(&native_path, "root", None, false);
    let original_native_bytes = fs::read(&native_path).unwrap();
    let path = temp.path().join("session_index.jsonl");
    index(&path, "root", "not automatically discovered");
    assert_eq!(
        row(&store(temp.path(), None).list(false).unwrap(), "root")["title"],
        "root native question"
    );
    let service = store(temp.path(), Some(path.clone()));
    let initial = service.list(false).unwrap();
    let uid = row(&initial, "root")["uid"].as_str().unwrap().to_owned();
    let detail = service.snapshot(&uid, "").unwrap();
    let frozen = service.search_pool().unwrap();
    let reads = service.index().reads();
    index(&path, "root", "changed title");
    let changed = service.list(false).unwrap();
    assert_eq!(row(&changed, "root")["title"], "changed title");
    assert_ne!(initial["sig"], changed["sig"]);
    // A rename touches no native file: no summary is re-read.
    assert_eq!(service.index().reads(), reads);
    let updated = service.snapshot(&uid, "").unwrap();
    assert_eq!(detail.cursor(), updated.cursor());
    assert_ne!(detail.revision(), updated.revision());
    // The frozen search pool keeps the rows it was admitted with.
    assert_eq!(
        service.search_view(&frozen, &uid).unwrap().metadata()["title"],
        "not automatically discovered"
    );
    let query = MessageQuery {
        start: detail.cursor()["end"].as_u64().unwrap(),
        head: detail.cursor()["head"].as_str().unwrap().to_owned(),
        anchor: detail.cursor()["anchor"].as_str().unwrap().to_owned(),
        append: "1".into(),
        ..Default::default()
    };
    let batch = updated.messages(&query).unwrap();
    assert_eq!(batch["reset"], false);
    assert_eq!(batch["messages"], json!([]));
    fs::write(&path, "{bad}\n").unwrap();
    let fallback = service.list(false).unwrap();
    assert_eq!(row(&fallback, "root")["title"], "root native question");
    assert!(service.snapshot(&uid, "").is_ok());
    assert!(service.search_snapshot().is_ok());
    fs::remove_file(&path).unwrap();
    let missing = service.list(false).unwrap();
    assert_eq!(row(&missing, "root")["title"], "root native question");
    index(&path, "root", "recovered");
    assert_eq!(
        row(&service.list(false).unwrap(), "root")["title"],
        "recovered"
    );
    assert_eq!(fs::read(&native_path).unwrap(), original_native_bytes);
}

#[test]
fn own_nearest_ancestor_and_agent_titles() {
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("sessions");
    native(&dir.join("root.jsonl"), "root", None, false);
    native(
        &dir.join("middle.jsonl"),
        "middle",
        Some(("root", 0)),
        false,
    );
    native(&dir.join("leaf.jsonl"), "leaf", Some(("middle", 0)), false);
    native(&dir.join("agent.jsonl"), "agent", Some(("root", 0)), true);
    let path = temp.path().join("names.jsonl");
    put(
        &path,
        &[
            json!({"id":"root","thread_name":"Root name","updated_at":"2026-09-11T10:00:00Z"}),
            json!({"id":"middle","thread_name":"Middle name"}),
            json!({"id":"agent","thread_name":"Must not replace helper"}),
        ],
    );
    let service = store(temp.path(), Some(path.clone()));
    let list = service.list(false).unwrap();
    assert_eq!(row(&list, "root")["title"], "Root name");
    assert_eq!(row(&list, "middle")["title"], "Middle name");
    assert_eq!(row(&list, "leaf")["title"], "Middle name");
    assert_eq!(row(&list, "leaf")["renamed_to"], Value::Null);
    assert_eq!(row(&list, "middle")["renamed_at"], Value::Null);
    let uid = row(&list, "root")["uid"].as_str().unwrap().to_owned();
    let agent = service
        .messages(
            &uid,
            &MessageQuery {
                agent: "agent".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_ne!(agent["meta"]["title"], "Must not replace helper");
    assert_eq!(agent["meta"]["parent_title"], "Root name");
    assert_eq!(agent["meta"]["uid"], uid);
    index(&path, "leaf", "Own name");
    assert_eq!(
        row(&service.list(false).unwrap(), "leaf")["title"],
        "Own name"
    );
}

/// The inherited title follows `forked_from_id`,
/// not `history_base.thread_id` — a rewind past the
/// parent's fork point reads R's bytes but is Q's child.
#[test]
fn inherited_title_follows_forked_from_id_not_history_base() {
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("sessions");
    native(&dir.join("R.jsonl"), "R", None, false);
    let cut = fs::read(dir.join("R.jsonl"))
        .unwrap()
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap() as u64
        + 1;
    native(&dir.join("Q.jsonl"), "Q", Some(("R", cut)), false);
    let fork = |sid: &str, base: Value| {
        put(
            &dir.join(format!("{sid}.jsonl")),
            &[
                json!({"type":"session_meta","timestamp":"2026-09-11T10:00:00Z","payload":{
                    "id":sid,"cwd":"/synthetic/names","timestamp":"2026-09-11T10:00:00Z",
                    "forked_from_id":"Q","history_base":base}}),
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":format!("{sid} native question")}]},"timestamp":"2026-09-11T10:00:00Z"}),
            ],
        );
    };
    fork("A", json!({"thread_id":"R","end_byte_offset":cut}));
    // A legacy fork without history_base inherits the name the same way.
    fork("L", Value::Null);
    let path = temp.path().join("names.jsonl");
    put(
        &path,
        &[
            json!({"id":"Q","thread_name":"Q-name"}),
            json!({"id":"R","thread_name":"R-name"}),
        ],
    );
    let service = store(temp.path(), Some(path));
    let list = service.list(false).unwrap();
    for sid in ["A", "Q", "R", "L"] {
        assert_eq!(row(&list, sid)["supported"], true, "{sid}");
    }
    assert_eq!(row(&list, "A")["title"], "Q-name");
    assert_eq!(row(&list, "A")["root_sid"], "R");
    assert_eq!(row(&list, "A")["fork_depth"], 2);
    assert_eq!(row(&list, "Q")["title"], "Q-name");
    assert_eq!(row(&list, "R")["title"], "R-name");
    assert_eq!(row(&list, "L")["title"], "Q-name");
    assert_eq!(row(&list, "L")["root_sid"], "R");
}

#[test]
fn relative_and_missing_files_follow_python_fallbacks() {
    assert!(
        load(Path::new("relative/index"), None)
            .unwrap()
            .names
            .is_empty()
    );
    let temp = tempfile::Builder::new()
        .prefix("sessiondock-relative-names-")
        .tempdir_in(".")
        .unwrap();
    let cwd = std::env::current_dir().unwrap();
    let relative = temp.path().strip_prefix(&cwd).unwrap().join("index.jsonl");
    assert!(!relative.is_absolute());
    index(&relative, "id", "relative title");
    assert_eq!(
        load(&relative, None).unwrap().names["id"].title,
        "relative title"
    );
}

#[test]
fn same_size_atomic_replacement_invalidates_cache_and_open_handle() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("index.jsonl");
    index(&path, "id", "first title");
    let first = load(&path, None).unwrap();
    let cached = load(&path, Some(&first)).unwrap();
    assert!(Arc::ptr_eq(&first, &cached));
    assert!(!changed(&path, Some(&first)).unwrap());
    let before = fs::metadata(&path).unwrap();
    let replacement = temp.path().join("replacement.jsonl");
    index(&replacement, "id", "other title");
    fs::File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(before.modified().unwrap()))
        .unwrap();
    assert_eq!(before.len(), fs::metadata(&replacement).unwrap().len());
    fs::rename(&replacement, &path).unwrap();
    assert!(changed(&path, Some(&first)).unwrap());
    let second = load(&path, Some(&first)).unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(second.names["id"].title, "other title");
    assert_eq!(first.names["id"].title, "first title");
}

#[cfg(unix)]
#[test]
fn follows_symlinks_and_hardlinks_while_tracking_replacements() {
    use std::os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    };
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("index");
    index(&path, "id", "title");
    let link = temp.path().join("link");
    symlink(&path, &link).unwrap();
    assert_eq!(load(&link, None).unwrap().names["id"].title, "title");
    fs::remove_file(&link).unwrap();
    fs::hard_link(&path, &link).unwrap();
    assert_eq!(load(&path, None).unwrap().names["id"].title, "title");
    fs::remove_file(&link).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    assert!(load(&path, None).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let socket = temp.path().join("socket");
    let _listener = UnixListener::bind(&socket).unwrap();
    assert!(load(&socket, None).is_err());
    let directory = temp.path().join("directory");
    fs::create_dir(&directory).unwrap();
    index(&directory.join("index"), "id", "one");
    let first = load(&directory.join("index"), None).unwrap();
    fs::rename(&directory, temp.path().join("old-directory")).unwrap();
    fs::create_dir(&directory).unwrap();
    index(&directory.join("index"), "id", "two");
    assert!(changed(&directory.join("index"), Some(&first)).unwrap());
    let parent_link = temp.path().join("parent-link");
    symlink(&directory, &parent_link).unwrap();
    assert_eq!(
        load(&parent_link.join("index"), None).unwrap().names["id"].title,
        "two"
    );
}

/// A renamed main session carries the local
/// `/rename <name>` as an inferred, uncounted `command` event at
/// `renamed_at`, before the first later message; a full read shows it, an
/// append never re-sends it, search does not match it, agents and unnamed
/// or timeless entries have none, and a later rename recomposes it
/// without touching the file.
#[test]
fn renamed_codex_sessions_carry_the_inferred_rename_command_event() {
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("sessions");
    put(
        &dir.join("root.jsonl"),
        &[
            json!({"type":"session_meta","payload":{"id":"root","cwd":"/synthetic/names","timestamp":"2026-09-11T10:00:00Z"},"timestamp":"2026-09-11T10:00:00Z"}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"first question"}]},"timestamp":"2026-09-11T10:00:00Z"}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"first answer"}]},"timestamp":"2026-09-11T10:00:05Z"}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"second question"}]},"timestamp":"2026-09-11T10:01:00Z"}),
        ],
    );
    native(&dir.join("timeless.jsonl"), "timeless", None, false);
    native(&dir.join("agent.jsonl"), "agent", Some(("root", 0)), true);
    let path = temp.path().join("names.jsonl");
    put(
        &path,
        &[
            json!({"id":"root","thread_name":"Root name","updated_at":"2026-09-11T10:00:30Z"}),
            json!({"id":"timeless","thread_name":"No timestamp"}),
            json!({"id":"agent","thread_name":"Agent name","updated_at":"2026-09-11T10:00:30Z"}),
        ],
    );
    let service = store(temp.path(), Some(path.clone()));
    let list = service.list(false).unwrap();
    let uid = row(&list, "root")["uid"].as_str().unwrap().to_owned();
    let full = service.messages(&uid, &MessageQuery::default()).unwrap();
    let texts = |batch: &Value| {
        batch["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["text"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        texts(&full),
        [
            "first question",
            "first answer",
            "/rename Root name",
            "second question"
        ]
    );
    assert_eq!(
        full["message_total"], 3,
        "the inferred event is not counted"
    );
    assert_eq!(
        full["messages"][2],
        json!({"role": "command", "text": "/rename Root name", "ts": "2026-09-11T10:00:30.000Z",
               "name": null, "args": null, "counted": false, "inferred": true,
               "event_id": "rename:root:2026-09-11T10:00:30.000Z"})
    );
    // An append from the full read's cursor carries nothing new.
    let append = service
        .messages(
            &uid,
            &MessageQuery {
                start: full["end"].as_u64().unwrap(),
                head: full["version"]["head"].as_str().unwrap().to_owned(),
                anchor: full["anchor"].as_str().unwrap().to_owned(),
                append: "1".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(append["reset"], false);
    assert_eq!(append["messages"], json!([]));
    // Not searchable, not on agents or timeless names.
    let snapshot = service.snapshot(&uid, "").unwrap();
    assert!(
        snapshot
            .texts()
            .all(|(_, text)| !text.starts_with("/rename"))
    );
    let agent = service
        .messages(
            &uid,
            &MessageQuery {
                agent: "agent".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        texts(&agent)
            .iter()
            .all(|text| !text.starts_with("/rename"))
    );
    let timeless_uid = row(&list, "timeless")["uid"].as_str().unwrap().to_owned();
    let timeless = service
        .messages(&timeless_uid, &MessageQuery::default())
        .unwrap();
    assert_eq!(timeless["meta"]["title"], "No timestamp");
    assert!(
        texts(&timeless)
            .iter()
            .all(|text| !text.starts_with("/rename"))
    );
    // A later rename (after every message) recomposes the cached view: the
    // event moves to the end, the file is untouched.
    let reads = service.index().reads();
    put(
        &path,
        &[json!({"id":"root","thread_name":"Newer name","updated_at":"2026-09-11T10:05:00Z"})],
    );
    let renamed = service.messages(&uid, &MessageQuery::default()).unwrap();
    assert_eq!(
        texts(&renamed),
        [
            "first question",
            "first answer",
            "second question",
            "/rename Newer name"
        ]
    );
    assert_eq!(service.index().reads(), reads);
    assert_eq!(renamed["end"], full["end"]);
}
