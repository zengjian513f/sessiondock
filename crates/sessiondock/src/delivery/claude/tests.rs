use super::*;
use serde_json::json;

fn scope() -> Scope {
    Scope {
        uid: "claude:synthetic".into(),
        session_id: "native-session".into(),
        agent_id: None,
    }
}
fn target() -> Target {
    Target {
        host_instance: "synthetic-host".into(),
        terminal_id: "native-terminal".into(),
        ownership_epoch: "owner-one".into(),
    }
}
fn request(id: &str) -> Request {
    Request {
        id: id.into(),
        payload: Payload {
            scope: scope(),
            target: target(),
            text: "  same  prompt\n".into(),
            attachments: vec![],
        },
    }
}
fn cursor(offset: u64) -> Cursor {
    Cursor {
        source_identity: "synthetic-native-source".into(),
        offset,
        head: "head".into(),
        anchor: format!("anchor-{offset}"),
    }
}
fn machine() -> Machine {
    Machine::new("process-one".into()).unwrap()
}
fn record(id: &str, start: u64) -> NativeRecord {
    NativeRecord {
        source_identity: cursor(100).source_identity,
        id: id.into(),
        start,
        end: start + 20,
    }
}
fn context(id: &str, start: u64) -> NativeContext {
    NativeContext {
        scope: scope(),
        confirmation: cursor(100),
        record: record(id, start),
    }
}
fn composer(state: ComposerState, token: &str) -> Composer {
    Composer {
        target: target(),
        state,
        frame_token: token.into(),
        native_cursor: cursor(100),
    }
}
fn prepared(request: &Request) -> Prepared {
    Prepared {
        target: request.payload.target.clone(),
        observed_text: request.payload.text.clone(),
        observed_attachments: request.payload.attachments.clone(),
        frame_token: "prepared-frame".into(),
    }
}

fn commit(machine: &mut Machine, command: Command) -> Vec<Effect> {
    let previous = machine.snapshot().clone();
    let effects = machine.apply(command).unwrap();
    assert_eq!(machine.snapshot(), &previous);
    let [Effect::Persist { version, snapshot }] = effects.as_slice() else {
        panic!("only Persist may precede durable commit: {effects:?}")
    };
    assert_eq!(&snapshot.version, version);
    machine.apply(Command::Persisted(version.clone())).unwrap()
}

