//! Native actions own transcript changes; only their receipts are persisted here.
use super::*;

impl Bridge {
    pub(super) async fn action(
        self: &Arc<Self>,
        session: &Arc<Session>,
        method: &str,
        input: &Value,
    ) -> Result<Value, String> {
        if method == "fork"
            && ["before_turn_id", "message_id", "messageId", "prompt_index"]
                .iter()
                .any(|key| input.get(*key).is_some_and(|v| !v.is_null()))
        {
            return Err(
                "droid_sdk_unsupported: message-boundary fork requires native before identity"
                    .into(),
            );
        }
        let boundary = method == "rewind"
            || method == "fork" && input.get("before").is_some_and(|v| !v.is_null());
        let id = input["request_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("droid_sdk: action request_id is required")?;
        let key = format!("{method}:{id}");
        let previous = lock(&session.live).actions.get(&key).cloned();
        // Read the durable receipt before launching or reattaching a retired
        // source. A retry after service restart must not execute the action again.
        #[cfg(unix)]
        let previous = if previous.is_none()
            && self.state_dir.is_some()
            && lock(&session.live).checkpoint.is_none()
        {
            match std::fs::read(self.checkpoint_path(session)?) {
                Ok(bytes) => {
                    let saved: Value = serde_json::from_slice(&bytes)
                        .map_err(|e| format!("droid_sdk_checkpoint: {e}"))?;
                    saved["actions"].get(&key).cloned()
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("droid_sdk_checkpoint: {error}")),
            }
        } else {
            previous
        };
        if let Some(previous) = previous {
            if previous["input"] != *input {
                return Err(
                    "droid_sdk: action request_id already used with different input".into(),
                );
            }
            if let Some(receipt) = previous.get("receipt") {
                return Ok(receipt.clone());
            }
            return Err(previous["error"].as_str().unwrap_or("droid_sdk: action already attempted; outcome unknown, inspect native session before retrying").into());
        }
        self.ensure(session).await?;
        self.check_conflict(session).await?;
        let params = match method {
            "rename" => {
                json!({"title":input["title"].as_str().ok_or("droid_sdk: title must be a string")?})
            }
            "fork" | "rewind" if boundary => {
                json!({"sessionId":session.sid,
                    "messageId":input["before"].as_str().ok_or("droid_sdk: before must be a native message identity")?,
                    "filesToRestore":if method == "rewind" { input["files_to_restore"].clone() } else { json!([]) },
                    "filesToDelete":if method == "rewind" { input["files_to_delete"].clone() } else { json!([]) },
                    "forkTitle":input["title"].as_str().unwrap_or("消息前分支")})
            }
            "fork" => {
                let mut params = json!({});
                if let Some(title) = input.get("title") {
                    params["title"] = title.clone();
                }
                params
            }
            "compact" => {
                let mut params = json!({});
                if let Some(instructions) = input.get("custom_instructions") {
                    params["customInstructions"] = instructions.clone();
                }
                params
            }
            _ => return Err("droid_sdk: unsupported action".into()),
        };
        {
            let mut live = lock(&session.live);
            live.actions.insert(key.clone(), json!({"input":input}));
            if method == "compact" {
                live.compact = json!({"state":"compacting"});
            }
            live.save_checkpoint()
                .map_err(|e| format!("droid_sdk_checkpoint: {e}"))?;
            session.publish(&mut live);
        }
        let result = async {
            let native_method = if boundary { "droid.execute_rewind".into() } else { format!("droid.{method}_session") };
            let raw = session.control(json!({"method":native_method,"params":params})).await?;
            if method == "rename" {
                if raw["success"] == false { return Err("droid_sdk: native rename failed".into()); }
                return Ok(json!({"uid":session.uid,"renamed":true,"title":input["title"],"native":raw}));
            }
            let sid = raw["newSessionId"].as_str().ok_or("droid_sdk: native action did not return a session identity")?;
            if sid == session.sid {
                return Ok(json!({"uid":session.uid,"session_id":sid,"source_uid":session.uid,"native":raw}));
            }
            // A compaction handoff must not leave the old bridge addressing a
            // transport whose current native identity may have changed.
            if method == "compact" || boundary {
                self.retire_action_runtime(session).await?;
            }
            let path = self.find_native_action_path(sid)?;
            let uid = crate::sessions::uid_for("droid", &path);
            let child = Arc::new(Session::new(uid.clone(), sid.into(), session.cwd.clone(), path.clone(), &Value::Null));
            let title = std::fs::File::open(&path).ok().and_then(|file| {
                use std::io::BufRead;
                std::io::BufReader::new(file).lines().next().and_then(Result::ok)
            }).and_then(|line| serde_json::from_str::<Value>(&line).ok())
                .and_then(|row| row["title"].as_str().map(str::to_owned));
            let receipt = json!({"uid":uid,"session_id":sid,"cwd":session.cwd,"source_uid":session.uid,"native":raw,
                "session":{"uid":uid,"sid":sid,"source":"droid","cwd":session.cwd,"title":title.unwrap_or_else(||"新会话".into())}});
            lock(&self.state).sessions.entry(uid).or_insert(child);
            Ok(receipt)
        }.await;
        if (method == "compact" || boundary)
            && result.as_ref().err().is_some_and(|error: &String| {
                error.contains("timed out")
                    || error.contains("outcome unknown")
                    || error.contains("did not return a session identity")
            })
        {
            self.retire_action_runtime(session).await?;
        }
        {
            let mut live = lock(&session.live);
            match &result {
                Ok(receipt) => {
                    live.actions.get_mut(&key).unwrap()["receipt"] = receipt.clone();
                    if method == "compact" {
                        live.compact = json!({"state":"success","native":receipt["native"]});
                        live.context_usage = Value::Null;
                        live.items.clear();
                    }
                }
                Err(error) => {
                    live.actions.get_mut(&key).unwrap()["error"] = json!(error);
                    if method == "compact" {
                        live.compact = json!({"state":"failed","error":error});
                    }
                }
            }
            live.save_checkpoint()
                .map_err(|e| format!("droid_sdk_checkpoint: {e}"))?;
            session.publish(&mut live);
        }
        result
    }

    pub(super) async fn retire_action_runtime(&self, session: &Arc<Session>) -> Result<(), String> {
        let mut done = {
            let live = lock(&session.live);
            if let Some(stop) = &live.runtime_stop {
                stop.cancel();
            }
            live.runtime_done.clone()
        };
        if let Some(done) = done.as_mut() {
            tokio::time::timeout(Duration::from_secs(10), done.wait_for(|v| *v))
                .await
                .map_err(|_| "droid_sdk: action runtime stop has not been confirmed")?
                .map_err(|_| "droid_sdk: action stop channel closed")?;
        }
        Ok(())
    }

    fn find_native_action_path(&self, sid: &str) -> Result<PathBuf, String> {
        let name = format!("{sid}.jsonl");
        let entries = std::fs::read_dir(&self.root)
            .map_err(|e| format!("droid_sdk: native root unavailable ({})", e.kind()))?;
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_file() && path.file_name().is_some_and(|v| v == name.as_str()) {
                return Ok(path);
            }
            if path.is_dir() {
                for child in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                    let path = child.map_err(|e| e.to_string())?.path();
                    if path.is_file() && path.file_name().is_some_and(|v| v == name.as_str()) {
                        return Ok(path);
                    }
                }
            }
        }
        Err("droid_sdk: native action succeeded but its session file is not yet available; inspect the session list before retrying".into())
    }
}
