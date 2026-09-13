//! Claude question cards (Python `agenthub/claude_bridge.py`).
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

/// Subdirectory of `SESSIONDOCK_STATE_DIR` that holds one file per session.
pub const PROMPTS_DIRNAME: &str = "claude-prompts";
/// Python `claude_bridge.VERSION`: files with another version are ignored.
pub const VERSION: u64 = 1;
/// The hook subcommand name (`sessiondock claude-hook`).
pub const HOOK_SUBCOMMAND: &str = "claude-hook";
/// Largest hook payload the subcommand reads from stdin (Claude's are a few KiB).
pub const HOOK_INPUT_LIMIT: u64 = 4 * 1024 * 1024;

static SESSION_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{6,128}$").expect("session id regex"));

/// File revision the SSE loop compares (Python `revision`: `(mtime_ns, size)`).
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

/// Python `_questions`: the normalized question rows of a `tool_input`.
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

/// Python truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

/// Python `_atomic_json`: unchanged content is not rewritten (its mtime is the
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

    /// Python `_path`: only a plain session id names a file.
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

    /// Python `prompt`: the parsed file when it carries this version and at
    /// least one question; anything else (missing, malformed, stale) is `None`.
    pub fn prompt(&self, session_id: &str) -> Option<Value> {
        let value = self.read(session_id)?;
        let version_ok = value.get("version").and_then(Value::as_u64) == Some(VERSION);
        let has_questions = value.get("questions").is_some_and(truthy);
        (version_ok && has_questions).then_some(value)
    }

    /// Python `revision`: `(mtime_ns, size)` of the file, `None` when absent.
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

    /// Python `clear`: remove the file. With a `tool_use_id` the file is only
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

    /// Python `settle`: mark the native dialog finished (`submitted` /
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

    /// Python `handle`: one hook payload from Claude's stdin.
    pub fn handle(&self, data: &Value) {
        let session_id = data.get("session_id").map(text).unwrap_or_default();
        let Some(path) = self.path(&session_id) else {
            return;
        };
        let event = data.get("hook_event_name").map(text).unwrap_or_default();
        let tool_use_id = data.get("tool_use_id").map(text).unwrap_or_default();
        match event.as_str() {
            "PreToolUse"
                if data.get("tool_name").map(text).as_deref() == Some("AskUserQuestion") =>
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

/// Python `settings_path`: the hooks-only settings document Claude receives
/// through `--settings`. `command` is the absolute `sessiondock` binary and
/// `args` the exec-form subcommand (`claude-hook --state-dir DIR`), so the
/// hook does not depend on the CLI's cleared environment or on Python.
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
/// output is the caller's contract); the return value is for tests only.
pub fn run_hook(state_dir: &Path, input: &mut dyn io::Read) -> bool {
    use std::io::Read;
    let mut data = Vec::new();
    if Read::take(input, HOOK_INPUT_LIMIT)
        .read_to_end(&mut data)
        .is_err()
    {
        return false;
    }
    let Ok(value) = serde_json::from_slice::<Value>(&data) else {
        return false;
    };
    if !value.is_object() {
        return false;
    }
    PromptStore::new(state_dir).handle(&value);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(session: &str, event: &str, tool: &str, input: Value) -> Value {
        json!({
            "hook_event_name": event, "session_id": session,
            "tool_name": "AskUserQuestion", "tool_use_id": tool,
            "tool_input": input,
        })
    }

    #[test]
    fn question_hook_records_structure_and_matching_result_settles_it() {
        let temp = tempfile::tempdir().unwrap();
        let store = PromptStore::new(temp.path());
        let (sid, tool) = ("session-123", "toolu-question");
        store.handle(&hook(
            sid,
            "PreToolUse",
            tool,
            json!({"questions": [{
                "header": "颜色", "question": "选哪个？", "multiSelect": false,
                "options": [{"label": "红", "description": "暖色"}, "蓝"],
            }]}),
        ));
        let prompt = store.prompt(sid).unwrap();
        assert_eq!(prompt["id"], tool);
        assert_eq!(prompt["state"], "waiting");
        assert_eq!(prompt["version"], 1);
        assert_eq!(prompt["source"], "claude");
        assert!(prompt["created"].as_i64().unwrap() > 0);
        assert_eq!(
            prompt["questions"],
            json!([{
                "header": "颜色", "question": "选哪个？", "multiple": false,
                "options": [{"label": "红", "description": "暖色"},
                            {"label": "蓝", "description": ""}],
            }])
        );
        assert!(store.revision(sid).is_some());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.path(sid).unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
            let dir = fs::metadata(store.directory())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(dir & 0o777, 0o700);
        }

        store.handle(
            &json!({"hook_event_name": "PostToolUse", "session_id": sid, "tool_use_id": "other"}),
        );
        assert_eq!(store.prompt(sid).unwrap()["state"], "waiting");
        store.handle(&json!({"hook_event_name": "PostToolUseFailure", "session_id": sid, "tool_use_id": tool}));
        let cancelled = store.prompt(sid).unwrap();
        assert_eq!(cancelled["state"], "cancelled");
        assert!(cancelled["settled"].as_i64().unwrap() > 0);
        assert!(!store.clear(sid, "another-tool"));
        assert!(store.clear(sid, tool));
        assert!(store.prompt(sid).is_none());
        assert!(store.revision(sid).is_none());

        store.handle(&hook(
            sid,
            "PreToolUse",
            tool,
            json!({"questions": [{"question": "继续吗？"}]}),
        ));
        store.handle(
            &json!({"hook_event_name": "PostToolUse", "session_id": sid, "tool_use_id": tool}),
        );
        assert_eq!(store.prompt(sid).unwrap()["state"], "submitted");
        assert!(store.clear(sid, ""));
    }

    #[test]
    fn session_start_and_end_clear_a_stale_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let store = PromptStore::new(temp.path());
        for event in ["SessionStart", "SessionEnd"] {
            store.handle(&hook(
                "session-456",
                "PreToolUse",
                "ask",
                json!({"questions": [{"question": "还在吗？"}]}),
            ));
            assert!(store.prompt("session-456").is_some());
            store.handle(&json!({"hook_event_name": event, "session_id": "session-456"}));
            assert!(store.prompt("session-456").is_none());
        }
    }

    #[test]
    fn invalid_ids_events_and_shapes_write_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let store = PromptStore::new(temp.path());
        // Path-like or short ids never name a file.
        for bad in ["../x", "ab", "a/b", "", "x".repeat(129).as_str()] {
            store.handle(&hook(
                bad,
                "PreToolUse",
                "t",
                json!({"questions": [{"question": "q"}]}),
            ));
            assert!(store.path(bad).is_none());
        }
        // Another tool's PreToolUse and a question-less input are ignored.
        let mut other = hook(
            "session-789",
            "PreToolUse",
            "t",
            json!({"questions": [{"question": "q"}]}),
        );
        other["tool_name"] = json!("Bash");
        store.handle(&other);
        store.handle(&hook(
            "session-789",
            "PreToolUse",
            "t",
            json!({"questions": [{"header": "no question"}]}),
        ));
        store.handle(&hook(
            "session-789",
            "PreToolUse",
            "t",
            json!({"questions": "nope"}),
        ));
        assert!(
            !store.directory().exists()
                || fs::read_dir(store.directory()).unwrap().next().is_none()
        );
        // Settle without a file, or with a mismatching id, is false.
        assert!(!store.settle("session-789", "t", "submitted"));
        store.handle(&hook(
            "session-789",
            "PreToolUse",
            "t",
            json!({"questions": {"question": "single object row"}}),
        ));
        assert!(!store.settle("session-789", "u", "submitted"));
        assert!(!store.settle("session-789", "t", "weird"));
        assert_eq!(
            store.prompt("session-789").unwrap()["questions"][0]["question"],
            "single object row"
        );
        // A file of another version or without questions is not a prompt.
        fs::write(
            store.path("session-789").unwrap(),
            r#"{"version": 2, "questions": [{"question": "q"}]}"#,
        )
        .unwrap();
        assert!(store.prompt("session-789").is_none());
        fs::write(store.path("session-789").unwrap(), "not json").unwrap();
        assert!(store.prompt("session-789").is_none());
        assert!(store.revision("session-789").is_some());
    }

    #[test]
    fn unchanged_content_keeps_the_file_identity() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("p.json");
        atomic_json(&path, &json!({"a": 1})).unwrap();
        let first = fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        atomic_json(&path, &json!({"a": 1})).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), first);
        atomic_json(&path, &json!({"a": 2})).unwrap();
        assert_ne!(fs::metadata(&path).unwrap().modified().unwrap(), first);
        assert!(
            fs::read_dir(temp.path()).unwrap().count() == 1,
            "no temp file left"
        );
    }

    #[test]
    fn settings_are_passive_exec_form_hooks() {
        let value = settings(
            Path::new("/opt/x/bin/sessiondock"),
            Path::new("/var/lib/x/state"),
        );
        let hooks = &value["hooks"];
        for event in ["PreToolUse", "PostToolUse", "PostToolUseFailure"] {
            assert_eq!(hooks[event][0]["matcher"], "AskUserQuestion");
            assert_eq!(hooks[event][0]["hooks"][0]["type"], "command");
            assert_eq!(
                hooks[event][0]["hooks"][0]["command"],
                "/opt/x/bin/sessiondock"
            );
            assert_eq!(
                hooks[event][0]["hooks"][0]["args"],
                json!(["claude-hook", "--state-dir", "/var/lib/x/state"])
            );
        }
        for event in ["SessionStart", "SessionEnd"] {
            assert!(hooks[event][0].get("matcher").is_none());
            assert_eq!(hooks[event][0]["hooks"][0]["args"][0], "claude-hook");
        }
        assert!(!value.to_string().contains("permissionDecision"));
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bridge.json");
        write_settings(
            &path,
            Path::new("/opt/x/bin/sessiondock"),
            Path::new("/var/lib/x/state"),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
            value
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn run_hook_reads_stdin_and_tolerates_garbage() {
        let temp = tempfile::tempdir().unwrap();
        let payload = hook(
            "session-stdin",
            "PreToolUse",
            "t1",
            json!({"questions": [{"question": "q"}]}),
        )
        .to_string();
        assert!(run_hook(temp.path(), &mut payload.as_bytes()));
        assert!(
            PromptStore::new(temp.path())
                .prompt("session-stdin")
                .is_some()
        );
        assert!(!run_hook(temp.path(), &mut "not json".as_bytes()));
        assert!(!run_hook(temp.path(), &mut "[1,2]".as_bytes()));
        assert!(!run_hook(temp.path(), &mut "".as_bytes()));
    }
}
