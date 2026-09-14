//! Codex native acknowledgment adapter against a real rollout file appended by
//! a private fake "Codex CLI" shell script, observed through the real
//! `SessionStore` reader path (checked native input, raw index, projection).
//! No real CLI, model, native home or network is involved.
#![cfg(unix)]

use sessiondock::delivery::codex::Correlation;
use sessiondock::delivery::codex_adapter::{
    Absence, Boundary, Delivered, Observation, Outcome, PossibleMatch, TimestampCheck, Uncertainty,
    observe,
};
use sessiondock::sessions::{SessionRoots, SessionStore};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// Appends what a Codex TUI writes for one submitted prompt: a turn context,
/// `task_started`, the `response_item` user message and the `user_message`
/// telemetry event; `finish` closes the last turn with `task_complete`.
/// `<NL>` in a line becomes a newline inside the recorded prompt.
const FAKE_CODEX: &str = r#"#!/bin/sh
ROLLOUT="$1"
turn=0
while IFS= read -r line; do
  ts=$(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)
  case "$ts" in
    *N*) # BSD date (macOS) has no %N: take milliseconds from python3 instead
      ts=$(python3 -c 'import datetime; print(datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z")') ;;
  esac
  case "$line" in
    finish)
      printf '{"timestamp":"%s","type":"event_msg","payload":{"type":"task_complete","turn_id":"fake-turn-%s","duration_ms":1}}\n' "$ts" "$turn" >> "$ROLLOUT"
      ;;
    *)
      turn=$((turn+1))
      esc=$(printf '%s' "$line" | sed 's/\\/\\\\/g; s/"/\\"/g; s/<NL>/\\n/g')
      printf '{"timestamp":"%s","type":"turn_context","payload":{"model":"fake-codex"}}\n' "$ts" >> "$ROLLOUT"
      printf '{"timestamp":"%s","type":"event_msg","payload":{"type":"task_started","turn_id":"fake-turn-%s"}}\n' "$ts" "$turn" >> "$ROLLOUT"
      printf '{"timestamp":"%s","type":"response_item","payload":{"type":"message","role":"user","turn_id":"fake-turn-%s","content":[{"type":"input_text","text":"%s"}]}}\n' "$ts" "$turn" "$esc" >> "$ROLLOUT"
      printf '{"timestamp":"%s","type":"event_msg","payload":{"type":"user_message","message":"%s"}}\n' "$ts" "$esc" >> "$ROLLOUT"
      ;;
  esac
done
"#;

struct Fixture {
    _temp: tempfile::TempDir,
    rollout: PathBuf,
    cli: PathBuf,
    store: SessionStore,
    uid: String,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap()
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("codex-root");
        let rollout = root.join("sessions").join("rollout-fake.jsonl");
        fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        fs::write(
            &rollout,
            concat!(
                r#"{"timestamp":"2026-09-12T08:00:00.000Z","type":"session_meta","payload":{"id":"fake-codex-session","timestamp":"2026-09-12T08:00:00.000Z","cwd":"/synthetic"}}"#,
                "\n",
                r#"{"timestamp":"2026-09-12T08:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","turn_id":"human-0","content":[{"type":"input_text","text":"hello from before"}]}}"#,
                "\n",
            ),
        )
        .unwrap();
        let cli = temp.path().join("fake-codex.sh");
        fs::write(&cli, FAKE_CODEX).unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
        let store = SessionStore::new(SessionRoots {
            codex: Some(root),
            ..Default::default()
        });
        let list = store.list(true).unwrap();
        let uid = list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["path"] == rollout.to_string_lossy().as_ref())
            .expect("the fake rollout is inventoried")["uid"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            _temp: temp,
            rollout,
            cli,
            store,
            uid,
        }
    }

    /// Capture the fixed boundary from a fresh frozen view, as the executor
    /// would immediately before its durable Enter marker.
    fn boundary(&self, sequence: u64) -> Boundary {
        self.store.list(true).unwrap();
        let snapshot = self.store.snapshot(&self.uid, "").unwrap();
        Boundary::capture(&snapshot.native_checkpoint(), sequence, now_ms())
    }

    /// Feed lines to the fake CLI's stdin and wait for it to exit.
    fn submit(&self, lines: &[&str]) {
        let mut child = Command::new("/bin/sh")
            .arg(&self.cli)
            .arg(&self.rollout)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        {
            let mut stdin = child.stdin.take().unwrap();
            for line in lines {
                writeln!(stdin, "{line}").unwrap();
            }
        }
        assert!(child.wait().unwrap().success());
    }

    fn observe(&self, boundary: &Boundary, text: &str) -> Observation {
        self.store.list(true).unwrap();
        let snapshot = self.store.snapshot(&self.uid, "").unwrap();
        observe(
            &snapshot,
            boundary,
            &Delivered {
                uid: self.uid.clone(),
                text: text.into(),
                media: Vec::new(),
            },
        )
    }
}

fn possible(observation: &Observation) -> &PossibleMatch {
    match &observation.outcome {
        Outcome::Possible(found) => found,
        other => panic!("expected a possible match: {other:?}"),
    }
}

/// Physical `[start, end)` of the n-th LF-terminated line at or after `from`.
fn line_range(path: &Path, from: u64, index: usize) -> (u64, u64) {
    let bytes = fs::read(path).unwrap();
    let mut start = from as usize;
    for _ in 0..index {
        start += bytes[start..].iter().position(|b| *b == b'\n').unwrap() + 1;
    }
    let end = start + bytes[start..].iter().position(|b| *b == b'\n').unwrap() + 1;
    (start as u64, end as u64)
}

