//! Synthetic Codex rollouts in private temporary directories, read through the
//! real `SessionStore` inventory (CheckedNative + RawIndex + projection).
//! No CLI is started and no native home is touched.
use super::*;
use crate::delivery::codex::{
    Command, DraftObservation, DraftState, Effect, EnterResult, Error as MachineError, Machine,
    Payload, PreparedEvidence, Request, Target,
};
use crate::sessions::{SessionRoots, SessionStore, ViewSnapshot};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

const DELIVERED: &str = "2026-09-12T10:00:00.000Z";
const BEFORE: &str = "2026-09-12T09:59:59.999Z";
const AFTER: &str = "2026-09-12T10:00:00.001Z";

fn ms(text: &str) -> u64 {
    parse_rfc3339_ms(text).unwrap()
}

fn row(ts: Option<&str>, kind: &str, payload: Value) -> Value {
    let mut row = json!({"type": kind, "payload": payload});
    if let Some(ts) = ts {
        row["timestamp"] = json!(ts);
    }
    row
}

fn meta(id: &str) -> Value {
    row(
        Some("2026-09-12T09:00:00.000Z"),
        "session_meta",
        json!({"id": id, "timestamp": "2026-09-12T09:00:00.000Z", "cwd": "/synthetic"}),
    )
}

fn user(ts: Option<&str>, text: &str, turn: Option<&str>) -> Value {
    let mut payload = json!({"type": "message", "role": "user",
        "content": [{"type": "input_text", "text": text}]});
    if let Some(turn) = turn {
        payload["turn_id"] = json!(turn);
    }
    row(ts, "response_item", payload)
}

fn event(ts: Option<&str>, kind: &str, extra: Value) -> Value {
    let mut payload = json!({"type": kind});
    if let Some(fields) = extra.as_object() {
        for (key, value) in fields {
            payload[key] = value.clone();
        }
    }
    row(ts, "event_msg", payload)
}

fn encoded(rows: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for row in rows {
        writeln!(bytes, "{row}").unwrap();
    }
    bytes
}

fn write_rows(path: &Path, rows: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, encoded(rows)).unwrap();
}

fn append(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
}

struct Fixture {
    _temp: TempDir,
    root: std::path::PathBuf,
    path: std::path::PathBuf,
    store: SessionStore,
    uid: String,
}

impl Fixture {
    fn new(rows: &[Value]) -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("codex");
        let path = root.join("sessions").join("rollout-synthetic.jsonl");
        write_rows(&path, rows);
        let store = SessionStore::new(SessionRoots {
            codex: Some(root.clone()),
            ..Default::default()
        });
        let uid = uid_of(&store, &path);
        Self {
            _temp: temp,
            root,
            path,
            store,
            uid,
        }
    }

    /// Force the shared 500 ms inventory TTL like a real poll would after
    /// the TTL elapsed, then take the immutable main-session view.
    fn fresh(&self) -> Arc<ViewSnapshot> {
        self.store.list(true).unwrap();
        self.store.snapshot(&self.uid, "").unwrap()
    }

    fn fresh_uid(&self, uid: &str) -> Arc<ViewSnapshot> {
        self.store.list(true).unwrap();
        self.store.snapshot(uid, "").unwrap()
    }

    fn boundary(&self) -> Boundary {
        Boundary::capture(&self.fresh().native_checkpoint(), 7, ms(DELIVERED))
    }

    fn delivered(&self, text: &str) -> Delivered {
        Delivered {
            uid: self.uid.clone(),
            text: text.into(),
            media: Vec::new(),
        }
    }

    fn observe(&self, boundary: &Boundary, text: &str) -> Observation {
        observe(&self.fresh(), boundary, &self.delivered(text))
    }
}

