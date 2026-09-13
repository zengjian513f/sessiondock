use super::*;
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};

fn directory() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    dir
}
fn codex_request(id: &str) -> codex::Request {
    codex::Request {
        request_id: id.into(),
        payload: codex::Payload {
            uid: "codex:synthetic".into(),
            target: codex::Target {
                host_instance: "host".into(),
                session_id: "terminal".into(),
                ownership_epoch: "owner".into(),
            },
            text: "private synthetic prompt".into(),
            media: vec![],
        },
    }
}
fn scope() -> claude::Scope {
    claude::Scope {
        uid: "claude:synthetic".into(),
        session_id: "native".into(),
        agent_id: None,
    }
}
fn claude_request(id: &str) -> claude::Request {
    claude::Request {
        id: id.into(),
        payload: claude::Payload {
            scope: scope(),
            target: claude::Target {
                host_instance: "host".into(),
                terminal_id: "terminal-claude".into(),
                ownership_epoch: "owner".into(),
            },
            text: "private synthetic prompt".into(),
            attachments: vec![],
        },
    }
}
fn submit(engine: &mut DeliveryEngine, id: &str) -> Result<DispatchBatch, Error> {
    engine.apply_codex(
        codex::Command::Submit {
            request: codex_request(id),
            now_ms: 1234,
        },
        None,
    )
}
fn enqueue(engine: &mut DeliveryEngine, id: &str) -> Result<DispatchBatch, Error> {
    engine.apply_claude(claude::Command::Enqueue {
        request: claude_request(id),
        now_ms: 1234,
    })
}
fn claim(batch: DispatchBatch) {
    drop(batch.claim().unwrap());
}
fn inspected(engine: &mut DeliveryEngine, id: &str) -> codex::Operation {
    let mut actions = submit(engine, id).unwrap().claim().unwrap();
    let Action::Codex(CodexAction::InspectComposer { operation, .. }) = actions.remove(0) else {
        panic!("inspect")
    };
    operation
}
fn draft(operation: codex::Operation) -> codex::Command {
    codex::Command::DraftObserved {
        operation,
        observation: codex::DraftObservation {
            target: codex_request("x").payload.target,
            state: codex::DraftState::Empty,
            token: "empty-frame".into(),
            native_cursor: codex::NativeCursor {
                source_identity: "source".into(),
                position: 100,
                head: "head".into(),
                anchor: "anchor".into(),
            },
        },
    }
}

#[test]
fn commit_precedes_each_external_action_and_duplicate_callback_is_stale() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    let op = inspected(&mut engine, "request-one");
    assert_eq!(
        engine.codex.snapshot().receipts["request-one"].state,
        codex::State::CheckingDraft
    );
    let command = draft(op);
    let batch = engine.apply_codex(command.clone(), None).unwrap();
    assert_eq!(
        engine.store.snapshots().unwrap().0.receipts["request-one"].state,
        codex::State::PrepareInFlight
    );
    let mut actions = batch.claim().unwrap();
    let Action::Codex(CodexAction::Prepare {
        operation, payload, ..
    }) = actions.remove(0)
    else {
        panic!("prepare")
    };
    assert!(matches!(
        engine.apply_codex(command, None),
        Err(Error::Codex(codex::Error::StaleOperation))
    ));
    let prepared = codex::Command::Prepared {
        operation,
        evidence: codex::PreparedEvidence {
            target: payload.target,
            observed_text: payload.text,
            observed_media: vec![],
            frame_token: "prepared".into(),
        },
    };
    let batch = engine.apply_codex(prepared.clone(), None).unwrap();
    assert_eq!(
        engine.store.snapshots().unwrap().0.receipts["request-one"].state,
        codex::State::EnterInFlight
    );
    assert!(matches!(
        batch.claim().unwrap().as_slice(),
        [Action::Codex(CodexAction::Enter { .. })]
    ));
    assert!(matches!(
        engine.apply_codex(prepared, None),
        Err(Error::Codex(codex::Error::StaleOperation))
    ));
}

#[test]
fn duplicate_submit_replays_and_conflicting_payload_fails() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    inspected(&mut engine, "request-one");
    let replay = submit(&mut engine, "request-one").unwrap();
    assert_eq!(replay.action_count(), 0);
    assert!(replay.replays()[0].known);
    assert!(!replay.replays()[0].pending);
    let mut changed = codex_request("request-one");
    changed.payload.text.push('!');
    assert!(matches!(
        engine.apply_codex(
            codex::Command::Submit {
                request: changed,
                now_ms: 1
            },
            None
        ),
        Err(Error::Codex(codex::Error::Conflict))
    ));
}

