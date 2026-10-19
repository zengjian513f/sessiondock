//! Fork identities and boundaries only. The native CLI writes every transcript.
use super::*;

impl Bridge {
    fn source_path(&self, uid: &str, sid: &str) -> Result<PathBuf, String> {
        for entry in std::fs::read_dir(&self.root)
            .map_err(|e| format!("claude_sdk: native root unavailable ({})", e.kind()))?
        {
            let entry = entry
                .map_err(|e| format!("claude_sdk: native project unavailable ({})", e.kind()))?;
            let path = entry.path().join(format!("{sid}.jsonl"));
            if crate::sessions::uid_for("claude", &path) == uid && path.is_file() {
                return Ok(path);
            }
        }
        Err("claude_sdk: fork source no longer exists under the native root".into())
    }

    pub(super) fn restore_fork(&self, target: &Value) -> Result<Arc<Session>, String> {
        let uid = target["uid"]
            .as_str()
            .ok_or("claude_sdk: fork uid missing")?;
        let sid = target["session_id"]
            .as_str()
            .ok_or("claude_sdk: fork SID missing")?;
        let cwd = PathBuf::from(
            target["cwd"]
                .as_str()
                .ok_or("claude_sdk: fork cwd missing")?,
        );
        let path = self
            .root
            .join(project_key(&cwd))
            .join(format!("{sid}.jsonl"));
        if crate::sessions::uid_for("claude", &path) != uid {
            return Err(
                "claude_sdk: persisted fork identity no longer matches its native path".into(),
            );
        }
        let metadata = &target["sdk_fork"];
        let mut session = Session::new(uid.into(), sid.into(), cwd, path, &Value::Null);
        session.fork_metadata = metadata.clone();
        if !session.path.is_file() && metadata["empty"] != true {
            let source = self.source_path(
                metadata["source_uid"]
                    .as_str()
                    .ok_or("claude_sdk: persisted fork source uid missing")?,
                metadata["source_session_id"]
                    .as_str()
                    .ok_or("claude_sdk: persisted fork source SID missing")?,
            )?;
            let at = metadata["resume_at"]
                .as_str()
                .ok_or("claude_sdk: persisted fork boundary missing")?;
            session.fork = Some((source, at.into()));
        }
        Ok(Arc::new(session))
    }

    pub(super) async fn fork_session(
        self: &Arc<Self>,
        source: &Arc<Session>,
        input: &Value,
    ) -> Result<Value, String> {
        let request = input["request_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("claude_sdk: fork request_id is required")?;
        let key = format!("{}:{request}", source.uid);
        {
            let state = lock(&self.state);
            if let Some((previous, receipt)) = state.forks.get(&key) {
                if previous != input {
                    return Err(
                        "claude_sdk: fork request_id already used with different input".into(),
                    );
                }
                return receipt.clone().ok_or_else(|| "claude_sdk: fork already attempted; native outcome may be unknown, inspect the session list before a new explicit request".into());
            }
        }
        let before = input
            .get("before")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_str()
                    .ok_or("claude_sdk: before must be a native user UUID")
            })
            .transpose()?;
        let metadata = if source.path.is_file() {
            let text = std::fs::read_to_string(&source.path)
                .map_err(|e| format!("claude_sdk: fork source unavailable ({})", e.kind()))?;
            let records: Vec<Value> = text
                .lines()
                .filter_map(|line| parse_json(line).ok())
                .filter(|row| {
                    row["sessionId"] == source.sid
                        && row["isSidechain"] != true
                        && row["uuid"].is_string()
                        && row.get("parentUuid").is_some()
                })
                .collect();
            let at = if let Some(before) = before {
                let selected = records
                    .iter()
                    .find(|row| row["uuid"] == before && row["type"] == "user")
                    .ok_or(
                        "claude_sdk: selected native user message is not present in this session",
                    )?;
                selected["parentUuid"].as_str().map(str::to_owned)
            } else {
                records
                    .last()
                    .and_then(|row| row["uuid"].as_str())
                    .map(str::to_owned)
            };
            if let Some(at) = &at
                && !records.iter().any(|row| row["uuid"] == *at)
            {
                return Err(
                    "claude_sdk: native fork predecessor is unavailable; history was not changed"
                        .into(),
                );
            }
            json!({"source_uid":source.uid,"source_session_id":source.sid,"resume_at":at,
                "before":before,"empty":at.is_none()})
        } else if before.is_some() {
            return Err("claude_sdk: selected native user message has not been persisted".into());
        } else if source.fork_metadata.is_object() {
            source.fork_metadata.clone()
        } else {
            json!({"source_uid":source.uid,"source_session_id":source.sid,"empty":true})
        };
        let sid = uuid()?;
        let path = self
            .root
            .join(project_key(&source.cwd))
            .join(format!("{sid}.jsonl"));
        let uid = crate::sessions::uid_for("claude", &path);
        let target = json!({"uid":uid,"session_id":sid,"cwd":source.cwd,"sdk_fork":metadata});
        let fork = self.restore_fork(&target)?;
        {
            let source_live = lock(&source.live);
            let mut live = lock(&fork.live);
            live.model = source_live.model.clone();
            live.effort = source_live.effort.clone();
        }
        {
            let mut state = lock(&self.state);
            // Reserve before the native process is dispatched. A lost or
            // rejected initialize cannot create a second branch on retry.
            state.forks.insert(key.clone(), (input.clone(), None));
            state.sessions.insert(uid.clone(), fork.clone());
        }
        let _runtime = fork.runtime.read().await;
        let _operation = fork.operations.lock().await;
        self.ensure(&fork).await?;
        let mut receipt = fork.receipt();
        receipt["session"] = json!({"uid":uid,"session_id":sid,"source":"claude","cwd":source.cwd,
            "title":"分支会话","sdk_fork":metadata,"model":lock(&fork.live).model,
            "started":chrono::DateTime::from_timestamp(
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64, 0)
                .unwrap_or_default().to_rfc3339()});
        lock(&self.state).forks.get_mut(&key).unwrap().1 = Some(receipt.clone());
        Ok(receipt)
    }
}
