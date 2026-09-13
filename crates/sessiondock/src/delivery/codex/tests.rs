use super::*;
use serde_json::json;

fn target() -> Target {
    Target {
        host_instance: "host-instance-one".into(),
        session_id: "terminal-one".into(),
        ownership_epoch: "lease-one".into(),
    }
}

fn request(id: &str) -> Request {
    Request {
        request_id: id.into(),
        payload: Payload {
            uid: "codex:synthetic".into(),
            target: target(),
            text: "  echo  one\n".into(),
            media: Vec::new(),
        },
    }
}

fn cursor(position: u64) -> NativeCursor {
    NativeCursor {
        source_identity: "synthetic-rollout-generation".into(),
        position,
        head: "head".into(),
        anchor: format!("anchor-{position}"),
    }
}

fn machine() -> Machine {
    Machine::new("process-one".into()).unwrap()
}

fn persist(machine: &mut Machine, command: Command) -> Vec<Effect> {
    let before = machine.snapshot().clone();
    let effects = machine.apply(command).unwrap();
    assert_eq!(
        machine.snapshot(),
        &before,
        "uncommitted proposals must not be published as durable state"
    );
    let [Effect::Persist { version, snapshot }] = effects.as_slice() else {
        panic!("expected only durable proposal: {effects:?}")
    };
    assert_eq!(snapshot.version, *version);
    machine.apply(Command::Persisted(version.clone())).unwrap()
}

fn inspect(machine: &mut Machine, request: Request) -> Operation {
    let effects = persist(
        machine,
        Command::Submit {
            request,
            now_ms: 1000,
        },
    );
    let [Effect::InspectComposer { operation, .. }] = effects.as_slice() else {
        panic!("expected inspection")
    };
    operation.clone()
}

fn draft(state: DraftState, token: &str) -> DraftObservation {
    DraftObservation {
        target: target(),
        state,
        token: token.into(),
        native_cursor: cursor(100),
    }
}

