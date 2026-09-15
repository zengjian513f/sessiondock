//! Synthetic pagination grants and semantic checkpoints; no native I/O or CLI.
use super::*;
use crate::sessions::{Candidate, FileStamp, Parsed, View};
use std::{path::PathBuf, sync::Arc};

const AGED: Duration = Duration::from_secs(1);

const UID: &str = "claude:synthetic-page-owner";
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

fn event(index: usize, end: u64) -> Event {
    Event {
        end,
        message: json!({"role":"assistant","text":format!("message-{index}")}),
        media: vec![],
    }
}
fn image(encoded: &str) -> crate::media::NativeImage {
    crate::media::NativeImage::from_block(&json!({"type":"image","source":{
        "type":"base64","media_type":"image/png","data":encoded}}))
    .unwrap()
    .unwrap()
}
fn snapshot(events: Vec<Event>, bytes: &[u8], identity: &str, agent: &str) -> ViewSnapshot {
    let meta = json!({"uid":UID,"agent_id":agent,"source":"claude","sid":"synthetic-page",
        "cwd":"/synthetic","supported":true});
    let leaf = events
        .iter()
        .filter(|event| event.end != 0)
        .cloned()
        .collect::<Vec<_>>();
    let parsed = Arc::new(Parsed {
        candidate: Candidate {
            source: "claude",
            root: PathBuf::from("/synthetic"),
            path: PathBuf::from("/synthetic/page.jsonl"),
            data: PathBuf::from("/synthetic/page.jsonl"),
            summary: None,
            stamps: vec![FileStamp {
                size: bytes.len() as u64,
                modified: 0,
                identity: "inode:ctime".into(),
                file_identity: "inode".into(),
            }],
        },
        raw_index: crate::sessions::native_input::RawIndex::scan(bytes).unwrap(),
        _fixture: None,
        committed: bytes.len(),
        meta: meta.clone(),
        native_id: Ok("synthetic-page".into()),
        encoded: crate::sessions::EncodedEvents::build(&leaf, true, None).unwrap(),
        events: leaf,
        unsupported: None,
        raw_error: None,
        pin: None,
    });
    let inherited = events
        .iter()
        .filter(|event| event.end == 0)
        .cloned()
        .collect::<Vec<_>>();
    let sources = vec![parsed.candidate.clone()];
    let inherited_encoded =
        Arc::new(crate::sessions::EncodedEvents::build(&inherited, true, None).unwrap());
    ViewSnapshot::new(Arc::new(View::new(crate::sessions::ViewParts {
        parsed,
        meta,
        inherited: Arc::new(inherited),
        inherited_encoded,
        dependencies: vec![UID.into()],
        identity: identity.into(),
        native_scope: Err(SessionError::new(501, "synthetic scope unused")),
        sources,
    })))
}
fn plain(count: usize) -> ViewSnapshot {
    snapshot(
        (0..count).map(|index| event(index, 3)).collect(),
        b"{}\n",
        "synthetic-identity",
        "",
    )
}
fn grant(snapshot: &ViewSnapshot, next: usize, stop: usize, total: usize) -> PageGrant {
    PageGrant {
        uid: UID.into(),
        agent: snapshot.view.meta["agent_id"].as_str().unwrap().into(),
        checkpoint: MessageQuery {
            start: snapshot.view.parsed.committed as u64,
            head: snapshot.head.clone(),
            anchor: snapshot.anchor.clone(),
            ..Default::default()
        },
        next,
        stop,
        total,
        issued: Instant::now(),
    }
}
/// The pre-batch-44 200-event page keeps these choreographies exact; the
/// 2000-event default is covered by `default_page_events_fill_a_gap_in_one_read`.
const MAX_EVENTS: usize = 200;
fn store() -> PageStore {
    PageStore::with_page_events(MAX_EVENTS)
}
fn initial(snapshot: &ViewSnapshot, pages: &PageStore) -> Value {
    snapshot
        .messages_with_pages(
            &MessageQuery {
                window: "1".into(),
                ..Default::default()
            },
            &MediaStore::new(),
            None,
            pages,
        )
        .unwrap()
}
fn read(snapshot: &ViewSnapshot, pages: &PageStore, token: &str, agent: &str) -> Value {
    let grant = pages.lookup(token, UID, agent).unwrap();
    snapshot
        .history_page(grant, token, &MediaStore::new(), None, pages)
        .unwrap()
}
fn select(snapshot: &ViewSnapshot) -> Vec<Selected<'_>> {
    snapshot
        .view
        .events()
        .filter(|event| event.message["role"] != "status")
        .enumerate()
        .map(|(index, event)| Selected { index, event })
        .collect()
}
fn media_read(
    snapshot: &ViewSnapshot,
    pages: &PageStore,
    token: &str,
    agent: &str,
) -> Result<Value, SessionError> {
    let grant = pages.lookup_media(token, UID, agent)?;
    snapshot.media_page(grant, token, &MediaStore::new(), None, pages)
}

