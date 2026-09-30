//! Per-session CLI state that native history cannot tell (docs/cli-state.md):
//! whether the editor accepts input, its visible text, whether the managed
//! instance answers, and the sends the CLI still holds in its own queue.
//! The same shape for every agent; the frontend never branches on source.
use super::input::InputStatus;
use super::store::QueuedSend;
use crate::delivery::target::Failure;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// A failed observation is retried after this, not on every watcher tick.
pub const FAILURE_BACKOFF: Duration = Duration::from_secs(3);
/// Queued sends of an instance that has answered nothing for this long are
/// marked `lost`: the CLI cannot deliver them any more.
pub const LOST_AFTER: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Instance {
    /// `true` after a successful screen read, `false` after a failed one,
    /// `null` before the first attempt.
    pub running: Option<bool>,
    /// The screen shows the CLI's busy indicator (spinner, "esc to
    /// interrupt") on the last successful read; `null` after a failed read
    /// or for CLIs without a recognized busy indicator (Grok, OpenCode).
    pub busy: Option<bool>,
}
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Editor {
    /// Visible editor text when the CLI's composer is recognized (Claude,
    /// Codex); `null` for CLIs without text extraction or an unknown screen.
    pub text: Option<String>,
}
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct CliState {
    pub observed_at: Option<f64>,
    pub instance: Instance,
    pub input: Option<InputStatus>,
    pub editor: Editor,
    pub queued: Vec<QueuedSend>,
}
impl CliState {
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("CliState serializes")
    }
}
impl PartialEq for InputStatus {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state && self.code == other.code && self.message == other.message
    }
}

/// The last observation of one identity key.
#[derive(Default)]
struct Entry {
    attempted: Option<Instant>,
    observed_at: Option<f64>,
    running: Option<bool>,
    input: Option<InputStatus>,
    editor_text: Option<String>,
    busy: Option<bool>,
    failing_since: Option<Instant>,
    /// Text of the send the latest native echo retired, kept to recognize it
    /// when the CLI puts it back into the editor (Esc before any output).
    retired: Option<String>,
    /// Claude's visible transcript above the editor (never serialized).
    transcript: Option<String>,
}

/// One observation result: the classified input state, editor text and busy
/// indicator, or the failure that kept the screen unreadable (instance gone,
/// host error).
pub type Observation = Result<(InputStatus, Option<String>, Option<bool>), Failure>;

#[derive(Default)]
pub struct Registry {
    entries: Mutex<HashMap<String, Entry>>,
    /// uid → identity key, so packet builders read state without resolving.
    keys: Mutex<HashMap<String, String>>,
}

pub fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

impl Registry {
    pub fn remember(&self, uid: &str, key: &str) {
        self.keys
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(uid.into(), key.into());
    }
    pub fn key_of(&self, uid: &str) -> Option<String> {
        self.keys
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(uid)
            .cloned()
    }
    /// The cached state when it is younger than `max_age`, or when the last
    /// attempt failed less than the backoff ago (no fresh read is due).
    pub fn fresh(&self, key: &str, max_age: Duration, queued: Vec<QueuedSend>) -> Option<CliState> {
        let entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let entry = entries.get(key)?;
        let attempted = entry.attempted?;
        let age = attempted.elapsed();
        let due = if entry.running == Some(false) {
            age >= FAILURE_BACKOFF
        } else {
            age >= max_age
        };
        if due {
            return None;
        }
        Some(Self::state(entry, queued))
    }
    /// Remembers the text of a send whose native record just retired it.
    pub fn retired(&self, key: &str, text: &str) {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.entry(key.into()).or_default().retired = Some(text.into());
    }
    /// Stores the transcript of the reading just recorded (`None` when the
    /// read failed or the CLI has no recognized editor).
    pub fn set_transcript(&self, key: &str, transcript: Option<String>) {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.entry(key.into()).or_default().transcript = transcript;
    }
    pub fn transcript(&self, key: &str) -> Option<String> {
        let entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.get(key)?.transcript.clone()
    }
    /// Native history holds output after the input the editor now shows (a
    /// CLI rewind or history recall): forget it as a returned send.
    pub fn answered(&self, key: &str, text: &str) {
        use crate::delivery::driver::same_text_ignoring_whitespace as same;
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let Some(entry) = entries.get_mut(key) else {
            return;
        };
        if entry
            .retired
            .as_deref()
            .is_some_and(|sent| same(text, sent))
        {
            entry.retired = None;
            if entry
                .input
                .is_some_and(|input| input.code == super::input::INPUT_RETURNED)
            {
                entry.input = Some(super::input::input_pending());
            }
        }
    }
    pub fn current(&self, key: &str, queued: Vec<QueuedSend>) -> CliState {
        let entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        match entries.get(key) {
            Some(entry) => Self::state(entry, queued),
            None => Self::state(&Entry::default(), queued),
        }
    }
    /// Stores one observation; returns whether the instance has been failing
    /// for `LOST_AFTER`, so the caller can mark its queued sends lost.
    pub fn record(
        &self,
        key: &str,
        observation: &Observation,
        queued: Vec<QueuedSend>,
    ) -> (CliState, bool) {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let entry = entries.entry(key.into()).or_default();
        let now = Instant::now();
        entry.attempted = Some(now);
        match observation {
            Ok((input, text, busy)) => {
                entry.observed_at = Some(unix_now());
                entry.running = Some(true);
                entry.input = Some(returned(*input, text.as_deref(), entry, &queued));
                entry.editor_text = text.clone();
                entry.busy = *busy;
                entry.failing_since = None;
            }
            Err(_) => {
                entry.running = Some(false);
                entry.input = None;
                entry.editor_text = None;
                entry.busy = None;
                entry.failing_since.get_or_insert(now);
            }
        }
        let lost = entry
            .failing_since
            .is_some_and(|since| since.elapsed() >= LOST_AFTER);
        (Self::state(entry, queued), lost)
    }
    fn state(entry: &Entry, queued: Vec<QueuedSend>) -> CliState {
        CliState {
            observed_at: entry.observed_at,
            instance: Instance {
                running: entry.running,
                busy: entry.busy,
            },
            input: entry.input,
            editor: Editor {
                text: entry.editor_text.clone(),
            },
            queued,
        }
    }
}