#[test]
fn precommit_failure_retains_exact_pending_and_undurable_replay() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    engine.commit_fault = Some((
        store::Error::Io("synthetic precommit", std::io::ErrorKind::Other),
        false,
    ));
    assert!(matches!(
        submit(&mut engine, "request-one"),
        Err(Error::Store(store::Error::Io(..)))
    ));
    let persisted = fs::read(dir.path().join(store::LEDGER_FILENAME)).unwrap();
    let replay = submit(&mut engine, "request-one").unwrap();
    assert_eq!(replay.action_count(), 0);
    assert!(!replay.replays()[0].known);
    assert!(replay.replays()[0].pending);
    assert_eq!(
        fs::read(dir.path().join(store::LEDGER_FILENAME)).unwrap(),
        persisted
    );
    assert!(
        engine
            .codex_outbox("codex:synthetic", None)
            .unwrap()
            .outbox
            .is_empty()
    );
    assert!(matches!(
        enqueue(&mut engine, "request-two"),
        Err(Error::CommitPending)
    ));
    assert!(matches!(
        submit(&mut engine, "request-other"),
        Err(Error::Codex(codex::Error::PersistencePending))
    ));
    let batch = engine.retry_commit().unwrap();
    assert_eq!(batch.action_count(), 1);
    batch.claim().unwrap();
    assert_eq!(engine.codex.snapshot().version.revision, 1);
    assert!(matches!(engine.retry_commit(), Err(Error::CommitPending)));
}

#[test]
fn claude_precommit_failure_replay_is_not_durable() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    engine.commit_fault = Some((store::Error::Changed, false));
    assert!(matches!(
        enqueue(&mut engine, "request-one"),
        Err(Error::Store(store::Error::Changed))
    ));
    let replay = enqueue(&mut engine, "request-one").unwrap();
    assert!(!replay.replays()[0].known);
    assert!(replay.replays()[0].pending);
    assert!(engine.retry_commit().unwrap().claim().unwrap().is_empty());
    assert_eq!(
        engine.claude_outbox(&scope()).unwrap().outbox[0].state,
        "persisted"
    );
}

#[test]
fn lost_persist_ack_is_retryable_and_reopen_restores_both_epochs() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    let old_codex = engine.codex.snapshot().version.clone();
    let old_claude = engine.claude.snapshot().version.clone();
    let op = inspected(&mut engine, "request-one");
    engine.commit_fault = Some((store::Error::Changed, true));
    assert!(matches!(
        engine.apply_codex(draft(op), None),
        Err(Error::Store(store::Error::Changed))
    ));
    let retry = engine.retry_commit().unwrap();
    assert_eq!(retry.action_count(), 0);
    retry.claim().unwrap();
    drop(engine);
    let mut engine = DeliveryEngine::open(dir.path()).unwrap();
    assert_ne!(engine.codex.snapshot().version.epoch, old_codex.epoch);
    assert_ne!(engine.claude.snapshot().version.epoch, old_claude.epoch);
    assert_eq!(
        engine.codex.snapshot().receipts["request-one"].state,
        codex::State::Uncertain
    );
    let (cs, hs) = engine.store.snapshots().unwrap();
    assert_eq!(&cs, engine.codex.snapshot());
    assert_eq!(&hs, engine.claude.snapshot());
    assert!(engine.dispatch.is_none());
    assert_eq!(
        submit(&mut engine, "request-one").unwrap().action_count(),
        0
    );
}

#[test]
fn claude_flow_is_serial_and_restart_does_not_reinject() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    enqueue(&mut engine, "request-one")
        .unwrap()
        .claim()
        .unwrap();
    let batch = engine
        .apply_claude(claude::Command::DispatchNext { scope: scope() })
        .unwrap();
    let mut actions = batch.claim().unwrap();
    let Action::Claude(ClaudeAction::InspectComposer { operation, target }) = actions.remove(0)
    else {
        panic!("inspect")
    };
    let command = claude::Command::ComposerObserved {
        operation,
        composer: claude::Composer {
            target,
            state: claude::ComposerState::Empty,
            frame_token: "frame".into(),
            native_cursor: claude::Cursor {
                source_identity: "source".into(),
                offset: 100,
                head: "head".into(),
                anchor: "anchor".into(),
            },
        },
    };
    let batch = engine.apply_claude(command.clone()).unwrap();
    assert_eq!(
        engine.store.snapshots().unwrap().1.receipts["request-one"].state,
        claude::State::PrepareInFlight
    );
    assert!(matches!(
        batch.claim().unwrap().as_slice(),
        [Action::Claude(ClaudeAction::Prepare { .. })]
    ));
    assert!(matches!(
        engine.apply_claude(command),
        Err(Error::Claude(claude::Error::Stale))
    ));
    drop(engine);
    let mut restored = DeliveryEngine::open(dir.path()).unwrap();
    assert_eq!(
        restored.claude_outbox(&scope()).unwrap().outbox[0].state,
        "ambiguous"
    );
    assert_eq!(
        enqueue(&mut restored, "request-one")
            .unwrap()
            .action_count(),
        0
    );
}

