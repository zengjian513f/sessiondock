use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::Ordering},
};

use serde_json::{Value, json};

use super::*;

fn temp() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    directory
}

fn private_write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn stars_visibility_noop_revisions_and_restart() {
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    let empty = store.snapshot().unwrap();
    assert_eq!(empty.revision(), 0);
    let starred = store.set_starred("codex:one", true).unwrap();
    assert_eq!(starred.revision(), 1);
    assert_eq!(starred.row("codex:one")["starred"], true);
    assert_eq!(empty.row("codex:one"), json!({}));
    let again = store.set_starred("codex:one", true).unwrap();
    assert!(Arc::ptr_eq(&starred, &again));
    assert_eq!(
        starred.row("codex:one")["starred_at"],
        again.row("codex:one")["starred_at"]
    );
    store
        .set_fork_visibility(&["codex:one".into(), "codex:two".into()], true)
        .unwrap();
    store.set_starred("codex:one", false).unwrap();
    assert_eq!(store.snapshot().unwrap().revision(), 3);
    assert_eq!(
        store.snapshot().unwrap().row("codex:one"),
        json!({"fork_parent_visible":true})
    );
    drop(store);
    let recovered = MetadataStore::open(root.path()).unwrap();
    assert_eq!(recovered.snapshot().unwrap().revision(), 3);
    assert_eq!(
        recovered.snapshot().unwrap().row("codex:two")["fork_parent_visible"],
        true
    );
    recovered
        .set_fork_visibility(&["codex:one".into(), "codex:two".into()], false)
        .unwrap();
    assert!(recovered.snapshot().unwrap().is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(root.path().join(METADATA_FILENAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn pure_enrichment_uses_full_topology_source_and_node_boundaries() {
    let snapshot = MetadataSnapshot::empty()
        .with_starred("codex:parent", true, 100.0)
        .unwrap();
    let topology = vec![
        json!({"uid":"codex:parent", "source":"codex", "sid":"parent"}),
        json!({"uid":"codex:child", "source":"codex", "sid":"child", "forked_from_id":"parent"}),
        json!({"uid":"claude:parent", "source":"claude", "sid":"parent"}),
        json!({"uid":"other:parent", "source":"codex", "node_id":"other", "sid":"parent"}),
        json!({"uid":"codex:unsupported", "source":"codex", "sid":"unknown", "forked_from_id":"child", "supported":false}),
    ];
    assert_eq!(
        fork_parent_uids(&topology),
        ["codex:parent".to_owned()].into()
    );
    let mut subset = topology[0].clone();
    snapshot.enrich_one(&mut subset, &topology);
    assert_eq!(subset["fork_parent"], true);
    assert_eq!(subset["fork_parent_visible"], false);
    assert_eq!(subset["starred_at"], 100.0);
    assert!(topology[0].get("starred").is_none());
    let cleared = snapshot.with_starred("codex:parent", false, 101.0).unwrap();
    cleared.enrich_one(&mut subset, &[]);
    for key in [
        "starred",
        "starred_at",
        "fork_parent",
        "fork_parent_visible",
    ] {
        assert!(subset.get(key).is_none())
    }
}

#[test]
fn concurrent_updates_preserve_every_row_and_single_revision_order() {
    let root = temp();
    let store = Arc::new(MetadataStore::open(root.path()).unwrap());
    let workers: Vec<_> = (0..16)
        .map(|id| {
            let store = store.clone();
            std::thread::spawn(move || {
                store
                    .set_starred(&format!("claude:thread-{id}"), true)
                    .unwrap()
                    .revision()
            })
        })
        .collect();
    let mut revisions: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    revisions.sort();
    assert_eq!(revisions, (1..=16).collect::<Vec<_>>());
    assert_eq!(store.snapshot().unwrap().len(), 16);
    drop(store);
    assert_eq!(
        MetadataStore::open(root.path())
            .unwrap()
            .snapshot()
            .unwrap()
            .len(),
        16
    );
}

#[test]
fn sequential_writers_read_each_others_latest_metadata() {
    let root = temp();
    let first = MetadataStore::open(root.path()).unwrap();
    let second = MetadataStore::open(root.path()).unwrap();
    first.set_starred("claude:a", true).unwrap();
    second.set_starred("claude:b", true).unwrap();
    assert_eq!(first.snapshot().unwrap().row("claude:b")["starred"], true);
    assert_eq!(second.snapshot().unwrap().row("claude:a")["starred"], true);
}

#[test]
fn tolerant_metadata_reads_leave_the_file_unchanged_until_a_write() {
    for bytes in [
        b"not json".as_slice(),
        br#"{"schema_version":9,"revision":1,"sessions":{}}"#,
        br#"{"schema_version":1,"revision":1,"sessions":{},"future_field":true}"#,
        br#"{"schema_version":1,"revision":1,"sessions":{"claude:a":{"unknown":true}}}"#,
        br#"{"schema_version":1,"revision":1,"sessions":{"claude:a":{},"claude:a":{}}}"#,
        br#"{"schema_version":1,"revision":1,"revision":2,"sessions":{}}"#,
        br#"{"schema_version":1,"revision":1,"sessions":{"claude:a":{"starred":true}}}"#,
    ] {
        let root = temp();
        let path = root.path().join(METADATA_FILENAME);
        private_write(&path, bytes);
        assert!(MetadataStore::open(root.path()).is_ok());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn large_metadata_and_unicode_keys_roundtrip_without_losing_rows() {
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    let keys = [
        "../native".to_owned(),
        "claude:中文\n标记".to_owned(),
        "x".repeat(257),
    ];
    for uid in &keys {
        store.set_starred(uid, true).unwrap();
    }
    assert!(store.set_starred("", true).is_err());
    store.set_fork_visibility(&keys, true).unwrap();
    let previous = store.snapshot().unwrap();
    assert!(previous.with_starred("claude:a", true, f64::NAN).is_err());
    drop(store);
    let path = root.path().join(METADATA_FILENAME);
    let mut bytes = vec![b' '; 4 * 1024 * 1024 + 1];
    bytes.extend_from_slice(&fs::read(&path).unwrap());
    private_write(&path, &bytes);
    let reopened = MetadataStore::open(root.path()).unwrap();
    assert_eq!(reopened.snapshot().unwrap().len(), keys.len());
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn many_metadata_rows_and_long_stop_reasons_are_preserved() {
    let mut snapshot = MetadataSnapshot::empty();
    for index in 0..10_001 {
        snapshot.document.sessions.insert(
            format!("claude:{index}"),
            serde_json::from_value(json!({"starred":true,"starred_at":1.0})).unwrap(),
        );
    }
    let larger = snapshot.with_starred("claude:next", true, 1.0).unwrap();
    assert_eq!(larger.len(), 10_002);
    let uids = (0..1001).map(|i| format!("claude:{i}")).collect::<Vec<_>>();
    assert!(larger.with_fork_visibility(&uids, true).is_ok());
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    let reason = "说明\n".repeat(2049);
    store
        .stop_activity(
            "claude:a",
            ActivityStop {
                at: 1.0,
                state: StopState::Idle,
                inferred: false,
                reason: reason.clone(),
            },
        )
        .unwrap();
    drop(store);
    let reopened = MetadataStore::open(root.path()).unwrap();
    assert_eq!(
        reopened.snapshot().unwrap().row("claude:a")["stopped"]["reason"],
        reason
    );
}

#[test]
fn failed_write_or_replace_keeps_old_arc_and_cleans_only_this_writes_temp() {
    let root = temp();
    let orphan = root.path().join(".metadata-tmp-previous-process");
    private_write(&orphan, b"preserve old temporary data");
    let store = MetadataStore::open(root.path()).unwrap();
    let previous = store.set_starred("claude:a", true).unwrap();
    let original = fs::read(root.path().join(METADATA_FILENAME)).unwrap();
    for point in [1, 2] {
        store.disk.failpoint.store(point, Ordering::Relaxed);
        assert!(store.set_starred("claude:b", true).is_err());
        assert!(Arc::ptr_eq(&previous, &store.snapshot().unwrap()));
        assert_eq!(
            fs::read(root.path().join(METADATA_FILENAME)).unwrap(),
            original
        );
        assert_eq!(fs::read(&orphan).unwrap(), b"preserve old temporary data");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    }
    store.disk.failpoint.store(0, Ordering::Relaxed);
    assert_eq!(store.set_starred("claude:b", true).unwrap().revision(), 2);
}

#[test]
fn post_replace_sync_failure_recovers_on_the_next_read() {
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    let previous = store.set_starred("claude:a", true).unwrap();
    store.disk.failpoint.store(3, Ordering::Relaxed);
    assert_eq!(
        store.set_starred("claude:b", true).unwrap_err().code,
        "metadata_commit_uncertain"
    );
    assert_eq!(previous.row("claude:b"), json!({}));
    assert_eq!(store.snapshot().unwrap().row("claude:b")["starred"], true);
    store.disk.failpoint.store(0, Ordering::Relaxed);
    assert_eq!(store.set_starred("claude:c", true).unwrap().revision(), 3);
}

#[test]
fn external_metadata_edits_are_read_before_the_next_update() {
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    let previous = store.set_starred("claude:a", true).unwrap();
    let path = root.path().join(METADATA_FILENAME);
    private_write(&path, br#"{"schema_version":1,"revision":5,"sessions":{"claude:external":{"fork_parent_visible":true}}}"#);
    let next = store.set_starred("claude:b", true).unwrap();
    assert_eq!(next.row("claude:external")["fork_parent_visible"], true);
    assert_eq!(next.revision(), 6);
    assert_eq!(previous.row("claude:a")["starred"], true);
}

#[test]
fn metadata_directory_is_created_and_unrelated_files_are_preserved() {
    let root = temp();
    let created = root.path().join("missing");
    drop(MetadataStore::open(&created).unwrap());
    assert!(created.is_dir());
    let unrelated = root.path().join("session-meta.json");
    private_write(&unrelated, b"synthetic unrelated metadata");
    drop(MetadataStore::open(root.path()).unwrap());
    assert_eq!(
        fs::read(&unrelated).unwrap(),
        b"synthetic unrelated metadata"
    );
}

#[cfg(unix)]
#[test]
fn linked_metadata_and_existing_permissions_follow_os_access() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = temp();
    let outside = temp();
    let external = outside.path().join("synthetic.json");
    let original = br#"{"schema_version":1,"revision":0,"sessions":{}}"#;
    private_write(&external, original);
    symlink(outside.path(), root.path().join("linked-directory")).unwrap();
    drop(MetadataStore::open(&root.path().join("linked-directory")).unwrap());
    for hard in [false, true] {
        let target = temp();
        let alias = target.path().join(METADATA_FILENAME);
        if hard {
            fs::hard_link(&external, &alias).unwrap();
        } else {
            symlink(&external, &alias).unwrap();
        }
        let store = MetadataStore::open(target.path()).unwrap();
        store.set_starred("claude:linked", true).unwrap();
        assert_eq!(fs::read(&external).unwrap(), original);
    }
    let target = temp();
    private_write(&target.path().join(METADATA_FILENAME), original);
    fs::set_permissions(
        target.path().join(METADATA_FILENAME),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let store = MetadataStore::open(target.path()).unwrap();
    store.set_starred("claude:readable", true).unwrap();
}

#[test]
fn activity_and_pending_confirmed_timeline_are_persistent_domain_only() {
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    store.set_starred("claude:a", true).unwrap();
    let stopped = store
        .stop_activity(
            "claude:a",
            ActivityStop {
                at: 100.0,
                reason: "synthetic confirmed stop".into(),
                state: StopState::Aborted,
                inferred: false,
            },
        )
        .unwrap();
    let old = json!({"state":"working", "ts":"1970-01-01T00:01:39Z"});
    let new = json!({"state":"working", "ts":"1970-01-01T00:01:41Z"});
    assert_eq!(
        stopped.resolve_activity("claude:a", &old)["state"],
        "aborted"
    );
    assert_eq!(stopped.resolve_activity("claude:a", &new), new);
    assert!(Arc::ptr_eq(
        &stopped,
        &store.clear_inferred_activity_stop("claude:a").unwrap()
    ));
    store
        .stop_activity(
            "claude:a",
            ActivityStop {
                at: 101.0,
                reason: "synthetic inferred stop".into(),
                state: StopState::Idle,
                inferred: true,
            },
        )
        .unwrap();
    assert!(
        store
            .clear_inferred_activity_stop("claude:a")
            .unwrap()
            .row("claude:a")
            .get("stopped")
            .is_none()
    );
    store
        .begin_timeline_rewind(
            "claude:a",
            PendingRewind {
                from_tip: "old-leaf".into(),
                stale_end: 123,
                started_at: 102.0,
            },
        )
        .unwrap();
    assert!(store.snapshot().unwrap().timeline("claude:a").is_none());
    drop(store);
    let store = MetadataStore::open(root.path()).unwrap();
    assert!(
        store
            .snapshot()
            .unwrap()
            .pending_rewind("claude:a")
            .is_some()
    );
    assert!(store.snapshot().unwrap().timeline("claude:a").is_none());
    let confirmed = store
        .finish_timeline_rewind("claude:a", "kept-leaf")
        .unwrap();
    assert_eq!(confirmed.timeline("claude:a").unwrap().stale_end, 123);
    assert!(confirmed.pending_rewind("claude:a").is_none());
    assert_eq!(confirmed.row("claude:a")["starred"], true);
    assert_eq!(
        store
            .finish_timeline_rewind("claude:a", "unconfirmed-leaf")
            .err()
            .unwrap()
            .code,
        "rewind_not_pending"
    );
    let on_disk: Value =
        serde_json::from_slice(&fs::read(root.path().join(METADATA_FILENAME)).unwrap()).unwrap();
    assert_eq!(on_disk["schema_version"], SCHEMA_VERSION);
    assert_eq!(on_disk["revision"], confirmed.revision());
}

#[test]
fn timeline_pin_round_trip_versioning_retire_and_clear() {
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    store.set_starred("claude:pin", true).unwrap();
    let pin = TimelinePin {
        tip: "a2".into(),
        stale_end: 512,
        target: Some("u3".into()),
        pinned_at: Some(1_789_120_800.0),
    };
    let pinned = store.set_timeline_pin("claude:pin", pin.clone()).unwrap();
    assert_eq!(pinned.revision(), 2);
    assert_eq!(pinned.timeline("claude:pin"), Some(&pin));
    assert_eq!(pinned.timeline_revision("claude:pin"), 1);
    assert_eq!(pinned.row("claude:pin")["starred"], true);
    // Re-pinning the same tip/target/boundary is a no-op: same Arc, revision
    // and original pinned_at, even with a newer timestamp.
    let again = store
        .set_timeline_pin(
            "claude:pin",
            TimelinePin {
                pinned_at: Some(1_789_120_900.0),
                ..pin.clone()
            },
        )
        .unwrap();
    assert!(Arc::ptr_eq(&pinned, &again));
    // A moved boundary is a new pin with a new timeline revision.
    let moved = store
        .set_timeline_pin(
            "claude:pin",
            TimelinePin {
                stale_end: 640,
                ..pin.clone()
            },
        )
        .unwrap();
    assert_eq!(moved.revision(), 3);
    assert_eq!(moved.timeline_revision("claude:pin"), 2);
    assert_eq!(moved.timeline("claude:pin").unwrap().stale_end, 640);
    drop(store);
    let restarted = MetadataStore::open(root.path()).unwrap();
    let recovered = restarted.snapshot().unwrap();
    assert_eq!(recovered.revision(), 3);
    assert_eq!(
        recovered.timeline("claude:pin").unwrap().target.as_deref(),
        Some("u3")
    );
    assert_eq!(
        recovered.timeline("claude:pin").unwrap().pinned_at,
        Some(1_789_120_800.0)
    );
    let on_disk: Value =
        serde_json::from_slice(&fs::read(root.path().join(METADATA_FILENAME)).unwrap()).unwrap();
    assert_eq!(on_disk["sessions"]["claude:pin"]["timeline"]["tip"], "a2");
    assert_eq!(on_disk["sessions"]["claude:pin"]["timeline_revision"], 2);
    // Clearing removes only the pin; the star and revision counter remain.
    let cleared = restarted.clear_timeline_pin("claude:pin").unwrap();
    assert_eq!(cleared.revision(), 4);
    assert!(cleared.timeline("claude:pin").is_none());
    assert_eq!(cleared.timeline_revision("claude:pin"), 3);
    assert_eq!(cleared.row("claude:pin")["starred"], true);
    assert!(Arc::ptr_eq(
        &cleared,
        &restarted.clear_timeline_pin("claude:pin").unwrap()
    ));
    assert!(Arc::ptr_eq(
        &cleared,
        &restarted.clear_timeline_pin("claude:never").unwrap()
    ));
    // Invalid pins never reach disk.
    {
        let (tip, target): (&str, Option<&str>) = ("", None);
        assert!(
            restarted
                .set_timeline_pin(
                    "claude:pin",
                    TimelinePin {
                        tip: tip.into(),
                        stale_end: 1,
                        target: target.map(str::to_owned),
                        pinned_at: None,
                    },
                )
                .is_err()
        );
    }
    assert_eq!(restarted.snapshot().unwrap().revision(), 4);
    // A pre-existing document without the optional fields still loads.
    drop(restarted);
    private_write(
        &root.path().join(METADATA_FILENAME),
        br#"{"schema_version":1,"revision":9,"sessions":{"claude:old":{"timeline":{"tip":"leaf","stale_end":7},"timeline_revision":1}}}"#,
    );
    let legacy = MetadataStore::open(root.path()).unwrap();
    let old = legacy.snapshot().unwrap();
    assert_eq!(
        old.timeline("claude:old"),
        Some(&TimelinePin {
            tip: "leaf".into(),
            stale_end: 7,
            target: None,
            pinned_at: None
        })
    );
}

/// The first
/// relation is permanent, empty entries are skipped, other preferences never
/// touch it, and the row carries it.
#[test]
fn spawn_parent_is_recorded_once_and_enriches_rows() {
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    let parent = SpawnedBy {
        source: "claude".into(),
        sid: "parent-sid".into(),
    };
    assert!(store.snapshot().unwrap().spawned_uids().is_empty());
    assert_eq!(
        store
            .record_spawn_parents(&[("grok:child".into(), parent.clone())])
            .unwrap(),
        1
    );
    assert_eq!(
        store.snapshot().unwrap().spawned_uids(),
        ["grok:child".to_owned()].into()
    );
    // Only one spawner: a later clue does not rewrite the first relation.
    assert_eq!(
        store
            .record_spawn_parents(&[(
                "grok:child".into(),
                SpawnedBy {
                    source: "codex".into(),
                    sid: "other".into()
                }
            )])
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .record_spawn_parents(&[
                ("grok:child".into(), parent.clone()),
                (
                    "codex:bad".into(),
                    SpawnedBy {
                        source: String::new(),
                        sid: String::new()
                    }
                ),
            ])
            .unwrap(),
        0
    );
    assert_eq!(store.snapshot().unwrap().revision(), 1);
    assert_eq!(store.snapshot().unwrap().row("codex:bad"), json!({}));
    let original = json!({"uid":"grok:child", "title":"demo"});
    let mut rows = vec![original.clone(), json!({"uid":"claude:root"})];
    store.snapshot().unwrap().enrich(&mut rows);
    assert_eq!(
        rows[0]["spawned_by"],
        json!({"source":"claude", "sid":"parent-sid"})
    );
    assert!(rows[1].get("spawned_by").is_none());
    assert!(original.get("spawned_by").is_none());
    let mut one = original.clone();
    store.snapshot().unwrap().enrich_one(&mut one, &[]);
    assert_eq!(one["spawned_by"]["sid"], "parent-sid");
    // Stars and the relation do not overwrite each other; a stale decoration is replaced.
    store.set_starred("grok:child", true).unwrap();
    let mut stale = json!({"uid":"grok:child", "spawned_by":{"source":"x","sid":"y"}});
    store
        .snapshot()
        .unwrap()
        .enrich(std::slice::from_mut(&mut stale));
    assert_eq!(stale["spawned_by"]["source"], "claude");
    assert_eq!(stale["starred"], true);
    store.set_starred("grok:child", false).unwrap();
    assert_eq!(
        store.snapshot().unwrap().row("grok:child"),
        json!({"spawned_by":{"source":"claude","sid":"parent-sid"}})
    );
    // Parent strings are retained as supplied, after trimming.
    for parent_value in [
        SpawnedBy {
            source: "claude".into(),
            sid: "with space".into(),
        },
        SpawnedBy {
            source: "x".repeat(33),
            sid: "sid".into(),
        },
        SpawnedBy {
            source: "claude".into(),
            sid: "bad\u{7}".into(),
        },
    ] {
        assert!(
            store
                .record_spawn_parents(&[(format!("codex:new-{}", parent_value.sid), parent_value)])
                .is_ok()
        );
    }
    assert!(
        store
            .record_spawn_parents(&[("bad uid".into(), parent.clone())])
            .is_ok()
    );
    let trimmed = MetadataSnapshot::empty()
        .with_spawn_parents(&[(
            " codex:new ".into(),
            SpawnedBy {
                source: " codex ".into(),
                sid: " sid-1 ".into(),
            },
        )])
        .unwrap();
    assert_eq!(
        trimmed.spawned_by("codex:new"),
        Some(&SpawnedBy {
            source: "codex".into(),
            sid: "sid-1".into()
        })
    );
    // Durable across restart with the documented key.
    drop(store);
    let disk: Value =
        serde_json::from_slice(&fs::read(root.path().join(METADATA_FILENAME)).unwrap()).unwrap();
    assert_eq!(
        disk["sessions"]["grok:child"],
        json!({"spawned_by":{"source":"claude","sid":"parent-sid"}})
    );
    let restarted = MetadataStore::open(root.path()).unwrap();
    assert_eq!(
        restarted.snapshot().unwrap().spawned_by("grok:child"),
        Some(&parent)
    );
}

#[test]
fn nest_display_overrides_spawned_by_and_clears() {
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    let spawned = SpawnedBy {
        source: "claude".into(),
        sid: "parent-sid".into(),
    };
    store
        .record_spawn_parents(&[("grok:child".into(), spawned.clone())])
        .unwrap();
    let attached = store
        .set_nest_display(
            "grok:child",
            Some(SpawnedBy {
                source: "codex".into(),
                sid: "other".into(),
            }),
            false,
        )
        .unwrap();
    assert_eq!(attached.revision(), 2);
    assert_eq!(
        attached.nest_parent("grok:child"),
        Some(&SpawnedBy {
            source: "codex".into(),
            sid: "other".into()
        })
    );
    assert!(!attached.nest_independent("grok:child"));
    let independent = store.set_nest_display("grok:child", None, true).unwrap();
    assert!(independent.nest_independent("grok:child"));
    assert!(independent.nest_parent("grok:child").is_none());
    let restored = store.set_nest_display("grok:child", None, false).unwrap();
    assert!(!restored.nest_independent("grok:child"));
    assert!(restored.nest_parent("grok:child").is_none());
    assert_eq!(restored.spawned_by("grok:child"), Some(&spawned));
    let again = store.set_nest_display("grok:child", None, false).unwrap();
    assert!(Arc::ptr_eq(&restored, &again));
    let mut row = json!({"uid":"grok:child", "nest_parent":{"source":"stale","sid":"x"}});
    restored.enrich(std::slice::from_mut(&mut row));
    assert!(row.get("nest_parent").is_none());
    assert!(row.get("nest_independent").is_none());
    assert_eq!(
        row["spawned_by"],
        json!({"source":"claude","sid":"parent-sid"})
    );
    let independent = store.set_nest_display("grok:child", None, true).unwrap();
    independent.enrich(std::slice::from_mut(&mut row));
    assert_eq!(row["nest_independent"], true);
    assert_eq!(
        row["spawned_by"],
        json!({"source":"claude","sid":"parent-sid"})
    );
}