/// Claude writes the user record at Enter; an Esc before any output then
/// puts the prompt back into its editor while history keeps the record
/// (measured on Claude Code 2.1.284). Editor text equal to a send of this
/// session names that case instead of the generic nonempty-editor block.
fn returned(
    input: InputStatus,
    text: Option<&str>,
    entry: &Entry,
    queued: &[QueuedSend],
) -> InputStatus {
    use crate::delivery::driver::same_text_ignoring_whitespace as same;
    let Some(text) = text.filter(|_| input.code == super::input::INPUT_PENDING) else {
        return input;
    };
    let sent = entry.retired.iter().map(String::as_str);
    if sent
        .chain(queued.iter().map(|row| row.text.as_str()))
        .any(|sent| !sent.trim().is_empty() && same(text, sent))
    {
        super::input::input_returned()
    } else {
        input
    }
}

/// Visible editor text for CLIs whose composer the driver can read.
pub fn editor_text(
    source: &str,
    capture: &crate::delivery::driver::ScreenCapture,
) -> Option<String> {
    use crate::delivery::driver;
    match source {
        "claude" => driver::inspect(capture).text,
        "codex" => driver::inspect_codex(capture).text,
        _ => None,
    }
}

/// Claude's transcript above its editor; other CLIs have no rewind to follow.
pub fn transcript(
    source: &str,
    capture: &crate::delivery::driver::ScreenCapture,
) -> Option<String> {
    (source == "claude")
        .then(|| crate::delivery::driver::transcript(capture))
        .flatten()
}

/// Whether the screen shows the CLI's busy indicator, for CLIs the driver
/// recognizes; the same patterns delivery uses before it types.
pub fn screen_busy(source: &str, capture: &crate::delivery::driver::ScreenCapture) -> Option<bool> {
    use crate::delivery::driver;
    match source {
        "claude" => Some(driver::busy_screen(&capture.text)),
        "codex" => Some(driver::codex_busy_screen(&capture.text)),
        _ => None,
    }
}

/// Codex handles a bare /model in the TUI: it opens a settings menu and
/// never emits a native user/command echo. This says only that SEND handed
/// the command to the terminal, not that the user finished choosing a model.
/// Match the full delivered prompt so quotes, attachments and ordinary
/// messages mentioning /model still use native echo reconciliation.
pub(super) fn expects_native_echo(source: &str, text: &str) -> bool {
    !(source == "codex" && text.trim() == "/model")
}

impl super::Conversations {
    /// Persist positive TUI queue evidence, without retiring the send. A
    /// visible entry can match only one receipt, including already marked
    /// receipts, so repeated observations cannot confirm extra duplicates.
    pub(super) fn observe_screen_queue(
        &self,
        key: &str,
        source: &str,
        capture: &crate::delivery::driver::ScreenCapture,
    ) -> Result<(), Failure> {
        use crate::delivery::driver;
        if source != "codex" {
            return Ok(());
        }
        let queued = self.store.queued(key);
        // Older versions enqueued this TUI-only command. Correct the ledger
        // through the normal observation path, including after a restart;
        // no screen change or elapsed timeout can stand in for a native echo.
        let local: Vec<_> = queued
            .iter()
            .filter(|row| !expects_native_echo(source, &row.text))
            .map(|row| row.request_id.clone())
            .collect();
        if !local.is_empty() {
            self.store.retire_queued(key, &local)?;
        }
        if !queued
            .iter()
            .any(|row| row.state == "queued" && row.cli_queued_at.is_none())
        {
            return Ok(());
        }
        let mut visible = driver::codex_queued_texts(capture);
        let mut marks = Vec::new();
        for row in queued {
            if row.state != "queued" {
                continue;
            }
            if let Some(index) = visible.iter().position(|text| {
                !row.text.trim().is_empty()
                    && driver::same_text_ignoring_whitespace(text, &row.text)
            }) {
                visible.remove(index);
                if row.cli_queued_at.is_none() {
                    marks.push((row.request_id, unix_now()));
                }
            }
        }
        if !marks.is_empty() {
            self.store.mark_cli_queued(key, &marks)?;
        }
        Ok(())
    }
}

