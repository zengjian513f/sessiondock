//! The byte renderer (`messages_body`, `history_page_body`) against the
//! `Value` renderer it must match byte for byte: hot and cold reads of the
//! three providers, appends that extend the cached bytes, rewrites that drop
//! them, the byte budget, concurrent readers sharing one fill, windows and
//! pages spliced from the cache, and the Codex rename event in the middle.
use super::*;
use crate::media::MediaStore;
use crate::sessions::{MessageQuery, PageStore, SessionRoots, SessionStore, uid_for};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const NOW: &str = "2026-09-11T10:00:00.123Z";

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The three provider fixtures in a private root (so tests may append).
fn fixture_store() -> (TempDir, SessionStore) {
    let temp = TempDir::new().unwrap();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    copy_tree(&fixtures, temp.path());
    let store = SessionStore::new(SessionRoots {
        claude: Some(temp.path().join("claude")),
        codex: Some(temp.path().join("codex")),
        grok: Some(temp.path().join("grok")),
    });
    (temp, store)
}

fn claude_row(id: &str, parent: Value, role: &str, text: &str) -> Value {
    json!({"type": role, "uuid": id, "parentUuid": parent,
           "sessionId": "synthetic-session", "cwd": "/workspace/demo",
           "timestamp": NOW,
           "message": {"role": role, "content": [{"type": "text", "text": text}]}})
}

fn claude_chain(count: usize) -> Vec<Value> {
    (0..count)
        .map(|i| {
            claude_row(
                &format!("m{i}"),
                if i == 0 {
                    Value::Null
                } else {
                    json!(format!("m{}", i - 1))
                },
                if i % 2 == 0 { "user" } else { "assistant" },
                &format!("message {i} \"quoted\"\n第二行"),
            )
        })
        .collect()
}

fn write_rows(path: &Path, rows: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = fs::File::create(path).unwrap();
    for row in rows {
        writeln!(file, "{row}").unwrap()
    }
}

fn append_rows(path: &Path, rows: &[Value]) {
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    for row in rows {
        writeln!(file, "{row}").unwrap()
    }
}

/// A private Claude root with `count` chained messages.
fn claude_store(count: usize) -> (TempDir, PathBuf, SessionStore, String) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("claude");
    let file = root.join("project/session.jsonl");
    write_rows(&file, &claude_chain(count));
    let store = SessionStore::new(SessionRoots {
        claude: Some(root),
        ..Default::default()
    });
    let uid = store.list(false).unwrap()["sessions"][0]["uid"]
        .as_str()
        .unwrap()
        .to_owned();
    (temp, file, store, uid)
}

fn claude_request(uid: &str, owner: &Candidate) -> ViewRequest {
    ViewRequest {
        uid: uid.to_owned(),
        agent: String::new(),
        owner: owner.clone(),
        selected: None,
        pin: None,
        row: json!({"uid": uid, "title": "t", "source": "claude", "supported": true,
            "cwd": "/workspace/demo", "agent_items": []}),
    }
}

fn window() -> MessageQuery {
    MessageQuery {
        window: "1".into(),
        ..Default::default()
    }
}

fn continuation(body: &[u8], append: bool) -> MessageQuery {
    let value: Value = serde_json::from_slice(body).unwrap();
    MessageQuery {
        start: value["end"].as_u64().unwrap(),
        head: value["version"]["head"].as_str().unwrap().into(),
        anchor: value["anchor"].as_str().unwrap().into(),
        append: if append { "1".into() } else { String::new() },
        ..Default::default()
    }
}

/// The `Value` renderer's document with the `prompt` the handler appends.
fn value_document(
    snapshot: &ViewSnapshot,
    query: &MessageQuery,
    media: &MediaStore,
    pages: &PageStore,
) -> Vec<u8> {
    let mut value = snapshot
        .messages_with_pages(query, media, None, pages)
        .unwrap();
    value["prompt"] = Value::Null;
    serde_json::to_vec(&value).unwrap()
}

