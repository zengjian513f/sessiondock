//! Session-local background work recovered from attributed Claude records.
use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};

use super::{Lineage, graph_id, normalized, tag_value, truthy};

pub(in crate::sessions::providers) fn background_metadata(
    path: &Path,
    records: &[(Value, u64)],
    agent: &str,
    branch: &Lineage,
) -> Value {
    let owner = if agent.is_empty() {
        path.file_stem()
    } else {
        path.parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
    }
    .and_then(std::ffi::OsStr::to_str);
    let mut calls = BTreeMap::new();
    let mut commands = BTreeMap::new();
    let mut timers = BTreeMap::new();
    let mut wakeup: Option<Value> = None;
    for (record, _) in records {
        let kind = record["type"].as_str();
        let queue = kind == Some("queue-operation");
        let on_branch = graph_id(record, agent).is_none_or(|id| {
            branch
                .active
                .as_ref()
                .is_none_or(|active| active.contains(id))
        });
        if !on_branch || !owned(record, owner, agent, queue) {
            continue;
        }
        match kind {
            Some("assistant") => {
                for part in parts(record).filter(|part| part["type"] == "tool_use") {
                    if matches!(
                        part["name"].as_str(),
                        Some(
                            "Bash"
                                | "Monitor"
                                | "TaskStop"
                                | "CronCreate"
                                | "CronDelete"
                                | "ScheduleWakeup"
                        )
                    ) && let Some(call) = part["id"].as_str()
                    {
                        calls.insert(call, part);
                    }
                }
            }
            Some("user") => {
                for part in parts(record)
                    .filter(|part| part["type"] == "tool_result" && part["is_error"] != true)
                {
                    let Some(call) = part["tool_use_id"].as_str().and_then(|id| calls.get(id))
                    else {
                        continue;
                    };
                    let result = &record["toolUseResult"];
                    match call["name"].as_str() {
                        Some(name @ ("Bash" | "Monitor")) => {
                            if let Some(command) = command(name, &call["input"], result, part) {
                                commands
                                    .insert(command["task"].as_str().unwrap().to_owned(), command);
                            }
                        }
                        Some("TaskStop") => {
                            if let Some(task) = result["task_id"].as_str() {
                                commands.remove(task);
                            }
                        }
                        Some("CronCreate")
                            if result["durable"] == false
                                || (result["durable"].is_null()
                                    && call["input"]["durable"] != true) =>
                        {
                            if let (Some(id), Some(prompt), Some(cron)) = (
                                result["id"].as_str(),
                                call["input"]["prompt"].as_str(),
                                call["input"]["cron"].as_str(),
                            ) {
                                let mut timer = json!({"task":id,"prompt":prompt,"schedule":cron});
                                if let Some(at) = normalized(&record["timestamp"]).as_str() {
                                    timer["created_at"] = json!(at);
                                }
                                if let Some(at) = epoch(&result["nextFireAt"]) {
                                    timer["due_at"] = json!(at);
                                }
                                timers.insert(id.to_owned(), (timer, result["recurring"] != false));
                            }
                        }
                        Some("CronDelete") => {
                            if let Some(task) = result["id"].as_str() {
                                timers.remove(task);
                            }
                        }
                        Some("ScheduleWakeup") if result["stopped"] == true => wakeup = None,
                        Some("ScheduleWakeup") if agent.is_empty() => {
                            if let (Some(task), Some(prompt), Some(at)) = (
                                part["tool_use_id"].as_str(),
                                call["input"]["prompt"].as_str(),
                                epoch(&result["scheduledFor"]),
                            ) {
                                let mut timer = json!({"task":task,"prompt":prompt,"due_at":at});
                                if let Some(at) = normalized(&record["timestamp"]).as_str() {
                                    timer["created_at"] = json!(at);
                                }
                                if let Some(delay) = result["clampedDelaySeconds"].as_u64() {
                                    timer["delay"] = json!(format!("{delay}s"));
                                }
                                wakeup = Some(timer);
                            }
                        }
                        _ => {}
                    }
                }
                if record["origin"]["kind"] == "task-notification" || record["isMeta"] == true {
                    if let Some(text) = record["message"]["content"].as_str() {
                        end_commands(&mut commands, text);
                    }
                    for text in parts(record).filter_map(|part| part["text"].as_str()) {
                        end_commands(&mut commands, text);
                    }
                }
            }
            Some("queue-operation") => {
                if let Some(text) = record["content"].as_str() {
                    end_commands(&mut commands, text);
                }
            }
            Some("system") if record["subtype"] == "scheduled_task_fire" => {
                if record["taskKind"] == "loop" {
                    wakeup = None;
                } else if let Some(task) = record["taskId"].as_str()
                    && let Some((timer, recurring)) = timers.get_mut(task)
                {
                    if *recurring {
                        timer.as_object_mut().unwrap().remove("due_at");
                    } else {
                        timers.remove(task);
                    }
                }
            }
            Some("attachment") if record["attachment"]["type"] == "session_cron_carry" => {
                let carry = &record["attachment"];
                if carry["wakeup"].is_null() {
                    wakeup = None;
                }
                for task in carry["ended"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    timers.remove(task);
                }
                for task in carry["tasks"].as_array().into_iter().flatten() {
                    if let (Some(id), Some(prompt), Some(cron)) = (
                        task["id"].as_str(),
                        task["prompt"].as_str(),
                        task["cron"].as_str(),
                    ) {
                        let (timer, _) = timers.entry(id.to_owned()).or_insert_with(|| {
                            (
                                json!({"task":id,"prompt":prompt,"schedule":cron}),
                                task["recurring"] == true,
                            )
                        });
                        if let Some(at) = epoch(&task["createdAt"]) {
                            timer["created_at"] = json!(at);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    json!({
        "background_commands": commands.into_values().collect::<Vec<_>>(),
        "background_timers": timers.into_values().map(|(timer, _)| timer).chain(wakeup).collect::<Vec<_>>()
    })
}

fn owned(record: &Value, owner: Option<&str>, agent: &str, queue: bool) -> bool {
    match (owner, record["sessionId"].as_str()) {
        (Some(want), Some(actual)) if want == actual => {}
        (Some(_), None) if queue && agent.is_empty() => {}
        _ => return false,
    }
    if agent.is_empty() {
        !truthy(&record["isSidechain"]) && record["agentId"].is_null()
    } else {
        match record["agentId"].as_str() {
            Some(id) => id == agent,
            None => record["isSidechain"] == true,
        }
    }
}

fn parts(record: &Value) -> impl Iterator<Item = &Value> {
    record["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
}

fn epoch(value: &Value) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(value.as_i64()?).map(|at| at.to_rfc3339())
}

fn command(name: &str, input: &Value, native: &Value, result: &Value) -> Option<Value> {
    let task = native[if name == "Bash" {
        "backgroundTaskId"
    } else {
        "taskId"
    }]
    .as_str()
    .filter(|id| !id.is_empty())?;
    let mut command = json!({"task":task});
    for key in ["command", "description"] {
        if let Some(text) = input[key].as_str().filter(|s| !s.is_empty()) {
            command[key] = json!(text);
        }
    }
    // The native Bash result's display suffix already grants this specific
    // file through the selected view's usual tool-result references.
    if name == "Bash"
        && let Some(path) =
            result_text(&result["content"]).find_map(|text| bash_output_ref(task, text))
        && native["backgroundOutputPath"]
            .as_str()
            .is_none_or(|typed| typed == path)
    {
        command["ref"] = json!(path);
    }
    Some(command)
}

fn result_text(content: &Value) -> impl Iterator<Item = &str> {
    content.as_str().into_iter().chain(
        content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str()),
    )
}

fn bash_output_ref<'a>(task: &str, text: &'a str) -> Option<&'a str> {
    let (_, output) = text.rsplit_once(". Output is being written to: ")?;
    let suffix = format!("/{task}.output");
    let path = output.split_whitespace().next()?.strip_suffix('.')?;
    if !path.ends_with(&suffix) || !Path::new(path).is_absolute() {
        return None;
    }
    Some(path)
}

fn end_commands(commands: &mut BTreeMap<String, Value>, text: &str) {
    for block in text.split("<task-notification>").skip(1) {
        let Some((notice, _)) = block.split_once("</task-notification>") else {
            continue;
        };
        let Some(task) = tag_value(notice, "task-id") else {
            continue;
        };
        if tag_value(notice, "status")
            .as_deref()
            .is_some_and(|status| matches!(status, "completed" | "failed" | "stopped" | "killed"))
            || tag_value(notice, "event").is_some_and(|event| event.starts_with("[Monitor expired"))
        {
            commands.remove(&task);
        }
    }
}