/// A native record that answers a queued send: a user/command message
/// (`enqueue == false`) or the CLI's own enqueue entry for the same text.
#[derive(Clone, Debug, PartialEq)]
pub struct Echo {
    pub hash: String,
    pub ts: Option<f64>,
    pub enqueue: bool,
}

/// SHA-256 of the JSON string of the trimmed text: the frontend and the
/// SEND receipt (`echo_hash`) use the same digest.
pub fn echo_hash(text: &str) -> String {
    super::store::fingerprint(&serde_json::json!(text.trim()))
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::input::InputState;

    fn ready() -> InputStatus {
        InputStatus {
            state: InputState::Ready,
            code: "",
            message: "",
        }
    }

    #[test]
    fn echo_hash_matches_send_receipt_digest() {
        let prompt = "  hello\n";
        let receipt = super::super::store::fingerprint(&serde_json::json!(prompt.trim()));
        assert_eq!(echo_hash(prompt), receipt.as_str().unwrap());
        assert_eq!(echo_hash("hello"), echo_hash("hello  \n"));
    }

    #[test]
    fn fresh_reading_is_shared_and_failures_back_off() {
        let registry = Registry::default();
        assert!(
            registry
                .fresh("k", Duration::from_secs(1), vec![])
                .is_none()
        );
        let (state, lost) = registry.record(
            "k",
            &Ok((ready(), Some("draft".into()), Some(false))),
            vec![],
        );
        assert!(!lost);
        assert_eq!(state.instance.running, Some(true));
        assert_eq!(state.editor.text.as_deref(), Some("draft"));
        assert_eq!(state.input.map(|i| i.state), Some(InputState::Ready));
        assert!(
            registry
                .fresh("k", Duration::from_secs(1), vec![])
                .is_some()
        );
        assert!(registry.fresh("k", Duration::ZERO, vec![]).is_none());
        let failure = Failure::new(409, "terminal_unlinked", "gone");
        let (state, lost) = registry.record("k", &Err(failure), vec![]);
        assert!(!lost);
        assert_eq!(state.instance.running, Some(false));
        assert!(state.input.is_none());
        // A failed reading is not retried before the backoff, whatever max_age says.
        assert!(registry.fresh("k", Duration::ZERO, vec![]).is_some());
    }

    #[test]
    fn editor_holding_a_retired_send_is_reported_as_returned() {
        let registry = Registry::default();
        let pending = super::super::input::input_pending();
        let (state, _) = registry.record(
            "k",
            &Ok((pending, Some("哈希为什么".into()), Some(false))),
            vec![],
        );
        assert_eq!(
            state.input.map(|i| i.code),
            Some(super::super::input::INPUT_PENDING)
        );
        registry.retired("k", "Reply with\nALPHA.");
        let (state, _) = registry.record(
            "k",
            &Ok((pending, Some("Reply withALPHA.".into()), Some(false))),
            vec![],
        );
        assert_eq!(
            state.input.map(|i| i.code),
            Some(super::super::input::INPUT_RETURNED)
        );
        // Output after that input (a rewind or history recall) means the
        // CLI answered it: the editor text is no longer an Esc-returned send.
        registry.answered("k", "Reply with ALPHA.");
        assert_eq!(
            registry.current("k", vec![]).input.map(|i| i.code),
            Some(super::super::input::INPUT_PENDING)
        );
        let (state, _) = registry.record(
            "k",
            &Ok((pending, Some("Reply withALPHA.".into()), Some(false))),
            vec![],
        );
        assert_eq!(
            state.input.map(|i| i.code),
            Some(super::super::input::INPUT_PENDING)
        );
        // A ready editor is never rewritten.
        let (state, _) = registry.record(
            "k",
            &Ok((ready(), Some("Reply with ALPHA.".into()), Some(false))),
            vec![],
        );
        assert_eq!(state.input.map(|i| i.state), Some(InputState::Ready));
    }

    #[test]
    fn unknown_session_serializes_with_nulls() {
        let registry = Registry::default();
        let value = registry.current("none", vec![]).to_value();
        assert!(value["observed_at"].is_null());
        assert!(value["instance"]["running"].is_null());
        assert!(value["instance"]["busy"].is_null());
        assert!(value["input"].is_null());
        assert!(value["editor"]["text"].is_null());
        assert_eq!(value["queued"], serde_json::json!([]));
    }
}