fn body_document(
    snapshot: &ViewSnapshot,
    query: &MessageQuery,
    media: &MediaStore,
    pages: &PageStore,
) -> Vec<u8> {
    snapshot
        .messages_body(query, media, None, pages)
        .unwrap()
        .finish(Some(&Value::Null))
}

/// Replace every 32-hex grant token with a fixed marker so documents that
/// mint their own grants can be compared.
fn without_tokens(bytes: &[u8]) -> String {
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let mut out = String::with_capacity(text.len());
    let chars = text.as_bytes();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == b'"'
            && i + 33 < chars.len()
            && chars[i + 33] == b'"'
            && chars[i + 1..i + 33]
                .iter()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            out.push_str("\"<token>\"");
            i += 34;
            continue;
        }
        out.push(chars[i] as char);
        i += 1;
    }
    out
}

#[test]
fn hot_and_cold_bodies_equal_the_value_renderer_for_three_providers() {
    let (_temp, store) = fixture_store();
    let media = MediaStore::new();
    let pages = PageStore::default();
    let list = store.list(false).unwrap();
    let rows = list["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for row in rows {
        let uid = row["uid"].as_str().unwrap();
        let cold = store.snapshot(uid, "").unwrap();
        let builds = encoded::RETAINED_BUILDS.get();
        let full = body_document(&cold, &MessageQuery::default(), &media, &pages);
        assert_eq!(
            full,
            value_document(&cold, &MessageQuery::default(), &media, &pages),
            "{uid} full"
        );
        assert!(full.ends_with(b",\"prompt\":null}"));
        for query in [
            window(),
            continuation(&full, false),
            continuation(&full, true),
            MessageQuery {
                anchor: "forged".into(),
                ..continuation(&full, true)
            },
            MessageQuery {
                start: 1,
                ..continuation(&full, false)
            },
        ] {
            assert_eq!(
                body_document(&cold, &query, &media, &pages),
                value_document(&cold, &query, &media, &pages),
                "{uid} {query:?}"
            );
        }
        // Hot: the very same snapshot, the very same bytes, no new encoding.
        let hot = store.snapshot(uid, "").unwrap();
        assert!(Arc::ptr_eq(&cold, &hot));
        assert_eq!(
            body_document(&hot, &MessageQuery::default(), &media, &pages),
            full
        );
        assert_eq!(encoded::RETAINED_BUILDS.get(), builds);
        // The document without media splices is also what the text renderer
        // serializes (no fixture carries images).
        let mut text = store.messages(uid, &MessageQuery::default()).unwrap();
        text["prompt"] = Value::Null;
        assert_eq!(serde_json::to_vec(&text).unwrap(), full);
        // Agent views carry no `prompt`: the handler omits it, and the
        // document is the same up to that field.
        let agentless = hot
            .messages_body(&MessageQuery::default(), &media, None, &pages)
            .unwrap()
            .finish(None);
        assert!(!agentless.ends_with(b"\"prompt\":null}"));
        let mut with_prompt = agentless[..agentless.len() - 1].to_vec();
        with_prompt.extend_from_slice(b",\"prompt\":null}");
        assert_eq!(with_prompt, full);
    }
}