fn arm_prepare(machine: &mut Machine, request: Request) -> Operation {
    let operation = inspect(machine, request);
    let effects = persist(
        machine,
        Command::DraftObserved {
            operation,
            observation: draft(DraftState::Empty, "empty-frame"),
        },
    );
    let [
        Effect::InjectPrepare {
            operation,
            expected_frame,
            overwrite,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("expected prepare")
    };
    assert_eq!(expected_frame, "empty-frame");
    assert!(!overwrite);
    operation.clone()
}

fn arm_enter(machine: &mut Machine, request: Request) -> Operation {
    let operation = arm_prepare(machine, request.clone());
    let effects = persist(
        machine,
        Command::Prepared {
            operation,
            evidence: PreparedEvidence {
                target: target(),
                observed_text: request.payload.text,
                observed_media: request.payload.media,
                frame_token: "prepared-frame".into(),
            },
        },
    );
    let [
        Effect::InjectEnter {
            operation,
            expected_frame,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("expected Enter")
    };
    assert_eq!(expected_frame, "prepared-frame");
    operation.clone()
}

fn uncertain(machine: &mut Machine, request: Request) -> Operation {
    let operation = arm_enter(machine, request);
    assert!(
        persist(
            machine,
            Command::EnterFinished {
                operation: operation.clone(),
                result: EnterResult::TransportReturned
            }
        )
        .is_empty()
    );
    operation
}

fn ack(machine: &Machine, id: &str) -> AckEvidence {
    let row = &machine.snapshot().receipts[id];
    AckEvidence {
        uid: row.request.payload.uid.clone(),
        validated_confirmation: row.confirmation.clone().unwrap(),
        record: NativeAcceptance {
            source_identity: cursor(100).source_identity,
            record_id: "native-user-one".into(),
            start: 120,
            end: 180,
            turn_id: "native-turn-one".into(),
        },
        text: row.request.payload.text.trim().into(),
        observed_media: row.request.payload.media.clone(),
        real_user_input: true,
        correlation: Correlation::OperationTurn {
            enter_operation: row.enter_operation.clone().unwrap(),
            turn_id: "native-turn-one".into(),
        },
    }
}

#[test]
fn persist_is_a_real_barrier_and_duplicate_commit_cannot_replay_injection() {
    let mut machine = machine();
    let request = request("request-one");
    let effects = machine
        .apply(Command::Submit {
            request: request.clone(),
            now_ms: 1,
        })
        .unwrap();
    let [Effect::Persist { version, .. }] = effects.as_slice() else {
        panic!()
    };
    assert!(machine.snapshot().receipts.is_empty());
    assert_eq!(
        machine
            .apply(Command::Persisted(Version {
                epoch: "old-process".into(),
                revision: version.revision
            }))
            .unwrap_err(),
        Error::StaleOperation
    );
    assert!(matches!(
        machine
            .apply(Command::Submit { request, now_ms: 2 })
            .unwrap()
            .as_slice(),
        [Effect::Replay { pending: true, .. }]
    ));
    assert!(matches!(
        machine
            .apply(Command::Persisted(version.clone()))
            .unwrap()
            .as_slice(),
        [Effect::InspectComposer { .. }]
    ));
    assert_eq!(
        machine
            .apply(Command::Persisted(version.clone()))
            .unwrap_err(),
        Error::StaleOperation
    );
    let operation = machine.token(&machine.snapshot().receipts["request-one"]);
    let proposal = machine
        .apply(Command::DraftObserved {
            operation,
            observation: draft(DraftState::Empty, "frame"),
        })
        .unwrap();
    let [Effect::Persist { version, snapshot }] = proposal.as_slice() else {
        panic!()
    };
    assert_eq!(
        snapshot.receipts["request-one"].state,
        State::PrepareInFlight
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::CheckingDraft
    );
    assert!(matches!(
        machine
            .apply(Command::Persisted(version.clone()))
            .unwrap()
            .as_slice(),
        [Effect::InjectPrepare { .. }]
    ));
    assert_eq!(
        machine
            .apply(Command::Persisted(version.clone()))
            .unwrap_err(),
        Error::StaleOperation
    );
}

#[test]
fn request_id_binds_all_payload_fields_and_never_inspects_on_replay() {
    let mut machine = machine();
    let original = request("request-one");
    inspect(&mut machine, original.clone());
    let revision = machine.snapshot().version.clone();
    assert!(matches!(
        machine
            .apply(Command::Submit {
                request: original.clone(),
                now_ms: 999
            })
            .unwrap()
            .as_slice(),
        [Effect::Replay { pending: false, .. }]
    ));
    for changed in 0..5 {
        let mut conflicting = original.clone();
        match changed {
            0 => conflicting.payload.text.push(' '),
            1 => conflicting.payload.uid.push('2'),
            2 => conflicting.payload.target.host_instance.push('2'),
            3 => conflicting.payload.target.ownership_epoch.push('2'),
            _ => conflicting.payload.media.push(MediaRef(json!({
                "id": "attachment",
                "content_digest": "digest",
            }))),
        }
        assert_eq!(
            machine
                .apply(Command::Submit {
                    request: conflicting,
                    now_ms: 0
                })
                .unwrap_err(),
            Error::Conflict
        );
    }
    assert_eq!(machine.snapshot().version, revision);
}

#[test]
fn draft_consent_is_versioned_and_rechecked_before_clear_or_paste() {
    let mut machine = machine();
    let operation = inspect(&mut machine, request("request-one"));
    assert!(
        persist(
            &mut machine,
            Command::DraftObserved {
                operation,
                observation: draft(DraftState::Editing, "draft-one")
            }
        )
        .is_empty()
    );
    assert_eq!(
        machine
            .apply(Command::ConfirmDraft {
                request_id: "request-one".into(),
                uid: "codex:synthetic".into(),
                token: "stale".into()
            })
            .unwrap_err(),
        Error::Conflict
    );
    let effects = persist(
        &mut machine,
        Command::ConfirmDraft {
            request_id: "request-one".into(),
            uid: "codex:synthetic".into(),
            token: "draft-one".into(),
        },
    );
    let [Effect::InspectComposer { operation, .. }] = effects.as_slice() else {
        panic!()
    };
    assert!(
        persist(
            &mut machine,
            Command::DraftObserved {
                operation: operation.clone(),
                observation: draft(DraftState::Editing, "changed-draft")
            }
        )
        .is_empty()
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"]
            .draft_token
            .as_deref(),
        Some("changed-draft")
    );
    let effects = persist(
        &mut machine,
        Command::ConfirmDraft {
            request_id: "request-one".into(),
            uid: "codex:synthetic".into(),
            token: "changed-draft".into(),
        },
    );
    let [Effect::InspectComposer { operation, .. }] = effects.as_slice() else {
        panic!()
    };
    let effects = persist(
        &mut machine,
        Command::DraftObserved {
            operation: operation.clone(),
            observation: draft(DraftState::Editing, "changed-draft"),
        },
    );
    assert!(
        matches!(effects.as_slice(), [Effect::InjectPrepare { overwrite: true, expected_frame, .. }] if expected_frame == "changed-draft")
    );
}

#[test]
fn failed_inspection_is_retryable_but_every_armed_write_is_not() {
    let mut machine = machine();
    let operation = inspect(&mut machine, request("request-one"));
    persist(
        &mut machine,
        Command::InspectionFailed {
            operation,
            reason: "no composer proof".into(),
        },
    );
    assert!(machine.snapshot().receipts["request-one"].retryable());
    let effects = persist(
        &mut machine,
        Command::Retry {
            request_id: "request-one".into(),
            uid: "codex:synthetic".into(),
        },
    );
    let [Effect::InspectComposer { operation, .. }] = effects.as_slice() else {
        panic!()
    };
    let effects = persist(
        &mut machine,
        Command::DraftObserved {
            operation: operation.clone(),
            observation: draft(DraftState::Empty, "empty"),
        },
    );
    let [Effect::InjectPrepare { operation, .. }] = effects.as_slice() else {
        panic!()
    };
    persist(
        &mut machine,
        Command::PrepareFailed {
            operation: operation.clone(),
            reason: "paste outcome unknown".into(),
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::Uncertain
    );
    assert!(!machine.snapshot().receipts["request-one"].retryable());
    assert_eq!(
        machine
            .apply(Command::Retry {
                request_id: "request-one".into(),
                uid: "codex:synthetic".into()
            })
            .unwrap_err(),
        Error::WrongState
    );
    assert_eq!(
        machine
            .apply(Command::Discard {
                request_id: "request-one".into(),
                uid: "codex:synthetic".into()
            })
            .unwrap_err(),
        Error::WrongState
    );
}

#[test]
fn changed_prepared_composer_never_authorizes_enter() {
    let mut machine = machine();
    let operation = arm_prepare(&mut machine, request("request-one"));
    let effects = persist(
        &mut machine,
        Command::Prepared {
            operation,
            evidence: PreparedEvidence {
                target: target(),
                observed_text: "later user's draft".into(),
                observed_media: vec![],
                frame_token: "different".into(),
            },
        },
    );
    assert!(effects.is_empty());
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::Uncertain
    );
}

#[test]
fn transport_success_is_not_native_ack_and_new_followups_need_not_wait_for_idle() {
    let mut machine = machine();
    uncertain(&mut machine, request("request-one"));
    let row = &machine.snapshot().receipts["request-one"];
    assert_eq!(row.state, State::Uncertain);
    assert!(row.accepted.is_none());
    assert!(row.visible_in_outbox());
    // There is deliberately no inferred idle/activity gate on Codex's queue.
    inspect(&mut machine, request("request-two"));
    assert_eq!(machine.snapshot().receipts.len(), 2);
}

#[test]
fn invalid_native_records_are_rejected_and_causal_text_match_is_accepted() {
    let mut machine = machine();
    uncertain(&mut machine, request("request-one"));
    let valid = ack(&machine, "request-one");
    for variant in 0..7 {
        let mut bad = valid.clone();
        match variant {
            0 => bad.record.start = 99,
            1 => bad.record.source_identity = "replaced-rollout".into(),
            2 => bad.validated_confirmation.anchor = "changed-prefix".into(),
            3 => bad.real_user_input = false,
            4 => bad.record.turn_id.clear(),
            5 => bad.text = "echo one".into(),
            _ => bad.correlation = Correlation::NativeRequestId("another-request".into()),
        }
        assert_eq!(
            machine
                .apply(Command::NativeAck {
                    request_id: "request-one".into(),
                    evidence: bad
                })
                .unwrap_err(),
            Error::UnprovenAcknowledgment
        );
    }
    persist(
        &mut machine,
        Command::NativeAck {
            request_id: "request-one".into(),
            evidence: valid,
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::Acknowledged
    );
    assert!(!machine.snapshot().receipts["request-one"].visible_in_outbox());
    assert!(matches!(
        machine
            .apply(Command::Submit {
                request: request("request-one"),
                now_ms: 99
            })
            .unwrap()
            .as_slice(),
        [Effect::Replay { .. }]
    ));
}

#[test]
fn one_native_turn_or_record_cannot_acknowledge_two_requests() {
    let mut machine = machine();
    uncertain(&mut machine, request("request-one"));
    let first = ack(&machine, "request-one");
    persist(
        &mut machine,
        Command::NativeAck {
            request_id: "request-one".into(),
            evidence: first,
        },
    );
    uncertain(&mut machine, request("request-two"));
    let second = ack(&machine, "request-two");
    assert_eq!(
        machine
            .apply(Command::NativeAck {
                request_id: "request-two".into(),
                evidence: second
            })
            .unwrap_err(),
        Error::UnprovenAcknowledgment
    );
}

#[test]
fn moving_watch_cursor_never_moves_confirmation_or_silently_accepts_reset() {
    let mut machine = machine();
    uncertain(&mut machine, request("request-one"));
    let operation = machine.token(&machine.snapshot().receipts["request-one"]);
    persist(
        &mut machine,
        Command::AdvanceWatch {
            operation,
            previous: cursor(100),
            next: cursor(800),
            validated_confirmation: cursor(100),
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].confirmation,
        Some(cursor(100))
    );
    let effects = machine
        .apply(Command::InspectNative {
            request_id: "request-one".into(),
            uid: "codex:synthetic".into(),
            replay: true,
        })
        .unwrap();
    assert!(
        matches!(effects.as_slice(), [Effect::InspectNative { from, .. }] if from.position == 100)
    );
    let effects = machine
        .apply(Command::InspectNative {
            request_id: "request-one".into(),
            uid: "codex:synthetic".into(),
            replay: false,
        })
        .unwrap();
    assert!(
        matches!(effects.as_slice(), [Effect::InspectNative { from, .. }] if from.position == 800)
    );
    let operation = machine.token(&machine.snapshot().receipts["request-one"]);
    persist(
        &mut machine,
        Command::NativeReset {
            operation,
            reason: "rollout prefix changed".into(),
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].confirmation,
        Some(cursor(100))
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::Uncertain
    );
}

#[test]
fn stop_and_completion_require_the_exact_accepted_turn() {
    let mut machine = machine();
    uncertain(&mut machine, request("request-one"));
    assert_eq!(
        machine
            .apply(Command::RequestStop {
                request_id: "request-one".into(),
                uid: "codex:synthetic".into()
            })
            .unwrap_err(),
        Error::WrongState
    );
    let proof = ack(&machine, "request-one");
    persist(
        &mut machine,
        Command::NativeAck {
            request_id: "request-one".into(),
            evidence: proof,
        },
    );
    let effects = persist(
        &mut machine,
        Command::RequestStop {
            request_id: "request-one".into(),
            uid: "codex:synthetic".into(),
        },
    );
    assert!(
        matches!(effects.as_slice(), [Effect::InterruptTurn { turn_id, .. }] if turn_id == "native-turn-one")
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::StopRequested
    );
    let mut completed = CompletionEvidence {
        uid: "codex:synthetic".into(),
        validated_confirmation: cursor(100),
        source_identity: cursor(100).source_identity,
        turn_id: "earlier-unrelated-turn".into(),
        record_end: 240,
        outcome: Completion::Stopped,
    };
    assert_eq!(
        machine
            .apply(Command::NativeCompletion {
                request_id: "request-one".into(),
                evidence: completed.clone()
            })
            .unwrap_err(),
        Error::UnprovenAcknowledgment
    );
    completed.turn_id = "native-turn-one".into();
    persist(
        &mut machine,
        Command::NativeCompletion {
            request_id: "request-one".into(),
            evidence: completed,
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::Stopped
    );
    assert!(!machine.snapshot().receipts["request-one"].retryable());
}

#[test]
fn recovery_after_prepare_or_enter_commit_is_uncertain_and_emits_no_write() {
    for enter in [false, true] {
        let mut original = machine();
        let old_operation = if enter {
            arm_enter(&mut original, request("request-one"))
        } else {
            arm_prepare(&mut original, request("request-one"))
        };
        // Memory-only serde roundtrip simulates persisted bytes, never disk I/O.
        let snapshot: Snapshot =
            serde_json::from_str(&serde_json::to_string(original.snapshot()).unwrap()).unwrap();
        let (mut recovered, effects) = Machine::restore(snapshot, "process-two".into()).unwrap();
        let [Effect::Persist { version, snapshot }] = effects.as_slice() else {
            panic!()
        };
        assert_eq!(snapshot.receipts["request-one"].state, State::Uncertain);
        assert!(
            recovered
                .apply(Command::Persisted(version.clone()))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            recovered
                .apply(Command::PrepareFailed {
                    operation: old_operation,
                    reason: "late callback".into()
                })
                .unwrap_err(),
            Error::StaleOperation
        );
        assert_eq!(
            recovered
                .apply(Command::Retry {
                    request_id: "request-one".into(),
                    uid: "codex:synthetic".into()
                })
                .unwrap_err(),
            Error::WrongState
        );
    }
}

#[test]
fn recovery_before_write_requires_explicit_retry_and_new_epoch() {
    let mut original = machine();
    inspect(&mut original, request("request-one"));
    assert!(Machine::restore(original.snapshot().clone(), "process-one".into()).is_err());
    let (mut recovered, effects) =
        Machine::restore(original.snapshot().clone(), "process-two".into()).unwrap();
    let [Effect::Persist { version, .. }] = effects.as_slice() else {
        panic!()
    };
    assert!(
        recovered
            .apply(Command::Persisted(version.clone()))
            .unwrap()
            .is_empty()
    );
    assert!(recovered.snapshot().receipts["request-one"].retryable());
    assert_eq!(recovered.snapshot().version.epoch, "process-two");
    assert!(recovered.snapshot().version.revision > original.snapshot().version.revision);
}

#[test]
fn discard_retains_a_tombstone_and_is_scoped_and_idempotent() {
    let mut machine = machine();
    inspect(&mut machine, request("request-one"));
    assert_eq!(
        machine
            .apply(Command::Discard {
                request_id: "request-one".into(),
                uid: "codex:other".into()
            })
            .unwrap_err(),
        Error::Missing
    );
    persist(
        &mut machine,
        Command::Discard {
            request_id: "request-one".into(),
            uid: "codex:synthetic".into(),
        },
    );
    assert!(matches!(
        machine
            .apply(Command::Discard {
                request_id: "request-one".into(),
                uid: "codex:synthetic".into()
            })
            .unwrap()
            .as_slice(),
        [Effect::Replay { .. }]
    ));
    assert!(matches!(
        machine
            .apply(Command::Submit {
                request: request("request-one"),
                now_ms: 50
            })
            .unwrap()
            .as_slice(),
        [Effect::Replay { .. }]
    ));
    assert!(!machine.snapshot().receipts["request-one"].visible_in_outbox());
}

#[test]
fn corrupted_restore_and_invalid_or_oversized_requests_fail_closed() {
    let mut original = machine();
    uncertain(&mut original, request("request-one"));
    for variant in 0..3 {
        let mut bad = original.snapshot().clone();
        match variant {
            0 => bad.schema += 1,
            1 => bad.receipts.get_mut("request-one").unwrap().confirmation = None,
            _ => bad.receipts.get_mut("request-one").unwrap().state = State::FailedBeforeWrite,
        }
        assert!(Machine::restore(bad, "process-two".into()).is_err());
    }
    assert!(
        original
            .apply(Command::Submit {
                request: request(""),
                now_ms: 0
            })
            .is_err()
    );
    let mut large = request("request-big");
    large.payload.text = "x".repeat(MAX_PAYLOAD_BYTES + 1);
    assert!(
        original
            .apply(Command::Submit {
                request: large,
                now_ms: 0
            })
            .is_err()
    );
}

#[test]
fn receipt_history_beyond_old_count_and_bytes_accepts_host_sized_text_and_replay() {
    let mut original = machine();
    inspect(&mut original, request("request-seed"));
    let mut snapshot = original.snapshot().clone();
    let mut seed = snapshot.receipts.remove("request-seed").unwrap();
    seed.state = State::Discarded;
    for index in 0..4096 {
        let mut row = seed.clone();
        row.request.request_id = format!("request-{index:04}");
        row.request.payload.text = "x".repeat(2050);
        snapshot
            .receipts
            .insert(row.request.request_id.clone(), row);
    }
    let (mut restored, effects) = Machine::restore(snapshot, "process-two".into()).unwrap();
    let Effect::Persist { version, .. } = &effects[0] else {
        panic!()
    };
    restored.apply(Command::Persisted(version.clone())).unwrap();
    for id in ["request-large-one", "request-large-two"] {
        let mut large = request(id);
        large.payload.text = "x".repeat(1024 * 1024);
        let operation = inspect(&mut restored, large.clone());
        let replay = restored
            .apply(Command::Submit {
                request: large,
                now_ms: 3,
            })
            .unwrap();
        assert!(matches!(
            replay.as_slice(),
            [Effect::Replay { pending: false, .. }]
        ));
        persist(
            &mut restored,
            Command::DraftObserved {
                operation,
                observation: draft(DraftState::Editing, "draft"),
            },
        );
    }
    assert_eq!(restored.snapshot().receipts.len(), 4098);
}

#[test]
fn draft_confirmation_cannot_overtake_another_critical_submission() {
    let mut machine = machine();
    let operation = inspect(&mut machine, request("request-one"));
    persist(
        &mut machine,
        Command::DraftObserved {
            operation,
            observation: draft(DraftState::Editing, "draft"),
        },
    );
    inspect(&mut machine, request("request-two"));
    assert_eq!(
        machine
            .apply(Command::ConfirmDraft {
                request_id: "request-one".into(),
                uid: "codex:synthetic".into(),
                token: "draft".into()
            })
            .unwrap_err(),
        Error::WrongState
    );
    // Different UIDs must not evade the physical terminal's critical section.
    let mut alias = request("request-three");
    alias.payload.uid = "codex:alias".into();
    assert_eq!(
        machine
            .apply(Command::Submit {
                request: alias,
                now_ms: 0
            })
            .unwrap_err(),
        Error::WrongState
    );
}

#[test]
fn attachment_evidence_and_persisted_association_are_required() {
    let mut machine = machine();
    let mut with_media = request("request-one");
    with_media.payload.media.push(MediaRef(json!({
        "id": "synthetic-ref",
        "content_digest": "synthetic-content-digest",
    })));
    uncertain(&mut machine, with_media);
    let mut evidence = ack(&machine, "request-one");
    evidence.observed_media.clear();
    assert_eq!(
        machine
            .apply(Command::NativeAck {
                request_id: "request-one".into(),
                evidence
            })
            .unwrap_err(),
        Error::UnprovenAcknowledgment
    );
    let mut evidence = ack(&machine, "request-one");
    evidence.correlation = Correlation::PossibleTextMatch;
    persist(
        &mut machine,
        Command::NativeAck {
            request_id: "request-one".into(),
            evidence,
        },
    );
    let (mut restored, effects) =
        Machine::restore(machine.snapshot().clone(), "process-two".into()).unwrap();
    let [Effect::Persist { version, .. }] = effects.as_slice() else {
        panic!()
    };
    restored.apply(Command::Persisted(version.clone())).unwrap();
    assert_eq!(
        restored.snapshot().receipts["request-one"].state,
        State::Acknowledged
    );
}

#[test]
fn failed_native_completion_is_terminal_not_a_retryable_delivery_failure() {
    let mut machine = machine();
    uncertain(&mut machine, request("request-one"));
    let evidence = ack(&machine, "request-one");
    persist(
        &mut machine,
        Command::NativeAck {
            request_id: "request-one".into(),
            evidence,
        },
    );
    let evidence = CompletionEvidence {
        uid: "codex:synthetic".into(),
        validated_confirmation: cursor(100),
        source_identity: cursor(100).source_identity,
        turn_id: "native-turn-one".into(),
        record_end: 999,
        outcome: Completion::Failed,
    };
    persist(
        &mut machine,
        Command::NativeCompletion {
            request_id: "request-one".into(),
            evidence: evidence.clone(),
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::Completed
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].completion,
        Some(Completion::Failed)
    );
    assert!(!machine.snapshot().receipts["request-one"].retryable());
    assert!(matches!(
        machine
            .apply(Command::NativeCompletion {
                request_id: "request-one".into(),
                evidence
            })
            .unwrap()
            .as_slice(),
        [Effect::Replay { .. }]
    ));
}

#[test]
fn recovery_of_a_stop_request_does_not_interrupt_a_later_turn() {
    let mut original = machine();
    uncertain(&mut original, request("request-one"));
    let evidence = ack(&original, "request-one");
    persist(
        &mut original,
        Command::NativeAck {
            request_id: "request-one".into(),
            evidence,
        },
    );
    persist(
        &mut original,
        Command::RequestStop {
            request_id: "request-one".into(),
            uid: "codex:synthetic".into(),
        },
    );
    let (mut restored, effects) =
        Machine::restore(original.snapshot().clone(), "process-two".into()).unwrap();
    let [Effect::Persist { version, .. }] = effects.as_slice() else {
        panic!()
    };
    assert!(
        restored
            .apply(Command::Persisted(version.clone()))
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        restored
            .apply(Command::RequestStop {
                request_id: "request-one".into(),
                uid: "codex:synthetic".into()
            })
            .unwrap()
            .as_slice(),
        [Effect::Replay { .. }]
    ));
}

#[test]
fn same_offset_prefix_replacement_is_not_a_watch_advance() {
    let mut machine = machine();
    uncertain(&mut machine, request("request-one"));
    let operation = machine.token(&machine.snapshot().receipts["request-one"]);
    let mut changed = cursor(100);
    changed.anchor = "new-text-at-same-offset".into();
    assert_eq!(
        machine
            .apply(Command::AdvanceWatch {
                operation,
                previous: cursor(100),
                next: changed,
                validated_confirmation: cursor(100)
            })
            .unwrap_err(),
        Error::UnprovenAcknowledgment
    );
}

#[test]
fn dismiss_hides_an_uncertain_receipt_without_forgetting_it_and_discards_pre_write_rows() {
    let mut machine = machine();
    let operation = uncertain(&mut machine, request("request-one"));
    let revision = machine.snapshot().receipts["request-one"].revision;
    // Display-only: the row keeps its state, revision and tombstone.
    assert!(
        persist(
            &mut machine,
            Command::Dismiss {
                request_id: "request-one".into(),
                uid: "codex:synthetic".into(),
            },
        )
        .is_empty()
    );
    let row = &machine.snapshot().receipts["request-one"];
    assert!(row.dismissed);
    assert_eq!(row.state, State::Uncertain);
    assert_eq!(row.revision, revision);
    assert!(!row.visible_in_outbox());
    assert!(!row.retryable());
    assert!(matches!(
        machine
            .apply(Command::Dismiss {
                request_id: "request-one".into(),
                uid: "codex:synthetic".into(),
            })
            .unwrap()
            .as_slice(),
        [Effect::Replay { .. }]
    ));
    assert!(matches!(
        machine
            .apply(Command::Submit {
                request: request("request-one"),
                now_ms: 50,
            })
            .unwrap()
            .as_slice(),
        [Effect::Replay { .. }]
    ));
    assert_eq!(
        machine
            .apply(Command::Retry {
                request_id: "request-one".into(),
                uid: "codex:synthetic".into(),
            })
            .unwrap_err(),
        Error::WrongState
    );
    // The persisted row's in-flight operation token is still the same.
    assert!(
        machine
            .apply(Command::InspectNative {
                request_id: "request-one".into(),
                uid: "codex:synthetic".into(),
                replay: false,
            })
            .is_ok()
    );
    let _ = operation;
    // A pre-write row is discarded outright; a live boundary is display-only.
    inspect(&mut machine, request("request-two"));
    persist(
        &mut machine,
        Command::Dismiss {
            request_id: "request-two".into(),
            uid: "codex:synthetic".into(),
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-two"].state,
        State::Discarded
    );
    let in_flight = arm_enter(&mut machine, request("request-three"));
    persist(
        &mut machine,
        Command::Dismiss {
            request_id: "request-three".into(),
            uid: "codex:synthetic".into(),
        },
    );
    assert!(machine.snapshot().receipts["request-three"].dismissed);
    assert_eq!(
        machine.snapshot().receipts["request-three"].state,
        State::EnterInFlight
    );
    assert!(Machine::restore(machine.snapshot().clone(), "during-enter".into()).is_ok());
    assert!(
        persist(
            &mut machine,
            Command::EnterFinished {
                operation: in_flight,
                result: EnterResult::TransportReturned,
            }
        )
        .is_empty()
    );
    assert!(machine.snapshot().receipts["request-three"].dismissed);
    assert_eq!(
        machine.snapshot().receipts["request-three"].state,
        State::Uncertain
    );
    assert_eq!(
        machine
            .apply(Command::Dismiss {
                request_id: "request-one".into(),
                uid: "codex:other".into(),
            })
            .unwrap_err(),
        Error::Missing
    );
    // Restore keeps the dismissal and rejects a dismissed pre-write row.
    let snapshot = machine.snapshot().clone();
    let (restored, _) = Machine::restore(snapshot.clone(), "process-two".into()).unwrap();
    assert!(restored.snapshot().receipts["request-one"].dismissed);
    let mut corrupted = snapshot;
    corrupted.receipts.get_mut("request-two").unwrap().dismissed = true;
    assert!(Machine::restore(corrupted, "process-three".into()).is_err());
}

#[test]
fn dismiss_during_prepare_keeps_one_shot_callback_and_restart_state() {
    let mut machine = machine();
    let req = request("dismiss-prepare");
    let operation = arm_prepare(&mut machine, req.clone());
    let revision = machine.snapshot().receipts["dismiss-prepare"].revision;
    assert!(
        persist(
            &mut machine,
            Command::Dismiss {
                request_id: "dismiss-prepare".into(),
                uid: "codex:synthetic".into(),
            }
        )
        .is_empty()
    );
    let row = &machine.snapshot().receipts["dismiss-prepare"];
    assert!(row.dismissed && !row.visible_in_outbox() && !row.retryable());
    assert_eq!(row.revision, revision);
    assert!(Machine::restore(machine.snapshot().clone(), "during-prepare".into()).is_ok());
    let effects = persist(
        &mut machine,
        Command::Prepared {
            operation: operation.clone(),
            evidence: PreparedEvidence {
                target: target(),
                observed_text: req.payload.text,
                observed_media: req.payload.media,
                frame_token: "prepared-frame".into(),
            },
        },
    );
    let [
        Effect::InjectEnter {
            operation: enter, ..
        },
    ] = effects.as_slice()
    else {
        panic!("dismissal must preserve the already authorized prepare callback");
    };
    assert!(
        persist(
            &mut machine,
            Command::EnterFinished {
                operation: enter.clone(),
                result: EnterResult::TransportReturned,
            }
        )
        .is_empty()
    );
    assert!(machine.snapshot().receipts["dismiss-prepare"].dismissed);
    assert!(
        machine
            .apply(Command::Prepared {
                operation,
                evidence: PreparedEvidence {
                    target: target(),
                    observed_text: "ignored".into(),
                    observed_media: vec![],
                    frame_token: "prepared-frame".into(),
                },
            })
            .is_err(),
        "a callback cannot authorize a second Enter"
    );
}

#[test]
fn unicode_request_ids_survive_serialized_restore_and_replay() {
    for id in ["x".to_owned(), " ../短🦀 ?# ".to_owned(), "🦀".repeat(128)] {
        let mut original = machine();
        uncertain(&mut original, request(&id));
        let encoded = serde_json::to_vec(original.snapshot()).unwrap();
        let decoded = serde_json::from_slice(&encoded).unwrap();
        let (mut restored, effects) = Machine::restore(decoded, "process-two".into()).unwrap();
        let [Effect::Persist { version, .. }] = effects.as_slice() else {
            panic!("restore must persist its new epoch")
        };
        restored.apply(Command::Persisted(version.clone())).unwrap();
        assert!(restored.snapshot().receipts.contains_key(&id));
        let before = restored.snapshot().clone();
        let replay = restored
            .apply(Command::Submit {
                request: request(&id),
                now_ms: 2000,
            })
            .unwrap();
        assert_eq!(restored.snapshot(), &before);
        assert!(!replay.iter().any(|effect| matches!(
            effect,
            Effect::Persist { .. } | Effect::InjectPrepare { .. } | Effect::InjectEnter { .. }
        )));
        let mut changed = request(&id);
        changed.payload.text = "different text".into();
        assert!(
            restored
                .apply(Command::Submit {
                    request: changed,
                    now_ms: 2000
                })
                .is_err()
        );
    }
    let mut original = machine();
    assert!(
        original
            .apply(Command::Submit {
                request: request(&"🦀".repeat(129)),
                now_ms: 0
            })
            .is_err()
    );
}
