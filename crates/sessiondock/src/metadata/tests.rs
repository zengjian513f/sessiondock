use std::{
    fs,
    path::Path,
    process::Command,
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
    assert!(root.path().join(LOCK_FILENAME).is_file());
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
fn second_writer_handle_and_process_are_refused_without_removing_the_lock() {
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    assert_eq!(
        MetadataStore::open(root.path()).err().unwrap().code,
        "metadata_writer_locked"
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "metadata::tests::lock_probe_subprocess",
            "--nocapture",
        ])
        .env("AGENTHUB_METADATA_TEST_LOCK_DIRECTORY", root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated lock child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "lock child test did not run"
    );
    drop(store);
    assert!(root.path().join(LOCK_FILENAME).is_file());
    MetadataStore::open(root.path()).unwrap();
}

#[test]
fn a_transient_duplicate_handle_does_not_extend_the_writer_lifetime() {
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    // A concurrently spawned child can briefly inherit an open file description
    // before CLOEXEC runs. The writer's explicit lifetime must end at Drop,
    // without depending on that unrelated process reaching exec first.
    let duplicate = store.disk.duplicate_lock_for_test();
    drop(store);
    let replacement = MetadataStore::open(root.path()).unwrap();
    drop(duplicate);
    assert_eq!(
        MetadataStore::open(root.path()).err().unwrap().code,
        "metadata_writer_locked"
    );
    drop(replacement);
}

#[test]
fn lock_probe_subprocess() {
    let Some(directory) = std::env::var_os("AGENTHUB_METADATA_TEST_LOCK_DIRECTORY") else {
        return;
    };
    assert_eq!(
        MetadataStore::open(Path::new(&directory))
            .err()
            .unwrap()
            .code,
        "metadata_writer_locked"
    );
}

#[test]
fn malformed_unknown_schema_duplicate_keys_and_unknown_fields_are_never_overwritten() {
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
        assert!(MetadataStore::open(root.path()).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn oversized_file_and_invalid_transitions_preserve_previous_snapshot_and_disk() {
    let root = temp();
    let path = root.path().join(METADATA_FILENAME);
    private_write(&path, &vec![b' '; MAX_BYTES + 1]);
    assert_eq!(MetadataStore::open(root.path()).err().unwrap().status, 413);
    assert_eq!(fs::metadata(&path).unwrap().len(), MAX_BYTES as u64 + 1);
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    let previous = store.set_starred("claude:a", true).unwrap();
    let original = fs::read(root.path().join(METADATA_FILENAME)).unwrap();
    for uid in ["", "../native", "claude:a\n", &"x".repeat(257)] {
        assert!(store.set_starred(uid, true).is_err())
    }
    assert!(
        store
            .set_fork_visibility(&["claude:b".into(), "../invalid".into()], true)
            .is_err()
    );
    assert!(previous.with_starred("claude:a", true, f64::NAN).is_err());
    assert!(Arc::ptr_eq(&previous, &store.snapshot().unwrap()));
    assert_eq!(
        fs::read(root.path().join(METADATA_FILENAME)).unwrap(),
        original
    );
}

#[test]
fn record_and_field_budgets_apply_before_any_persistence() {
    let mut snapshot = MetadataSnapshot::empty();
    for index in 0..MAX_RECORDS {
        snapshot.document.sessions.insert(
            format!("claude:{index}"),
            serde_json::from_value(json!({"starred":true,"starred_at":1.0})).unwrap(),
        );
    }
    assert_eq!(
        snapshot
            .with_starred("claude:over-budget", true, 1.0)
            .err()
            .unwrap()
            .status,
        413
    );
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    assert!(
        store
            .stop_activity(
                "claude:a",
                ActivityStop {
                    at: 1.0,
                    state: StopState::Idle,
                    inferred: false,
                    reason: "x".repeat(2049)
                }
            )
            .is_err()
    );
    assert_eq!(store.snapshot().unwrap().revision(), 0);
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
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 3);
    }
    store.disk.failpoint.store(0, Ordering::Relaxed);
    assert_eq!(store.set_starred("claude:b", true).unwrap().revision(), 2);
}