fn uid_of(store: &SessionStore, path: &Path) -> String {
    let list = store.list(true).unwrap();
    list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["path"] == path.to_string_lossy().as_ref())
        .unwrap()["uid"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn possible(observation: &Observation) -> &PossibleMatch {
    match &observation.outcome {
        Outcome::Possible(found) => found,
        other => panic!("expected a possible match: {other:?}"),
    }
}

fn turn(turn: &str, text: &str, ts: &str) -> Vec<Value> {
    vec![
        row(Some(ts), "turn_context", json!({"model": "synthetic"})),
        event(Some(ts), "task_started", json!({"turn_id": turn})),
        user(Some(ts), text, Some(turn)),
        event(Some(ts), "user_message", json!({"message": text})),
    ]
}

#[test]
fn exact_match_after_boundary_is_a_possible_text_match_with_physical_range() {
    let fixture = Fixture::new(&[meta("sid-one"), user(Some(BEFORE), "hello", Some("t0"))]);
    let boundary = fixture.boundary();
    assert_eq!(
        boundary.confirmation.position,
        fs::metadata(&fixture.path).unwrap().len()
    );
    assert_eq!(boundary.sequence, 7);

    let mut rows = turn("t1", "hello", AFTER);
    rows.push(event(
        Some(AFTER),
        "task_complete",
        json!({"turn_id": "t1", "duration_ms": 5}),
    ));
    append(&fixture.path, &encoded(&rows));
    let record_start = boundary.confirmation.position + encoded(&rows[..2]).len() as u64;
    let record_end = record_start + encoded(&rows[2..3]).len() as u64;
    let complete_end = boundary.confirmation.position + encoded(&rows).len() as u64;

    let observation = fixture.observe(&boundary, "hello");
    assert_eq!(observation.sequence, 7);
    let found = possible(&observation);
    assert_eq!(found.evidence.correlation, Correlation::PossibleTextMatch);
    assert!(found.evidence.real_user_input);
    assert_eq!(found.evidence.uid, fixture.uid);
    assert_eq!(found.evidence.text, "hello");
    assert!(found.evidence.observed_media.is_empty());
    assert_eq!(found.evidence.validated_confirmation, boundary.confirmation);
    assert_eq!(
        found.evidence.record,
        NativeAcceptance {
            source_identity: boundary.confirmation.source_identity.clone(),
            record_id: format!("codex-line:{record_start}-{record_end}"),
            start: record_start,
            end: record_end,
            turn_id: "t1".into(),
        }
    );
    assert_eq!(
        found.record,
        MatchedRecord {
            start: record_start,
            end: record_end,
            turn_id: Some("t1".into()),
            recorded_ms: Some(ms(AFTER)),
            timestamp: TimestampCheck::Verified,
        }
    );
    let completion = found.completion.as_ref().unwrap();
    assert_eq!(completion.turn_id, "t1");
    assert_eq!(completion.outcome, Completion::Succeeded);
    assert_eq!(completion.record_end, complete_end);
    assert_eq!(completion.validated_confirmation, boundary.confirmation);
    let watch = observation.watch.as_ref().unwrap();
    assert_eq!(watch.position, complete_end);
    assert_eq!(watch.source_identity, boundary.confirmation.source_identity);
    assert_ne!(watch.anchor, boundary.confirmation.anchor);
}

#[test]
fn the_machine_still_refuses_a_possible_text_match() {
    let fixture = Fixture::new(&[meta("sid-one")]);
    let boundary = fixture.boundary();
    append(&fixture.path, &encoded(&turn("t1", "echo  one", AFTER)));
    let observation = fixture.observe(&boundary, "  echo  one\n");
    let evidence = possible(&observation).evidence.clone();

    let target = Target {
        host_instance: "host-one".into(),
        session_id: "terminal-one".into(),
        ownership_epoch: "lease-one".into(),
    };
    let request = Request {
        request_id: "request-one".into(),
        payload: Payload {
            uid: fixture.uid.clone(),
            target: target.clone(),
            text: "  echo  one\n".into(),
            media: Vec::new(),
        },
    };
    let mut machine = Machine::new("process-one".into()).unwrap();
    let persist = |machine: &mut Machine, command: Command| {
        let effects = machine.apply(command).unwrap();
        let [Effect::Persist { version, .. }] = effects.as_slice() else {
            panic!("expected a durable proposal: {effects:?}")
        };
        machine.apply(Command::Persisted(version.clone())).unwrap()
    };
    let effects = persist(
        &mut machine,
        Command::Submit {
            request,
            now_ms: ms(DELIVERED),
        },
    );
    let [Effect::InspectComposer { operation, .. }] = effects.as_slice() else {
        panic!("expected inspection")
    };
    let effects = persist(
        &mut machine,
        Command::DraftObserved {
            operation: operation.clone(),
            observation: DraftObservation {
                target: target.clone(),
                state: DraftState::Empty,
                token: "empty-frame".into(),
                native_cursor: boundary.confirmation.clone(),
            },
        },
    );
    let [Effect::InjectPrepare { operation, .. }] = effects.as_slice() else {
        panic!("expected prepare")
    };
    let effects = persist(
        &mut machine,
        Command::Prepared {
            operation: operation.clone(),
            evidence: PreparedEvidence {
                target,
                observed_text: "  echo  one\n".into(),
                observed_media: Vec::new(),
                frame_token: "prepared-frame".into(),
            },
        },
    );
    let [Effect::InjectEnter { operation, .. }] = effects.as_slice() else {
        panic!("expected Enter")
    };
    assert!(
        persist(
            &mut machine,
            Command::EnterFinished {
                operation: operation.clone(),
                result: EnterResult::TransportReturned,
            },
        )
        .is_empty()
    );
    // Same UID, same fixed confirmation cursor, real user input, trimmed text
    // equal, record after the boundary: everything but the correlation holds,
    // and the domain still refuses to acknowledge from text alone.
    assert_eq!(
        machine
            .apply(Command::NativeAck {
                request_id: "request-one".into(),
                evidence,
            })
            .unwrap_err(),
        MachineError::UnprovenAcknowledgment
    );
    assert_eq!(
        machine.snapshot().receipts["request-one"].state,
        crate::delivery::codex::State::Uncertain
    );
}

#[test]
fn only_the_ends_are_trimmed_internal_whitespace_stays_significant() {
    let fixture = Fixture::new(&[meta("sid-one")]);
    let boundary = fixture.boundary();
    append(
        &fixture.path,
        &encoded(&[user(Some(AFTER), "echo one", None)]),
    );
    assert_eq!(
        fixture.observe(&boundary, "  echo  one\n").outcome,
        Outcome::Absent(Absence { skipped_earlier: 0 })
    );
    append(
        &fixture.path,
        &encoded(&[user(Some(AFTER), "echo  one", None)]),
    );
    let observation = fixture.observe(&boundary, "  echo  one\n");
    let found = possible(&observation);
    assert_eq!(found.evidence.text, "echo  one");
    assert_eq!(found.record.turn_id, None);
    assert_eq!(found.evidence.record.turn_id, "");
    assert_eq!(found.completion, None);
}

#[test]
fn utf8_multiline_text_matches_exactly() {
    let fixture = Fixture::new(&[meta("sid-one")]);
    let boundary = fixture.boundary();
    append(
        &fixture.path,
        &encoded(&[user(Some(AFTER), "第一行\n第二行 🚀", Some("t1"))]),
    );
    assert!(matches!(
        fixture
            .observe(&boundary, " 第一行\n  第二行 🚀 \n")
            .outcome,
        Outcome::Absent(_)
    ));
    append(
        &fixture.path,
        &encoded(&[user(Some(AFTER), "第一行\n  第二行 🚀", Some("t2"))]),
    );
    let observation = fixture.observe(&boundary, " 第一行\n  第二行 🚀 \n");
    let found = possible(&observation);
    assert_eq!(found.evidence.text, "第一行\n  第二行 🚀");
    assert_eq!(found.record.turn_id.as_deref(), Some("t2"));
}

#[test]
fn an_earlier_identical_input_after_the_boundary_makes_the_match_ambiguous() {
    let fixture = Fixture::new(&[meta("sid-one")]);
    let boundary = fixture.boundary();
    // A human typed the same text into the TUI just before our Enter; its
    // record lands after the boundary with an earlier timestamp.
    append(
        &fixture.path,
        &encoded(&[user(Some(BEFORE), "same text", Some("t1"))]),
    );
    assert_eq!(
        fixture.observe(&boundary, "same text").outcome,
        Outcome::Absent(Absence { skipped_earlier: 1 })
    );
    append(
        &fixture.path,
        &encoded(&[user(Some(AFTER), "same text", Some("t2"))]),
    );
    let observation = fixture.observe(&boundary, "same text");
    assert_eq!(
        observation.outcome,
        Outcome::Uncertain(Uncertainty::Ambiguous { candidates: 2 })
    );
    assert!(observation.watch.is_some());
    // Two later identical inputs are just as ambiguous.
    let fixture = Fixture::new(&[meta("sid-two")]);
    let boundary = fixture.boundary();
    append(
        &fixture.path,
        &encoded(&[
            user(Some(AFTER), "same text", Some("t1")),
            user(Some(AFTER), "same text", Some("t2")),
        ]),
    );
    assert_eq!(
        fixture.observe(&boundary, "same text").outcome,
        Outcome::Uncertain(Uncertainty::Ambiguous { candidates: 2 })
    );
}

#[test]
fn records_at_or_before_the_boundary_and_unfinished_tails_are_ignored() {
    let fixture = Fixture::new(&[meta("sid-one"), user(Some(AFTER), "hello", Some("t0"))]);
    let boundary = fixture.boundary();
    let observation = fixture.observe(&boundary, "hello");
    assert_eq!(
        observation.outcome,
        Outcome::Absent(Absence { skipped_earlier: 0 })
    );
    assert_eq!(observation.watch.as_ref().unwrap(), &boundary.confirmation);

    let mut partial = encoded(&[user(Some(AFTER), "hello", Some("t1"))]);
    partial.pop();
    append(&fixture.path, &partial);
    let observation = fixture.observe(&boundary, "hello");
    assert!(matches!(observation.outcome, Outcome::Absent(_)));
    assert_eq!(observation.watch.as_ref().unwrap(), &boundary.confirmation);
    append(&fixture.path, b"\n");
    let observation = fixture.observe(&boundary, "hello");
    assert_eq!(
        possible(&observation).record.start,
        boundary.confirmation.position
    );
}

#[test]
fn rewrite_or_truncation_below_the_boundary_is_a_checkpoint_mismatch() {
    let rows = [
        meta("sid-one"),
        user(Some(BEFORE), "first", Some("t0")),
        user(Some(BEFORE), "second", Some("t1")),
    ];
    let fixture = Fixture::new(&rows);
    let boundary = fixture.boundary();
    append(
        &fixture.path,
        &encoded(&[user(Some(AFTER), "hello", Some("t2"))]),
    );
    assert!(matches!(
        fixture.observe(&boundary, "hello").outcome,
        Outcome::Possible(_)
    ));

    // Rewrite an earlier record (different length) and keep the tail.
    let mut rewritten = vec![meta("sid-one"), user(Some(BEFORE), "first!!", Some("t0"))];
    rewritten.extend_from_slice(&rows[2..]);
    rewritten.push(user(Some(AFTER), "hello", Some("t2")));
    write_rows(&fixture.path, &rewritten);
    let observation = fixture.observe(&boundary, "hello");
    assert_eq!(
        observation.outcome,
        Outcome::Uncertain(Uncertainty::CheckpointMismatch)
    );
    assert_eq!(observation.watch, None);

    // Truncate below the boundary: the position is no longer a checkpoint.
    write_rows(&fixture.path, &rows[..2]);
    assert_eq!(
        fixture.observe(&boundary, "hello").outcome,
        Outcome::Uncertain(Uncertainty::CheckpointMismatch)
    );
    // Restore the exact original prefix: the boundary is valid again.
    write_rows(&fixture.path, &rows);
    append(
        &fixture.path,
        &encoded(&[user(Some(AFTER), "hello", Some("t2"))]),
    );
    assert!(matches!(
        fixture.observe(&boundary, "hello").outcome,
        Outcome::Possible(_)
    ));
}

#[test]
fn absent_or_invalid_timestamps_pass_and_earlier_ones_are_skipped() {
    let fixture = Fixture::new(&[meta("sid-one")]);
    let boundary = fixture.boundary();
    append(
        &fixture.path,
        &encoded(&[user(Some("not-a-date"), "invalid", Some("t1"))]),
    );
    let found = possible(&fixture.observe(&boundary, "invalid"))
        .record
        .clone();
    assert_eq!(found.timestamp, TimestampCheck::Absent);
    assert_eq!(found.recorded_ms, None);

    append(
        &fixture.path,
        &encoded(&[user(None, "missing", Some("t2"))]),
    );
    assert_eq!(
        possible(&fixture.observe(&boundary, "missing"))
            .record
            .timestamp,
        TimestampCheck::Absent
    );

    append(
        &fixture.path,
        &encoded(&[user(Some(BEFORE), "earlier", Some("t3"))]),
    );
    assert_eq!(
        fixture.observe(&boundary, "earlier").outcome,
        Outcome::Absent(Absence { skipped_earlier: 1 })
    );

    // Numeric epoch milliseconds are normalized by the projection too.
    let numeric = json!({"timestamp": ms(AFTER), "type": "response_item",
        "payload": {"type": "message", "role": "user", "turn_id": "t4",
                    "content": [{"type": "input_text", "text": "numeric"}]}});
    append(&fixture.path, &encoded(&[numeric]));
    let found = possible(&fixture.observe(&boundary, "numeric"))
        .record
        .clone();
    assert_eq!(found.timestamp, TimestampCheck::Verified);
    assert_eq!(found.recorded_ms, Some(ms(AFTER)));
    // Exactly the delivery instant is not earlier.
    append(
        &fixture.path,
        &encoded(&[user(Some(DELIVERED), "exact", Some("t5"))]),
    );
    assert_eq!(
        possible(&fixture.observe(&boundary, "exact"))
            .record
            .recorded_ms,
        Some(ms(DELIVERED))
    );
}

#[test]
fn inherited_fork_prefix_is_never_evidence() {
    let parent_rows = [
        meta("codex-parent"),
        user(Some(AFTER), "shared text", Some("p1")),
    ];
    let fixture = Fixture::new(&parent_rows);
    let cut = encoded(&parent_rows).len();
    let child = fixture.root.join("sessions").join("rollout-child.jsonl");
    write_rows(
        &child,
        &[row(
            Some("2026-09-12T09:30:00.000Z"),
            "session_meta",
            json!({"id": "codex-child", "forked_from_id": "codex-parent",
                   "history_mode": "paginated",
                   "history_base": {"thread_id": "codex-parent", "end_byte_offset": cut},
                   "timestamp": "2026-09-12T09:30:00.000Z", "cwd": "/synthetic"}),
        )],
    );
    let child_uid = uid_of(&fixture.store, &child);
    let snapshot = fixture.fresh_uid(&child_uid);
    // The inherited parent input is visible history for the child ...
    let history = snapshot
        .messages(&crate::sessions::MessageQuery::default())
        .unwrap();
    assert_eq!(history["messages"][0]["text"], "shared text");
    // ... but it is not a record after the child's own boundary.
    let boundary = Boundary::capture(&snapshot.native_checkpoint(), 1, ms(DELIVERED));
    let delivered = Delivered {
        uid: child_uid.clone(),
        text: "shared text".into(),
        media: Vec::new(),
    };
    assert_eq!(
        observe(&fixture.fresh_uid(&child_uid), &boundary, &delivered).outcome,
        Outcome::Absent(Absence { skipped_earlier: 0 })
    );
    append(
        &child,
        &encoded(&[user(Some(AFTER), "shared text", Some("c1"))]),
    );
    let observation = observe(&fixture.fresh_uid(&child_uid), &boundary, &delivered);
    let found = possible(&observation);
    assert_eq!(found.record.start, boundary.confirmation.position);
    assert_eq!(found.record.turn_id.as_deref(), Some("c1"));
    assert_eq!(
        found.evidence.record.source_identity,
        boundary.confirmation.source_identity
    );
}

#[test]
fn protocol_injections_telemetry_and_status_are_not_user_input() {
    let fixture = Fixture::new(&[meta("sid-one")]);
    let boundary = fixture.boundary();
    let text = "run the tests";
    append(
        &fixture.path,
        &encoded(&[
            row(
                Some(AFTER),
                "response_item",
                json!({"type": "message", "role": "developer",
                "content": [{"type": "input_text", "text": text}]}),
            ),
            row(
                Some(AFTER),
                "response_item",
                json!({"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": text}],
                "internal_chat_message_metadata_passthrough": {"content_item_kinds": ["goal.internal_context"]}}),
            ),
            user(
                Some(AFTER),
                &format!("<user_instructions>\n{text}\n</user_instructions>"),
                Some("t1"),
            ),
            event(Some(AFTER), "user_message", json!({"message": text})),
            event(Some(AFTER), "task_started", json!({"turn_id": "t1"})),
            row(
                Some(AFTER),
                "response_item",
                json!({"type": "message", "role": "assistant",
                "phase": "final_answer", "content": [{"type": "output_text", "text": text}]}),
            ),
        ]),
    );
    assert_eq!(
        fixture.observe(&boundary, text).outcome,
        Outcome::Absent(Absence { skipped_earlier: 0 })
    );
    // An aborted-turn prefix is stripped exactly like the history view.
    let abort = "<turn_aborted>\nTurn interrupted by user\n</turn_aborted>\n";
    append(
        &fixture.path,
        &encoded(&[
            user(Some(AFTER), &format!("{abort}{text}"), Some("t2")),
            event(
                Some(AFTER),
                "turn_aborted",
                json!({"turn_id": "t2", "reason": "user"}),
            ),
        ]),
    );
    let observation = fixture.observe(&boundary, text);
    let found = possible(&observation);
    assert_eq!(found.evidence.text, text);
    assert_eq!(
        found.completion.as_ref().map(|c| c.outcome),
        Some(Completion::Stopped)
    );
}

