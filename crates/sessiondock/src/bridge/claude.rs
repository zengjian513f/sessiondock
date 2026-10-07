//! Claude question cards.
//!
//! Claude Code shows an `AskUserQuestion` dialog in its TUI before the
//! `tool_use` record reaches the transcript. The `sessiondock claude-hook`
//! subcommand (installed through the settings file `settings()` writes) is
//! Claude's `PreToolUse` / `PostToolUse` / `PostToolUseFailure` hook for that
//! tool plus `SessionStart` / `SessionEnd`; it passively records the question
//! into `<state dir>/claude-prompts/<session_id>.json` (0600, atomic replace)
//! and never approves, denies or rewrites the tool call. The Web service reads
//! the file for the live `prompt` field (`bridge::live`).

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{SystemTime, UNIX_EPOCH},
};

use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::json_compat::truthy;

/// Claude's workspace trust runs before hooks or native history exist. Read
/// both option rows and the selection from the current screen, not a saved
/// hook question. The two-row menu needs at most one arrow before Enter.
pub fn startup_prompt(screen: &str) -> Option<Value> {
    let clean = crate::delivery::driver::strip_ansi(screen).replace('\r', "");
    let lines: Vec<_> = clean.lines().map(str::trim_end).collect();
    let start = lines
        .iter()
        .rposition(|line| line.trim() == "Accessing workspace:")?;
    let block = &lines[start..];
    let footer = block
        .iter()
        .position(|line| line.trim() == "Enter to confirm · Esc to cancel")?;
    if block[footer + 1..]
        .iter()
        .any(|line| !line.trim().is_empty())
        || !block[..footer]
            .iter()
            .any(|line| line.trim_start().starts_with("Quick safety check:"))
    {
        return None;
    }
    fn option(line: &str) -> Option<(&str, bool)> {
        let line = line.trim();
        let selected = line.starts_with('❯');
        let label = line.strip_prefix('❯').unwrap_or(line).trim();
        matches!(label, "No, exit" | "Yes, I trust this folder").then_some((label, selected))
    }
    let first = block[..footer]
        .iter()
        .position(|line| option(line).is_some())?;
    let rows: Vec<_> = block[first..footer]
        .iter()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if rows.len() != 2 {
        return None;
    }
    let options: Vec<_> = rows
        .iter()
        .map(|line| option(line))
        .collect::<Option<_>>()?;
    let trust = options
        .iter()
        .position(|(label, _)| *label == "Yes, I trust this folder")?;
    if options.iter().filter(|(_, selected)| *selected).count() != 1
        || options
            .iter()
            .filter(|(label, _)| *label == "No, exit")
            .count()
            != 1
    {
        return None;
    }
    let selected = options.iter().position(|(_, selected)| *selected)?;
    let mut keys = Vec::new();
    if trust != selected {
        keys.push(if trust > selected { "Down" } else { "Up" });
    }
    keys.push("Enter");
    let question = block[1..first]
        .iter()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    let digest = format!("{:x}", Sha256::digest(question.as_bytes()));
    Some(json!({
        "id": format!("claude-startup:{}", &digest[..16]),
        "source": "claude", "kind": "folder_trust", "state": "waiting",
        "questions": [{"header": "目录信任确认", "question": question, "multiple": false,
            "options": [{"label": "信任并继续", "keys": keys},
                        {"label": "退出", "keys": ["Escape"]}]}],
    }))
}

/// Subdirectory of `SESSIONDOCK_STATE_DIR` that holds one file per session.
pub const PROMPTS_DIRNAME: &str = "claude-prompts";
/// Files with another version are ignored.
pub const VERSION: u64 = 1;
/// The hook subcommand name (`sessiondock claude-hook`).
pub const HOOK_SUBCOMMAND: &str = "claude-hook";

static SESSION_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{6,128}$").expect("session id regex"));

/// File revision the SSE loop compares: `(mtime_ns, size)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Revision {
    pub modified_ns: i128,
    pub size: u64,
}