#[test]
fn append_extends_the_cached_bytes_and_equals_a_cold_projection() {
    let (_temp, file, store, uid) = claude_store(40);
    let media = MediaStore::new();
    let pages = PageStore::default();
    let first = store.snapshot(&uid, "").unwrap();
    let before = body_document(&first, &MessageQuery::default(), &media, &pages);
    let messages_before = first.view.parsed.encoded.message_count();
    append_rows(
        &file,
        &[
            claude_row("m40", json!("m39"), "user", "appended 40"),
            claude_row("m41", json!("m40"), "assistant", "appended 41"),
        ],
    );
    let extended = store.snapshot(&uid, "").unwrap();
    let encoded = &extended.view.parsed.encoded;
    assert_eq!(encoded.message_count(), messages_before + 2);
    // Every old message was copied, the tail was serialized (status
    // events are re-encoded, they are not cached messages).
    assert_eq!(encoded.reused(), messages_before);
    // The extended bytes are exactly what a process that never saw the old
    // file projects.
    let cold_store = SessionStore::new(SessionRoots {
        claude: Some(file.parent().unwrap().parent().unwrap().to_path_buf()),
        ..Default::default()
    });
    let cold = cold_store.snapshot(&uid, "").unwrap();
    let full = body_document(&extended, &MessageQuery::default(), &media, &pages);
    assert_eq!(
        full,
        body_document(&cold, &MessageQuery::default(), &media, &pages)
    );
    assert_eq!(
        full,
        value_document(&extended, &MessageQuery::default(), &media, &pages)
    );
    assert_ne!(full, before);
    // The increment from the old checkpoint is the appended tail only.
    let delta = extended
        .messages_body(&continuation(&before, false), &media, None, &pages)
        .unwrap();
    assert_eq!(delta.message_count(), 2);
    let delta = delta.finish(None);
    let value: Value = serde_json::from_slice(&delta).unwrap();
    assert_eq!(value["reset"], false);
    assert_eq!(value["messages"][1]["text"], "appended 41");
    assert_eq!(
        delta,
        extended
            .messages_body(&continuation(&before, false), &media, None, &pages)
            .unwrap()
            .finish(None)
    );
    assert_eq!(
        without_tokens(&delta),
        without_tokens(&{
            let value = extended
                .messages_with_pages(&continuation(&before, false), &media, None, &pages)
                .unwrap();
            serde_json::to_vec(&value).unwrap()
        })
    );
}

#[test]
fn rewrite_and_truncation_drop_the_cached_bytes() {
    let (_temp, file, store, uid) = claude_store(12);
    let media = MediaStore::new();
    let pages = PageStore::default();
    let first = store.snapshot(&uid, "").unwrap();
    let before = body_document(&first, &MessageQuery::default(), &media, &pages);
    // Truncation to a shorter, different history: nothing is reused.
    let mut rows = claude_chain(6);
    rows[0]["message"]["content"][0]["text"] = json!("rewritten opening");
    write_rows(&file, &rows);
    let rewritten = store.snapshot(&uid, "").unwrap();
    assert!(!Arc::ptr_eq(&first, &rewritten));
    assert_eq!(rewritten.view.parsed.encoded.reused(), 0);
    let full = body_document(&rewritten, &MessageQuery::default(), &media, &pages);
    assert_eq!(
        full,
        value_document(&rewritten, &MessageQuery::default(), &media, &pages)
    );
    let value: Value = serde_json::from_slice(&full).unwrap();
    assert_eq!(value["messages"][0]["text"], "rewritten opening");
    assert_eq!(value["message_total"], 6);
    // The old checkpoint resets (and its stale anchor is derived from the
    // cached bytes, not by re-serializing the prefix).
    let reset = body_document(&rewritten, &continuation(&before, false), &media, &pages);
    let value: Value = serde_json::from_slice(&reset).unwrap();
    assert_eq!(value["reset"], true);
    assert_eq!(
        reset,
        value_document(&rewritten, &continuation(&before, false), &media, &pages)
    );
}

#[test]
fn stale_checkpoints_are_validated_from_the_cached_bytes() {
    let (_temp, file, store, uid) = claude_store(10);
    let media = MediaStore::new();
    let pages = PageStore::default();
    let first = store.snapshot(&uid, "").unwrap();
    let before = body_document(&first, &MessageQuery::default(), &media, &pages);
    append_rows(&file, &[claude_row("m10", json!("m9"), "user", "tail")]);
    let extended = store.snapshot(&uid, "").unwrap();
    let query = continuation(&before, false);
    // A checkpoint below the committed end: the semantic anchor of that
    // prefix comes from `digest_upto` and equals the re-serialized digest.
    let parsed = &extended.view.parsed;
    let end = query.start as usize;
    assert!(end < parsed.committed);
    assert_eq!(
        parsed
            .encoded
            .digest_upto(&parsed.events, end as u64)
            .unwrap(),
        projection_digest(&parsed.events, end)
    );
    assert!(extended.valid_checkpoint(&query));
    let delta: Value =
        serde_json::from_slice(&body_document(&extended, &query, &media, &pages)).unwrap();
    assert_eq!(delta["reset"], false);
    assert_eq!(delta["messages"].as_array().unwrap().len(), 1);
}

