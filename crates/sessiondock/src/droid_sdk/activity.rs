//! Native notification projection. Activity does not imply ownership or a live turn.
use super::*;

fn label(n: &Value, key: &str) -> String {
    n[key].as_str().unwrap_or("").to_owned()
}
fn state(value: &str) -> &str {
    match value {
        "connected" => "已连接",
        "connecting" => "连接中",
        "failed" | "error" => "失败",
        "disabled" => "已禁用",
        "disconnected" => "未连接",
        "running" | "in_progress" => "运行中",
        "completed" | "success" => "完成",
        "mission_accepted" => "任务已接受",
        "mission_paused" => "任务已暂停",
        "mission_resumed" => "任务已恢复",
        "mission_run_started" => "任务开始",
        "worker_started" => "工作进程开始",
        "worker_completed" => "工作进程完成",
        "worker_failed" => "工作进程失败",
        "worker_paused" => "工作进程暂停",
        "worker_selected_feature" => "已选择功能",
        "paused" => "已暂停",
        "pending" | "queued" => "等待中",
        "settled_after_execution" => "执行完成",
        "settled_without_execution" => "未执行",
        "settled_unknown" => "执行已结束，结果未报告",
        _ => value,
    }
}
fn row(live: &mut Live, key: &str, text: String, n: &Value) {
    let kind = n["type"].as_str().unwrap_or("");
    let label = if kind.starts_with("hook_") {
        "Hook"
    } else if kind.starts_with("mcp_") {
        "MCP"
    } else if kind.starts_with("mission_") {
        "任务"
    } else if kind.starts_with("tool_") {
        "后台任务"
    } else {
        "子代理"
    };
    protocol::upsert(
        live,
        json!({"id":format!("activity:{key}"),"role":"system",
        "text":text,"native_type":label}),
    );
}

