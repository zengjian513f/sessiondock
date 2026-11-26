//! Session-scoped Grok ACP background work.
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Background {
    commands: BTreeMap<String, Command>,
    finished: BTreeSet<String>,
    timers: BTreeMap<String, Value>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Command {
    tool: Option<String>,
    command: Option<String>,
    description: Option<String>,
    output: Option<String>,
    clipped: bool,
}

fn nonempty(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !s.trim().is_empty())
}

pub(super) fn task_id(value: &Value) -> Option<String> {
    nonempty(&value["task_id"])
        .or_else(|| nonempty(&value["id"]))
        .map(str::to_owned)
        .or_else(|| value["task_id"].as_u64().map(|id| id.to_string()))
}

pub(super) fn finished(value: &Value) -> bool {
    matches!(
        value["status"].as_str(),
        Some("completed" | "failed" | "cancelled" | "exited")
    )
}

fn tail(text: &str) -> &str {
    let mut start = text.len().saturating_sub(64 * 1024);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn output_body(text: &str, clipped: bool) -> Value {
    let kept = tail(text);
    let clipped = clipped || kept.len() < text.len();
    let mut result = json!({"output":kept,"clipped":clipped});
    if clipped {
        result["note"] = json!("只显示 Grok 提供的最近输出，无法补读更早的输出");
    }
    result
}

pub(super) fn queried_output(snapshot: &Value, sid: &str, wanted: &str) -> Option<Value> {
    let result = snapshot.get("result").unwrap_or(snapshot);
    let task = result["tasks"].as_array()?.iter().find(|task| {
        task_id(task).as_deref() == Some(wanted)
            && task["is_backgrounded"] == true
            && task["owner_session_id"] == sid
            && matches!(task["kind"].as_str(), None | Some("bash" | "monitor"))
    })?;
    if task["completed"] == true {
        return Some(Value::Null);
    }
    let output = task["output"].as_str()?;
    Some(output_body(
        output,
        task["truncated"] == true
            || task["output_total_bytes"]
                .as_u64()
                .is_some_and(|size| size > output.len() as u64),
    ))
}

impl Background {
    pub fn commands(&self, live: bool) -> Vec<Value> {
        self.commands
            .iter()
            .map(|(id, command)| {
                let mut row = json!({"task":id});
                if let Some(text) = &command.command {
                    row["command"] = json!(text);
                }
                if let Some(text) = &command.description {
                    row["description"] = json!(text);
                }
                if live {
                    row["output_available"] = json!(true);
                }
                row
            })
            .collect()
    }

    pub fn timers(&self) -> Vec<&Value> {
        self.timers.values().collect()
    }

    pub fn contains(&self, task: &str) -> bool {
        self.commands.contains_key(task)
    }

    pub fn has_tasks(&self) -> bool {
        !self.commands.is_empty() || !self.timers.is_empty()
    }

    pub fn is_finished(&self, task: &str) -> bool {
        self.finished.contains(task)
    }

    pub fn output(&self, task: &str) -> Option<Value> {
        let command = self.commands.get(task)?;
        Some(output_body(
            command.output.as_deref().unwrap_or_default(),
            command.clipped,
        ))
    }

    pub fn update(&mut self, update: &Value, sid: &str) -> bool {
        match update["sessionUpdate"].as_str() {
            Some("task_backgrounded") => {
                if let Some(id) = task_id(update)
                    && !self.finished.contains(&id)
                    && update["owner_session_id"]
                        .as_str()
                        .is_none_or(|owner| owner == sid)
                {
                    let command = self.commands.entry(id).or_default();
                    command.update_label(update);
                    if let Some(tool) = nonempty(&update["tool_call_id"]) {
                        command.tool = Some(tool.to_owned());
                    }
                }
            }
            Some("background_tasks") => {
                if let Some(tasks) = update["tasks"].as_array() {
                    let mut present = BTreeSet::new();
                    for task in tasks {
                        let Some(id) = task_id(task) else { continue };
                        if task["owner_session_id"]
                            .as_str()
                            .is_some_and(|owner| owner != sid)
                            || !matches!(task["kind"].as_str(), None | Some("bash" | "monitor"))
                        {
                            continue;
                        }
                        if finished(task) {
                            self.finish(id);
                            continue;
                        }
                        // Grok's registry includes unowned rows in its list.
                        // A session event or explicit owner proves membership.
                        if !self.finished.contains(&id)
                            && (self.commands.contains_key(&id) || task["owner_session_id"] == sid)
                        {
                            self.commands
                                .entry(id.clone())
                                .or_default()
                                .update_label(task);
                            present.insert(id);
                        }
                    }
                    if update["truncated"] != true {
                        self.commands.retain(|id, _| present.contains(id));
                    }
                }
            }
            Some("task_completed") => {
                let snapshot = &update["task_snapshot"];
                if snapshot["owner_session_id"]
                    .as_str()
                    .is_none_or(|owner| owner == sid)
                    && let Some(id) = task_id(snapshot).or_else(|| task_id(update))
                {
                    self.finish(id);
                }
            }
            Some("scheduled_task_created" | "scheduled_task_fired") => {
                self.update_timer(update);
            }
            Some("scheduled_task_deleted") => {
                if let Some(id) = task_id(update) {
                    self.timers.remove(&id);
                }
            }
            Some("monitor_event") => {
                if let Some(id) = task_id(update)
                    && let Some(command) = self.commands.get_mut(&id)
                    && let Some(text) = update["event_text"].as_str()
                {
                    let mut next = command.output.take().unwrap_or_default();
                    next.push_str(text);
                    command.set_output(&next, false);
                }
            }
            _ => return false,
        }
        true
    }

    fn finish(&mut self, id: String) {
        self.commands.remove(&id);
        self.finished.insert(id);
    }

    fn update_timer(&mut self, update: &Value) {
        let Some(id) = task_id(update) else { return };
        let due = update["next_fire_at"].as_str().and_then(|value| {
            chrono::DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|time| time.to_utc().to_rfc3339())
        });
        if update["sessionUpdate"] == "scheduled_task_fired"
            && update.get("next_fire_at").is_some_and(Value::is_null)
        {
            self.timers.remove(&id);
            return;
        }
        let Some(prompt) = nonempty(&update["prompt"]) else {
            return;
        };
        let mut timer = json!({"task":id,"prompt":prompt});
        if let Some(schedule) = nonempty(&update["human_schedule"]) {
            timer["schedule"] = json!(schedule);
        }
        if let Some(due) = due {
            timer["due_at"] = json!(due);
        }
        self.timers.insert(id, timer);
    }

    pub fn observe_shell_output(&mut self, update: &Value) -> bool {
        let output = &update["rawOutput"];
        if output["type"] != "Bash" || update["status"] != "in_progress" {
            return false;
        }
        let Some(tool) = nonempty(&update["toolCallId"]) else {
            return false;
        };
        let Some(command) = self
            .commands
            .values_mut()
            .find(|cmd| cmd.tool.as_deref() == Some(tool))
        else {
            return false;
        };
        let text = output["output_for_prompt"].as_str();
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            command.set_output(text, output["truncated"] == true);
        } else if let Some(bytes) = output["output"].as_array()
            && !bytes.is_empty()
        {
            let bytes: Vec<_> = bytes
                .iter()
                .filter_map(|byte| u8::try_from(byte.as_u64()?).ok())
                .collect();
            command.set_output(
                &String::from_utf8_lossy(&bytes),
                output["truncated"] == true,
            );
        } else if output["truncated"] == true {
            command.clipped = true;
        }
        true
    }
}

