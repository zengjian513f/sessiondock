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
                entry.input = Some(*input);
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