#[test]
fn unclaimed_batch_blocks_then_drop_freezes_across_providers() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    let batch = submit(&mut engine, "request-one").unwrap();
    assert!(matches!(
        enqueue(&mut engine, "request-two"),
        Err(Error::DispatchPending)
    ));
    drop(batch);
    assert!(matches!(
        enqueue(&mut engine, "request-two"),
        Err(Error::Frozen)
    ));
    assert!(matches!(
        engine.codex_outbox("codex:synthetic", None),
        Err(Error::Frozen)
    ));
    assert_eq!(
        engine.logs(0, 128).unwrap().last().unwrap().event,
        LogEvent::Frozen
    );
}

#[test]
fn normal_open_initializes_missing_ledger_and_reopens_existing() {
    let dir = directory();
    let engine = DeliveryEngine::open(dir.path()).unwrap();
    drop(engine);
    assert!(DeliveryEngine::initialize(dir.path()).is_err());
    DeliveryEngine::open(dir.path()).unwrap();
}

#[test]
fn callers_cannot_forge_persisted_and_codex_agent_is_rejected() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    assert!(matches!(
        engine.apply_codex(
            codex::Command::Persisted(engine.codex.snapshot().version.clone()),
            None
        ),
        Err(Error::InternalCommand)
    ));
    claim(
        engine
            .apply_codex(
                codex::Command::Submit {
                    request: codex_request("request-one"),
                    now_ms: 0,
                },
                Some("child"),
            )
            .unwrap(),
    );
    assert!(
        engine
            .codex_outbox("codex:synthetic", Some("child"))
            .is_ok()
    );
}

#[test]
fn opaque_media_is_persisted_for_outbox_projection() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    let mut request = codex_request("request-one");
    request
        .payload
        .media
        .push(codex::MediaRef(json!({"src":"preview"})));
    claim(
        engine
            .apply_codex(codex::Command::Submit { request, now_ms: 0 }, None)
            .unwrap(),
    );
    let mut request = claude_request("request-one");
    request
        .payload
        .attachments
        .push(claude::Attachment(json!({"src":"preview"})));
    claim(
        engine
            .apply_claude(claude::Command::Enqueue { request, now_ms: 0 })
            .unwrap(),
    );
    assert_eq!(engine.codex.snapshot().version.revision, 1);
    assert_eq!(engine.claude.snapshot().version.revision, 1);
}

#[test]
fn claude_projection_matches_entire_scope_and_wire_shape() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    enqueue(&mut engine, "request-one")
        .unwrap()
        .claim()
        .unwrap();
    let mut different = scope();
    different.agent_id = Some("child".into());
    assert!(engine.claude_outbox(&different).unwrap().outbox.is_empty());
    different = scope();
    different.session_id = "request-other".into();
    assert!(engine.claude_outbox(&different).unwrap().outbox.is_empty());
    let json = serde_json::to_value(engine.claude_outbox(&scope()).unwrap()).unwrap();
    assert_eq!(
        json["outbox"][0],
        serde_json::json!({ "id":"request-one", "uid":"claude:synthetic", "text":"private synthetic prompt", "media":[], "created":1234, "state":"persisted", "attempts":0, "server":true })
    );
    assert!(json["outbox_version"]["epoch"].is_string());
    assert_eq!(json["outbox_version"]["revision"], 1);
}