#[test]
fn byte_budget_evicts_the_least_recently_used_file() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap().join("claude");
    let deps = super::tests::MapDeps::default();
    let mut sessions = Vec::new();
    for index in 0..3 {
        let path = root.join(format!("project/session-{index}.jsonl"));
        write_rows(&path, &claude_chain(20));
        let owner = super::tests::candidate("claude", &root, &path);
        let uid = uid_for("claude", &path);
        sessions.push((uid, owner));
    }
    let mut probe = Views::new();
    let (uid, owner) = &sessions[0];
    probe.open(&claude_request(uid, owner), &deps).unwrap();
    let one = probe.stats().bytes;
    assert!(one > 0);
    let mut views = Views::with_byte_limit(one * 2 + one / 2);
    for (uid, owner) in &sessions {
        views.open(&claude_request(uid, owner), &deps).unwrap();
    }
    let stats = views.stats();
    assert_eq!(stats.files, 2, "{stats:?}");
    assert!(stats.bytes <= one * 2 + one / 2);
    assert!(views.cached(&sessions[0].0, "").is_none());
    assert!(views.cached(&sessions[1].0, "").is_some());
    assert!(views.cached(&sessions[2].0, "").is_some());
    // The retained bytes are the accounted bytes (plus one separator per
    // message): the budget charges a view once for its tree and its bytes.
    let encoded = &views
        .cached(&sessions[2].0, "")
        .unwrap()
        .view
        .parsed
        .encoded;
    assert!(encoded.retained() >= encoded.total_len());
    assert!(encoded.retained() < encoded.total_len() + encoded.message_count());
    assert!(stats.bytes >= encoded.total_len() * 2);
}

