//! The live `prompt` of a session view.
//!
//! `LivePrompts` answers `/api/messages` and `/api/watch` for one main
//! session: a Claude session reads its question-card file
//! (`bridge::claude`, cleared once the matching native answer/tool_result is
//! in the records — Python `_claude_prompt`); a Codex session reads the
//! approval dialog off its managed instance's screen (`bridge::codex`,
//! Python `_codex_prompt`); a subagent view or any other source has no
//! prompt. Nothing here sends keys: answering goes through `/api/term/send`
//! under the page's own lease exactly like the native console.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use ptyhost_client::{BoundTarget, CaptureKind, ControlOp, ControlReply, HostClient};
use serde_json::Value;

use super::{claude::PromptStore, claude::Revision, codex};
use crate::{sessions::ViewSnapshot, state::AppState};

/// Python `_codex_prompt` re-looks the pane up once per second while missing.
const CODEX_LOOKUP_INTERVAL: Duration = Duration::from_secs(1);
/// The last 80 joined scrollback rows.
const CODEX_CAPTURE_LINES: usize = 80;

/// Which live prompt a view can have. `None` for subagents and other sources.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptScope {
    Claude { sid: String },
    Codex { uid: String },
}

impl PromptScope {
    /// Only a main Claude/Codex session; the SID
    /// comes from the validated records of this exact view, never the client.
    pub fn of(snapshot: &ViewSnapshot) -> Option<Self> {
        let scope = snapshot.native_scope().ok()?;
        if scope.agent_id.is_some() {
            return None;
        }
        match scope.source.as_str() {
            "claude" => Some(Self::Claude {
                sid: scope.session_id,
            }),
            "codex" => Some(Self::Codex { uid: scope.uid }),
            _ => None,
        }
    }
}

/// Cached managed-instance target for one Codex watcher, so a 0.5 s screen
/// poll does not re-run host discovery: the target is looked up (through the
/// shared, TTL-cached runtime observation) only while missing, at most once a
/// second, and dropped as soon as a capture fails (instance gone).
#[derive(Default)]
pub struct CodexProbe {
    target: Option<BoundTarget>,
    looked_up: Option<Instant>,
}

struct CodexCapture {
    client: HostClient,
}

/// Prompt sources configured for this process.
pub struct LivePrompts {
    claude: Option<PromptStore>,
    codex: Option<CodexCapture>,
}

impl LivePrompts {
    /// `state_dir` enables Claude question cards; `host_dir` (the ptyhost
    /// directory, same as the terminal transport) enables Codex approvals.
    pub fn new(
        state_dir: Option<&std::path::Path>,
        host_dir: Option<&std::path::Path>,
    ) -> Result<Self, ptyhost_client::Error> {
        let codex = match host_dir {
            Some(directory) => Some(CodexCapture {
                client: HostClient::new(
                    directory,
                    ptyhost_client::Limits {
                        max_line_bytes: 4 * 1024 * 1024,
                        operation_timeout: Duration::from_secs(10),
                        ..Default::default()
                    },
                )?,
            }),
            None => None,
        };
        Ok(Self {
            claude: state_dir.map(PromptStore::new),
            codex,
        })
    }

    pub fn claude_store(&self) -> Option<&PromptStore> {
        self.claude.as_ref()
    }

    /// The file stamp the SSE loop polls.
    pub fn claude_revision(&self, sid: &str) -> Option<Revision> {
        self.claude.as_ref()?.revision(sid)
    }

    /// The file as is, for `prompt_only` packets.
    pub fn claude_prompt_raw(&self, sid: &str) -> Value {
        self.claude
            .as_ref()
            .and_then(|store| store.prompt(sid))
            .unwrap_or(Value::Null)
    }

    /// The card stays until the matching native
    /// answer really is in the records. Claude does not always emit
    /// `PostToolUseFailure` when the dialog is dismissed with Esc, but the
    /// failed `tool_result` with the same `tool_use_id` always lands; the id is
    /// unique within a session, so a record beats a `waiting` file.
    pub fn claude_prompt(&self, sid: &str, messages: &Value) -> Value {
        let Some(store) = &self.claude else {
            return Value::Null;
        };
        let Some(prompt) = store.prompt(sid) else {
            return Value::Null;
        };
        let tool_id = prompt["id"].as_str().unwrap_or("");
        let answered = !tool_id.is_empty()
            && messages.as_array().is_some_and(|rows| {
                rows.iter().any(|message| {
                    message["call_id"].as_str() == Some(tool_id)
                        && matches!(message["role"].as_str(), Some("answer" | "tool_result"))
                })
            });
        if answered {
            store.clear(sid, tool_id);
            return Value::Null;
        }
        prompt
    }