#[test]
fn malformed_missing_and_cross_scope_tokens_fail_with_distinct_statuses() {
    let pages = store();
    let snapshot = plain(10);
    let token = pages.issue_page(grant(&snapshot, 0, 10, 10)).unwrap();
    for invalid in [
        "".to_owned(),
        "a".repeat(31),
        "a".repeat(33),
        "A".repeat(32),
        "g".repeat(32),
        "../page".into(),
    ] {
        assert_eq!(pages.lookup(&invalid, UID, "").err().unwrap().status, 400);
    }
    assert_eq!(
        pages.lookup(&"0".repeat(32), UID, "").err().unwrap().status,
        404
    );
    assert_eq!(
        pages
            .lookup(&token, "claude:other", "")
            .err()
            .unwrap()
            .status,
        403
    );
    assert_eq!(
        pages
            .lookup(&token, UID, "other-agent")
            .err()
            .unwrap()
            .status,
        403
    );
    assert!(pages.lookup(&token, UID, "").is_ok());
}

#[test]
fn expiry_uses_manual_issued_time_and_removes_only_expired_grants() {
    let pages = store();
    let snapshot = plain(10);
    let expired = pages.issue_page(grant(&snapshot, 0, 10, 10)).unwrap();
    let live = pages.issue_page(grant(&snapshot, 0, 10, 10)).unwrap();
    pages.age(&expired, GRANT_TTL + AGED);
    assert_eq!(pages.lookup(&expired, UID, "").err().unwrap().status, 410);
    assert_eq!(pages.lookup(&expired, UID, "").err().unwrap().status, 404);
    assert!(pages.lookup(&live, UID, "").is_ok());
    pages.age(&live, GRANT_TTL + AGED);
    let newest = pages.issue_page(grant(&snapshot, 0, 10, 10)).unwrap();
    assert_eq!(pages.lookup(&live, UID, "").err().unwrap().status, 404);
    assert!(pages.lookup(&newest, UID, "").is_ok());
}

#[test]
fn grant_cap_evicts_oldest_without_pinning_or_consuming_newer_grants() {
    let pages = store();
    let snapshot = plain(1);
    let oldest = pages.issue_page(grant(&snapshot, 0, 1, 1)).unwrap();
    pages.age(&oldest, Duration::from_secs(2));
    for _ in 1..MAX_GRANTS {
        pages.issue_page(grant(&snapshot, 0, 1, 1)).unwrap();
    }
    assert_eq!(pages.len(), MAX_GRANTS);
    let newest = pages.issue_page(grant(&snapshot, 0, 1, 1)).unwrap();
    assert_eq!(pages.len(), MAX_GRANTS);
    assert_eq!(pages.lookup(&oldest, UID, "").err().unwrap().status, 404);
    assert!(pages.lookup(&newest, UID, "").is_ok());
    // Grants only retain strings/checkpoints, not the source view's Arc.
    assert_eq!(Arc::strong_count(&snapshot.view), 1);
}

#[test]
fn repeated_page_reads_do_not_consume_grant_or_advance_live_cursor() {
    let snapshot = plain(1001);
    let pages = store();
    let batch = initial(&snapshot, &pages);
    let token = batch["partial"]["cursor"].as_str().unwrap();
    let before = snapshot.cursor();
    let first = read(&snapshot, &pages, token, "");
    let again = read(&snapshot, &pages, token, "");
    assert_eq!(first["messages"], again["messages"]);
    assert_eq!(first["page"]["start"], 100);
    assert_eq!(first["page"]["end"], 300);
    assert_eq!(first["page"]["remaining"], 201);
    for value in [&first, &again] {
        assert!(value.get("end").is_none());
        assert!(value.get("anchor").is_none());
        assert!(value.get("meta").is_none());
        let next = pages
            .lookup(value["page"]["next"].as_str().unwrap(), UID, "")
            .unwrap();
        assert_eq!((next.next, next.stop, next.total), (300, 501, 1001));
        assert_eq!(next.checkpoint.anchor, snapshot.anchor);
    }
    assert!(pages.lookup(token, UID, "").is_ok());
    assert_eq!(snapshot.cursor(), before);
}