fn enqueue(machine: &mut Machine, id: &str) {
    assert!(
        commit(
            machine,
            Command::Enqueue {
                request: request(id),
                now_ms: 1
            }
        )
        .is_empty()
    );
}
fn dispatch(machine: &mut Machine) -> Operation {
    let effects = commit(machine, Command::DispatchNext { scope: scope() });
    let [Effect::InspectComposer { operation, .. }] = effects.as_slice() else {
        panic!()
    };
    operation.clone()
}
fn begin_prepare(machine: &mut Machine, id: &str) -> Operation {
    enqueue(machine, id);
    let operation = dispatch(machine);
    let effects = commit(
        machine,
        Command::ComposerObserved {
            operation,
            composer: composer(ComposerState::Empty, "empty"),
        },
    );
    let [
        Effect::PrepareInput {
            operation,
            overwrite,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!()
    };
    assert!(!overwrite);
    operation.clone()
}
fn begin_enter(machine: &mut Machine, id: &str) -> Operation {
    let operation = begin_prepare(machine, id);
    let effects = commit(
        machine,
        Command::Prepared {
            operation,
            evidence: prepared(&request(id)),
        },
    );
    let [Effect::PressEnter { operation, .. }] = effects.as_slice() else {
        panic!()
    };
    operation.clone()
}
fn submitted(machine: &mut Machine, id: &str) -> Operation {
    let operation = begin_enter(machine, id);
    commit(
        machine,
        Command::InjectionFinished {
            operation: operation.clone(),
            transport_returned: true,
        },
    );
    operation
}
fn queue(machine: &Machine, id: &str, event: &str, start: u64) -> QueueEvidence {
    QueueEvidence {
        context: context(event, start),
        change: QueueChange::Enqueue {
            text: request(id).payload.text.trim().into(),
            attachments: vec![],
            association: Association::VerifiedEnter(
                machine.snapshot().receipts[id].enter.clone().unwrap(),
            ),
        },
    }
}
fn user(machine: &Machine, id: &str, event: &str, start: u64) -> UserEvidence {
    UserEvidence {
        context: context(event, start),
        text: request(id).payload.text.trim().into(),
        attachments: vec![],
        turn: Turn {
            user_uuid: event.into(),
            parent_turn_uuid: Some("earlier-parent-turn".into()),
        },
        real_human_input: true,
        association: Association::VerifiedEnter(
            machine.snapshot().receipts[id].enter.clone().unwrap(),
        ),
    }
}

#[test]
fn local_queue_is_durable_fifo_and_is_not_native_receipt() {
    let mut machine = machine();
    enqueue(&mut machine, "request-two");
    enqueue(&mut machine, "request-one");
    assert_eq!(
        machine.snapshot().receipts["request-two"].state,
        State::Queued
    );
    assert!(
        machine.snapshot().receipts["request-two"]
            .accepted
            .is_none()
    );
    assert!(
        machine.snapshot().receipts["request-two"]
            .native_queue
            .is_none()
    );
    let operation = dispatch(&mut machine);
    assert_eq!(
        operation.request_id, "request-two",
        "enqueue order, not lexical ID order"
    );
    assert_eq!(
        machine
            .apply(Command::DispatchNext { scope: scope() })
            .unwrap_err(),
        Error::Busy
    );
    commit(
        &mut machine,
        Command::Cancel {
            id: "request-two".into(),
            scope: scope(),
        },
    );
    assert_eq!(dispatch(&mut machine).request_id, "request-one");
}

#[test]
fn no_effect_before_exact_commit_and_no_second_effect_on_duplicate_commit() {
    let mut machine = machine();
    enqueue(&mut machine, "request-one");
    let effects = machine
        .apply(Command::DispatchNext { scope: scope() })
        .unwrap();
    let [Effect::Persist { version, snapshot }] = effects.as_slice() else {
        panic!()
    };
    assert_eq!(snapshot.receipts["request-one"].state, State::Inspecting);
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::Queued
    );
    assert_eq!(
        machine
            .apply(Command::Persisted(Version {
                epoch: "stale".into(),
                revision: version.revision
            }))
            .unwrap_err(),
        Error::Stale
    );
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
        Error::Stale
    );
}

#[test]
fn idempotency_compares_all_payload_fields_before_touching_a_later_draft() {
    let mut machine = machine();
    enqueue(&mut machine, "request-one");
    for changed in 0..5 {
        let mut conflict = request("request-one");
        match changed {
            0 => conflict.payload.text.push(' '),
            1 => conflict.payload.scope.agent_id = Some("child".into()),
            2 => conflict.payload.scope.session_id.push('2'),
            3 => conflict.payload.target.ownership_epoch.push('2'),
            _ => conflict.payload.attachments.push(Attachment(json!({
                "id": "file",
                "digest": "digest",
            }))),
        }
        assert_eq!(
            machine
                .apply(Command::Enqueue {
                    request: conflict,
                    now_ms: 9
                })
                .unwrap_err(),
            Error::Conflict
        );
    }
    assert!(matches!(
        machine
            .apply(Command::Enqueue {
                request: request("request-one"),
                now_ms: 9
            })
            .unwrap()
            .as_slice(),
        [Effect::Replay { .. }]
    ));
}

#[test]
fn pending_enqueue_replays_only_status_until_persisted() {
    let mut machine = machine();
    let effects = machine
        .apply(Command::Enqueue {
            request: request("request-one"),
            now_ms: 1,
        })
        .unwrap();
    assert!(matches!(
        machine
            .apply(Command::Enqueue {
                request: request("request-one"),
                now_ms: 2
            })
            .unwrap()
            .as_slice(),
        [Effect::Replay { pending: true, .. }]
    ));
    let [Effect::Persist { version, .. }] = effects.as_slice() else {
        panic!()
    };
    machine.apply(Command::Persisted(version.clone())).unwrap();
}