#[test]
fn eight_concurrent_readers_fill_the_bytes_once() {
    let (_temp, _file, store, uid) = claude_store(700);
    let store = Arc::new(store);
    let media = Arc::new(MediaStore::new());
    let pages = Arc::new(PageStore::default());
    let (bodies, builds): (Vec<Vec<u8>>, Vec<usize>) = (0..8)
        .map(|_| {
            let (store, media, pages, uid) =
                (store.clone(), media.clone(), pages.clone(), uid.clone());
            std::thread::spawn(move || {
                let snapshot = store.snapshot(&uid, "").unwrap();
                let body = body_document(&snapshot, &MessageQuery::default(), &media, &pages);
                (body, encoded::RETAINED_BUILDS.get())
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .unzip();
    assert!(bodies.iter().all(|body| *body == bodies[0]));
    // Exactly one of the eight threads encoded the file; the rest waited
    // for the view lock and spliced the same bytes.
    assert_eq!(builds.iter().sum::<usize>(), 1, "{builds:?}");
    assert_eq!(store.view_stats().unwrap().files, 1);
    let value: Value = serde_json::from_slice(&bodies[0]).unwrap();
    assert_eq!(value["message_total"], 700);
}

#[test]
fn windows_and_history_pages_are_spliced_from_the_cached_view() {
    let (_temp, _file, store, uid) = claude_store(700);
    let media = MediaStore::new();
    let pages = PageStore::with_page_events(200);
    let snapshot = store.snapshot(&uid, "").unwrap();
    let builds = encoded::RETAINED_BUILDS.get();
    let body = snapshot
        .messages_body(&window(), &media, None, &pages)
        .unwrap();
    assert_eq!(body.message_count(), 600);
    let body = body.finish(Some(&Value::Null));
    assert_eq!(
        without_tokens(&body),
        without_tokens(&value_document(&snapshot, &window(), &media, &pages))
    );
    assert_eq!(encoded::RETAINED_BUILDS.get(), builds);
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["partial"]["head"], 100);
    assert_eq!(value["partial"]["tail"], 500);
    assert_eq!(
        value["messages"][99]["text"],
        "message 99 \"quoted\"\n第二行"
    );
    assert_eq!(
        value["messages"][100]["text"],
        "message 200 \"quoted\"\n第二行"
    );
    // History pages through the grant: the byte page equals the Value page
    // (its continuation grant differs, everything else is identical).
    let token = value["partial"]["cursor"].as_str().unwrap();
    let grant = pages.lookup(token, &uid, "").unwrap();
    let page = snapshot
        .history_page_body(grant.clone(), token, &media, None, &pages)
        .unwrap();
    let reference = snapshot
        .history_page(grant, token, &media, None, &pages)
        .unwrap();
    assert_eq!(
        without_tokens(&page),
        without_tokens(&serde_json::to_vec(&reference).unwrap())
    );
    let page: Value = serde_json::from_slice(&page).unwrap();
    assert_eq!(page["page"]["start"], 100);
    assert_eq!(page["messages"].as_array().unwrap().len(), 100);
    assert_eq!(
        page["messages"][0]["text"],
        "message 100 \"quoted\"\n第二行"
    );
    assert_eq!(encoded::RETAINED_BUILDS.get(), builds);
}

#[test]
fn the_codex_rename_event_splits_runs_without_changing_the_document() {
    // Synthetic view: 5 leaf messages, the rename after the second.
    let events = (0..5)
        .map(|i| Event {
            end: 10 + i as u64,
            message: json!({"role": if i % 2 == 0 {"user"} else {"assistant"},
                "text": format!("m{i}"), "ts": format!("2026-09-11T10:00:0{i}.000Z"),
                "name": null, "args": null}),
            media: Vec::new(),
        })
        .collect::<Vec<_>>();
    let status = Event {
        end: 12,
        message: json!({"role":"status","text":"working","state":"working","ts":"2026-09-11T10:00:02.500Z"}),
        media: Vec::new(),
    };
    let mut leaf = events.clone();
    leaf.insert(3, status);
    let meta = json!({"uid":"codex:synthetic-rename","agent_id":"","source":"codex",
        "sid":"thread-1","cwd":"/synthetic","supported":true,
        "renamed_at":"2026-09-11T10:00:01.500Z","renamed_to":"Named"});
    let bytes = b"{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n";
    let parsed = Arc::new(Parsed {
        candidate: Candidate {
            source: "codex",
            root: PathBuf::from("/synthetic"),
            path: PathBuf::from("/synthetic/rollout.jsonl"),
            data: PathBuf::from("/synthetic/rollout.jsonl"),
            summary: None,
            stamps: vec![super::super::FileStamp {
                size: bytes.len() as u64,
                modified: 0,
                identity: "inode:ctime".into(),
                file_identity: "inode".into(),
            }],
        },
        raw_index: native_input::RawIndex::scan(&bytes[..]).unwrap(),
        _fixture: None,
        committed: bytes.len(),
        meta: meta.clone(),
        native_id: Ok("thread-1".into()),
        encoded: EncodedEvents::build(&leaf, true, None).unwrap(),
        events: leaf,
        unsupported: None,
        raw_error: None,
        pin: None,
    });
    let inherited = vec![Event {
        end: 0,
        message: json!({"role":"user","text":"inherited","ts":"2026-09-11T09:00:00.000Z",
            "name": null, "args": null}),
        media: Vec::new(),
    }];
    let snapshot = ViewSnapshot::new(Arc::new(View::new(ViewParts {
        inherited_encoded: Arc::new(EncodedEvents::build(&inherited, true, None).unwrap()),
        inherited: Arc::new(inherited),
        sources: vec![parsed.candidate.clone()],
        parsed,
        meta,
        identity: "synthetic-identity".into(),
        native_scope: Err(SessionError::new(501, "unused")),
        dependencies: vec!["codex:synthetic-rename".into()],
    })));
    assert_eq!(snapshot.view.rename_at, Some(3));
    assert_eq!(snapshot.view.slot(0), Slot::Inherited(0));
    assert_eq!(snapshot.view.slot(1), Slot::Leaf(0));
    assert_eq!(snapshot.view.slot(3), Slot::Rename);
    assert_eq!(snapshot.view.slot(4), Slot::Leaf(2));
    let media = MediaStore::new();
    let pages = PageStore::default();
    for query in [MessageQuery::default(), window()] {
        assert_eq!(
            body_document(&snapshot, &query, &media, &pages),
            value_document(&snapshot, &query, &media, &pages)
        );
    }
    let value: Value = serde_json::from_slice(&body_document(
        &snapshot,
        &MessageQuery::default(),
        &media,
        &pages,
    ))
    .unwrap();
    let texts = value["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        ["inherited", "m0", "m1", "/rename Named", "m2", "m3", "m4"]
    );
    assert_eq!(value["activity"]["state"], "working");
    assert_eq!(value["message_total"], 6);
}

#[test]
fn media_messages_are_projected_per_request_and_still_match_the_value_renderer() {
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
    let image = |data: &str| json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":data}});
    let mut rows = claude_chain(6);
    // Two embedded images, twenty embedded images, a typed file reference
    // and a text-discovered reference (`media_more` grants are covered by
    // the synthetic view below).
    rows[1]["message"]["content"] = json!([{"type":"text","text":"two"}, image(PNG), image(PNG)]);
    rows[3]["message"]["content"] = Value::Array(
        std::iter::once(json!({"type":"text","text":"twenty"}))
            .chain((0..20).map(|_| image(PNG)))
            .collect(),
    );
    rows[4]["message"]["content"] = json!([
        {"type":"text","text":"file"},
        {"type":"image","source":{"path":"/private/shot.png"}}
    ]);
    rows[5]["message"]["content"] =
        json!([{"type":"text","text":"see ![shot](/private/shot.png)"}]);
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("claude");
    let file = root.join("project/session.jsonl");
    write_rows(&file, &rows);
    let store = SessionStore::new(SessionRoots {
        claude: Some(root),
        ..Default::default()
    });
    let uid = store.list(false).unwrap()["sessions"][0]["uid"]
        .as_str()
        .unwrap()
        .to_owned();
    let pages = PageStore::default();
    let snapshot = store.snapshot(&uid, "").unwrap();
    // The encoder flags exactly the events whose projection is not a pure
    // function of the event: typed images and text-discovered references.
    let entries = snapshot.view.parsed.encoded.entries();
    let events = &snapshot.view.parsed.events;
    assert_eq!(entries.len(), events.len());
    for (entry, event) in entries.iter().zip(events) {
        let discovered = crate::media::discover(&event.message).len();
        assert_eq!(entry.special, !event.media.is_empty() || discovered > 0);
        assert_eq!(entry.discovered, discovered);
    }
    let special = entries.iter().filter(|entry| entry.special).count();
    assert!(special >= 4 && special < entries.len(), "{special}");
    assert_eq!(
        entries.iter().filter(|entry| entry.discovered > 0).count(),
        1
    );
    let media = MediaStore::new();
    let cold = body_document(&snapshot, &MessageQuery::default(), &media, &pages);
    assert_eq!(
        without_tokens(&cold),
        without_tokens(&value_document(
            &snapshot,
            &MessageQuery::default(),
            &media,
            &pages
        ))
    );
    let value: Value = serde_json::from_slice(&cold).unwrap();
    let messages = value["messages"].as_array().unwrap();
    let with_media = |predicate: &dyn Fn(&Value) -> bool| {
        messages
            .iter()
            .enumerate()
            .find(|(_, message)| predicate(message))
            .map(|(index, _)| index)
            .unwrap()
    };
    let embedded = with_media(&|m| m["media"][0]["src"].is_string());
    let file = with_media(&|m| m["media"][0]["error"]["code"] == "media_files_disabled");
    // Claude projects one message per image block: 22 embedded descriptors.
    assert_eq!(
        messages
            .iter()
            .filter(|m| m["media"][0]["src"].is_string())
            .count(),
        22
    );
    assert_eq!(messages[file]["text"], "[图片]");
    assert_eq!(
        messages
            .iter()
            .filter(|m| m["media"][0]["error"]["code"] == "media_files_disabled")
            .count(),
        2,
        "the typed file reference and the text-discovered one"
    );
    assert!(messages[0].get("media").is_none());
    // Embedded descriptors keep their tokens across reads, and a hot read
    // registers them again: a fresh store answers the same `src`.
    let fresh = MediaStore::new();
    let hot = body_document(&snapshot, &MessageQuery::default(), &fresh, &pages);
    assert_eq!(without_tokens(&hot), without_tokens(&cold));
    let src = messages[embedded]["media"][0]["src"].as_str().unwrap();
    let again: Value = serde_json::from_slice(&hot).unwrap();
    assert_eq!(again["messages"][embedded]["media"][0]["src"], src);
    let token = src.rsplit('/').next().unwrap();
    assert!(fresh.ticket(token).is_some());
    for query in [window(), continuation(&cold, false)] {
        assert_eq!(
            without_tokens(&body_document(&snapshot, &query, &media, &pages)),
            without_tokens(&value_document(&snapshot, &query, &media, &pages))
        );
    }
}

#[test]
fn a_message_with_more_than_sixteen_images_mints_its_grant_per_request() {
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
    let image = || {
        crate::media::NativeImage::from_block(&json!({"type":"image","source":{
            "type":"base64","media_type":"image/png","data":PNG}}))
        .unwrap()
        .unwrap()
    };
    let events = (0..3)
        .map(|i| Event {
            end: 10 + i as u64,
            message: json!({"role":"user","text":format!("m{i}"),"ts":NOW,"name":null,"args":null}),
            media: if i == 1 {
                (0..20).map(|_| image()).collect()
            } else {
                Vec::new()
            },
        })
        .collect::<Vec<_>>();
    let meta = json!({"uid":"codex:synthetic-media","agent_id":"","source":"codex",
        "sid":"thread-1","cwd":"/synthetic","supported":true});
    let bytes = b"{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n";
    let parsed = Arc::new(Parsed {
        candidate: Candidate {
            source: "codex",
            root: PathBuf::from("/synthetic"),
            path: PathBuf::from("/synthetic/rollout.jsonl"),
            data: PathBuf::from("/synthetic/rollout.jsonl"),
            summary: None,
            stamps: vec![super::super::FileStamp {
                size: bytes.len() as u64,
                modified: 0,
                identity: "inode:ctime".into(),
                file_identity: "inode".into(),
            }],
        },
        raw_index: native_input::RawIndex::scan(&bytes[..]).unwrap(),
        _fixture: None,
        committed: bytes.len(),
        meta: meta.clone(),
        native_id: Ok("thread-1".into()),
        encoded: EncodedEvents::build(&events, true, None).unwrap(),
        events,
        unsupported: None,
        raw_error: None,
        pin: None,
    });
    let snapshot = ViewSnapshot::new(Arc::new(View::new(ViewParts {
        inherited_encoded: Arc::new(EncodedEvents::build(&[], true, None).unwrap()),
        inherited: Arc::new(Vec::new()),
        sources: vec![parsed.candidate.clone()],
        parsed,
        meta,
        identity: "synthetic-identity".into(),
        native_scope: Err(SessionError::new(501, "unused")),
        dependencies: vec!["codex:synthetic-media".into()],
    })));
    let media = MediaStore::new();
    let pages = PageStore::default();
    let body = body_document(&snapshot, &MessageQuery::default(), &media, &pages);
    assert_eq!(
        without_tokens(&body),
        without_tokens(&value_document(
            &snapshot,
            &MessageQuery::default(),
            &media,
            &pages
        ))
    );
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["messages"][1]["media"].as_array().unwrap().len(), 16);
    assert_eq!(value["messages"][1]["media_more"]["remaining"], 4);
    let token = value["messages"][1]["media_more"]["cursor"]
        .as_str()
        .unwrap();
    assert!(
        pages
            .lookup_media(token, "codex:synthetic-media", "")
            .is_ok()
    );
    // Every read mints its own grant; the rest of the document is stable.
    let again = body_document(&snapshot, &MessageQuery::default(), &media, &pages);
    assert_ne!(again, body);
    assert_eq!(without_tokens(&again), without_tokens(&body));
}