#[test]
fn initial_window_and_pages_use_nonstatus_indexes_even_when_offsets_are_identical() {
    let mut events = Vec::new();
    for index in 0..1001 {
        events.push(Event {
            end: 3,
            message: json!({"role":"status","text":"working","state":"working"}),
            media: vec![],
        });
        let mut next = event(index, 3);
        if index == 120 {
            next.message["counted"] = json!(false);
        }
        events.push(next);
    }
    let snapshot = snapshot(events, b"{}\n", "synthetic-identity", "exact-agent");
    let pages = store();
    let batch = initial(&snapshot, &pages);
    assert_eq!(batch["partial"]["head"], 100);
    assert_eq!(batch["partial"]["tail"], 500);
    assert_eq!(batch["partial"]["omitted"], 401);
    assert_eq!(batch["messages"][99]["text"], "message-99");
    assert_eq!(batch["messages"][100]["text"], "message-501");
    assert_eq!(batch["message_total"], 1000); // counted:false still occupies an index.
    let token = batch["partial"]["cursor"].as_str().unwrap();
    let saved = pages.lookup(token, UID, "exact-agent").unwrap();
    assert_eq!((saved.next, saved.stop, saved.total), (100, 501, 1001));
    assert_eq!(saved.checkpoint.start, 3);
    assert_eq!(saved.checkpoint.head, snapshot.head);
    assert_eq!(saved.checkpoint.anchor, snapshot.anchor);
    let page = read(&snapshot, &pages, token, "exact-agent");
    assert_eq!(page["messages"].as_array().unwrap().len(), 200);
    assert_eq!(page["messages"][0]["text"], "message-100");
    assert_eq!(page["messages"][199]["text"], "message-299");
}