#[test]
fn foreign_scope_media_and_blank_text_stay_uncertain() {
    let fixture = Fixture::new(&[meta("sid-one")]);
    let boundary = fixture.boundary();
    append(
        &fixture.path,
        &encoded(&[user(Some(AFTER), "hello", Some("t1"))]),
    );
    let snapshot = fixture.fresh();

    let mut other_uid = fixture.delivered("hello");
    other_uid.uid = "codex:0000000000000000".into();
    let observation = observe(&snapshot, &boundary, &other_uid);
    assert_eq!(
        observation.outcome,
        Outcome::Uncertain(Uncertainty::ForeignScope)
    );
    assert_eq!(observation.watch, None);

    let mut foreign = boundary.clone();
    foreign.confirmation.source_identity = "another-rollout".into();
    assert_eq!(
        observe(&snapshot, &foreign, &fixture.delivered("hello")).outcome,
        Outcome::Uncertain(Uncertainty::ForeignScope)
    );

    let mut media = fixture.delivered("hello");
    media.media.push(MediaRef {
        id: "image-one".into(),
        content_digest: "sha256:0".into(),
    });
    assert_eq!(
        observe(&snapshot, &boundary, &media).outcome,
        Outcome::Uncertain(Uncertainty::MediaUnsupported)
    );
    assert_eq!(
        observe(&snapshot, &boundary, &fixture.delivered(" \n\t")).outcome,
        Outcome::Uncertain(Uncertainty::UnmatchableText)
    );
    // Over-long cursor fields are a sanitized read failure, not a match.
    let mut long = boundary.clone();
    long.confirmation.anchor = "a".repeat(513);
    assert_eq!(
        observe(&snapshot, &long, &fixture.delivered("hello")).outcome,
        Outcome::Uncertain(Uncertainty::Unreadable { status: 400 })
    );
}

