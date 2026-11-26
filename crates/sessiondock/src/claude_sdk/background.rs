//! Claude's native task stream. A task registry entry carries membership,
//! while the matching top-level Bash/Monitor call establishes ownership.
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) fn observe(
    tasks: &mut Vec<Value>,
    starts: &mut BTreeMap<String, Value>,
    level_seen: &mut bool,
    frame: &Value,
) {
    if frame["subtype"] == "background_tasks_changed" {
        let Some(level) = frame["tasks"].as_array() else {
            return;
        };
        *tasks = level
            .iter()
            .filter(|task| task["task_id"].as_str().is_some_and(|id| !id.is_empty()))
            .map(|task| {
                let mut next = task.clone();
                if let Some(prior) = task["task_id"]
                    .as_str()
                    .and_then(|id| starts.get(id))
                    .filter(|prior| same_run(prior, task))
                    .or_else(|| tasks.iter().find(|prior| same_run(prior, task)))
                {
                    if let Some(call) = prior.get("tool_use_id").filter(|value| value.is_string()) {
                        next["tool_use_id"] = call.clone();
                    }
                    // A level can omit a parent Claude no longer tracks.
                    if next["parent_task_id"].is_null() && prior["parent_task_id"].is_string() {
                        next["parent_task_id"] = prior["parent_task_id"].clone();
                    }
                }
                next["is_backgrounded"] = json!(true);
                next
            })
            .collect();
        *level_seen = true;
        return;
    }
    let Some(id) = frame["task_id"].as_str().filter(|id| !id.is_empty()) else {
        return;
    };
    match frame["subtype"].as_str() {
        Some("task_started") => {
            if starts.get(id).is_some_and(|prior| {
                matches!((prior["run_id"].as_str(), frame["run_id"].as_str()),
                    (Some(prior), Some(next)) if prior > next)
            }) {
                return;
            }
            starts.insert(id.to_owned(), frame.clone());
            if let Some(task) = tasks.iter_mut().find(|task| same_run(task, frame)) {
                if let Some(call) = frame.get("tool_use_id").filter(|value| value.is_string()) {
                    task["tool_use_id"] = call.clone();
                }
                if let Some(parent) = frame
                    .get("parent_task_id")
                    .filter(|value| value.is_string())
                {
                    task["parent_task_id"] = parent.clone();
                }
            } else if !*level_seen {
                tasks.retain(|task| task["task_id"] != frame["task_id"]);
                tasks.push(frame.clone());
            }
        }
        Some("task_updated") => {
            let patch = &frame["patch"];
            if matches!(
                patch["status"].as_str(),
                Some("completed" | "failed" | "killed")
            ) {
                tasks.retain(|task| !same_run(task, frame));
                if starts.get(id).is_some_and(|start| same_run(start, frame)) {
                    starts.remove(id);
                }
            } else if let Some(task) = tasks.iter_mut().find(|task| same_run(task, frame)) {
                for key in ["description", "is_backgrounded"] {
                    if let Some(value) = patch.get(key) {
                        task[key] = value.clone();
                    }
                }
            }
        }
        Some("task_notification")
            if matches!(
                frame["status"].as_str(),
                Some("completed" | "failed" | "stopped")
            ) =>
        {
            tasks.retain(|task| !same_run(task, frame));
            if starts.get(id).is_some_and(|start| same_run(start, frame)) {
                starts.remove(id);
            }
        }
        _ => {}
    }
}

fn same_run(task: &Value, event: &Value) -> bool {
    task["task_id"] == event["task_id"]
        && match (task["run_id"].as_str(), event["run_id"].as_str()) {
            (Some(task), Some(event)) => task == event,
            _ => true,
        }
}

pub(super) fn commands(tasks: &[Value], items: &[Value]) -> Vec<Value> {
    tasks
        .iter()
        .filter_map(|task| {
            if task["task_type"] != "local_bash"
                || task["is_backgrounded"] != true
                || task["ambient"] == true
                || task["awaited"] == true
                || task["off_host"] == true
                || !task["parent_task_id"].is_null()
            {
                return None;
            }
            let id = task["task_id"].as_str()?;
            let call = items.iter().find(|row| {
                row.get("parent_tool_use_id").is_some_and(Value::is_null)
                    && matches!(row["name"].as_str(), Some("Bash" | "Monitor"))
                    && (row["result"]["background_task_id"] == id
                        || (task["tool_use_id"].is_string()
                            && row["call_id"] == task["tool_use_id"]))
            })?;
            let mut command = json!({"task": id});
            if let Some(input) = call["input"]["command"].as_str().filter(|s| !s.is_empty()) {
                command["command"] = json!(input);
            }
            if let Some(description) = task["description"].as_str().filter(|s| !s.is_empty()) {
                command["description"] = json!(description);
            }
            Some(command)
        })
        .collect()
}

/// The stream can drop internal tool result fields. The exact native Bash
/// notice carries the task ID in older versions too.
pub(super) fn result_task<'a>(result: &'a Value, body: &'a str) -> Option<&'a str> {
    result["backgroundTaskId"].as_str().or_else(|| {
        let (notice, _) = body.rsplit_once(". Output is being written to: ")?;
        let line = notice.lines().next_back()?;
        if let Some(id) = line
            .strip_prefix("Command running in background with ID: ")
            .or_else(|| line.strip_prefix("Command was manually backgrounded by user with ID: "))
        {
            return Some(id);
        }
        if line.starts_with("Command was moved to the background (ID: ")
            || line.starts_with("Command did not complete within ")
        {
            return line
                .split_once("(ID: ")?
                .1
                .split_once(')')
                .map(|(id, _)| id);
        }
        None
    })
}

pub(super) fn monitor_task<'a>(result: &'a Value, body: &'a str) -> Option<&'a str> {
    result["taskId"].as_str().or_else(|| {
        body.strip_prefix("Monitor started (task ")?
            .split_once(", ")
            .map(|(id, _)| id)
    })
}
