use std::{fs, os::unix::fs::PermissionsExt, sync::atomic::Ordering};

use serde_json::Value;

use super::*;

fn temp() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

fn private_write(path: &Path, bytes: &[u8]) {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

fn setup() -> (
    tempfile::TempDir,
    DeliveryStore,
    codex::Machine,
    claude::Machine,
) {
    let root = temp();
    let store =
        DeliveryStore::initialize(root.path(), "codex-one".into(), "claude-one".into()).unwrap();
    (
        root,
        store,
        codex::Machine::new("codex-one".into()).unwrap(),
        claude::Machine::new("claude-one".into()).unwrap(),
    )
}

fn codex_request() -> codex::Request {
    codex::Request {
        request_id: "codex-request-one".into(),
        payload: codex::Payload {
            uid: "codex:synthetic".into(),
            target: codex::Target {
                host_instance: "fixture-host".into(),
                session_id: "fixture-terminal".into(),
                ownership_epoch: "fixture-owner".into(),
            },
            text: " exact  text\n".into(),
            media: vec![codex::MediaRef {
                id: "fixture-media".into(),
                content_digest: "fixture-digest".into(),
            }],
        },
    }
}

fn claude_request() -> claude::Request {
    claude::Request {
        id: "claude-request-one".into(),
        payload: claude::Payload {
            scope: claude::Scope {
                uid: "claude:synthetic".into(),
                session_id: "fixture-native-session".into(),
                agent_id: None,
            },
            target: claude::Target {
                host_instance: "fixture-host".into(),
                terminal_id: "fixture-terminal".into(),
                ownership_epoch: "fixture-owner".into(),
            },
            text: " exact  text\n".into(),
            attachments: vec![claude::Attachment {
                id: "fixture-media".into(),
                digest: "fixture-digest".into(),
            }],
        },
    }
}

fn codex_commit(
    store: &DeliveryStore,
    machine: &mut codex::Machine,
    command: codex::Command,
) -> Vec<codex::Effect> {
    let expected = machine.snapshot().version.clone();
    let effects = machine.apply(command).unwrap();
    assert_eq!(effects.len(), 1);
    let ack = store.commit_codex(&expected, &effects[0]).unwrap();
    machine.apply(ack).unwrap()
}

fn claude_commit(
    store: &DeliveryStore,
    machine: &mut claude::Machine,
    command: claude::Command,
) -> Vec<claude::Effect> {
    let expected = machine.snapshot().version.clone();
    let effects = machine.apply(command).unwrap();
    assert_eq!(effects.len(), 1);
    let ack = store.commit_claude(&expected, &effects[0]).unwrap();
    machine.apply(ack).unwrap()
}

fn submit(store: &DeliveryStore, machine: &mut codex::Machine) -> codex::Operation {
    let effects = codex_commit(
        store,
        machine,
        codex::Command::Submit {
            request: codex_request(),
            now_ms: 100,
        },
    );
    let [codex::Effect::InspectComposer { operation, .. }] = effects.as_slice() else {
        panic!()
    };
    operation.clone()
}

fn draft(operation: codex::Operation) -> codex::Command {
    codex::Command::DraftObserved {
        operation,
        observation: codex::DraftObservation {
            target: codex_request().payload.target,
            state: codex::DraftState::Empty,
            token: "empty-frame".into(),
            native_cursor: codex::NativeCursor {
                source_identity: "fixture-source".into(),
                position: 100,
                head: "head".into(),
                anchor: "anchor".into(),
            },
        },
    }
}

#[test]
fn missing_ledger_is_never_an_empty_idempotency_database() {
    let root = temp();
    assert!(matches!(
        DeliveryStore::open(root.path()),
        Err(Error::MissingLedger)
    ));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    let store = DeliveryStore::initialize(root.path(), "one".into(), "two".into()).unwrap();
    drop(store);
    fs::remove_file(root.path().join(LEDGER_FILENAME)).unwrap();
    assert!(matches!(
        DeliveryStore::open(root.path()),
        Err(Error::MissingLedger)
    ));
    assert!(matches!(
        DeliveryStore::initialize(root.path(), "new".into(), "new".into()),
        Err(Error::AlreadyInitialized)
    ));
}

#[test]
fn dedicated_directory_and_explicit_initialization_do_not_touch_foreign_state() {
    let root = temp();
    private_write(
        &root.path().join("session-metadata.json"),
        b"private unrelated fixture",
    );
    assert!(matches!(
        DeliveryStore::open(root.path()),
        Err(Error::ForeignDirectory)
    ));
    assert!(matches!(
        DeliveryStore::initialize(root.path(), "one".into(), "two".into()),
        Err(Error::AlreadyInitialized)
    ));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    assert!(matches!(
        DeliveryStore::initialize(&root.path().join("missing"), "one".into(), "two".into()),
        Err(Error::Io(..))
    ));
    assert!(!root.path().join("missing").exists());
}

#[test]
fn one_file_retains_two_independent_full_payload_ledgers() {
    let (root, store, mut codex, mut claude) = setup();
    submit(&store, &mut codex);
    claude_commit(
        &store,
        &mut claude,
        claude::Command::Enqueue {
            request: claude_request(),
            now_ms: 100,
        },
    );
    assert_eq!(
        store.snapshots().unwrap(),
        (codex.snapshot().clone(), claude.snapshot().clone())
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    for name in [LOCK_FILENAME, LEDGER_FILENAME] {
        assert_eq!(
            fs::metadata(root.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    drop(store);
    let reopened = DeliveryStore::open(root.path()).unwrap();
    let (first, second) = reopened.snapshots().unwrap();
    assert_eq!(first.receipts["codex-request-one"].request, codex_request());
    assert_eq!(
        second.receipts["claude-request-one"].request,
        claude_request()
    );
}

#[test]
fn disk_ack_is_exact_typed_and_does_not_execute_the_followup_effect() {
    let (_root, store, mut codex, _) = setup();
    let expected = codex.snapshot().version.clone();
    let effects = codex
        .apply(codex::Command::Submit {
            request: codex_request(),
            now_ms: 100,
        })
        .unwrap();
    let mut wrong = effects[0].clone();
    if let codex::Effect::Persist { version, .. } = &mut wrong {
        version.revision += 1;
    }
    assert!(matches!(
        store.commit_codex(&expected, &wrong),
        Err(Error::TokenMismatch)
    ));
    let ack = store.commit_codex(&expected, &effects[0]).unwrap();
    assert!(codex.snapshot().receipts.is_empty());
    assert_eq!(store.snapshots().unwrap().0.receipts.len(), 1);
    let followup = codex.apply(ack).unwrap();
    assert!(matches!(
        followup.as_slice(),
        [codex::Effect::InspectComposer { .. }]
    ));
    assert!(matches!(
        store.commit_codex(&expected, &followup[0]),
        Err(Error::NotPersist)
    ));
    let repeated = store.commit_codex(&expected, &effects[0]).unwrap();
    assert_eq!(
        codex.apply(repeated).unwrap_err(),
        codex::Error::StaleOperation
    );
}

#[test]
fn version_cas_rejects_stale_epoch_revision_and_changed_same_token() {
    let (_root, store, mut codex, _) = setup();
    let expected = codex.snapshot().version.clone();
    let effects = codex
        .apply(codex::Command::Submit {
            request: codex_request(),
            now_ms: 100,
        })
        .unwrap();
    let mut foreign = expected.clone();
    foreign.epoch = "other-process".into();
    assert!(matches!(
        store.commit_codex(&foreign, &effects[0]),
        Err(Error::Conflict)
    ));
    let ack = store.commit_codex(&expected, &effects[0]).unwrap();
    codex.apply(ack).unwrap();
    let mut changed = effects[0].clone();
    if let codex::Effect::Persist { snapshot, .. } = &mut changed {
        snapshot
            .receipts
            .get_mut("codex-request-one")
            .unwrap()
            .request
            .payload
            .text
            .push('!');
    }
    assert!(matches!(
        store.commit_codex(&expected, &changed),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store.commit_codex(&foreign, &effects[0]),
        Err(Error::Conflict)
    ));
}

#[test]
fn persistence_does_not_allow_erasing_or_mutating_retained_request_payload() {
    let (_root, store, mut codex, mut claude) = setup();
    submit(&store, &mut codex);
    claude_commit(
        &store,
        &mut claude,
        claude::Command::Enqueue {
            request: claude_request(),
            now_ms: 100,
        },
    );
    for field in 0..7 {
        let expected = codex.snapshot().version.clone();
        let mut next = codex.snapshot().clone();
        next.version.revision += 1;
        let row = next.receipts.get_mut("codex-request-one").unwrap();
        match field {
            0 => row.request.payload.text.push(' '),
            1 => row.request.payload.uid.push('x'),
            2 => row.request.payload.target.host_instance.push('x'),
            3 => row.request.payload.target.ownership_epoch.push('x'),
            4 => row.request.payload.media[0].content_digest.push('x'),
            5 => row.created_ms += 1,
            _ => {
                next.receipts.clear();
            }
        }
        let effect = codex::Effect::Persist {
            version: next.version.clone(),
            snapshot: next,
        };
        assert!(
            matches!(
                store.commit_codex(&expected, &effect),
                Err(Error::PayloadConflict)
            ),
            "field {field}"
        );
    }
    for field in 0..4 {
        let expected = claude.snapshot().version.clone();
        let mut next = claude.snapshot().clone();
        next.version.revision += 1;
        let row = next.receipts.get_mut("claude-request-one").unwrap();
        match field {
            0 => row.request.payload.attachments[0].digest.push('x'),
            1 => row.request.payload.scope.agent_id = Some("child-agent".into()),
            2 => row.request.payload.text.push(' '),
            _ => {
                next.receipts.clear();
            }
        }
        let effect = claude::Effect::Persist {
            version: next.version.clone(),
            snapshot: next,
        };
        assert!(matches!(
            store.commit_claude(&expected, &effect),
            Err(Error::PayloadConflict)
        ));
    }
}

#[test]
fn before_commit_failure_never_confirms_or_changes_memory_or_disk() {
    for point in [1, 2] {
        let (root, store, mut codex, _) = setup();
        let before = fs::read(root.path().join(LEDGER_FILENAME)).unwrap();
        let expected = codex.snapshot().version.clone();
        let effects = codex
            .apply(codex::Command::Submit {
                request: codex_request(),
                now_ms: 100,
            })
            .unwrap();
        store.disk.failpoint.store(point, Ordering::Relaxed);
        assert!(matches!(
            store.commit_codex(&expected, &effects[0]),
            Err(Error::Io(..))
        ));
        assert_eq!(fs::read(root.path().join(LEDGER_FILENAME)).unwrap(), before);
        assert!(store.snapshots().unwrap().0.receipts.is_empty());
        assert!(codex.snapshot().receipts.is_empty());
        assert_eq!(
            fs::read_dir(root.path()).unwrap().count(),
            2,
            "only our own failed temp is cleaned"
        );
        store.disk.failpoint.store(0, Ordering::Relaxed);
        let ack = store.commit_codex(&expected, &effects[0]).unwrap();
        assert!(matches!(
            codex.apply(ack).unwrap().as_slice(),
            [codex::Effect::InspectComposer { .. }]
        ));
    }
}

#[test]
fn post_rename_uncertainty_freezes_both_providers_and_restart_never_reprepares() {
    for point in [3, 4] {
        let (root, store, mut codex, mut claude) = setup();
        let operation = submit(&store, &mut codex);
        let expected = codex.snapshot().version.clone();
        let effects = codex.apply(draft(operation)).unwrap();
        store.disk.failpoint.store(point, Ordering::Relaxed);
        assert!(matches!(
            store.commit_codex(&expected, &effects[0]),
            Err(Error::Uncertain)
        ));
        assert_eq!(codex.snapshot().receipts["codex-request-one"].attempts, 0);
        assert!(matches!(store.snapshots(), Err(Error::Uncertain)));
        assert!(matches!(
            store.commit_codex(&expected, &effects[0]),
            Err(Error::Uncertain)
        ));
        let other_expected = claude.snapshot().version.clone();
        let other = claude
            .apply(claude::Command::Enqueue {
                request: claude_request(),
                now_ms: 100,
            })
            .unwrap();
        assert!(matches!(
            store.commit_claude(&other_expected, &other[0]),
            Err(Error::Uncertain)
        ));
        drop(store);
        let reopened = DeliveryStore::open(root.path()).unwrap();
        let loaded = reopened.snapshots().unwrap().0;
        assert_eq!(
            loaded.receipts["codex-request-one"].state,
            codex::State::PrepareInFlight
        );
        let expected = loaded.version.clone();
        let (mut recovered, effects) = codex::Machine::restore(loaded, "codex-two".into()).unwrap();
        let ack = reopened.commit_codex(&expected, &effects[0]).unwrap();
        assert!(recovered.apply(ack).unwrap().is_empty());
        assert_eq!(
            recovered.snapshot().receipts["codex-request-one"].state,
            codex::State::Uncertain
        );
        assert_eq!(
            recovered.snapshot().receipts["codex-request-one"].attempts,
            1
        );
        assert!(matches!(
            recovered
                .apply(codex::Command::Submit {
                    request: codex_request(),
                    now_ms: 999
                })
                .unwrap()
                .as_slice(),
            [codex::Effect::Replay { .. }]
        ));
    }
}

#[test]
fn reopening_requires_exact_fresh_epoch_restore_before_normal_commits() {
    let (root, store, mut codex, mut claude) = setup();
    submit(&store, &mut codex);
    claude_commit(
        &store,
        &mut claude,
        claude::Command::Enqueue {
            request: claude_request(),
            now_ms: 100,
        },
    );
    drop(store);
    let reopened = DeliveryStore::open(root.path()).unwrap();
    let (first, second) = reopened.snapshots().unwrap();
    let expected = first.version.clone();
    let mut forged = first.clone();
    forged.version.revision += 1;
    let effect = codex::Effect::Persist {
        version: forged.version.clone(),
        snapshot: forged.clone(),
    };
    assert!(matches!(
        reopened.commit_codex(&expected, &effect),
        Err(Error::InvalidRecovery)
    ));
    forged.version.epoch = "codex-two".into();
    let effect = codex::Effect::Persist {
        version: forged.version.clone(),
        snapshot: forged,
    };
    assert!(
        matches!(
            reopened.commit_codex(&expected, &effect),
            Err(Error::InvalidRecovery)
        ),
        "CheckingDraft must become FailedBeforeWrite during recovery"
    );
    let (mut recovered, effects) = codex::Machine::restore(first, "codex-two".into()).unwrap();
    let ack = reopened.commit_codex(&expected, &effects[0]).unwrap();
    assert!(recovered.apply(ack).unwrap().is_empty());
    assert_eq!(
        recovered
            .apply(reopened.commit_codex(&expected, &effects[0]).unwrap())
            .unwrap_err(),
        codex::Error::StaleOperation
    );
    let expected = second.version.clone();
    let (mut recovered, effects) = claude::Machine::restore(second, "claude-two".into()).unwrap();
    assert!(
        recovered
            .apply(reopened.commit_claude(&expected, &effects[0]).unwrap())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        recovered.snapshot().receipts["claude-request-one"].state,
        claude::State::Queued
    );
}

#[test]
fn claude_prepare_boundary_is_durable_and_recovery_keeps_it_uncertain() {
    let (root, store, _, mut machine) = setup();
    let request = claude_request();
    claude_commit(
        &store,
        &mut machine,
        claude::Command::Enqueue {
            request: request.clone(),
            now_ms: 100,
        },
    );
    let effects = claude_commit(
        &store,
        &mut machine,
        claude::Command::DispatchNext {
            scope: request.payload.scope.clone(),
        },
    );
    let [claude::Effect::InspectComposer { operation, .. }] = effects.as_slice() else {
        panic!()
    };
    let expected = machine.snapshot().version.clone();
    let effects = machine
        .apply(claude::Command::ComposerObserved {
            operation: operation.clone(),
            composer: claude::Composer {
                target: request.payload.target,
                state: claude::ComposerState::Empty,
                frame_token: "empty".into(),
                native_cursor: claude::Cursor {
                    source_identity: "fixture-source".into(),
                    offset: 100,
                    head: "head".into(),
                    anchor: "anchor".into(),
                },
            },
        })
        .unwrap();
    store.disk.failpoint.store(3, Ordering::Relaxed);
    assert!(matches!(
        store.commit_claude(&expected, &effects[0]),
        Err(Error::Uncertain)
    ));
    assert!(!machine.snapshot().receipts["claude-request-one"].attempted);
    drop(store);
    let reopened = DeliveryStore::open(root.path()).unwrap();
    let snapshot = reopened.snapshots().unwrap().1;
    let expected = snapshot.version.clone();
    let (mut machine, effects) = claude::Machine::restore(snapshot, "claude-two".into()).unwrap();
    assert!(
        machine
            .apply(reopened.commit_claude(&expected, &effects[0]).unwrap())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        machine.snapshot().receipts["claude-request-one"].state,
        claude::State::Uncertain
    );
}

#[test]
fn strict_json_rejects_nested_unknown_missing_duplicate_and_bad_schema() {
    let (_root, store, mut codex, _) = setup();
    submit(&store, &mut codex);
    let document = store.state.lock().unwrap().document.clone();
    let raw = serde_json::to_value(&document).unwrap();
    let pretty = serde_json::to_vec_pretty(&document).unwrap();
    assert_eq!(json::decode(&pretty).unwrap(), document);
    let mut reversed = raw.as_object().unwrap().iter().collect::<Vec<_>>();
    reversed.reverse();
    let reversed: serde_json::Map<String, Value> = reversed
        .into_iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    assert_eq!(
        json::decode(&serde_json::to_vec(&reversed).unwrap()).unwrap(),
        document
    );
    for mode in 0..5 {
        let mut bad = raw.clone();
        match mode {
            0 => {
                bad["codex"]["receipts"]["codex-request-one"]["request"]["payload"]["unknown"] =
                    true.into();
            }
            1 => {
                bad["codex"]["receipts"]["codex-request-one"]
                    .as_object_mut()
                    .unwrap()
                    .remove("issue");
            }
            2 => {
                bad["claude"]["next_sequence"] = 0.into();
            }
            3 => {
                bad["schema"] = 99.into();
            }
            _ => {
                bad["codex_previous"]["revision"] = 200.into();
            }
        }
        assert!(
            json::decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "mode {mode}"
        );
    }
    let text = String::from_utf8(serde_json::to_vec(&document).unwrap()).unwrap();
    let duplicates = [
        text.replacen("\"schema\":1", "\"schema\":1,\"schema\":1", 1),
        text.replacen(
            "\"epoch\":\"codex-one\"",
            "\"epoch\":\"codex-one\",\"epoch\":\"codex-one\"",
            1,
        ),
        text.replacen("\"text\":", "\"text\":\"omitted duplicate\",\"text\":", 1),
    ];
    for duplicate in duplicates {
        assert_eq!(
            json::decode(duplicate.as_bytes()).unwrap_err(),
            Error::Invalid
        );
    }
    let receipt = serde_json::to_string(&document.codex.receipts["codex-request-one"]).unwrap();
    let duplicated = text.replacen(
        &format!("\"codex-request-one\":{receipt}"),
        &format!("\"codex-request-one\":{receipt},\"codex-request-one\":{receipt}"),
        1,
    );
    assert_eq!(
        json::decode(duplicated.as_bytes()).unwrap_err(),
        Error::Invalid
    );
    assert!(json::decode(format!("{text} null").as_bytes()).is_err());
    assert!(json::decode(&vec![b' '; MAX_BYTES + 1]).is_err());
    assert!(json::decode(format!("{}0{}", "[".repeat(65), "]".repeat(65)).as_bytes()).is_err());
}

#[test]
fn malformed_or_missing_disk_state_is_not_overwritten_by_open() {
    for bad in [b"{".as_slice(), b"{}", b"null", b"{\"schema\":999}"] {
        let (root, store, _, _) = setup();
        drop(store);
        private_write(&root.path().join(LEDGER_FILENAME), bad);
        assert!(DeliveryStore::open(root.path()).is_err());
        assert_eq!(fs::read(root.path().join(LEDGER_FILENAME)).unwrap(), bad);
    }
}

#[test]
fn outside_changes_are_not_silently_overwritten_even_for_persist_replay() {
    let (root, store, mut codex, _) = setup();
    let expected = codex.snapshot().version.clone();
    let effects = codex
        .apply(codex::Command::Submit {
            request: codex_request(),
            now_ms: 100,
        })
        .unwrap();
    let ack = store.commit_codex(&expected, &effects[0]).unwrap();
    codex.apply(ack).unwrap();
    let mut bytes = fs::read(root.path().join(LEDGER_FILENAME)).unwrap();
    bytes.push(b' ');
    private_write(&root.path().join(LEDGER_FILENAME), &bytes);
    assert!(matches!(store.snapshots(), Err(Error::Changed)));
    assert!(matches!(
        store.commit_codex(&expected, &effects[0]),
        Err(Error::Changed)
    ));
    assert_eq!(fs::read(root.path().join(LEDGER_FILENAME)).unwrap(), bytes);
}

#[test]
fn os_lease_excludes_second_writer_and_drop_unlocks_duplicated_description() {
    let (root, store, _, _) = setup();
    let duplicate = store.disk.duplicate_lock();
    assert!(matches!(
        DeliveryStore::open(root.path()),
        Err(Error::WriterLocked)
    ));
    drop(store);
    let reopened = DeliveryStore::open(root.path()).unwrap();
    assert!(duplicate.metadata().unwrap().is_file());
    drop(duplicate);
    assert!(matches!(
        DeliveryStore::open(root.path()),
        Err(Error::WriterLocked)
    ));
    drop(reopened);
    assert!(DeliveryStore::open(root.path()).is_ok());
}

#[test]
fn persistent_parent_symlink_replacement_cannot_redirect_a_read_or_write() {
    use std::os::unix::fs::symlink;
    let parent = temp();
    let original = parent.path().join("store");
    fs::create_dir(&original).unwrap();
    fs::set_permissions(&original, fs::Permissions::from_mode(0o700)).unwrap();
    let store = DeliveryStore::initialize(&original, "one".into(), "two".into()).unwrap();
    let outside = temp();
    let outside_store =
        DeliveryStore::initialize(outside.path(), "outside-one".into(), "outside-two".into())
            .unwrap();
    drop(outside_store);
    let outside_before = fs::read(outside.path().join(LEDGER_FILENAME)).unwrap();
    fs::rename(&original, parent.path().join("retained-store")).unwrap();
    symlink(outside.path(), &original).unwrap();
    assert!(matches!(store.snapshots(), Err(Error::UnsafePath)));
    let mut machine = codex::Machine::new("one".into()).unwrap();
    let expected = machine.snapshot().version.clone();
    let effects = machine
        .apply(codex::Command::Submit {
            request: codex_request(),
            now_ms: 100,
        })
        .unwrap();
    assert!(matches!(
        store.commit_codex(&expected, &effects[0]),
        Err(Error::UnsafePath)
    ));
    assert_eq!(
        fs::read(outside.path().join(LEDGER_FILENAME)).unwrap(),
        outside_before
    );
    assert!(DeliveryStore::open(&original).is_err());
}

#[test]
fn lock_replacement_links_hardlinks_and_nonprivate_permissions_are_rejected() {
    let (root, store, _, _) = setup();
    fs::rename(
        root.path().join(LOCK_FILENAME),
        root.path().join("retained-lock"),
    )
    .unwrap();
    private_write(&root.path().join(LOCK_FILENAME), b"");
    assert!(matches!(store.snapshots(), Err(Error::Changed)));
    drop(store);
    for mode in [0, 1, 2] {
        let (root, store, _, _) = setup();
        drop(store);
        let ledger = root.path().join(LEDGER_FILENAME);
        match mode {
            0 => fs::set_permissions(&ledger, fs::Permissions::from_mode(0o644)).unwrap(),
            1 => {
                fs::hard_link(&ledger, root.path().join("hardlink")).unwrap();
            }
            _ => {
                fs::rename(&ledger, root.path().join("retained")).unwrap();
                std::os::unix::fs::symlink("retained", &ledger).unwrap();
            }
        }
        assert!(DeliveryStore::open(root.path()).is_err());
    }
    let root = temp();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        DeliveryStore::initialize(root.path(), "one".into(), "two".into()),
        Err(Error::UnsafePermissions)
    ));
}

#[test]
fn owned_abandoned_temp_is_preserved_and_cannot_reinitialize_missing_ledger() {
    let (root, store, _, _) = setup();
    drop(store);
    let path = root
        .path()
        .join(".delivery-tmp-0123456789abcdef0123456789abcdef");
    private_write(&path, b"incomplete fixture");
    let store = DeliveryStore::open(root.path()).unwrap();
    assert!(store.snapshots().is_ok());
    drop(store);
    assert_eq!(fs::read(&path).unwrap(), b"incomplete fixture");
    fs::remove_file(root.path().join(LEDGER_FILENAME)).unwrap();
    assert!(matches!(
        DeliveryStore::open(root.path()),
        Err(Error::MissingLedger)
    ));
    assert!(matches!(
        DeliveryStore::initialize(root.path(), "new".into(), "new".into()),
        Err(Error::AlreadyInitialized)
    ));
}

#[test]
fn partial_initialization_is_evidence_not_permission_to_create_an_empty_ledger() {
    let root = temp();
    let disk = disk::Disk::open(root.path(), true).unwrap();
    drop(disk);
    assert!(matches!(
        DeliveryStore::open(root.path()),
        Err(Error::MissingLedger)
    ));
    assert!(matches!(
        DeliveryStore::initialize(root.path(), "one".into(), "two".into()),
        Err(Error::AlreadyInitialized)
    ));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn replacing_an_ancestor_not_only_the_root_with_a_symlink_is_detected() {
    use std::os::unix::fs::symlink;
    let base = temp();
    let ancestor = base.path().join("ancestor");
    let root = ancestor.join("ledger");
    fs::create_dir(&ancestor).unwrap();
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let store = DeliveryStore::initialize(&root, "one".into(), "two".into()).unwrap();
    let outside = temp();
    let outside_root = outside.path().join("ledger");
    fs::create_dir(&outside_root).unwrap();
    fs::set_permissions(&outside_root, fs::Permissions::from_mode(0o700)).unwrap();
    let other = DeliveryStore::initialize(&outside_root, "other".into(), "other".into()).unwrap();
    drop(other);
    let before = fs::read(outside_root.join(LEDGER_FILENAME)).unwrap();
    fs::rename(&ancestor, base.path().join("original-ancestor")).unwrap();
    symlink(outside.path(), &ancestor).unwrap();
    assert!(matches!(store.snapshots(), Err(Error::UnsafePath)));
    assert_eq!(
        fs::read(outside_root.join(LEDGER_FILENAME)).unwrap(),
        before
    );
}

#[test]
fn os_writer_lock_child_probe() {
    let Some(path) = std::env::var_os("AGENTHUB_DELIVERY_TEST_CHILD_DIRECTORY") else {
        return;
    };
    assert!(matches!(
        DeliveryStore::open(Path::new(&path)),
        Err(Error::WriterLocked)
    ));
}

#[test]
fn a_separate_test_process_cannot_acquire_the_active_writer_lease() {
    let (root, _store, _, _) = setup();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "delivery::store::tests::os_writer_lock_child_probe",
            "--nocapture",
        ])
        .env("AGENTHUB_DELIVERY_TEST_CHILD_DIRECTORY", root.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn durable_enter_claim_survives_an_unacknowledged_commit_and_is_not_replayed() {
    let (root, store, mut machine, _) = setup();
    let inspect = submit(&store, &mut machine);
    let effects = codex_commit(&store, &mut machine, draft(inspect));
    let [codex::Effect::InjectPrepare { operation, .. }] = effects.as_slice() else {
        panic!()
    };
    let expected = machine.snapshot().version.clone();
    let payload = codex_request().payload;
    let effects = machine
        .apply(codex::Command::Prepared {
            operation: operation.clone(),
            evidence: codex::PreparedEvidence {
                target: payload.target,
                observed_text: payload.text,
                observed_media: payload.media,
                frame_token: "prepared".into(),
            },
        })
        .unwrap();
    store.disk.failpoint.store(3, Ordering::Relaxed);
    assert!(matches!(
        store.commit_codex(&expected, &effects[0]),
        Err(Error::Uncertain)
    ));
    assert!(
        machine.snapshot().receipts["codex-request-one"]
            .enter_operation
            .is_none()
    );
    drop(store);
    let store = DeliveryStore::open(root.path()).unwrap();
    let snapshot = store.snapshots().unwrap().0;
    assert_eq!(
        snapshot.receipts["codex-request-one"].state,
        codex::State::EnterInFlight
    );
    let enter = snapshot.receipts["codex-request-one"]
        .enter_operation
        .clone()
        .unwrap();
    let expected = snapshot.version.clone();
    let (mut machine, effects) = codex::Machine::restore(snapshot, "codex-two".into()).unwrap();
    assert!(
        machine
            .apply(store.commit_codex(&expected, &effects[0]).unwrap())
            .unwrap()
            .is_empty()
    );
    let row = &machine.snapshot().receipts["codex-request-one"];
    assert_eq!(row.state, codex::State::Uncertain);
    assert_eq!(row.enter_operation.as_ref(), Some(&enter));
}

#[test]
fn cancelled_and_discarded_tombstones_still_deduplicate_complete_payload_after_restart() {
    let (root, store, mut codex, mut claude) = setup();
    submit(&store, &mut codex);
    codex_commit(
        &store,
        &mut codex,
        codex::Command::Discard {
            request_id: codex_request().request_id,
            uid: codex_request().payload.uid,
        },
    );
    claude_commit(
        &store,
        &mut claude,
        claude::Command::Enqueue {
            request: claude_request(),
            now_ms: 100,
        },
    );
    claude_commit(
        &store,
        &mut claude,
        claude::Command::Cancel {
            id: claude_request().id,
            scope: claude_request().payload.scope,
        },
    );
    drop(store);
    let store = DeliveryStore::open(root.path()).unwrap();
    let (first, second) = store.snapshots().unwrap();
    let expected = first.version.clone();
    let (mut codex, effects) = codex::Machine::restore(first, "codex-two".into()).unwrap();
    codex
        .apply(store.commit_codex(&expected, &effects[0]).unwrap())
        .unwrap();
    let expected = second.version.clone();
    let (mut claude, effects) = claude::Machine::restore(second, "claude-two".into()).unwrap();
    claude
        .apply(store.commit_claude(&expected, &effects[0]).unwrap())
        .unwrap();
    assert_eq!(
        codex.snapshot().receipts["codex-request-one"].state,
        codex::State::Discarded
    );
    assert_eq!(
        claude.snapshot().receipts["claude-request-one"].state,
        claude::State::Cancelled
    );
    assert!(matches!(
        codex
            .apply(codex::Command::Submit {
                request: codex_request(),
                now_ms: 999
            })
            .unwrap()
            .as_slice(),
        [codex::Effect::Replay { .. }]
    ));
    assert!(matches!(
        claude
            .apply(claude::Command::Enqueue {
                request: claude_request(),
                now_ms: 999
            })
            .unwrap()
            .as_slice(),
        [claude::Effect::Replay { .. }]
    ));
    let mut changed = codex_request();
    changed.payload.media[0].content_digest.push('x');
    assert!(
        codex
            .apply(codex::Command::Submit {
                request: changed,
                now_ms: 999
            })
            .is_err()
    );
    let mut changed = claude_request();
    changed.payload.scope.agent_id = Some("different-child".into());
    assert!(
        claude
            .apply(claude::Command::Enqueue {
                request: changed,
                now_ms: 999
            })
            .is_err()
    );
}