pub(super) fn observe(live: &mut Live, n: &Value, sid: Option<&str>, owns_view: bool) {
    let kind = n["type"].as_str().unwrap_or("");
    match kind {
        "child_session_available" => {
            let sid = label(n, "childSessionId");
            live.activity.insert(format!("child:{sid}"), n.clone());
            row(
                live,
                &format!("child:{sid}"),
                format!(
                    "子代理可用 · {} · {}\n{}",
                    label(n, "subagentType"),
                    label(n, "description"),
                    sid
                ),
                n,
            );
        }
        "hook_execution_started" | "hook_execution_completed" => {
            let key = format!("hook:{}", label(n, "hookId"));
            let hidden = n["hiddenFromUserViews"] == true
                || live
                    .activity
                    .get(&key)
                    .is_some_and(|v| v["hiddenFromUserViews"] == true);
            if hidden {
                live.activity.insert(key.clone(), json!({"type":n["type"],"hookId":n["hookId"],"hiddenFromUserViews":true,"hookStatus":n["hookStatus"]}));
                live.items
                    .retain(|item| item["id"] != format!("activity:{key}"));
                return;
            }
            let event = n["hookEventName"]
                .as_str()
                .or_else(|| {
                    live.activity
                        .get(&key)
                        .and_then(|old| old["hookEventName"].as_str())
                })
                .unwrap_or("")
                .to_owned();
            let mut stored = n.clone();
            stored["hookEventName"] = json!(event);
            if let Some(results) = stored["hookResults"].as_array_mut() {
                for result in results {
                    if result["suppressOutput"] == true
                        && let Some(fields) = result.as_object_mut()
                    {
                        fields.remove("stdout");
                        fields.remove("stderr");
                    }
                }
            }
            live.activity.insert(key.clone(), stored);
            let state = if kind == "hook_execution_started" {
                "运行中"
            } else if n["hookStatus"] == "error" {
                "失败"
            } else {
                "完成"
            };
            let results = n["hookResults"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| r["suppressOutput"] != true)
                .map(|r| {
                    format!(
                        "退出码 {}\n{}{}",
                        r["exitCode"],
                        label(r, "stdout"),
                        label(r, "stderr")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            row(
                live,
                &key,
                format!(
                    "Hook {} · {}{}\n{}",
                    event,
                    state,
                    if n["hookPreventedAction"] == true {
                        " · 已阻止操作"
                    } else {
                        ""
                    },
                    results
                ),
                n,
            );
        }
        "mcp_status_changed" => {
            let servers = n["servers"].as_array().into_iter().flatten().map(|s| json!({"name":s["name"],"status":s["status"],"toolCount":s["toolCount"],"error":s["error"]})).collect::<Vec<_>>();
            live.activity.insert(
                "mcp".into(),
                json!({"servers":servers,"summary":n["summary"]}),
            );
            if servers.is_empty() {
                live.items.retain(|item| item["id"] != "activity:mcp");
                return;
            }
            let text = servers
                .iter()
                .map(|s| {
                    format!(
                        "{} · {}{}",
                        label(s, "name"),
                        state(s["status"].as_str().unwrap_or("")),
                        s["error"]
                            .as_str()
                            .map(|e| format!(" · {e}"))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            row(live, "mcp", format!("MCP 状态\n{text}"), n);
        }
        "mcp_auth_required" | "mcp_auth_completed" => {
            let key = format!("mcp-auth:{}", label(n, "serverName"));
            // Authentication state/URLs are not needed to display status.
            row(
                live,
                &key,
                format!(
                    "MCP {} · {}\n{}",
                    label(n, "serverName"),
                    if kind == "mcp_auth_required" {
                        "需要认证"
                    } else {
                        n["outcome"].as_str().unwrap_or("完成")
                    },
                    label(n, "message")
                ),
                n,
            );
        }
        "mission_state_changed"
        | "mission_features_changed"
        | "mission_progress_entry"
        | "mission_heartbeat"
        | "mission_worker_started"
        | "mission_worker_completed" => {
            let key = if kind.starts_with("mission_worker_") {
                format!("mission-worker:{}", label(n, "workerSessionId"))
            } else {
                kind.to_owned()
            };
            live.activity.insert(key.clone(), n.clone());
            let text = match kind {
                "mission_state_changed" => {
                    format!("任务 · {}", state(n["state"].as_str().unwrap_or("")))
                }
                "mission_features_changed" => format!(
                    "任务功能\n{}",
                    n["features"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|f| format!(
                            "{} · {}",
                            label(f, "description"),
                            state(f["status"].as_str().unwrap_or(""))
                        ))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
                "mission_progress_entry" => format!(
                    "任务进度\n{}",
                    n["progressLog"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|e| format!(
                            "{} {} {}",
                            label(e, "timestamp"),
                            state(e["type"].as_str().unwrap_or("")),
                            e["message"].as_str().or(e["title"].as_str()).unwrap_or("")
                        ))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
                "mission_heartbeat" => return,
                "mission_worker_started" => {
                    format!("任务工作进程运行中 · {}", label(n, "workerSessionId"))
                }
                _ => format!(
                    "任务工作进程完成 · {} · 退出码 {}",
                    label(n, "workerSessionId"),
                    n["exitCode"]
                ),
            };
            row(live, &key, text, n);
        }
        "tool_execution_heartbeat" if n["detached"] == true => {
            let (Some(sid), Some(key)) = (sid, tool_id(n)) else {
                return;
            };
            if let Some(old) = live
                .background_tasks
                .iter_mut()
                .find(|v| v["id"] == key && v["session_id"] == sid)
            {
                // A late heartbeat cannot undo an already observed settlement.
                if old["detached"] == true && old["status"] != "running" {
                    return;
                }
                old["name"] = n["toolName"].clone();
                old["status"] = json!("running");
                old["detached"] = json!(true);
            } else {
                live.background_tasks.push(
                    json!({"id":key,"name":n["toolName"],"status":"running","detached":true,"session_id":sid}),
                );
            }
            if owns_view {
                row(
                    live,
                    &format!("background:{key}"),
                    format!("后台任务运行中 · {}", label(n, "toolName")),
                    n,
                );
            }
        }
        "tool_execution_phase_changed" => {
            let (Some(sid), Some(key)) = (sid, tool_id(n)) else {
                return;
            };
            let phase = n["phase"].as_str().unwrap_or_default();
            if owns_view {
                live.activity.insert(format!("tool:{key}"), n.clone());
            }
            if !matches!(
                phase,
                "settled_after_execution" | "settled_without_execution" | "settled_unknown"
            ) {
                return;
            }
            if let Some(index) = live
                .background_tasks
                .iter()
                .position(|v| v["id"] == key && v["session_id"] == sid)
            {
                let task = &mut live.background_tasks[index];
                if task["detached"] != true {
                    live.background_tasks.remove(index);
                    return;
                }
                task["status"] = json!(phase);
                task.as_object_mut().unwrap().remove("output");
                if owns_view {
                    row(
                        live,
                        &format!("background:{key}"),
                        format!("后台任务结束 · {} · {}", label(n, "toolName"), state(phase)),
                        n,
                    );
                }
            }
        }
        "tool_call" if n["toolUse"]["name"] == "Execute" => {
            let tool = &n["toolUse"];
            if let (Some(sid), Some(id), Some(command)) = (
                sid,
                tool["id"].as_str().filter(|id| !id.is_empty()),
                tool["input"]["command"].as_str(),
            ) {
                live.remember_background_command(sid, id, command);
            }
        }
        "create_message" => {
            if let Some(sid) = sid {
                for block in n["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] == "tool_use"
                        && block["name"] == "Execute"
                        && let Some(id) = block["id"].as_str().filter(|id| !id.is_empty())
                        && let Some(command) = block["input"]["command"].as_str()
                    {
                        live.remember_background_command(sid, id, command);
                    }
                }
            }
        }
        "tool_progress_update" => {
            let (Some(sid), Some(key), Some(output)) =
                (sid, tool_id(n), n["update"]["fullOutput"].as_str())
            else {
                return;
            };
            if let Some(task) = live.background_tasks.iter_mut().find(|task| {
                task["id"] == key
                    && task["session_id"] == sid
                    && (task["detached"] != true || task["status"] == "running")
            }) {
                let mut start = output.len().saturating_sub(64 * 1024);
                while !output.is_char_boundary(start) {
                    start += 1;
                }
                let clipped = start != 0;
                let mut body = json!({"output":&output[start..],"clipped":clipped});
                if clipped {
                    body["note"] = json!("只显示 Droid 提供的最近输出，无法补读更早的输出");
                }
                task["output"] = body;
            }
        }
        "llm_retry" => {
            live.retry = json!({"attempt":n["attempt"],"reason":n["reason"]});
        }
        _ => {}
    }
}

fn tool_id(notification: &Value) -> Option<&str> {
    notification["toolUseId"]
        .as_str()
        .filter(|id| !id.is_empty())
}

impl Live {
    fn remember_background_command(&mut self, sid: &str, id: &str, command: &str) {
        if let Some(task) = self
            .background_tasks
            .iter_mut()
            .find(|task| task["session_id"] == sid && task["id"] == id)
        {
            task["command"] = json!(command);
        } else {
            self.background_tasks
                .push(json!({"session_id":sid,"id":id,"command":command}));
        }
    }

    pub(super) fn background_task_statuses(&self, sid: &str) -> Vec<Value> {
        self.background_tasks.iter()
            .filter(|task| task["session_id"] == sid && task["detached"] == true)
            .map(|task| json!({
            "id":task["id"],"name":task["name"],"status":task["status"],"detached":task["detached"]
        })).collect()
    }

    pub(super) fn background_commands(&self, sid: &str) -> Vec<Value> {
        self.background_tasks
            .iter()
            .filter(|task| task["session_id"] == sid && task["status"] == "running")
            .map(|task| {
                let mut command = json!({"task":task["id"]});
                if let Some(name) = task["name"].as_str().filter(|name| !name.is_empty()) {
                    command["description"] = json!(name);
                }
                if let Some(text) = task["command"].as_str() {
                    command["command"] = json!(text);
                }
                if task["output"].is_object() {
                    command["output_available"] = json!(true);
                }
                command
            })
            .collect()
    }
}

impl Bridge {
    fn existing_background_views(&self, target: &Value, sid: &str) -> Vec<Arc<Session>> {
        let Some(owner) = target["uid"].as_str() else {
            return vec![];
        };
        let state = lock(&self.state);
        let existing = |uid: &str| state.sessions.get(uid).cloned();
        if target["agent"]
            .as_str()
            .is_some_and(|agent| !agent.is_empty())
        {
            let Some(native_uid) = target["native_uid"].as_str().filter(|id| *id != owner) else {
                return vec![];
            };
            existing(native_uid)
                .filter(|session| session.sid == sid)
                .into_iter()
                .chain(existing(owner))
                .collect()
        } else {
            existing(owner)
                .filter(|session| session.sid == sid)
                .into_iter()
                .collect()
        }
    }

    pub(super) fn background_tasks(&self, target: &Value) -> Result<Value, String> {
        let sid = target["session_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("droid_sdk: resolved session_id is required")?;
        for session in self.existing_background_views(target, sid) {
            let live = lock(&session.live);
            if owns_background_view(&live, &session, sid) {
                return Ok(json!({"raw":{"background_commands":live.background_commands(sid)}}));
            }
        }
        Ok(json!({"raw":{"background_commands":[]}}))
    }

    pub(super) fn background_output(&self, target: &Value, input: &Value) -> Result<Value, String> {
        let sid = target["session_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("droid_sdk: resolved session_id is required")?;
        let task = input["task"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("droid_sdk: missing background task identity")?;
        for session in self.existing_background_views(target, sid) {
            let live = lock(&session.live);
            if owns_background_view(&live, &session, sid) {
                let output = live
                    .background_tasks
                    .iter()
                    .find(|entry| {
                        entry["session_id"] == sid
                            && entry["id"] == task
                            && entry["status"] == "running"
                    })
                    .and_then(|entry| entry.get("output"));
                return Ok(json!({"raw":output}));
            }
        }
        Ok(json!({"raw":null}))
    }
}

fn owns_background_view(live: &Live, session: &Session, sid: &str) -> bool {
    (session.sid == sid && live.outbound.is_some())
        || live
            .background_tasks
            .iter()
            .any(|task| task["session_id"] == sid && task["detached"] == true)
}