#[test]
fn ordinary_append_keeps_original_gap_and_never_pages_new_tail_events() {
    let original = plain(1001);
    let pages = store();
    let batch = initial(&original, &pages);
    let mut events = original.view.all_events();
    events.extend((1001..1051).map(|index| event(index, 6)));
    let current = snapshot(events, b"{}\n{}\n", "synthetic-identity", "");
    let mut token = batch["partial"]["cursor"].as_str().unwrap().to_owned();
    let mut texts = Vec::new();
    loop {
        let page = read(&current, &pages, &token, "");
        assert_eq!(page["page"]["stop"], 501);
        texts.extend(
            page["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|message| message["text"].as_str().unwrap().to_owned()),
        );
        if let Some(next) = page["page"]["next"].as_str() {
            token = next.into();
        } else {
            assert_eq!(page["page"]["remaining"], 0);
            break;
        }
    }
    assert_eq!(
        texts,
        (100..501)
            .map(|index| format!("message-{index}"))
            .collect::<Vec<_>>()
    );
}

#[test]
fn changed_old_semantics_rewrite_rewind_or_inherited_identity_reject_checkpoint() {
    let original = plain(701);
    let pages = store();
    let batch = initial(&original, &pages);
    let token = batch["partial"]["cursor"].as_str().unwrap();
    let mut changed = original.view.all_events();
    changed[120].message["interrupted"] = json!(true);
    let cases = [
        snapshot(changed, b"{}\n{}\n", "synthetic-identity", ""),
        snapshot(
            original.view.all_events(),
            b"[]\n",
            "synthetic-identity",
            "",
        ),
        snapshot(vec![], b"", "synthetic-identity", ""),
        snapshot(
            original.view.all_events(),
            b"{}\n",
            "changed-parent-prefix",
            "",
        ),
        snapshot(
            original.view.all_events(),
            b"{}\n",
            "synthetic-identity",
            "other-agent",
        ),
    ];
    for current in cases {
        let grant = pages.lookup(token, UID, "").unwrap();
        assert_eq!(
            current
                .history_page(grant, token, &MediaStore::new(), None, &pages)
                .unwrap_err()
                .status,
            409
        );
    }
}

#[test]
fn inherited_zero_offset_events_are_paged_as_real_indexes() {
    let events = (0..701)
        .map(|index| event(index, if index < 650 { 0 } else { 3 }))
        .collect();
    let original = snapshot(events, b"{}\n", "fixed-parent-prefix", "");
    let pages = store();
    let batch = initial(&original, &pages);
    let token = batch["partial"]["cursor"].as_str().unwrap();
    let page = read(&original, &pages, token, "");
    assert_eq!(page["messages"].as_array().unwrap().len(), 101);
    assert_eq!(page["messages"][0]["text"], "message-100");
    assert_eq!(page["messages"][100]["text"], "message-200");
    assert!(page["page"]["next"].is_null());
}

#[test]
fn invalid_grant_ranges_and_totals_are_rejected_instead_of_sliced() {
    let snapshot = plain(10);
    let pages = store();
    for (next, stop, total) in [(0, 10, 11), (10, 10, 10), (11, 10, 10), (0, 11, 10)] {
        assert_eq!(
            snapshot
                .history_page(
                    grant(&snapshot, next, stop, total),
                    "unused",
                    &MediaStore::new(),
                    None,
                    &pages
                )
                .unwrap_err()
                .status,
            409
        );
    }
}

#[test]
fn initial_dynamic_window_prioritizes_tail_with_native_image_budget() {
    let image = image(PNG);
    let mut events = (0..701).map(|index| event(index, 3)).collect::<Vec<_>>();
    for event in &mut events {
        event.media.push(image.clone());
    }
    let snapshot = snapshot(events, b"{}\n", "images", "");
    let pages = store();
    let mut selected = select(&snapshot);
    let partial = window(&snapshot, &mut selected, &pages).unwrap();
    assert_eq!(partial["head"], 0);
    assert_eq!(partial["tail"], 128);
    assert_eq!(partial["omitted"], 573);
    assert_eq!(
        selected.first().unwrap().event.message["text"],
        "message-573"
    );
    assert_eq!(
        selected.last().unwrap().event.message["text"],
        "message-700"
    );
    let saved = pages
        .lookup(partial["cursor"].as_str().unwrap(), UID, "")
        .unwrap();
    assert_eq!((saved.next, saved.stop, saved.total), (0, 573, 701));
}

#[test]
fn complete_small_window_has_no_grant_and_large_json_tail_is_dynamic() {
    let small = plain(40);
    let pages = store();
    assert!(initial(&small, &pages)["partial"].is_null());
    assert_eq!(pages.len(), 0);
    let mut events = (0..10).map(|index| event(index, 3)).collect::<Vec<_>>();
    for event in &mut events {
        event.message["padding"] = json!("x".repeat(1024 * 1024));
    }
    let snapshot = snapshot(events, b"{}\n", "large-json", "");
    let mut selected = select(&snapshot);
    let partial = window(&snapshot, &mut selected, &pages).unwrap();
    assert_eq!(partial["head"], 0);
    assert_eq!(partial["tail"], 7);
    assert_eq!(partial["omitted"], 3);
    assert_eq!(selected[0].event.message["text"], "message-3");
}

#[test]
fn budget_accounts_for_json_overhead_and_final_response_wrapper() {
    let mut fitting = event(0, 3);
    let envelope = Budget::default().json_bytes;
    let empty_len = serde_json::to_vec(&fitting.message).unwrap().len();
    fitting.message["text"] =
        json!("x".repeat(MAX_JSON_BYTES - envelope - empty_len + "message-0".len() - 32));
    let mut budget = Budget::default();
    assert!(budget.take(&fitting, MAX_EVENTS).unwrap());
    assert_eq!(budget.json_bytes, MAX_JSON_BYTES);
    assert!(!budget.take(&event(1, 3), MAX_EVENTS).unwrap());
    let mut too_large = fitting.clone();
    too_large.message["extra"] = json!("x".repeat(64));
    assert!(Budget::default().take(&too_large, MAX_EVENTS).unwrap());
    assert!(
        validate_response(
            &json!({"messages":[fitting.message],"page":{"extra":"x".repeat(envelope+128)}})
        )
        .is_ok()
    );
}

#[test]
fn embedded_byte_and_image_count_budgets_are_distinct_and_disk_refs_are_not_decoded() {
    let large = image(&"AAAA".repeat(512 * 1024));
    let mut full = event(0, 3);
    full.media = vec![large.clone(); 16];
    let mut budget = Budget::default();
    assert!(budget.take(&full, MAX_EVENTS).unwrap());
    assert_eq!(budget.image_bytes, MAX_IMAGE_BYTES);
    let mut extra = event(1, 3);
    extra.media.push(large.clone());
    assert!(!budget.take(&extra, MAX_EVENTS).unwrap());
    let file = crate::media::NativeImage::from_block(
        &json!({"type":"image","path":"/synthetic/not-opened.png"}),
    )
    .unwrap()
    .unwrap();
    let mut files = event(0, 3);
    files.media = vec![file; DISPLAY_LIMIT];
    let mut budget = Budget::default();
    assert!(budget.take(&files, MAX_EVENTS).unwrap());
    assert_eq!(budget.image_bytes, 0);
    assert_eq!(budget.images, DISPLAY_LIMIT);
    assert_eq!(
        budget.json_bytes,
        Budget::default().json_bytes
            + serde_json::to_vec(&files.message).unwrap().len()
            + DISPLAY_LIMIT * 8192
            + 32
    );
    // Images past the inline limit are continued through media pages: they
    // are neither charged here nor a reason to call the message too large.
    let mut overflow = files.clone();
    overflow.media.extend(vec![large; MAX_IMAGES]);
    let mut displayed_only = Budget::default();
    assert!(displayed_only.take(&overflow, MAX_EVENTS).unwrap());
    assert_eq!(displayed_only.images, DISPLAY_LIMIT);
    assert_eq!(displayed_only.image_bytes, 0);
    assert_eq!(displayed_only.json_bytes, budget.json_bytes);
    let mut counted = Budget::default();
    for _ in 0..MAX_IMAGES / DISPLAY_LIMIT {
        assert!(counted.take(&overflow, MAX_EVENTS).unwrap());
    }
    assert_eq!(counted.images, MAX_IMAGES);
    assert!(!counted.take(&overflow, MAX_EVENTS).unwrap());
    let mut heavy_tail = event(0, 3);
    heavy_tail.media = vec![image(PNG)];
    heavy_tail
        .media
        .extend(vec![image(&"AAAA".repeat(512 * 1024)); 24]);
    let mut prefix = Budget::default();
    assert!(prefix.take(&heavy_tail, MAX_EVENTS).unwrap());
    assert_eq!(prefix.images, DISPLAY_LIMIT);
    assert_eq!(
        prefix.image_bytes,
        heavy_tail.media[..DISPLAY_LIMIT]
            .iter()
            .map(|image| image.encoded_len() / 4 * 3)
            .sum::<usize>()
    );

    let mut reference = event(0, 3);
    reference.message["text"] = json!("![image](./not-opened.png)");
    let mut lexical = Budget::default();
    assert!(lexical.take(&reference, MAX_EVENTS).unwrap());
    assert_eq!(lexical.images, 0);
    assert_eq!(
        lexical.json_bytes,
        Budget::default().json_bytes
            + serde_json::to_vec(&reference.message).unwrap().len()
            + 8192
            + 32
    );
}

#[test]
fn an_oversized_next_event_does_not_discard_progress_or_inspect_past_event_limit() {
    let mut oversized = event(1, 3);
    oversized.message["text"] = json!("x".repeat(MAX_JSON_BYTES));
    assert!(Budget::default().take(&oversized, MAX_EVENTS).unwrap());
    let mut progressed = Budget::default();
    assert!(progressed.take(&event(0, 3), MAX_EVENTS).unwrap());
    assert!(!progressed.take(&oversized, MAX_EVENTS).unwrap());
    assert_eq!(progressed.events, 1);
    let mut events = (0..MAX_EVENTS)
        .map(|index| event(index, 3))
        .collect::<Vec<_>>();
    events.push(oversized);
    let snapshot = snapshot(events, b"{}\n", "oversized-next", "");
    let pages = store();
    let token = pages
        .issue_page(grant(&snapshot, 0, MAX_EVENTS + 1, MAX_EVENTS + 1))
        .unwrap();
    let page = read(&snapshot, &pages, &token, "");
    assert_eq!(page["messages"].as_array().unwrap().len(), MAX_EVENTS);
    assert_eq!(page["page"]["remaining"], 1);
    let next = page["page"]["next"].as_str().unwrap();
    let grant = pages.lookup(next, UID, "").unwrap();
    let final_page = snapshot
        .history_page(grant, next, &MediaStore::new(), None, &pages)
        .unwrap();
    assert_eq!(final_page["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        final_page["messages"][0]["text"].as_str().unwrap().len(),
        MAX_JSON_BYTES
    );
    assert_eq!(final_page["page"]["remaining"], 0);
    assert!(final_page["page"]["next"].is_null());
    validate_response(&final_page).unwrap();
}

fn many_images(index: usize, count: usize) -> Event {
    let mut event = event(index, 3);
    event.media = (0..count).map(|_| image(PNG)).collect();
    event
}
fn media_grant(snapshot: &ViewSnapshot, index: usize, offset: usize, total: usize) -> MediaGrant {
    let event = select(snapshot)[index].event;
    MediaGrant {
        uid: UID.into(),
        agent: snapshot.view.meta["agent_id"].as_str().unwrap().into(),
        checkpoint: snapshot.checkpoint(),
        index,
        identity: event_identity(event),
        offset,
        total,
        issued: Instant::now(),
        next: None,
    }
}
fn descriptor_srcs(items: &Value) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            assert_eq!(item["lazy"], true);
            item["src"].as_str().unwrap().to_owned()
        })
        .collect()
}

