//! Synthetic native inputs only; projection and cursor boundaries for media.
use super::*;
use std::io::Write;
use tempfile::TempDir;

fn row(index: usize, image: bool) -> Value {
    let content = if image {
        // Valid base64 syntax but not an image. Only an actual media GET should
        // decode it; projection registers a private source without inspecting it.
        json!([{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}])
    } else {
        json!([{"type":"text","text":format!("synthetic message {index}")}])
    };
    json!({"type":"user","uuid":format!("u{index}"),
        "parentUuid":if index == 0 { Value::Null } else { json!(format!("u{}",index-1)) },
        "sessionId":"media-scope","message":{"role":"user","content":content}})
}

fn fixture(rows: &[Value]) -> (TempDir, PathBuf, SessionStore, String) {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("project/media.jsonl");
    fs::create_dir(path.parent().unwrap()).unwrap();
    let mut file = fs::File::create(&path).unwrap();
    for row in rows {
        writeln!(file, "{row}").unwrap();
    }
    let store = SessionStore::new(SessionRoots {
        claude: Some(temp.path().to_owned()),
        ..Default::default()
    });
    let list = store.list(false).unwrap();
    let uid = list["sessions"][0]["uid"].as_str().unwrap().to_owned();
    (temp, path, store, uid)
}

