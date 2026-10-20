//! Native v2 session controls. Keep native boundaries and error responses intact.
use super::*;

pub(super) fn delivery(input: &Value, default: &str) -> Result<String, String> {
    let value = input
        .get("delivery")
        .and_then(Value::as_str)
        .unwrap_or(default);
    if !["queue", "steer"].contains(&value) {
        return Err("OpenCode delivery 需要 queue/steer".into());
    }
    Ok(value.to_owned())
}

impl Bridge {
    pub(super) async fn action(
        self: &Arc<Self>,
        session: &Arc<Session>,
        method: &str,
        input: Value,
    ) -> Result<Value, String> {
        self.check_runtime(&session.sid, &session.cwd)?;
        let _operation = session.operation.lock().await;
        if method == "usage" {
            self.refresh(session).await?;
            let state = session
                .state
                .lock()
                .map_err(|_| "OpenCode SDK state poisoned")?;
            return Ok(json!({"usage":state.raw["usage"]}));
        }
        let control_key = if ["compact", "shell", "command"].contains(&method) {
            let key = format!("{method}:{}", string(&input, "request_id")?);
            if let Some(receipt) = session
                .state
                .lock()
                .map_err(|_| "OpenCode SDK state poisoned")?
                .controls
                .get(&key)
            {
                if receipt["pending"] == true {
                    return Err(
                        "OpenCode 原生操作尚未确认；同 request_id 不会重复执行，请先核对原生记录"
                            .into(),
                    );
                }
                return Ok(receipt.clone());
            }
            Some(key)
        } else {
            None
        };
        let base = format!("/api/session/{}", encoded(&session.sid));
        if method == "rename" {
            return self
                .transport
                .api(
                    &session.cwd,
                    "PATCH",
                    &base,
                    Some(json!({"title":string(&input,"title")?})),
                )
                .await;
        }
        if method == "fork" {
            let request = string(&input, "request_id")?;
            let receipt = format!("fork:{}:{request}", session.uid);
            let mut creates = self.creates.lock().await;
            let info = if let Some(result) = creates.get(&receipt) {
                if result.get("session").is_some() {
                    return Ok(result.clone());
                }
                if let Some(info) = result.get("fork_info") {
                    info.clone()
                } else {
                    return Err("OpenCode 原生分支尚未确认；同 request_id 不会重复创建，请先核对原生会话列表".into());
                }
            } else {
                let mut body = json!({});
                if let Some(before) = input.get("before").filter(|v| !v.is_null()) {
                    body["before"] = before.clone();
                }
                creates.insert(receipt.clone(), json!({"pending":true}));
                let response = self
                    .transport
                    .api(&session.cwd, "POST", &format!("{base}/fork"), Some(body))
                    .await
                    .map_err(|error| {
                        format!("OpenCode 原生分支未确认；同 request_id 不会重复创建：{error}")
                    })?;
                let info = response["data"].clone();
                // If later hydration fails, retry the returned native identity,
                // never execute fork a second time.
                creates.insert(receipt.clone(), json!({"fork_info":info}));
                info
            };
            let info = &info;
            let target = self.identity(info)?;
            let (fork, key) = self.install(&target, info).await?;
            let result = json!({"uid":fork.uid,"session_id":fork.sid,"cwd":fork.cwd,"key":key,
                "session":{"uid":fork.uid,"source":"opencode","session_id":fork.sid,"sid":fork.sid,
                "cwd":fork.cwd,"title":info["title"],"model":info["model"]},"capabilities":capabilities()});
            creates.insert(receipt, result.clone());
            return Ok(result);
        }
        if method == "agents" || method == "commands" {
            let collection = if method == "agents" {
                "agent"
            } else {
                "command"
            };
            let response = self
                .transport
                .api(
                    &session.cwd,
                    "GET",
                    &format!(
                        "/api/{collection}?location%5Bdirectory%5D={}",
                        encoded(&session.cwd.to_string_lossy())
                    ),
                    None,
                )
                .await?;
            let data = response["data"]
                .as_array()
                .ok_or("OpenCode catalog 返回格式未知")?;
            let data: Vec<_> = data.iter().map(|row| {
                if method == "agents" {
                    json!({"id":row["id"],"name":row["name"],"description":row["description"],"mode":row["mode"],"hidden":row["hidden"]})
                } else {
                    json!({"name":row["name"],"description":row["description"]})
                }
            }).collect();
            return Ok(json!({(method):data}));
        }
        let (verb, path, body) = match method {
            "queue_cancel" => (
                "DELETE",
                format!("{base}/inbox/{}", encoded(string(&input, "id")?)),
                None,
            ),
            "queue_update" => (
                "PATCH",
                format!("{base}/inbox/{}", encoded(string(&input, "id")?)),
                Some(json!({"delivery":delivery(&input,"steer")?})),
            ),
            "revert" => (
                "POST",
                format!("{base}/revert/stage"),
                Some(json!({"messageID":string(&input,"message_id")?,"files":false})),
            ),
            "unrevert" => ("DELETE", format!("{base}/revert"), None),
            "revert_commit" => ("POST", format!("{base}/revert/commit"), None),
            "compact" => (
                "POST",
                format!("{base}/compact"),
                Some(json!({"id":random_id("msg_")?,"delivery":delivery(&input,"steer")?})),
            ),
            "shell" => (
                "POST",
                format!("{base}/shell"),
                Some(json!({"id":random_id("msg_")?,"command":string(&input,"command")?})),
            ),
            "command" => {
                let body = json!({"name":string(&input,"name")?,"text":input["text"].as_str().unwrap_or_default(),"delivery":delivery(&input,"queue")?});
                ("POST", format!("{base}/command"), Some(body))
            }
            "settings" => {
                let current = session
                    .state
                    .lock()
                    .map_err(|_| "OpenCode SDK state poisoned")?
                    .raw["model"]
                    .clone();
                if let Some(model) = model(&input, Some(&current))? {
                    self.transport
                        .api(
                            &session.cwd,
                            "POST",
                            &format!("{base}/model"),
                            Some(json!({"model":model})),
                        )
                        .await?;
                }
                // Agent IDs and permissions are native values, never Codex policy enums.
                if let Some(agent) = input.get("native_agent").filter(|v| !v.is_null()) {
                    self.transport
                        .api(
                            &session.cwd,
                            "POST",
                            &format!("{base}/agent"),
                            Some(json!({"agent":agent})),
                        )
                        .await?;
                }
                if let Some(permissions) = input.get("permissions") {
                    self.transport
                        .api(
                            &session.cwd,
                            "PATCH",
                            &base,
                            Some(json!({"permissions":permissions})),
                        )
                        .await?;
                }
                self.refresh(session).await?;
                return Ok(json!({"ok":true}));
            }
            _ => return Err(format!("OpenCode SDK 不支持 {method}")),
        };
        if let Some(key) = &control_key {
            session
                .state
                .lock()
                .map_err(|_| "OpenCode SDK state poisoned")?
                .controls
                .insert(key.clone(), json!({"pending":true}));
        }
        let response = self
            .transport
            .api(&session.cwd, verb, &path, body)
            .await
            .map_err(|error| {
                if control_key.is_some() {
                    format!("OpenCode 原生操作未确认；同 request_id 不会重复执行：{error}")
                } else {
                    error
                }
            })?;
        if method == "queue_cancel" {
            let id = string(&input, "id")?;
            let mut state = session
                .state
                .lock()
                .map_err(|_| "OpenCode SDK state poisoned")?;
            // Delivered items are native messages and cancellation is a no-op.
            // Only remove an optimistic bubble still owned by the inbox.
            if !state.records.contains_key(id) {
                state.items.retain(|item| item["native_id"] != id);
            }
        }
        if ["revert", "unrevert", "revert_commit"].contains(&method) {
            // Native projection may remove messages. Re-seed the baseline instead
            // of leaving reverted live rows or inventing rewritten native records.
            let records = self.read_messages(&session.sid, &session.cwd, None).await?;
            let mut state = session
                .state
                .lock()
                .map_err(|_| "OpenCode SDK state poisoned")?;
            state.records = records
                .into_iter()
                .filter_map(|v| Some((v["id"].as_str()?.to_owned(), v)))
                .collect();
            state.items.clear();
            state.publish();
        }
        // Record successful native admission before a fallible observation: a
        // retry must not repeat shell execution or compaction after refresh fails.
        let result = json!({"ok":true,"native":response});
        if let Some(key) = control_key {
            session
                .state
                .lock()
                .map_err(|_| "OpenCode SDK state poisoned")?
                .controls
                .insert(key, result.clone());
        }
        if let Err(error) = self.refresh(session).await
            && let Ok(mut state) = session.state.lock()
        {
            state.error(&error);
        }
        session.wake.notify_one();
        Ok(result)
    }
}
