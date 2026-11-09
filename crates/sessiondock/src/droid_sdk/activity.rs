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

pub(super) fn observe(live: &mut Live, n: &Value) {
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
            let key = label(n, "toolUseId");
            let task = json!({"id":key,"name":n["toolName"],"status":"running","detached":true});
            if let Some(old) = live.background_tasks.iter_mut().find(|v| v["id"] == key) {
                *old = task;
            } else {
                live.background_tasks.push(task);
            }
            row(
                live,
                &format!("background:{key}"),
                format!("后台任务运行中 · {}", label(n, "toolName")),
                n,
            );
        }
        "tool_execution_phase_changed" => {
            let key = label(n, "toolUseId");
            let phase = label(n, "phase");
            live.activity.insert(format!("tool:{key}"), n.clone());
            if phase.starts_with("settled_")
                && let Some(task) = live.background_tasks.iter_mut().find(|v| v["id"] == key)
            {
                task["status"] = json!(phase);
                row(
                    live,
                    &format!("background:{key}"),
                    format!(
                        "后台任务结束 · {} · {}",
                        label(n, "toolName"),
                        state(&phase)
                    ),
                    n,
                );
            }
        }
        "llm_retry" => {
            live.retry = json!({"attempt":n["attempt"],"reason":n["reason"]});
        }
        _ => {}
    }
}