#[test]
fn draft_conflict_blocks_local_fifo_and_stale_consent_cannot_clear_changed_text() {
    let mut machine = machine();
    enqueue(&mut machine, "request-one");
    enqueue(&mut machine, "request-two");
    let operation = dispatch(&mut machine);
    assert!(
        commit(
            &mut machine,
            Command::ComposerObserved {
                operation,
                composer: composer(ComposerState::Editing, "draft-one")
            }
        )
        .is_empty()
    );
    assert_eq!(
        machine
            .apply(Command::DispatchNext { scope: scope() })
            .unwrap_err(),
        Error::Busy
    );
    assert_eq!(
        machine
            .apply(Command::ConfirmDraft {
                id: "request-one".into(),
                scope: scope(),
                token: "wrong".into()
            })
            .unwrap_err(),
        Error::Conflict
    );
    let effects = commit(
        &mut machine,
        Command::ConfirmDraft {
            id: "request-one".into(),
            scope: scope(),
            token: "draft-one".into(),
        },
    );
    let [Effect::InspectComposer { operation, .. }] = effects.as_slice() else {
        panic!()
    };
    assert!(
        commit(
            &mut machine,
            Command::ComposerObserved {
                operation: operation.clone(),
                composer: composer(ComposerState::Editing, "draft-two")
            }
        )
        .is_empty()
    );
    let effects = commit(
        &mut machine,
        Command::ConfirmDraft {
            id: "request-one".into(),
            scope: scope(),
            token: "draft-two".into(),
        },
    );
    let [Effect::InspectComposer { operation, .. }] = effects.as_slice() else {
        panic!()
    };
    assert!(
        matches!(commit(&mut machine, Command::ComposerObserved { operation: operation.clone(), composer: composer(ComposerState::Editing, "draft-two") }).as_slice(), [Effect::PrepareInput { overwrite: true, expected_frame, .. }] if expected_frame == "draft-two")
    );
}