    /// The approval visible on the unique managed
    /// instance of this UID, `None` (JSON null) when there is no instance, the
    /// capture fails (a vanishing pane is a normal exit race) or the screen
    /// shows no dialog.
    pub async fn codex_prompt(&self, state: &AppState, uid: &str, probe: &mut CodexProbe) -> Value {
        let Some(codex) = &self.codex else {
            return Value::Null;
        };
        if probe.target.is_none() {
            let due = probe
                .looked_up
                .is_none_or(|at| at.elapsed() >= CODEX_LOOKUP_INTERVAL);
            if !due {
                return Value::Null;
            }
            probe.looked_up = Some(Instant::now());
            probe.target = codex_target(state, uid).await;
        }
        let Some(target) = probe.target.clone() else {
            return Value::Null;
        };
        let capture = codex
            .client
            .request_bound(
                &target,
                ControlOp::Capture {
                    kind: CaptureKind::Scrollback,
                    styled: false,
                    join: true,
                    lines: CODEX_CAPTURE_LINES,
                },
            )
            .await;
        match capture {
            Ok(ControlReply::Capture(capture)) => {
                codex::approval_prompt(&capture.text).unwrap_or(Value::Null)
            }
            _ => {
                probe.target = None;
                Value::Null
            }
        }
    }

    /// Python `_session_prompt` for one packet: the value the `prompt` field
    /// carries (JSON null when there is none).
    pub async fn current(
        &self,
        state: &AppState,
        scope: &PromptScope,
        messages: &Value,
        probe: &mut CodexProbe,
    ) -> Value {
        match scope {
            PromptScope::Claude { sid } => self.claude_prompt(sid, messages),
            PromptScope::Codex { uid } => self.codex_prompt(state, uid, probe).await,
        }
    }
}

/// The unique guard-capable managed host whose verified association names
/// this UID (the same rule as the delivery executor's resolver), from the
/// display-grade shared observation (2 s TTL, single flight). No lease is
/// taken: a read-only capture is not terminal input.
async fn codex_target(state: &AppState, uid: &str) -> Option<BoundTarget> {
    let runtime = state.runtime.as_ref()?;
    let shared = crate::api::runtime::shared(state, runtime, false)
        .await
        .ok()?;
    let mut matches = shared
        .snapshot
        .hosts
        .iter()
        .filter_map(|host| host.bound_target())
        .filter(|target| target.uid() == uid);
    let target = matches.next()?.clone();
    if matches.next().is_some() {
        return None;
    }
    Some(target)
}

/// Shared handle stored in `AppState`.
pub type SharedPrompts = Arc<LivePrompts>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn prompts(temp: &tempfile::TempDir) -> LivePrompts {
        LivePrompts::new(Some(temp.path()), None).unwrap()
    }

    #[test]
    fn claude_prompt_is_cleared_only_by_the_matching_native_answer() {
        let temp = tempfile::tempdir().unwrap();
        let live = prompts(&temp);
        let store = live.claude_store().unwrap();
        store.handle(&json!({
            "hook_event_name": "PreToolUse", "session_id": "sid-000001",
            "tool_name": "AskUserQuestion", "tool_use_id": "toolu_1",
            "tool_input": {"questions": [{"question": "A or B?", "options": ["A", "B"]}]},
        }));
        // Unrelated records keep the card, even a tool_result of another id.
        let unrelated = json!([{"role": "user", "text": "hi"},
            {"role": "tool_result", "call_id": "toolu_0"}, {"role": "question", "call_id": "toolu_1"}]);
        let prompt = live.claude_prompt("sid-000001", &unrelated);
        assert_eq!(prompt["id"], "toolu_1");
        assert_eq!(prompt["state"], "waiting");
        assert!(store.prompt("sid-000001").is_some());
        // The same id as an `answer` (or `tool_result`) record clears the file.
        let answered = json!([{"role": "answer", "call_id": "toolu_1", "text": "A"}]);
        assert_eq!(live.claude_prompt("sid-000001", &answered), Value::Null);
        assert!(store.prompt("sid-000001").is_none());
        assert_eq!(live.claude_prompt("sid-000001", &answered), Value::Null);
        assert_eq!(live.claude_prompt_raw("sid-000001"), Value::Null);
        // A settled-but-unanswered card is still shown (the page renders "settling").
        store.handle(&json!({
            "hook_event_name": "PreToolUse", "session_id": "sid-000001",
            "tool_name": "AskUserQuestion", "tool_use_id": "toolu_2",
            "tool_input": {"questions": [{"question": "again?"}]},
        }));
        store.handle(&json!({"hook_event_name": "PostToolUse", "session_id": "sid-000001", "tool_use_id": "toolu_2"}));
        assert_eq!(
            live.claude_prompt("sid-000001", &json!([]))["state"],
            "submitted"
        );
        assert_eq!(live.claude_prompt_raw("sid-000001")["state"], "submitted");
        assert!(live.claude_revision("sid-000001").is_some());
        let tool_result = json!([{"role": "tool_result", "call_id": "toolu_2"}]);
        assert_eq!(live.claude_prompt("sid-000001", &tool_result), Value::Null);
        assert!(live.claude_revision("sid-000001").is_none());
    }

    #[test]
    fn unconfigured_sources_have_no_prompt() {
        let live = LivePrompts::new(None, None).unwrap();
        assert!(live.claude_store().is_none());
        assert_eq!(live.claude_prompt("sid-000001", &json!([])), Value::Null);
        assert_eq!(live.claude_prompt_raw("sid-000001"), Value::Null);
        assert!(live.claude_revision("sid-000001").is_none());
    }
}
