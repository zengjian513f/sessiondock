use super::*;
use crate::{
    delivery::claude::{Operation, Payload, Request, State, Target},
    sessions::{NativeScope, NativeUserInput},
};

fn scope() -> Scope {
    Scope {
        uid: "claude:0123456789abcdef".into(),
        session_id: "0d3c5a8e-4b7f-4c21-9a6d-2e1f3b4c5d6e".into(),
        agent_id: None,
    }
}

fn cursor(offset: u64) -> Cursor {
    Cursor {
        source_identity: "view-identity".into(),
        offset,
        head: format!("rs-m2-1:{offset:016x}"),
        anchor: format!("anchor-{offset}"),
    }
}

fn receipt(text: &str, confirmation: u64) -> Receipt {
    Receipt {
        request: Request {
            id: "request-0001".into(),
            payload: Payload {
                scope: scope(),
                target: Target {
                    host_instance: "synthetic-instance-0001abcd".into(),
                    terminal_id: "sessiondock-claude-0d3c5a8e".into(),
                    ownership_epoch: "guarded_v1".into(),
                },
                text: text.into(),
                attachments: Vec::new(),
            },
        },
        sequence: 1,
        created_ms: 1,
        revision: 3,
        state: State::Uncertain,
        attempted: true,
        draft_token: None,
        approved_draft: None,
        confirmation: Some(cursor(confirmation)),
        watch: Some(cursor(confirmation)),
        enter: Some(Operation {
            epoch: "epoch".into(),
            request_id: "request-0001".into(),
            revision: 2,
        }),
        native_queue: None,
        accepted: None,
        returned: None,
        outcome: None,
        completed_record: None,
        dismissed: false,
        issue: None,
    }
}

fn input(uuid: &str, start: u64, end: u64, text: &str) -> NativeUserInput {
    NativeUserInput {
        uuid: uuid.into(),
        start,
        end,
        text: text.into(),
        ts: "2026-09-12T10:00:00Z".into(),
    }
}

fn read(fence_valid: bool, current: u64, inputs: Vec<NativeUserInput>) -> NativeInputRead {
    NativeInputRead {
        scope: NativeScope {
            source: "claude".into(),
            uid: scope().uid,
            session_id: scope().session_id,
            agent_id: None,
        },
        current: fence_of(&cursor(current)),
        fence_valid,
        inputs,
    }
}

#[test]
fn exact_end_trimmed_text_after_the_fence_is_accepted_with_verified_enter() {
    let row = receipt("  hello  world \n", 100);
    let observed = read(true, 260, vec![input("uuid-a", 100, 260, "hello  world")]);
    let Observation::Accepted(evidence) = observe(&row, &scope(), &cursor(100), &observed) else {
        panic!("expected acceptance");
    };
    assert_eq!(evidence.turn.user_uuid, "uuid-a");
    assert_eq!(evidence.turn.parent_turn_uuid, None);
    assert_eq!(evidence.context.record.start, 100);
    assert_eq!(evidence.context.record.end, 260);
    assert_eq!(evidence.context.confirmation, cursor(100));
    assert_eq!(evidence.context.record.source_identity, "view-identity");
    assert!(evidence.real_human_input);
    assert_eq!(
        evidence.association,
        Association::VerifiedEnter(row.enter.clone().unwrap())
    );
}

#[test]
fn internal_whitespace_is_significant_and_first_match_in_byte_order_wins() {
    let row = receipt("a b", 0);
    let observed = read(
        true,
        400,
        vec![
            input("uuid-1", 0, 100, "a  b"),
            input("uuid-2", 100, 200, "a b"),
            input("uuid-3", 200, 300, "a b"),
        ],
    );
    let Observation::Accepted(evidence) = observe(&row, &scope(), &cursor(0), &observed) else {
        panic!("expected acceptance");
    };
    assert_eq!(evidence.turn.user_uuid, "uuid-2");
}

#[test]
fn records_before_the_fence_or_without_identity_never_acknowledge() {
    let row = receipt("hello", 300);
    let observed = read(
        true,
        500,
        vec![
            input("uuid-old", 100, 200, "hello"),
            input("", 300, 400, "hello"),
            input("uuid-empty", 400, 400, "hello"),
        ],
    );
    assert_eq!(
        observe(&row, &scope(), &cursor(300), &observed),
        Observation::Nothing {
            next: Some(cursor(500))
        }
    );
    // Reading from an advanced watch cursor never accepts a record that the
    // watch has already passed.
    let observed = read(true, 500, vec![input("uuid-a", 300, 400, "hello")]);
    assert_eq!(
        observe(&row, &scope(), &cursor(400), &observed),
        Observation::Nothing {
            next: Some(cursor(500))
        }
    );
}

#[test]
fn invalid_fence_or_changed_identity_is_reported_not_guessed() {
    let row = receipt("hello", 100);
    let observed = read(false, 500, vec![input("uuid-a", 100, 200, "hello")]);
    assert_eq!(
        observe(&row, &scope(), &cursor(100), &observed),
        Observation::FenceInvalid
    );
    let mut foreign = read(true, 500, vec![input("uuid-a", 100, 200, "hello")]);
    foreign.current.source_identity = "another-view".into();
    assert_eq!(
        observe(&row, &scope(), &cursor(100), &foreign),
        Observation::FenceInvalid
    );
}

#[test]
fn nothing_new_keeps_the_watch_and_missing_enter_never_matches() {
    let row = receipt("hello", 100);
    assert_eq!(
        observe(&row, &scope(), &cursor(100), &read(true, 100, Vec::new())),
        Observation::Nothing { next: None }
    );
    let mut unwritten = receipt("hello", 100);
    unwritten.enter = None;
    let observed = read(true, 300, vec![input("uuid-a", 100, 300, "hello")]);
    assert_eq!(
        observe(&unwritten, &scope(), &cursor(100), &observed),
        Observation::Nothing { next: None }
    );
}

#[test]
fn cursor_and_fence_round_trip() {
    let fence = fence_of(&cursor(42));
    assert_eq!(fence.offset, 42);
    assert_eq!(cursor_of(&fence), cursor(42));
    assert_eq!(prompt_key("  x y \n"), "x y");
}