#[test]
fn post_replace_sync_failure_is_uncertain_and_freezes_until_restart() {
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    let previous = store.set_starred("claude:a", true).unwrap();
    store.disk.failpoint.store(3, Ordering::Relaxed);
    assert_eq!(
        store.set_starred("claude:b", true).err().unwrap().code,
        "metadata_commit_uncertain"
    );
    assert_eq!(previous.row("claude:b"), json!({}));
    assert_eq!(store.snapshot().err().unwrap().status, 503);
    assert_eq!(
        store.set_starred("claude:c", true).err().unwrap().code,
        "metadata_commit_uncertain"
    );
    drop(store);
    let recovered = MetadataStore::open(root.path()).unwrap();
    assert_eq!(
        recovered.snapshot().unwrap().row("claude:b")["starred"],
        true
    );
    assert_eq!(recovered.snapshot().unwrap().revision(), 2);
}

#[test]
fn external_file_replacement_or_lock_replacement_does_not_get_overwritten() {
    let root = temp();
    let store = MetadataStore::open(root.path()).unwrap();
    let previous = store.set_starred("claude:a", true).unwrap();
    let path = root.path().join(METADATA_FILENAME);
    private_write(&path, b"externally corrupted data");
    assert_eq!(
        store.set_starred("claude:b", true).err().unwrap().code,
        "metadata_changed"
    );
    assert_eq!(fs::read(&path).unwrap(), b"externally corrupted data");
    assert!(Arc::ptr_eq(&previous, &store.snapshot().unwrap()));
    #[cfg(unix)]
    {
        // Simulate replacement, never remove a lock owned by another process.
        fs::rename(
            root.path().join(LOCK_FILENAME),
            root.path().join(".metadata-tmp-held-lock"),
        )
        .unwrap();
        private_write(&root.path().join(LOCK_FILENAME), b"");
        assert_eq!(
            store.set_starred("claude:c", true).err().unwrap().code,
            "metadata_changed"
        );
    }
}

#[test]
fn explicit_existing_dedicated_directory_is_required() {
    let root = temp();
    assert!(MetadataStore::open(&root.path().join("missing")).is_err());
    assert!(!root.path().join("missing").exists());
    private_write(
        &root.path().join("session-meta.json"),
        b"synthetic foreign Python metadata",
    );
    assert_eq!(
        MetadataStore::open(root.path()).err().unwrap().code,
        "metadata_foreign_directory"
    );
    assert!(!root.path().join(LOCK_FILENAME).exists());
}

#[cfg(unix)]
#[test]
fn symlink_permissions_and_hardlinks_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = temp();
    let outside = temp();
    let external = outside.path().join("synthetic.json");
    private_write(
        &external,
        br#"{"schema_version":1,"revision":0,"sessions":{}}"#,
    );
    symlink(outside.path(), root.path().join("linked-directory")).unwrap();
    assert!(MetadataStore::open(&root.path().join("linked-directory")).is_err());
    for filename in [METADATA_FILENAME, LOCK_FILENAME] {
        let target = temp();
        symlink(&external, target.path().join(filename)).unwrap();
        assert!(MetadataStore::open(target.path()).is_err());
    }
    let linked = temp();
    fs::hard_link(&external, linked.path().join(METADATA_FILENAME)).unwrap();
    assert!(MetadataStore::open(linked.path()).is_err());
    let permissions = temp();
    private_write(&permissions.path().join(METADATA_FILENAME), b"private");
    fs::set_permissions(
        permissions.path().join(METADATA_FILENAME),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert_eq!(
        MetadataStore::open(permissions.path())
            .err()
            .unwrap()
            .status,
        403
    );
    let store_root = temp();
    let store = MetadataStore::open(store_root.path()).unwrap();
    let old = store.set_starred("claude:a", true).unwrap();
    fs::set_permissions(store_root.path(), fs::Permissions::from_mode(0o500)).unwrap();
    assert_eq!(
        store.set_starred("claude:b", true).err().unwrap().status,
        403
    );
    assert!(Arc::ptr_eq(&old, &store.snapshot().unwrap()));
    fs::set_permissions(store_root.path(), fs::Permissions::from_mode(0o700)).unwrap();
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
    for (tip, target) in [
        ("", None),
        ("x".repeat(257).as_str(), None),
        ("a2", Some("bad\u{7}id")),
    ] {
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

/// Python `test_spawn_parent_is_recorded_once_and_enriches_rows`: the first
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
    // Malformed entries never reach disk; the pure transform trims and validates.
    for bad in [
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
                .record_spawn_parents(&[("codex:new".into(), bad)])
                .is_err()
        );
    }
    assert!(
        store
            .record_spawn_parents(&[("bad uid".into(), parent.clone())])
            .is_err()
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
