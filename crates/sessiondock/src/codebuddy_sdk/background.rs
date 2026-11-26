//! CodeBuddy stream-json shell tasks and the native per-session log directory.
use super::*;

pub(super) fn task_directory(
    profile: &CliProfile,
    transcript: &Path,
    sid: &str,
) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        if transcript.file_stem()?.to_str()? != sid {
            return None;
        }
        let project = transcript.parent()?.file_name()?;
        let temp = ["TMPDIR", "TMP", "TEMP"]
            .into_iter()
            .filter_map(|key| {
                profile.env.get(key).map(PathBuf::from).or_else(|| {
                    (!profile.env_remove.iter().any(|name| name == key))
                        .then(|| std::env::var_os(key).map(PathBuf::from))
                        .flatten()
                })
            })
            .find(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        if !temp.is_absolute() {
            return None;
        }
        // This is the native directory's OS user tag, never task ownership.
        let user = unsafe { libc::getuid() };
        let temp = temp.canonicalize().ok()?;
        Some(
            temp.join(format!("codebuddy-{user}"))
                .join(project)
                .join(sid)
                .join("bg-tasks"),
        )
    }
    #[cfg(not(unix))]
    {
        let _ = (profile, transcript, sid);
        None
    }
}

pub(super) fn observe(tasks: &mut Vec<Value>, directory: Option<&Path>, frame: &Value) {
    let Some(id) = frame["task_id"].as_str().filter(|id| !id.is_empty()) else {
        return;
    };
    match frame["subtype"].as_str() {
        Some("task_started")
            if matches!(
                frame["task_type"].as_str(),
                Some("Bash" | "PowerShell" | "Monitor")
            ) =>
        {
            let logged = directory
                .and_then(|root| paths(root, id))
                .is_some_and(|paths| {
                    paths.iter().any(|path| {
                        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file())
                    })
                });
            tasks.retain(|task| task["task_id"] != id);
            tasks.push(json!({"task_id":id,"tool_use_id":frame["tool_use_id"],
                "task_type":frame["task_type"],"description":frame["description"],
                "output_available":logged}));
        }
        Some("task_updated")
            if matches!(
                frame["patch"]["status"].as_str(),
                Some("completed" | "failed" | "killed")
            ) =>
        {
            tasks.retain(|task| task["task_id"] != id);
        }
        Some("task_notification")
            if matches!(
                frame["status"].as_str(),
                Some("completed" | "failed" | "stopped")
            ) =>
        {
            tasks.retain(|task| task["task_id"] != id);
        }
        _ => {}
    }
}

fn command(task: &Value, items: &[Value]) -> Option<Value> {
    let id = task["task_id"].as_str()?;
    let call_id = task["tool_use_id"].as_str().filter(|id| !id.is_empty())?;
    let call = items.iter().find(|row| {
        row["call_id"] == call_id
            && row["name"] == task["task_type"]
            && row.get("parent_tool_use_id").is_some_and(Value::is_null)
    })?;
    let mut entry = json!({"task":id});
    if let Some(input) = call["input"]["command"]
        .as_str()
        .filter(|text| !text.is_empty())
    {
        entry["command"] = json!(input);
    }
    if let Some(description) = task["description"].as_str().filter(|text| !text.is_empty()) {
        entry["description"] = json!(description);
    }
    if task["output_available"] == true {
        entry["output_available"] = json!(true);
    }
    Some(entry)
}

pub(super) fn commands(tasks: &[Value], items: &[Value]) -> Vec<Value> {
    tasks
        .iter()
        .filter_map(|task| command(task, items))
        .collect()
}

fn paths(directory: &Path, task: &str) -> Option<[PathBuf; 2]> {
    if task.is_empty()
        || !task
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    Some([
        directory.join(format!("{task}.stdout.log")),
        directory.join(format!("{task}.stderr.log")),
    ])
}

impl Bridge {
    /// Read a single task of a runtime already observed by this bridge.
    /// The transport has resolved the selected main session before calling this.
    pub(super) fn background_output(&self, uid: &str, sid: &str, task: &str) -> Option<Value> {
        let session = lock(&self.state).sessions.get(uid).cloned()?;
        if session.sid != sid {
            return None;
        }
        let directory = session.background_dir.as_deref()?;
        {
            let live = lock(&session.live);
            if !live.ready || live.outbound.is_none() {
                return None;
            }
            let task = live
                .background_tasks
                .iter()
                .find(|entry| entry["task_id"] == task)?;
            if command(task, &live.items)?["output_available"] != true {
                return None;
            }
        }

        const LIMIT: usize = 64 * 1024;
        const STDERR: &str = "\n\nstderr:\n";
        let [stdout_path, stderr_path] = paths(directory, task)?;
        let read = |path: &Path| crate::files::native_log_tail(path, LIMIT).ok();
        let stdout_read = read(&stdout_path);
        let stderr_read = read(&stderr_path);
        if stdout_read.is_none() && stderr_read.is_none() {
            return None;
        }
        let partial = stdout_read.is_none() || stderr_read.is_none();
        let (stdout, stdout_clipped) = stdout_read.unwrap_or_default();
        let (stderr, stderr_clipped) = stderr_read.unwrap_or_default();
        let both = !stdout.is_empty() && !stderr.is_empty();
        let mut stdout = String::from_utf8_lossy(&stdout);
        let mut stderr = String::from_utf8_lossy(&stderr);
        let limit = if both { LIMIT - STDERR.len() } else { LIMIT };
        let split = limit / 2;
        let stdout_limit = limit - stderr.len().min(split);
        let stderr_limit = limit - stdout.len().min(split);
        let clipped = stdout_clipped || stderr_clipped || stdout.len() + stderr.len() > limit;
        stdout = tail(stdout, stdout_limit);
        stderr = tail(stderr, stderr_limit);
        let mut result = json!({"output":format!("{stdout}{}{stderr}", if both { STDERR } else { "" }),
            "clipped":clipped});
        if both || partial || clipped {
            let mut notes = Vec::new();
            if both {
                notes.push("原生 stdout 和 stderr 分别保存，无法按时间交错排列");
            }
            if partial {
                notes.push("其中一个输出流当前不可读");
            }
            if clipped {
                notes.push("仅显示最近的可读内容，此视图无法补读省略的开头");
            }
            result["note"] = json!(notes.join("。"));
        }
        Some(result)
    }
}

fn tail(text: std::borrow::Cow<'_, str>, limit: usize) -> std::borrow::Cow<'_, str> {
    let start = text.len().saturating_sub(limit);
    let start = text.ceil_char_boundary(start);
    match text {
        std::borrow::Cow::Borrowed(text) => std::borrow::Cow::Borrowed(&text[start..]),
        std::borrow::Cow::Owned(mut text) => {
            text.drain(..start);
            std::borrow::Cow::Owned(text)
        }
    }
}