#[test]
fn hidden_tombstone_retained_and_diagnostics_are_bounded_without_prompt() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    enqueue(&mut engine, "request-one")
        .unwrap()
        .claim()
        .unwrap();
    engine
        .apply_claude(claude::Command::Cancel {
            id: "request-one".into(),
            scope: scope(),
        })
        .unwrap()
        .claim()
        .unwrap();
    assert!(engine.claude_outbox(&scope()).unwrap().outbox.is_empty());
    let replay = enqueue(&mut engine, "request-one").unwrap();
    assert!(replay.replays()[0].known);
    assert!(!replay.replays()[0].visible);
    assert_eq!(replay.action_count(), 0);
    assert_eq!(engine.claude.snapshot().receipts.len(), 1);
    for _ in 0..140 {
        engine.log(LogEvent::Committed, Some(Provider::Claude));
    }
    let logs = engine.logs(0, 128).unwrap();
    assert_eq!(logs.len(), 128);
    assert!(logs[0].sequence > 1);
    assert_eq!(engine.logs(0, 129).unwrap().len(), 128);
    let info = engine.receipts(Provider::Claude, 0, 1).unwrap();
    assert!(
        !serde_json::to_string(&(logs, info))
            .unwrap()
            .contains("private synthetic prompt")
    );
}

#[test]
fn state_mapping_preserves_uncertainty_and_never_calls_it_sent() {
    use claude::State as H;
    use codex::State as C;
    assert_eq!(codex_state(C::CheckingDraft), ("queued", 0, None));
    for state in [C::DraftConflict, C::FailedBeforeWrite] {
        assert_eq!(codex_state(state).0, "failed");
        assert_eq!(codex_state(state).1, 0);
    }
    for state in [C::PrepareInFlight, C::EnterInFlight, C::Uncertain] {
        assert_eq!(codex_state(state).0, "failed");
        assert_eq!(codex_state(state).1, 1);
        assert!(codex_state(state).2.is_some());
    }
    for state in [H::Queued, H::Inspecting] {
        assert_eq!(claude_state(state).0, "persisted");
    }
    for state in [
        H::DraftConflict,
        H::PrepareInFlight,
        H::EnterInFlight,
        H::Uncertain,
        H::NativeRemoved,
        H::NativeDequeued,
    ] {
        assert_eq!(claude_state(state).0, "ambiguous");
    }
    assert_eq!(claude_state(H::NativeQueued).0, "native_queued");
    assert_eq!(claude_state(H::Restored).0, "restored");
    assert_eq!(claude_state(H::Interrupted).0, "aborted");
}

#[test]
fn external_syntactic_edit_reloads_before_replay() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    inspected(&mut engine, "request-one");
    let path = dir.path().join(store::LEDGER_FILENAME);
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b' ');
    fs::write(path, bytes).unwrap();
    assert_eq!(
        submit(&mut engine, "request-one").unwrap().action_count(),
        0
    );
    assert_eq!(
        enqueue(&mut engine, "request-two").unwrap().action_count(),
        0
    );
    assert!(
        engine
            .claude_outbox(&scope())
            .unwrap()
            .outbox
            .iter()
            .any(|row| row.id == "request-two")
    );
    assert_eq!(
        engine
            .apply_claude(claude::Command::DispatchNext { scope: scope() })
            .unwrap()
            .action_count(),
        1
    );
}

#[test]
fn external_semantic_edit_reloads_both_machines() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    inspected(&mut engine, "request-one");

    let mut external = DeliveryEngine::open(dir.path()).unwrap();
    inspected(&mut external, "request-two");
    drop(external);

    assert_eq!(
        submit(&mut engine, "request-one").unwrap().action_count(),
        0
    );
    assert!(engine.codex_receipt("request-two").unwrap().is_some());
    assert_eq!(
        enqueue(&mut engine, "request-three")
            .unwrap()
            .action_count(),
        0
    );
    assert!(
        engine
            .claude_outbox(&scope())
            .unwrap()
            .outbox
            .iter()
            .any(|row| row.id == "request-three")
    );
    assert_eq!(
        engine
            .apply_claude(claude::Command::DispatchNext { scope: scope() })
            .unwrap()
            .action_count(),
        1
    );
}

#[test]
fn detached_batch_cannot_be_claimed_after_its_engine_is_dropped() {
    let dir = directory();
    let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
    let batch = submit(&mut engine, "request-one").unwrap();
    drop(engine);
    let mut reopened = DeliveryEngine::open(dir.path()).unwrap();
    assert!(matches!(batch.claim(), Err(Error::Frozen)));
    assert_eq!(
        reopened
            .codex_outbox("codex:synthetic", None)
            .unwrap()
            .outbox[0]
            .state,
        "failed"
    );
    assert!(reopened.dispatch.is_none());
}