#[test]
fn inline_projection_shows_sixteen_images_and_media_more_binds_a_media_grant() {
    let mut events = (0..5).map(|index| event(index, 3)).collect::<Vec<_>>();
    events[2] = many_images(2, 40);
    events[4] = many_images(4, 16);
    let snapshot = snapshot(events, b"{}\n", "synthetic-identity", "exact-agent");
    let pages = store();
    let batch = initial(&snapshot, &pages);
    assert!(batch["partial"].is_null());
    let messages = batch["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 5);
    assert!(messages[0].get("media").is_none());
    assert!(messages[0].get("media_more").is_none());
    assert_eq!(messages[4]["media"].as_array().unwrap().len(), 16);
    assert!(messages[4].get("media_more").is_none());
    let more = &messages[2]["media_more"];
    assert_eq!(
        messages[2]["media"].as_array().unwrap().len(),
        DISPLAY_LIMIT
    );
    assert_eq!(more["remaining"], 24);
    assert_eq!(more["total"], 40);
    let token = more["cursor"].as_str().unwrap();
    assert_eq!(token.len(), 32);
    let grant = pages.lookup_media(token, UID, "exact-agent").unwrap();
    assert_eq!((grant.index, grant.offset, grant.total), (2, 16, 40));
    assert_eq!(
        grant.identity,
        event_identity(&snapshot.view.all_events()[2])
    );
    assert_eq!(grant.checkpoint.start, 3);
    assert_eq!(grant.checkpoint.head, snapshot.head);
    assert_eq!(grant.checkpoint.anchor, snapshot.anchor);
    // The projected message carries media fields; identity ignores them.
    assert!(messages[2].get("media").is_some());
    let mut with_fields = snapshot.view.all_events()[2].clone();
    with_fields.message["media"] = json!([]);
    assert_ne!(event_identity(&with_fields), grant.identity);
    // A media grant is not a history page grant, whatever its scope.
    assert_eq!(
        pages
            .lookup(token, UID, "exact-agent")
            .err()
            .unwrap()
            .status,
        404
    );
}

#[test]
fn media_pages_advance_sixteen_at_a_time_and_stop_exactly_at_the_total() {
    let mut events = (0..3).map(|index| event(index, 3)).collect::<Vec<_>>();
    events[1] = many_images(1, 40);
    let snapshot = snapshot(events, b"{}\n", "synthetic-identity", "");
    let pages = store();
    let batch = initial(&snapshot, &pages);
    let before = snapshot.cursor();
    let first = batch["messages"][1]["media_more"]["cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    let inline = descriptor_srcs(&batch["messages"][1]["media"]);
    let mut token = first.clone();
    let mut seen = inline.clone();
    let mut bounds = Vec::new();
    loop {
        let grants_before = pages.len();
        let page = media_read(&snapshot, &pages, &token, "").unwrap();
        for absent in ["messages", "meta", "end", "anchor", "version"] {
            assert!(page.get(absent).is_none(), "{absent}");
        }
        let media = &page["media"];
        let start = page["page"]["start"].as_u64().unwrap() as usize;
        let end = page["page"]["end"].as_u64().unwrap() as usize;
        assert_eq!(page["page"]["cursor"], token);
        assert_eq!(page["page"]["total"], 40);
        assert_eq!(end - start, media.as_array().unwrap().len());
        assert!(!media.as_array().unwrap().is_empty());
        assert!(end - start <= DISPLAY_LIMIT);
        assert_eq!(page["page"]["remaining"], 40 - end);
        seen.extend(descriptor_srcs(media));
        bounds.push((start, end));
        let again = media_read(&snapshot, &pages, &token, "").unwrap();
        assert_eq!(again["media"], page["media"]);
        assert_eq!(again["page"]["start"], page["page"]["start"]);
        assert_eq!(again["page"]["next"], page["page"]["next"]);
        assert_eq!(
            pages.len(),
            grants_before + usize::from(!page["page"]["next"].is_null())
        );
        match page["page"]["next"].as_str() {
            Some(next) => {
                assert_ne!(next, token);
                assert_eq!(next.len(), 32);
                token = next.to_owned();
            }
            None => {
                assert_eq!(page["page"]["remaining"], 0);
                break;
            }
        }
    }
    assert_eq!(bounds, [(16, 32), (32, 40)]);
    assert_eq!(seen.len(), 40);
    assert!(pages.lookup_media(&first, UID, "").is_ok());
    assert_eq!(snapshot.cursor(), before);
    assert_eq!(initial(&snapshot, &pages)["end"], batch["end"]);
}

#[test]
fn a_lost_continuation_is_reissued_and_relinked_without_refreshing_the_page_grant() {
    let mut events = (0..2).map(|index| event(index, 3)).collect::<Vec<_>>();
    events[0] = many_images(0, 40);
    let snapshot = snapshot(events, b"{}\n", "synthetic-identity", "");
    let pages = store();
    let first = pages
        .issue_media(media_grant(&snapshot, 0, 16, 40))
        .unwrap();
    let next = media_read(&snapshot, &pages, &first, "").unwrap()["page"]["next"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        pages.lookup_media(&first, UID, "").unwrap().next.as_deref(),
        Some(next.as_str())
    );
    pages.age(&next, GRANT_TTL + AGED);
    let reissued = media_read(&snapshot, &pages, &first, "").unwrap()["page"]["next"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(reissued, next);
    assert_eq!(
        pages.lookup_media(&next, UID, "").err().unwrap().status,
        404
    );
    assert_eq!(
        pages.lookup_media(&first, UID, "").unwrap().next.as_deref(),
        Some(reissued.as_str())
    );
    assert_eq!(
        media_read(&snapshot, &pages, &first, "").unwrap()["page"]["next"],
        reissued
    );
    let last = media_read(&snapshot, &pages, &reissued, "").unwrap();
    assert_eq!(last["page"]["start"], 32);
    assert!(last["page"]["next"].is_null());
    // Linking never extends the page grant's own lifetime.
    pages.age(&first, GRANT_TTL + AGED);
    assert_eq!(
        pages.lookup_media(&first, UID, "").err().unwrap().status,
        410
    );
    assert!(pages.lookup_media(&reissued, UID, "").is_ok());
}

#[test]
fn media_tokens_are_parameter_checked_scoped_and_expire_like_history_pages() {
    let mut events = (0..2).map(|index| event(index, 3)).collect::<Vec<_>>();
    events[0] = many_images(0, 20);
    let snapshot = snapshot(events, b"{}\n", "synthetic-identity", "");
    let pages = store();
    let token = pages
        .issue_media(media_grant(&snapshot, 0, 16, 20))
        .unwrap();
    for invalid in [
        "".to_owned(),
        "a".repeat(31),
        "a".repeat(33),
        "A".repeat(32),
        "g".repeat(32),
        "../media".into(),
    ] {
        assert_eq!(
            pages.lookup_media(&invalid, UID, "").err().unwrap().status,
            400
        );
    }
    assert_eq!(
        pages
            .lookup_media(&"0".repeat(32), UID, "")
            .err()
            .unwrap()
            .status,
        404
    );
    assert_eq!(
        pages
            .lookup_media(&token, "claude:other", "")
            .err()
            .unwrap()
            .status,
        403
    );
    assert_eq!(
        pages
            .lookup_media(&token, UID, "other-agent")
            .err()
            .unwrap()
            .status,
        403
    );
    let page_token = pages.issue_page(grant(&snapshot, 0, 1, 2)).unwrap();
    assert_eq!(
        pages
            .lookup_media(&page_token, UID, "")
            .err()
            .unwrap()
            .status,
        404
    );
    assert!(pages.lookup_media(&token, UID, "").is_ok());
    pages.age(&token, GRANT_TTL + AGED);
    assert_eq!(
        pages.lookup_media(&token, UID, "").err().unwrap().status,
        410
    );
    assert_eq!(
        pages.lookup_media(&token, UID, "").err().unwrap().status,
        404
    );
}

#[test]
fn media_pages_without_a_page_store_still_report_the_remainder_without_a_cursor() {
    let mut events = (0..2).map(|index| event(index, 3)).collect::<Vec<_>>();
    events[1] = many_images(1, 17);
    let snapshot = snapshot(events, b"{}\n", "synthetic-identity", "");
    let batch = snapshot
        .messages_with_media(&MessageQuery::default(), &MediaStore::new())
        .unwrap();
    let message = &batch["messages"][1];
    assert_eq!(message["media"].as_array().unwrap().len(), DISPLAY_LIMIT);
    assert_eq!(
        message["media_more"],
        json!({"remaining": 1, "total": 17, "cursor": null})
    );
    // Text-only consumers never see either media field.
    let text = snapshot.messages(&MessageQuery::default()).unwrap();
    assert!(text["messages"][1].get("media").is_none());
    assert!(text["messages"][1].get("media_more").is_none());
}

#[test]
fn media_pages_reject_changed_messages_totals_checkpoints_and_ranges_with_conflict() {
    let mut events = (0..3).map(|index| event(index, 3)).collect::<Vec<_>>();
    events[1] = many_images(1, 40);
    let original = snapshot(events, b"{}\n", "synthetic-identity", "");
    let pages = store();
    let token = original
        .messages_with_pages(&MessageQuery::default(), &MediaStore::new(), None, &pages)
        .unwrap()["messages"][1]["media_more"]["cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(media_read(&original, &pages, &token, "").is_ok());

    // Ordinary append: same prefix, new events after the checkpoint.
    let mut appended = original.view.all_events();
    appended.push(event(3, 6));
    let appended = snapshot(appended, b"{}\n{}\n", "synthetic-identity", "");
    let page = media_read(&appended, &pages, &token, "").unwrap();
    assert_eq!(page["page"]["start"], 16);
    assert_eq!(page["page"]["end"], 32);

    let mut edited = original.view.all_events();
    edited[1].message["interrupted"] = json!(true);
    let mut fewer = original.view.all_events();
    fewer[1].media.truncate(20);
    let mut shifted = original.view.all_events();
    shifted.insert(0, event(9, 3));
    let cases = [
        snapshot(edited, b"{}\n", "synthetic-identity", ""),
        snapshot(fewer, b"{}\n", "synthetic-identity", ""),
        snapshot(shifted, b"{}\n", "synthetic-identity", ""),
        snapshot(vec![], b"", "synthetic-identity", ""),
        snapshot(
            original.view.all_events(),
            b"[]\n",
            "synthetic-identity",
            "",
        ),
        snapshot(
            original.view.all_events(),
            b"{}\n",
            "changed-parent-prefix",
            "",
        ),
        snapshot(
            original.view.all_events(),
            b"{}\n",
            "synthetic-identity",
            "other-agent",
        ),
    ];
    for current in cases {
        let error = match pages.lookup_media(&token, UID, "") {
            Ok(grant) => current
                .media_page(grant, &token, &MediaStore::new(), None, &pages)
                .unwrap_err(),
            Err(error) => error,
        };
        assert_eq!(error.status, 409);
    }
    // Direct identity mismatch on an otherwise identical timeline.
    let mut forged = media_grant(&original, 1, 16, 40);
    forged.identity = event_identity(&event(7, 3));
    assert_ne!(forged.identity, media_grant(&original, 1, 16, 40).identity);
    assert_eq!(
        original
            .media_page(forged, "unused", &MediaStore::new(), None, &pages)
            .unwrap_err()
            .status,
        409
    );
    for (index, offset, total) in [
        (1, 40, 40),
        (1, 41, 40),
        (1, 0, 40),
        (1, 16, 39),
        (3, 16, 40),
    ] {
        let mut grant = media_grant(&original, 1, offset, total);
        grant.index = index;
        assert_eq!(
            original
                .media_page(grant, "unused", &MediaStore::new(), None, &pages)
                .unwrap_err()
                .status,
            409,
            "index={index} offset={offset} total={total}"
        );
    }
    assert!(media_read(&original, &pages, &token, "").is_ok());
}

#[test]
fn history_pages_and_delta_batches_issue_media_grants_with_absolute_indexes() {
    let mut events = Vec::new();
    for index in 0..701 {
        events.push(Event {
            end: 3,
            message: json!({"role":"status","text":"working","state":"working"}),
            media: vec![],
        });
        events.push(if index == 150 || index == 700 {
            many_images(index, 20)
        } else {
            event(index, 3)
        });
    }
    let original = snapshot(events, b"{}\n", "synthetic-identity", "");
    let pages = store();
    let batch = initial(&original, &pages);
    let tail = &batch["messages"][batch["messages"].as_array().unwrap().len() - 1];
    assert_eq!(tail["text"], "message-700");
    let tail_grant = pages
        .lookup_media(tail["media_more"]["cursor"].as_str().unwrap(), UID, "")
        .unwrap();
    assert_eq!((tail_grant.index, tail_grant.total), (700, 20));
    let page = read(
        &original,
        &pages,
        batch["partial"]["cursor"].as_str().unwrap(),
        "",
    );
    let paged = &page["messages"][50];
    assert_eq!(paged["text"], "message-150");
    assert_eq!(paged["media"].as_array().unwrap().len(), DISPLAY_LIMIT);
    let grant = pages
        .lookup_media(paged["media_more"]["cursor"].as_str().unwrap(), UID, "")
        .unwrap();
    assert_eq!((grant.index, grant.offset, grant.total), (150, 16, 20));
    let continued = media_read(
        &original,
        &pages,
        paged["media_more"]["cursor"].as_str().unwrap(),
        "",
    )
    .unwrap();
    assert_eq!(continued["media"].as_array().unwrap().len(), 4);
    assert!(continued["page"]["next"].is_null());

    // Delta after an ordinary append: only the new event is projected, yet its
    // grant index counts every earlier non-status event.
    let mut appended = original.view.all_events();
    let mut tail = many_images(701, 20);
    tail.end = 6;
    appended.push(tail);
    let current = snapshot(appended, b"{}\n{}\n", "synthetic-identity", "");
    let delta = current
        .messages_with_pages(
            &MessageQuery {
                start: 3,
                head: original.head.clone(),
                anchor: original.anchor.clone(),
                append: "1".into(),
                ..Default::default()
            },
            &MediaStore::new(),
            None,
            &pages,
        )
        .unwrap();
    assert_eq!(delta["reset"], false);
    assert_eq!(delta["messages"].as_array().unwrap().len(), 1);
    let more = &delta["messages"][0]["media_more"];
    assert_eq!(more["remaining"], 4);
    let grant = pages
        .lookup_media(more["cursor"].as_str().unwrap(), UID, "")
        .unwrap();
    assert_eq!((grant.index, grant.offset, grant.total), (701, 16, 20));
    assert_eq!(grant.checkpoint.start, 6);
    let page = media_read(&current, &pages, more["cursor"].as_str().unwrap(), "").unwrap();
    assert_eq!(page["media"].as_array().unwrap().len(), 4);
}

#[test]
fn default_page_events_fill_a_gap_in_one_read_under_the_byte_budget() {
    let snapshot = plain(1001);
    let pages = PageStore::default();
    assert_eq!(pages.page_events(), DEFAULT_PAGE_EVENTS);
    assert_eq!(PageStore::with_page_events(0).page_events(), 1);
    assert_eq!(PageStore::with_page_events(1 << 20).page_events(), 10_000);
    let batch = initial(&snapshot, &pages);
    assert_eq!(batch["partial"]["omitted"], 401);
    let token = batch["partial"]["cursor"].as_str().unwrap();
    let page = read(&snapshot, &pages, token, "");
    assert_eq!(page["messages"].as_array().unwrap().len(), 401);
    assert_eq!(page["messages"][0]["text"], "message-100");
    assert_eq!(page["messages"][400]["text"], "message-500");
    assert_eq!(page["page"]["remaining"], 0);
    assert!(page["page"]["next"].is_null());
}