impl Command {
    fn update_label(&mut self, row: &Value) {
        if let Some(command) =
            nonempty(&row["display_command"]).or_else(|| nonempty(&row["command"]))
        {
            self.command = Some(command.to_owned());
        }
        if let Some(description) =
            nonempty(&row["monitor_description"]).or_else(|| nonempty(&row["description"]))
        {
            self.description = Some(description.to_owned());
        }
    }

    fn set_output(&mut self, text: &str, clipped: bool) {
        let kept = tail(text);
        self.clipped |= clipped || kept.len() < text.len();
        self.output = Some(kept.to_owned());
    }
}

/// The caller supplies the selected session's already resolved native update file.
pub(crate) fn read_background_updates(
    mut reader: impl BufRead,
    sid: &str,
) -> std::io::Result<Value> {
    use serde_json::value::RawValue;

    #[derive(serde::Deserialize)]
    struct Envelope<'a> {
        method: &'a str,
        #[serde(borrow)]
        params: &'a RawValue,
    }

    let mut background = Background::default();
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line)? != 0 {
        if !line.ends_with(b"\n") {
            break;
        }
        if let Ok(frame) = serde_json::from_slice::<Envelope<'_>>(&line)
            && frame.method == "_x.ai/session/update"
            && let Ok(params) = serde_json::from_str::<Value>(frame.params.get())
            && params["sessionId"] == sid
        {
            // Grok timestamps the persistence envelope, including timer
            // reannouncements. Only the advertised next fire is a task time.
            background.update(&params["update"], sid);
        }
        line.clear();
    }
    Ok(
        json!({"background_commands":background.commands(false),"background_timers":background.timers()}),
    )
}