#[test]
fn text_consumers_do_not_decode_or_serialize_private_image_payloads() {
    let (_temp, path, store, uid) = fixture(&[row(0, true)]);
    let before = fs::read(&path).unwrap();
    let snapshot = store.snapshot(&uid, "").unwrap();
    let text = snapshot.messages(&MessageQuery::default()).unwrap();
    assert!(
        text["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["text"] == "[图片]")
    );
    let pool = store.search_pool().unwrap();
    let scanned = store.search_view(&pool, &uid).unwrap();
    for value in [
        text,
        scanned.messages(&MessageQuery::default()).unwrap(),
        json!(crate::search::body(&scanned)),
    ] {
        let encoded = value.to_string();
        assert!(!encoded.contains("AAAA"));
        assert!(!encoded.contains("base64"));
        assert!(!encoded.contains("/api/media/"));
    }
    let media = crate::media::MediaStore::new();
    let batch = snapshot
        .messages_with_media(&MessageQuery::default(), &media)
        .unwrap();
    let descriptor = &batch["messages"][0]["media"][0];
    assert_eq!(descriptor["lazy"], true);
    assert!(descriptor.get("width").is_none());
    assert!(descriptor.get("mime").is_none());
    let token = descriptor["src"]
        .as_str()
        .unwrap()
        .strip_prefix("/api/media/")
        .unwrap();
    assert_eq!(
        media
            .materialize(media.ticket(token).unwrap(), None)
            .unwrap()
            .bytes(),
        &[0, 0, 0]
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn omitted_window_images_are_not_registered_and_selected_images_materialize_on_get() {
    let rows = (0..701)
        .map(|index| row(index, index == 150))
        .collect::<Vec<_>>();
    let (_temp, _path, store, uid) = fixture(&rows);
    let snapshot = store.snapshot(&uid, "").unwrap();
    let media = crate::media::MediaStore::new();
    let batch = snapshot
        .messages_with_media(
            &MessageQuery {
                window: "1".into(),
                ..Default::default()
            },
            &media,
        )
        .unwrap();
    assert_eq!(batch["partial"]["omitted"], 101);
    assert_eq!(batch["messages"].as_array().unwrap().len(), 600);
    assert!(!batch.to_string().contains("AAAA"));
    assert!(!batch.to_string().contains("/api/media/"));
    let full = snapshot
        .messages_with_media(&MessageQuery::default(), &media)
        .unwrap();
    let token = full["messages"][150]["media"][0]["src"]
        .as_str()
        .unwrap()
        .strip_prefix("/api/media/")
        .unwrap();
    assert_eq!(
        media
            .materialize(media.ticket(token).unwrap(), None)
            .unwrap()
            .bytes(),
        &[0, 0, 0]
    );
}

#[test]
fn append_projection_does_not_decode_already_consumed_images() {
    let (_temp, path, store, uid) = fixture(&[row(0, true)]);
    let first = store.messages(&uid, &MessageQuery::default()).unwrap();
    let query = MessageQuery {
        start: first["end"].as_u64().unwrap(),
        head: first["version"]["head"].as_str().unwrap().into(),
        anchor: first["anchor"].as_str().unwrap().into(),
        ..Default::default()
    };
    writeln!(
        fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{}",
        row(1, false)
    )
    .unwrap();
    let snapshot = store.snapshot(&uid, "").unwrap();
    let batch = snapshot
        .messages_with_media(&query, &crate::media::MediaStore::new())
        .unwrap();
    assert_eq!(
        batch["reset"], false,
        "new random registration token must not change semantic prefix"
    );
    assert_eq!(batch["messages"].as_array().unwrap().len(), 1);
    assert_eq!(batch["messages"][0]["text"], "synthetic message 1");
}

#[test]
fn deferred_image_source_survives_ordinary_append_and_old_view_release() {
    let mut record = row(0, true);
    record["message"]["content"][0]["source"]["data"] = json!(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="
    );
    let (_temp, path, store, uid) = fixture(&[record]);
    let media = crate::media::MediaStore::new();
    let snapshot = store.snapshot(&uid, "").unwrap();
    let first = snapshot
        .messages_with_media(&MessageQuery::default(), &media)
        .unwrap();
    let token = first["messages"][0]["media"][0]["src"]
        .as_str()
        .unwrap()
        .strip_prefix("/api/media/")
        .unwrap()
        .to_owned();
    let query = MessageQuery {
        start: first["end"].as_u64().unwrap(),
        head: first["version"]["head"].as_str().unwrap().into(),
        anchor: first["anchor"].as_str().unwrap().into(),
        ..Default::default()
    };
    drop(snapshot);
    writeln!(
        fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{}",
        row(1, false)
    )
    .unwrap();
    let latest = store.snapshot(&uid, "").unwrap();
    let delta = latest.messages_with_media(&query, &media).unwrap();
    assert_eq!(delta["reset"], false);
    assert_eq!(delta["messages"].as_array().unwrap().len(), 1);
    let current = latest
        .messages_with_media(&MessageQuery::default(), &media)
        .unwrap();
    assert_ne!(
        first["messages"][0]["media"][0]["src"],
        current["messages"][0]["media"][0]["src"]
    );
    drop(latest);
    drop(store);
    // No snapshot owner is needed to retain this separately charged embedded
    // descriptor. No decode was requested until after the provider reprojected.
    let blob = media
        .materialize(media.ticket(&token).unwrap(), None)
        .unwrap();
    assert_eq!(blob.mime(), "image/png");
    assert!(blob.bytes().starts_with(b"\x89PNG\r\n\x1a\n"));
}

#[test]
fn image_batch_has_no_message_count_rejection() {
    let rows = (0..257).map(|index| row(index, true)).collect::<Vec<_>>();
    let (_temp, _path, store, uid) = fixture(&rows);
    let snapshot = store.snapshot(&uid, "").unwrap();
    let projected = snapshot
        .messages_with_media(&MessageQuery::default(), &crate::media::MediaStore::new())
        .unwrap();
    assert_eq!(projected["messages"].as_array().unwrap().len(), 257);
    assert!(snapshot.messages(&MessageQuery::default()).is_ok());
}

#[test]
fn missing_file_roots_preserve_mixed_native_order_text_and_cursor() {
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
    let mut record = row(0, false);
    record["message"]["content"] = json!([
        {"type":"text","text":"before"},
        {"type":"image","source":{"path":"/private/first.png"}},
        {"type":"image","mime_type":"image/png","data":png},
        {"type":"image","source":{"path":"/private/last.png"}},
        {"type":"text","text":"after"}
    ]);
    let (_temp, path, store, uid) = fixture(&[record]);
    let before = fs::read(&path).unwrap();
    let snapshot = store.snapshot(&uid, "").unwrap();
    let text = snapshot.messages(&MessageQuery::default()).unwrap();
    let batch = snapshot
        .messages_with_files(
            &MessageQuery::default(),
            &crate::media::MediaStore::new(),
            None,
        )
        .unwrap();
    assert_eq!(text["anchor"], batch["anchor"]);
    let messages = batch["messages"].as_array().unwrap();
    assert_eq!(batch["message_total"], 5);
    assert_eq!(
        messages
            .iter()
            .map(|message| message["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["before", "[图片]", "[图片]", "[图片]", "after"]
    );
    let media = messages
        .iter()
        .flat_map(|message| message["media"].as_array().into_iter().flatten())
        .collect::<Vec<_>>();
    assert_eq!(media[0]["error"]["code"], "media_files_disabled");
    assert!(media[1]["src"].is_string());
    assert_eq!(media[2]["error"]["code"], "media_files_disabled");
    let rendered = batch.to_string();
    assert!(!rendered.contains("/private/first.png") && !rendered.contains("/private/last.png"));
    assert_eq!(fs::read(path).unwrap(), before);
}