/// One session's question-card file store rooted at `<state dir>/claude-prompts`.
#[derive(Clone, Debug)]
pub struct PromptStore {
    directory: PathBuf,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The normalized question rows of a `tool_input`.
/// Rows without a `question` are dropped; string options become
/// `{label, description: ""}`; `multiSelect`/`multiple` become `multiple`.
pub fn questions(tool_input: &Value) -> Vec<Value> {
    let rows: Vec<&Value> = match tool_input.get("questions") {
        Some(Value::Array(rows)) => rows.iter().collect(),
        Some(row @ Value::Object(_)) => vec![row],
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for row in rows {
        let Some(row) = row.as_object() else {
            continue;
        };
        let question = row.get("question").map(text).unwrap_or_default();
        if question.trim().is_empty() {
            continue;
        }
        let mut options = Vec::new();
        if let Some(Value::Array(items)) = row.get("options") {
            for option in items {
                match option {
                    Value::String(label) => {
                        options.push(json!({"label": label, "description": ""}));
                    }
                    Value::Object(object) => {
                        let label = object.get("label").filter(|label| truthy(label));
                        if let Some(label) = label {
                            options.push(json!({
                                "label": text(label),
                                "description": object.get("description").map(text).unwrap_or_default(),
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }
        let multiple =
            row.get("multiSelect").is_some_and(truthy) || row.get("multiple").is_some_and(truthy);
        out.push(json!({
            "header": row.get("header").map(text).unwrap_or_default(),
            "question": question,
            "options": options,
            "multiple": multiple,
        }));
    }
    out
}

/// Unchanged content is not rewritten (its mtime is the
/// revision browsers poll), else a private temp file replaces the target.
fn atomic_json(path: &Path, value: &Value) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "prompt path has no parent"))?;
    if let Err(error) = fs::create_dir_all(parent)
        && error.kind() != io::ErrorKind::AlreadyExists
    {
        return Err(error);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    let mut data = serde_json::to_string_pretty(value)?;
    data.push('\n');
    if fs::read_to_string(path).is_ok_and(|current| current == data) {
        return Ok(());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "prompt path has no name"))?;
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        use std::io::Write;
        if let Err(error) = file
            .write_all(data.as_bytes())
            .and_then(|()| file.sync_data())
        {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

impl PromptStore {
    /// `<state dir>/claude-prompts`; nothing is created until a hook writes.
    pub fn new(state_dir: &Path) -> Self {
        Self {
            directory: state_dir.join(PROMPTS_DIRNAME),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Only a plain session id names a file.
    pub fn path(&self, session_id: &str) -> Option<PathBuf> {
        SESSION_ID
            .is_match(session_id)
            .then(|| self.directory.join(format!("{session_id}.json")))
    }

    fn read(&self, session_id: &str) -> Option<Value> {
        let path = self.path(session_id)?;
        let data = fs::read(path).ok()?;
        serde_json::from_slice(&data).ok()
    }

    /// The parsed file when it carries this version and at
    /// least one question; anything else (missing, malformed, stale) is `None`.
    pub fn prompt(&self, session_id: &str) -> Option<Value> {
        let value = self.read(session_id)?;
        let version_ok = value.get("version").and_then(Value::as_u64) == Some(VERSION);
        let has_questions = value.get("questions").is_some_and(truthy);
        (version_ok && has_questions).then_some(value)
    }

    /// `(mtime_ns, size)` of the file, `None` when absent.
    pub fn revision(&self, session_id: &str) -> Option<Revision> {
        let metadata = fs::metadata(self.path(session_id)?).ok()?;
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|since| i128::try_from(since.as_nanos()).unwrap_or(i128::MAX))
            .unwrap_or(0);
        Some(Revision {
            modified_ns,
            size: metadata.len(),
        })
    }

    /// Remove the file. With a `tool_use_id` the file is only
    /// removed when it records that same id (or none at all).
    pub fn clear(&self, session_id: &str, tool_use_id: &str) -> bool {
        let Some(path) = self.path(session_id) else {
            return false;
        };
        if !tool_use_id.is_empty() {
            let current = self.read(session_id).unwrap_or(Value::Null);
            let id = current.get("id").map(text).unwrap_or_default();
            if !id.is_empty() && id != tool_use_id {
                return false;
            }
        }
        fs::remove_file(path).is_ok()
    }

    /// Mark the native dialog finished (`submitted` /
    /// `cancelled`) but keep the file. Claude appends the `tool_use` /
    /// `tool_result` records seconds after the Post hook; deleting here would
    /// let the page fall back from the question to a false "Working".
    pub fn settle(&self, session_id: &str, tool_use_id: &str, state: &str) -> bool {
        if !matches!(state, "submitted" | "cancelled") {
            return false;
        }
        let Some(path) = self.path(session_id) else {
            return false;
        };
        let Some(mut current) = self.read(session_id) else {
            return false;
        };
        if current.get("id").map(text).unwrap_or_default() != tool_use_id {
            return false;
        }
        let Some(object) = current.as_object_mut() else {
            return false;
        };
        object.insert("state".into(), json!(state));
        object.insert("settled".into(), json!(now_ms()));
        atomic_json(&path, &current).is_ok()
    }

    /// One hook payload from Claude's stdin.
    pub fn handle(&self, data: &Value) {
        let session_id = data.get("session_id").map(text).unwrap_or_default();
        let Some(path) = self.path(&session_id) else {
            return;
        };
        let event = data.get("hook_event_name").map(text).unwrap_or_default();
        let tool_use_id = data.get("tool_use_id").map(text).unwrap_or_default();
        // Only the main thread can open the TUI dialog: Claude denies
        // AskUserQuestion to every agent, yet its forks (prompt suggestion
        // and the like, `agent_id` set) still fire PreToolUse for it and are
        // then refused with no Post hook, which would strand a card no
        // terminal shows.
        let main_thread = data
            .get("agent_id")
            .is_none_or(|agent| text(agent).is_empty());
        match event.as_str() {
            "PreToolUse"
                if main_thread
                    && data.get("tool_name").map(text).as_deref() == Some("AskUserQuestion") =>
            {
                let questions = questions(data.get("tool_input").unwrap_or(&Value::Null));
                if !questions.is_empty() {
                    let _ = atomic_json(
                        &path,
                        &json!({
                            "version": VERSION,
                            "source": "claude",
                            "id": tool_use_id,
                            "created": now_ms(),
                            "state": "waiting",
                            "questions": questions,
                        }),
                    );
                }
            }
            "PostToolUse" => {
                self.settle(&session_id, &tool_use_id, "submitted");
            }
            "PostToolUseFailure" => {
                self.settle(&session_id, &tool_use_id, "cancelled");
            }
            // A normal exit and the following resume both drop an unfinished
            // dialog; a crash's leftovers disappear at the next takeover.
            "SessionStart" | "SessionEnd" => {
                self.clear(&session_id, "");
            }
            _ => {}
        }
    }
}

/// The hooks-only settings document Claude receives
/// through `--settings`. `command` is the absolute `sessiondock` binary and
/// `args` the exec-form subcommand (`claude-hook --state-dir DIR`), so the
/// hook does not depend on the CLI's cleared environment.
pub fn settings(command: &Path, state_dir: &Path) -> Value {
    let hook = json!({
        "type": "command",
        "command": command.to_string_lossy(),
        "args": [HOOK_SUBCOMMAND, "--state-dir", state_dir.to_string_lossy()],
    });
    let matched = json!([{"matcher": "AskUserQuestion", "hooks": [hook]}]);
    json!({
        "hooks": {
            "PreToolUse": matched,
            "PostToolUse": matched,
            "PostToolUseFailure": matched,
            "SessionStart": [{"hooks": [hook]}],
            "SessionEnd": [{"hooks": [hook]}],
        }
    })
}

/// Write `settings()` privately (0600, atomic replace, unchanged content kept).
pub fn write_settings(path: &Path, command: &Path, state_dir: &Path) -> io::Result<()> {
    atomic_json(path, &settings(command, state_dir))
}

/// The `claude-hook` subcommand body: read one JSON object from `input`,
/// record it, and report nothing. Failures never reach Claude (exit 0 with no
/// output is the caller's contract).
pub fn run_hook(state_dir: &Path, input: &mut dyn io::Read) {
    let mut data = Vec::new();
    if input.read_to_end(&mut data).is_err() {
        return;
    }
    let Ok(value) = serde_json::from_slice::<Value>(&data) else {
        return;
    };
    if !value.is_object() {
        return;
    }
    PromptStore::new(state_dir).handle(&value);
}