#[test]
fn queue_enqueue_dequeue_remove_are_not_committed_user_acceptance() {
    let mut machine = machine();
    submitted(&mut machine, "request-one");
    let proof = queue(&machine, "request-one", "queue-one", 120);
    commit(
        &mut machine,
        Command::ObserveQueue {
            id: "request-one".into(),
            evidence: proof,
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::NativeQueued
    );
    assert!(
        machine.snapshot().receipts["request-one"]
            .accepted
            .is_none()
    );
    commit(
        &mut machine,
        Command::ObserveQueue {
            id: "request-one".into(),
            evidence: QueueEvidence {
                context: context("dequeue-one", 160),
                change: QueueChange::Dequeue {
                    enqueue_record_id: "queue-one".into(),
                },
            },
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::NativeDequeued
    );
    assert!(
        machine.snapshot().receipts["request-one"]
            .accepted
            .is_none()
    );
    let mut proof = user(&machine, "request-one", "user-one", 200);
    proof.association = Association::QueueEntry("queue-one".into());
    commit(
        &mut machine,
        Command::ObserveUser {
            id: "request-one".into(),
            evidence: proof,
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::Accepted
    );
    assert!(machine.snapshot().receipts["request-one"].outcome.is_none());
    submitted(&mut machine, "request-two");
    let proof = queue(&machine, "request-two", "queue-two", 260);
    commit(
        &mut machine,
        Command::ObserveQueue {
            id: "request-two".into(),
            evidence: proof,
        },
    );
    commit(
        &mut machine,
        Command::ObserveQueue {
            id: "request-two".into(),
            evidence: QueueEvidence {
                context: context("remove-two", 300),
                change: QueueChange::Remove {
                    enqueue_record_id: "queue-two".into(),
                },
            },
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-two"].state,
        State::NativeRemoved
    );
    assert!(
        machine.snapshot().receipts["request-two"]
            .accepted
            .is_none()
    );
}

#[test]
fn native_queue_order_is_not_guessed_from_an_unidentified_dequeue() {
    let mut machine = machine();
    submitted(&mut machine, "request-one");
    submitted(&mut machine, "request-two");
    for (id, event, offset) in [
        ("request-one", "enqueue-one", 120),
        ("request-two", "enqueue-two", 160),
    ] {
        let proof = queue(&machine, id, event, offset);
        commit(
            &mut machine,
            Command::ObserveQueue {
                id: id.into(),
                evidence: proof,
            },
        );
    }
    let second = QueueEvidence {
        context: context("dequeue", 200),
        change: QueueChange::Dequeue {
            enqueue_record_id: "enqueue-two".into(),
        },
    };
    assert_eq!(
        machine
            .apply(Command::ObserveQueue {
                id: "request-two".into(),
                evidence: second.clone()
            })
            .unwrap_err(),
        Error::Busy
    );
    let unknown = QueueEvidence {
        context: context("dequeue", 200),
        change: QueueChange::Dequeue {
            enqueue_record_id: "".into(),
        },
    };
    assert_eq!(
        machine
            .apply(Command::ObserveQueue {
                id: "request-one".into(),
                evidence: unknown
            })
            .unwrap_err(),
        Error::Unproven
    );
    commit(
        &mut machine,
        Command::ObserveQueue {
            id: "request-one".into(),
            evidence: QueueEvidence {
                context: context("dequeue-first", 200),
                change: QueueChange::Dequeue {
                    enqueue_record_id: "enqueue-one".into(),
                },
            },
        },
    );
    commit(
        &mut machine,
        Command::ObserveQueue {
            id: "request-two".into(),
            evidence: second,
        },
    );
}

#[test]
fn hook_ids_and_nonmatching_text_are_rejected() {
    let mut rejecting_machine = machine();
    submitted(&mut rejecting_machine, "request-one");
    let valid = user(&rejecting_machine, "request-one", "native-user", 140);
    for index in 0..6 {
        let mut proof = valid.clone();
        match index {
            0 => proof.association = Association::QuestionToolHook("toolu-question".into()),
            1 => proof.text = "old restored draft. same  prompt".into(),
            2 => proof.text = "same prompt".into(),
            3 => proof.context.record.start = 99,
            4 => proof.context.confirmation.anchor = "replayed-old-prefix".into(),
            _ => proof.real_human_input = false,
        }
        assert_eq!(
            rejecting_machine
                .apply(Command::ObserveUser {
                    id: "request-one".into(),
                    evidence: proof
                })
                .unwrap_err(),
            Error::Unproven
        );
    }

    let mut possible_machine = machine();
    submitted(&mut possible_machine, "request-one");
    let mut proof = user(&possible_machine, "request-one", "native-user", 140);
    proof.association = Association::PossibleTextMatch;
    commit(
        &mut possible_machine,
        Command::ObserveUser {
            id: "request-one".into(),
            evidence: proof,
        },
    );
    assert_eq!(
        possible_machine.snapshot().receipts["request-one"].state,
        State::Accepted
    );
}

#[test]
fn manual_submission_after_failed_prepare_is_accepted_and_restorable() {
    let mut m = machine();
    let operation = begin_prepare(&mut m, "manual-request");
    commit(
        &mut m,
        Command::InjectionFailed {
            operation,
            reason: "prepared screen was clipped; no managed Enter".into(),
        },
    );
    let proof = UserEvidence {
        context: context("manual-user", 100),
        text: request("manual-request").payload.text,
        attachments: vec![],
        turn: Turn {
            user_uuid: "manual-user".into(),
            parent_turn_uuid: None,
        },
        real_human_input: true,
        association: Association::PossibleTextMatch,
    };
    let mut unproven = proof.clone();
    unproven.association = Association::NativeRequestId("manual-request".into());
    assert_eq!(
        m.apply(Command::ObserveUser {
            id: "manual-request".into(),
            evidence: unproven,
        })
        .unwrap_err(),
        Error::Unproven
    );
    commit(
        &mut m,
        Command::ObserveUser {
            id: "manual-request".into(),
            evidence: proof,
        },
    );
    let row = &m.snapshot().receipts["manual-request"];
    assert_eq!(row.state, State::Accepted);
    assert!(row.enter.is_none());
    let (restored, _) = Machine::restore(m.snapshot().clone(), "restarted".into()).unwrap();
    assert_eq!(
        restored.snapshot().receipts["manual-request"].state,
        State::Accepted
    );
}

#[test]
fn a_late_ack_for_another_request_cannot_consume_this_identical_prompt() {
    let mut machine = machine();
    submitted(&mut machine, "request-one");
    submitted(&mut machine, "request-two");
    let first = user(&machine, "request-one", "native-user-one", 140);
    assert_eq!(
        machine
            .apply(Command::ObserveUser {
                id: "request-two".into(),
                evidence: first.clone()
            })
            .unwrap_err(),
        Error::Unproven
    );
    commit(
        &mut machine,
        Command::ObserveUser {
            id: "request-one".into(),
            evidence: first,
        },
    );
    let duplicate_record = user(&machine, "request-two", "native-user-one", 140);
    assert_eq!(
        machine
            .apply(Command::ObserveUser {
                id: "request-two".into(),
                evidence: duplicate_record
            })
            .unwrap_err(),
        Error::Unproven
    );
}

#[test]
fn own_turn_completion_is_separate_from_parent_or_child_completion() {
    let mut machine = machine();
    submitted(&mut machine, "request-one");
    let proof = user(&machine, "request-one", "own-user", 140);
    let own_turn = proof.turn.clone();
    commit(
        &mut machine,
        Command::ObserveUser {
            id: "request-one".into(),
            evidence: proof,
        },
    );
    enqueue(&mut machine, "request-two"); // Accepted/working first turn does not prohibit queueing.
    let complete = CompletionEvidence {
        context: context("final", 220),
        turn: own_turn,
        ancestor_user_uuid: "own-user".into(),
        selected_lineage: true,
        outcome: Outcome::Succeeded,
    };
    for index in 0..4 {
        let mut wrong = complete.clone();
        match index {
            0 => wrong.turn.user_uuid = "earlier-parent-turn".into(),
            1 => wrong.ancestor_user_uuid = "child-user".into(),
            2 => wrong.context.scope.agent_id = Some("child-agent".into()),
            _ => wrong.selected_lineage = false,
        }
        assert!(
            machine
                .apply(Command::ObserveCompletion {
                    id: "request-one".into(),
                    evidence: wrong
                })
                .is_err()
        );
    }
    commit(
        &mut machine,
        Command::ObserveCompletion {
            id: "request-one".into(),
            evidence: complete,
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].outcome,
        Some(Outcome::Succeeded)
    );
    assert_eq!(
        machine.snapshot().receipts["request-two"].state,
        State::Queued
    );
}

#[test]
fn restored_input_is_not_retryable_and_late_native_user_can_correct_it() {
    let mut machine = machine();
    let enter = submitted(&mut machine, "request-one");
    commit(
        &mut machine,
        Command::ObserveReturn {
            id: "request-one".into(),
            evidence: ReturnEvidence {
                scope: scope(),
                target: target(),
                enter_operation: enter,
                witness_id: "explicit-return-observation".into(),
                restored: Some(prepared(&request("request-one"))),
            },
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::Restored
    );
    assert_eq!(
        machine
            .apply(Command::Cancel {
                id: "request-one".into(),
                scope: scope()
            })
            .unwrap_err(),
        Error::WrongState
    );
    commit(
        &mut machine,
        Command::Timeout {
            id: "request-one".into(),
            scope: scope(),
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::Restored
    );
    let proof = user(&machine, "request-one", "late-user", 140);
    commit(
        &mut machine,
        Command::ObserveUser {
            id: "request-one".into(),
            evidence: proof,
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::Accepted
    );
}

#[test]
fn stop_is_turn_specific_does_not_complete_or_interrupt_queued_followups() {
    let mut machine = machine();
    submitted(&mut machine, "request-one");
    let proof = user(&machine, "request-one", "own-user", 140);
    let turn = proof.turn.clone();
    commit(
        &mut machine,
        Command::ObserveUser {
            id: "request-one".into(),
            evidence: proof,
        },
    );
    enqueue(&mut machine, "request-two");
    assert!(
        matches!(commit(&mut machine, Command::RequestStop { id: "request-one".into(), scope: scope() }).as_slice(), [Effect::InterruptTurn { turn, .. }] if turn.user_uuid == "own-user")
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::StopRequested
    );
    assert_eq!(
        machine.snapshot().receipts["request-two"].state,
        State::Queued
    );
    commit(
        &mut machine,
        Command::ObserveCompletion {
            id: "request-one".into(),
            evidence: CompletionEvidence {
                context: context("interrupted", 200),
                turn,
                ancestor_user_uuid: "own-user".into(),
                selected_lineage: true,
                outcome: Outcome::Interrupted,
            },
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].outcome,
        Some(Outcome::Interrupted)
    );
    assert_eq!(dispatch(&mut machine).request_id, "request-two");
}

#[test]
fn cancellation_and_ui_dismissal_keep_idempotent_tombstones_without_fake_ack() {
    let mut machine = machine();
    enqueue(&mut machine, "request-one");
    commit(
        &mut machine,
        Command::Cancel {
            id: "request-one".into(),
            scope: scope(),
        },
    );
    assert!(matches!(
        machine
            .apply(Command::Enqueue {
                request: request("request-one"),
                now_ms: 9
            })
            .unwrap()
            .as_slice(),
        [Effect::Replay { .. }]
    ));
    submitted(&mut machine, "request-two");
    commit(
        &mut machine,
        Command::Dismiss {
            id: "request-two".into(),
            scope: scope(),
        },
    );
    let row = &machine.snapshot().receipts["request-two"];
    assert_eq!(row.state, State::Uncertain);
    assert!(row.accepted.is_none());
    assert!(!row.visible_in_outbox());
    assert!(matches!(
        machine
            .apply(Command::Enqueue {
                request: request("request-two"),
                now_ms: 99
            })
            .unwrap()
            .as_slice(),
        [Effect::Replay { .. }]
    ));
}

#[test]
fn recovery_preserves_queue_order_but_never_reissues_inflight_writes() {
    for step in 0..3 {
        let mut old = machine();
        let operation = match step {
            0 => {
                enqueue(&mut old, "request-one");
                dispatch(&mut old)
            }
            1 => begin_prepare(&mut old, "request-one"),
            _ => begin_enter(&mut old, "request-one"),
        };
        let serialized = serde_json::to_string(old.snapshot()).unwrap();
        let (mut recovered, effects) = Machine::restore(
            serde_json::from_str(&serialized).unwrap(),
            "process-two".into(),
        )
        .unwrap();
        let [Effect::Persist { version, .. }] = effects.as_slice() else {
            panic!()
        };
        assert!(
            recovered
                .apply(Command::Persisted(version.clone()))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            recovered
                .apply(Command::InjectionFailed {
                    operation,
                    reason: "old callback".into()
                })
                .unwrap_err(),
            Error::Stale
        );
        if step == 0 {
            assert_eq!(dispatch(&mut recovered).request_id, "request-one")
        } else {
            assert_eq!(
                recovered.snapshot().receipts["request-one"].state,
                State::Uncertain
            );
            assert_eq!(
                recovered
                    .apply(Command::DispatchNext { scope: scope() })
                    .unwrap_err(),
                Error::Missing
            );
        }
    }
}

#[test]
fn timeout_does_not_repeat_prepare_enter_or_turn_a_native_queue_into_failure() {
    let mut machine = machine();
    let operation = begin_enter(&mut machine, "request-one");
    assert!(
        commit(
            &mut machine,
            Command::Timeout {
                id: "request-one".into(),
                scope: scope()
            }
        )
        .is_empty()
    );
    assert_eq!(
        machine
            .apply(Command::InjectionFinished {
                operation,
                transport_returned: true
            })
            .unwrap_err(),
        Error::Stale
    );
    let proof = queue(&machine, "request-one", "enqueue", 120);
    commit(
        &mut machine,
        Command::ObserveQueue {
            id: "request-one".into(),
            evidence: proof,
        },
    );
    for _ in 0..3 {
        assert!(
            commit(
                &mut machine,
                Command::Timeout {
                    id: "request-one".into(),
                    scope: scope()
                }
            )
            .is_empty()
        );
    }
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::NativeQueued
    );
    assert_eq!(
        machine
            .apply(Command::Cancel {
                id: "request-one".into(),
                scope: scope()
            })
            .unwrap_err(),
        Error::WrongState
    );
}

#[test]
fn watch_advances_but_confirmation_remains_fixed_and_reset_is_rejected() {
    let mut machine = machine();
    submitted(&mut machine, "request-one");
    let operation = machine.token(&machine.snapshot().receipts["request-one"]);
    commit(
        &mut machine,
        Command::AdvanceWatch {
            operation,
            previous: cursor(100),
            next: cursor(500),
            confirmation: cursor(100),
        },
    );
    for (replay, offset) in [(false, 500), (true, 100)] {
        assert!(
            matches!(machine.apply(Command::InspectNative { id: "request-one".into(), scope: scope(), replay }).unwrap().as_slice(), [Effect::InspectNative { from, .. }] if from.offset == offset)
        );
    }
    let operation = machine.token(&machine.snapshot().receipts["request-one"]);
    assert_eq!(
        machine
            .apply(Command::AdvanceWatch {
                operation,
                previous: cursor(500),
                next: cursor(10),
                confirmation: cursor(100)
            })
            .unwrap_err(),
        Error::Unproven
    );
}

#[test]
fn pending_queue_has_no_arbitrary_count_limit_and_corrupt_recovery_still_fails() {
    let mut machine = machine();
    for index in 0..70 {
        enqueue(&mut machine, &format!("request-{index:03}"));
    }
    let before = machine.snapshot().clone();
    enqueue(&mut machine, "request-overflow");
    assert_eq!(machine.snapshot().receipts.len(), 71);
    let mut corrupt = before;
    corrupt.receipts.values_mut().next().unwrap().attempted = true;
    assert!(Machine::restore(corrupt, "process-two".into()).is_err());
    assert!(Machine::restore(machine.snapshot().clone(), "process-one".into()).is_err());
}

#[test]
fn receipt_history_beyond_old_count_and_bytes_accepts_host_sized_text_and_replay() {
    let mut original = machine();
    enqueue(&mut original, "request-seed");
    let mut snapshot = original.snapshot().clone();
    let seed = snapshot.receipts.remove("request-seed").unwrap();
    for index in 0..4096 {
        let mut row = seed.clone();
        row.request.id = format!("request-{index:04}");
        row.request.payload.text = "x".repeat(2050);
        row.sequence = index + 1;
        snapshot.receipts.insert(row.request.id.clone(), row);
    }
    snapshot.next_sequence = 4097;
    let (mut restored, effects) = Machine::restore(snapshot, "process-two".into()).unwrap();
    let Effect::Persist { version, .. } = &effects[0] else {
        panic!()
    };
    restored.apply(Command::Persisted(version.clone())).unwrap();
    for id in ["request-large-one", "request-large-two"] {
        let mut large = request(id);
        large.payload.text = "x".repeat(1024 * 1024);
        commit(
            &mut restored,
            Command::Enqueue {
                request: large.clone(),
                now_ms: 2,
            },
        );
        let replay = restored
            .apply(Command::Enqueue {
                request: large,
                now_ms: 3,
            })
            .unwrap();
        assert!(matches!(
            replay.as_slice(),
            [Effect::Replay { pending: false, .. }]
        ));
    }
    assert_eq!(restored.snapshot().receipts.len(), 4098);
    let mut oversized = request("request-too-large");
    oversized.payload.text = "x".repeat(1024 * 1024 + 1);
    assert!(
        restored
            .apply(Command::Enqueue {
                request: oversized,
                now_ms: 4
            })
            .is_err()
    );
}

#[test]
fn causal_text_native_enqueue_is_accepted() {
    let mut machine = machine();
    submitted(&mut machine, "request-one");
    let weak = QueueEvidence {
        context: context("enqueue", 120),
        change: QueueChange::Enqueue {
            text: "same  prompt".into(),
            attachments: vec![],
            association: Association::PossibleTextMatch,
        },
    };
    commit(
        &mut machine,
        Command::ObserveQueue {
            id: "request-one".into(),
            evidence: weak,
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::NativeQueued
    );
    let mut wrong = user(&machine, "request-one", "user", 140);
    wrong.attachments.push(Attachment(json!({
        "id": "foreign-file",
        "digest": "foreign-digest",
    })));
    assert_eq!(
        machine
            .apply(Command::ObserveUser {
                id: "request-one".into(),
                evidence: wrong
            })
            .unwrap_err(),
        Error::Unproven
    );
}

#[test]
fn dismissal_during_enter_does_not_invalidate_its_one_shot_callback() {
    let mut machine = machine();
    let operation = begin_enter(&mut machine, "request-one");
    commit(
        &mut machine,
        Command::Dismiss {
            id: "request-one".into(),
            scope: scope(),
        },
    );
    commit(
        &mut machine,
        Command::InjectionFinished {
            operation,
            transport_returned: true,
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::Uncertain
    );
    assert!(machine.snapshot().receipts["request-one"].dismissed);
    let proof = user(&machine, "request-one", "user", 140);
    commit(
        &mut machine,
        Command::ObserveUser {
            id: "request-one".into(),
            evidence: proof,
        },
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        State::Accepted
    );
    assert!(machine.snapshot().receipts["request-one"].dismissed);
}

#[test]
fn native_queue_and_stop_request_survive_restart_without_dispatching_any_write() {
    for stop in [false, true] {
        let mut old = machine();
        submitted(&mut old, "request-one");
        let expected = if stop {
            let proof = user(&old, "request-one", "user", 140);
            commit(
                &mut old,
                Command::ObserveUser {
                    id: "request-one".into(),
                    evidence: proof,
                },
            );
            commit(
                &mut old,
                Command::RequestStop {
                    id: "request-one".into(),
                    scope: scope(),
                },
            );
            State::StopRequested
        } else {
            let proof = queue(&old, "request-one", "queue", 120);
            commit(
                &mut old,
                Command::ObserveQueue {
                    id: "request-one".into(),
                    evidence: proof,
                },
            );
            State::NativeQueued
        };
        let (mut recovered, effects) =
            Machine::restore(old.snapshot().clone(), "process-two".into()).unwrap();
        let [Effect::Persist { version, .. }] = effects.as_slice() else {
            panic!()
        };
        assert!(
            recovered
                .apply(Command::Persisted(version.clone()))
                .unwrap()
                .is_empty()
        );
        assert_eq!(recovered.snapshot().receipts["request-one"].state, expected);
        assert_eq!(
            recovered
                .apply(Command::DispatchNext { scope: scope() })
                .unwrap_err(),
            Error::Missing
        );
        if stop {
            assert!(matches!(
                recovered
                    .apply(Command::RequestStop {
                        id: "request-one".into(),
                        scope: scope()
                    })
                    .unwrap()
                    .as_slice(),
                [Effect::Replay { .. }]
            ));
        }
    }
}

#[test]
fn restoration_witness_survives_serde_and_cannot_be_fabricated_by_state_only() {
    let mut old = machine();
    let operation = submitted(&mut old, "request-one");
    commit(
        &mut old,
        Command::ObserveReturn {
            id: "request-one".into(),
            evidence: ReturnEvidence {
                scope: scope(),
                target: target(),
                enter_operation: operation,
                witness_id: "linked-escape".into(),
                restored: None,
            },
        },
    );
    let mut corrupt = old.snapshot().clone();
    corrupt.receipts.get_mut("request-one").unwrap().returned = None;
    assert!(Machine::restore(corrupt, "process-two".into()).is_err());
    let snapshot = serde_json::from_str(&serde_json::to_string(old.snapshot()).unwrap()).unwrap();
    assert!(Machine::restore(snapshot, "process-two".into()).is_ok());
}