#[test]
fn codex_ack_fake_cli_records_are_classified_through_the_real_reader_path() {
    let fixture = Fixture::new();
    let text = "  hello from the web\n  second line  \n";

    // An identical earlier human input is before the boundary: not evidence.
    let boundary = fixture.boundary(1);
    assert_eq!(
        boundary.confirmation.position,
        fs::metadata(&fixture.rollout).unwrap().len()
    );
    assert_eq!(
        fixture.observe(&boundary, "hello from before").outcome,
        Outcome::Absent(Absence { skipped_earlier: 0 })
    );
    assert_eq!(
        fixture.observe(&boundary, text).outcome,
        Outcome::Absent(Absence { skipped_earlier: 0 })
    );

    fixture.submit(&["hello from the web<NL>  second line", "finish"]);
    let observation = fixture.observe(&boundary, text);
    assert_eq!(observation.sequence, 1);
    let found = possible(&observation);
    let (record_start, record_end) =
        line_range(&fixture.rollout, boundary.confirmation.position, 2);
    let file_len = fs::metadata(&fixture.rollout).unwrap().len();
    assert_eq!(found.evidence.correlation, Correlation::PossibleTextMatch);
    assert!(found.evidence.real_user_input);
    assert_eq!(found.evidence.uid, fixture.uid);
    assert_eq!(found.evidence.validated_confirmation, boundary.confirmation);
    assert_eq!(found.evidence.text, "hello from the web\n  second line");
    assert_eq!(found.evidence.record.start, record_start);
    assert_eq!(found.evidence.record.end, record_end);
    assert_eq!(found.evidence.record.turn_id, "fake-turn-1");
    assert_eq!(
        found.evidence.record.record_id,
        format!("codex-line:{record_start}-{record_end}")
    );
    assert_eq!(
        found.evidence.record.source_identity,
        boundary.confirmation.source_identity
    );
    assert_eq!(found.record.timestamp, TimestampCheck::Verified);
    assert!(found.record.recorded_ms.unwrap() >= boundary.delivered_ms);
    let completion = found.completion.as_ref().unwrap();
    assert_eq!(completion.turn_id, "fake-turn-1");
    assert_eq!(completion.record_end, file_len);
    let watch = observation.watch.as_ref().unwrap();
    assert_eq!(watch.position, file_len);
    assert_eq!(watch.source_identity, boundary.confirmation.source_identity);

    // Internal whitespace differs: the same words are not the same prompt.
    assert_eq!(
        fixture
            .observe(&boundary, "hello from the web\nsecond line")
            .outcome,
        Outcome::Absent(Absence { skipped_earlier: 0 })
    );

    // Row order is kept: the first causal matching record still wins.
    fixture.submit(&["hello from the web<NL>  second line", "finish"]);
    assert!(matches!(
        fixture.observe(&boundary, text).outcome,
        Outcome::Possible(_)
    ));

    // A new boundary after those records sees only what follows it.
    let later = fixture.boundary(2);
    assert!(later.confirmation.position > boundary.confirmation.position);
    assert_eq!(
        fixture.observe(&later, text).outcome,
        Outcome::Absent(Absence { skipped_earlier: 0 })
    );
    fixture.submit(&["third \"quoted\" \\ prompt"]);
    let observation = fixture.observe(&later, "third \"quoted\" \\ prompt\n");
    let found = possible(&observation);
    assert_eq!(
        found.record.start,
        line_range(&fixture.rollout, later.confirmation.position, 2).0
    );
    assert_eq!(found.record.turn_id.as_deref(), Some("fake-turn-1"));
    assert_eq!(found.completion, None, "no task_complete was written yet");
    // The original boundary still selects its first causal matching record.
    assert!(matches!(
        fixture.observe(&boundary, text).outcome,
        Outcome::Possible(_)
    ));
    assert!(matches!(
        fixture
            .observe(&boundary, "third \"quoted\" \\ prompt")
            .outcome,
        Outcome::Possible(_)
    ));

    // Cutting the rollout back to exactly the later boundary keeps both
    // fences valid: the first still sees its first matching record, the
    // later one sees nothing after itself.
    let bytes = fs::read(&fixture.rollout).unwrap();
    fs::write(
        &fixture.rollout,
        &bytes[..later.confirmation.position as usize],
    )
    .unwrap();
    assert!(matches!(
        fixture.observe(&boundary, text).outcome,
        Outcome::Possible(_)
    ));
    let observation = fixture.observe(&later, text);
    assert_eq!(
        observation.outcome,
        Outcome::Absent(Absence { skipped_earlier: 0 })
    );
    assert_eq!(observation.watch.as_ref(), Some(&later.confirmation));

    // Truncating below the first boundary invalidates every fence at or
    // beyond it: nothing after a missing prefix can be attributed.
    fs::write(
        &fixture.rollout,
        &bytes[..boundary.confirmation.position as usize - 1],
    )
    .unwrap();
    let observation = fixture.observe(&boundary, text);
    assert_eq!(
        observation.outcome,
        Outcome::Uncertain(Uncertainty::CheckpointMismatch)
    );
    assert_eq!(observation.watch, None);
    assert_eq!(
        fixture.observe(&later, text).outcome,
        Outcome::Uncertain(Uncertainty::CheckpointMismatch)
    );
}