#[test]
fn replay_policy_keeps_python_constants_window_and_rate_limit() {
    assert_eq!(CONFIRM_TIMEOUT_MS, 8_000);
    assert_eq!(REPLAY_INTERVAL_MS, 8_000);
    assert_eq!(TRACK_WINDOW_MS, 3_600_000);
    assert_eq!(POLL_INTERVAL_MS, 500);
    let policy = ReplayPolicy::default();
    let delivered = 1_000_000;
    let mut clock = ReplayClock::default();
    assert_eq!(clock.plan(&policy, delivered, delivered), ReadPlan::Watch);
    assert_eq!(
        clock.plan(&policy, delivered, delivered + 7_999),
        ReadPlan::Watch
    );
    assert_eq!(clock.last_replay_ms(), None);
    // First overdue tick replays from the fixed boundary at once ...
    assert_eq!(
        clock.plan(&policy, delivered, delivered + 8_000),
        ReadPlan::Replay
    );
    assert_eq!(clock.last_replay_ms(), Some(delivered + 8_000));
    // ... then only cheap tail reads until eight more seconds pass.
    for tick in 1..16 {
        assert_eq!(
            clock.plan(&policy, delivered, delivered + 8_000 + tick * 500),
            ReadPlan::Watch
        );
    }
    assert_eq!(
        clock.peek(&policy, delivered, delivered + 16_000),
        ReadPlan::Replay
    );
    assert_eq!(clock.last_replay_ms(), Some(delivered + 8_000));
    assert_eq!(
        clock.plan(&policy, delivered, delivered + 16_000),
        ReadPlan::Replay
    );
    // The window ends exactly one hour after delivery; the last tick inside
    // it may still replay, the first tick outside it stops polling and
    // nothing becomes retryable.
    assert_eq!(
        clock.plan(&policy, delivered, delivered + 3_599_999),
        ReadPlan::Replay
    );
    assert_eq!(
        clock.plan(&policy, delivered, delivered + 3_600_000),
        ReadPlan::Expired
    );
    assert_eq!(clock.last_replay_ms(), Some(delivered + 3_599_999));
    // A clock that went backwards is treated as not yet overdue.
    assert_eq!(
        clock.plan(&policy, delivered, delivered - 5),
        ReadPlan::Watch
    );

    let receipts = [("stuck", delivered), ("fresh", delivered + 60_000)];
    assert_eq!(
        earliest_tracked(&policy, receipts, delivered + 10),
        Some("stuck")
    );
    assert_eq!(
        earliest_tracked(&policy, receipts, delivered + 3_600_000),
        Some("fresh")
    );
    assert_eq!(
        earliest_tracked(&policy, receipts, delivered + 3_660_000),
        None
    );
}
