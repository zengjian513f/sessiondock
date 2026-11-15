//! CodeBuddy's CLI fork and conversation-only rewind control. Never rewrite transcripts.
use super::*;

impl Bridge {
    pub(super) async fn history_action(
        self: &Arc<Self>,
        source: &Arc<Session>,
        method: &str,
        input: &Value,
    ) -> Result<Value, String> {
        self.ensure(source).await?;
        let request = input["request_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("codebuddy_sdk: request_id is required")?;
        let key = format!("{method}:{request}");
        {
            let live = lock(&source.live);
            if let Some(previous) = live.actions.get(&key) {
                if previous["input"] != *input {
                    return Err(
                        "codebuddy_sdk: request_id already used with different input".into(),
                    );
                }
                if let Some(receipt) = previous.get("receipt") {
                    return Ok(receipt.clone());
                }
                return Err(previous["error"].as_str().unwrap_or("codebuddy_sdk: native operation was already attempted; inspect its outcome before a new request").to_owned());
            }
            if !live.turns.is_empty() || live.native_running {
                return Err(
                    "codebuddy_sdk_history_busy: 请等待当前任务结束后操作会话历史。".into(),
                );
            }
        }
        self.check_conflict(source).await?;
        let before = input
            .get(if method == "fork" {
                "before"
            } else {
                "message_id"
            })
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_str()
                    .ok_or("codebuddy_sdk: native user message ID required")
            })
            .transpose()?;
        if method == "revert" && before.is_none() {
            return Err("codebuddy_sdk: native user message ID required".into());
        }
        if let Some(before) = before {
            Self::preview_rewind(source, before).await?;
        }
        {
            let mut live = lock(&source.live);
            live.actions.insert(key.clone(), json!({"input":input}));
            live.save_checkpoint().map_err(|e| e.to_string())?;
        }
        let result = async {
            if method == "revert" {
                return Self::rewind(source, before.unwrap()).await;
            }
            let sid = uuid()?;
            let path = self
                .root
                .join(project_key(&source.cwd))
                .join(format!("{sid}.jsonl"));
            let uid = crate::sessions::uid_for("codebuddy", &path);
            let mut session = Session::new(
                uid.clone(),
                sid.clone(),
                source.cwd.clone(),
                path,
                &Value::Null,
            );
            session.fork_source = Some(source.sid.clone());
            let session = Arc::new(session);
            {
                let original = lock(&source.live);
                let mut live = lock(&session.live);
                live.model = original.model.clone();
                live.effort = original.effort.clone();
            }
            lock(&self.state)
                .sessions
                .insert(uid.clone(), session.clone());
            self.ensure(&session).await?;
            // initialize replies before the native resume/fork finishes. Await the
            // CLI-created identity, without a fabricated user/model turn.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
            while !session.path.is_file() {
                if tokio::time::Instant::now() >= deadline {
                    return Err("codebuddy_sdk: native fork persistence not confirmed".into());
                }
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
            if let Some(before) = before {
                Self::preview_rewind(&session, before).await?;
                Self::rewind(&session, before).await?;
            }
            let mut receipt = session.receipt();
            receipt["session"] = json!({"uid":uid,"sid":sid,"source":"codebuddy","cwd":session.cwd,
                "title":"分支会话","model":lock(&session.live).model});
            Ok(receipt)
        }
        .await;
        let mut live = lock(&source.live);
        match &result {
            Ok(receipt) => live.actions.get_mut(&key).unwrap()["receipt"] = receipt.clone(),
            Err(error) => live.actions.get_mut(&key).unwrap()["error"] = json!(error),
        }
        live.save_checkpoint().map_err(|e| e.to_string())?;
        result
    }

    async fn preview_rewind(session: &Session, before: &str) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let result = session
                .control(json!({"subtype":"rewind","session_id":session.sid,
                "user_message_id":before,"scope":"Conversation","dry_run":true}))
                .await?;
            if result["canRewind"] == true {
                return Ok(());
            }
            if result["error"] != "No active session" || tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "codebuddy_sdk: {}",
                    result["error"]
                        .as_str()
                        .unwrap_or("原生未提供此消息的回退点")
                ));
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }

    async fn rewind(session: &Session, before: &str) -> Result<Value, String> {
        let result = session
            .control(json!({"subtype":"rewind","session_id":session.sid,
            "user_message_id":before,"scope":"Conversation"}))
            .await?;
        if result["canRewind"] != true || result["historyRewound"] != true {
            return Err(format!(
                "codebuddy_sdk: {}",
                result["error"].as_str().unwrap_or("原生未确认对话回退")
            ));
        }
        let mut live = lock(&session.live);
        live.items.clear();
        live.streaming_blocks.clear();
        live.error = None;
        live.error_kind = None;
        live.last_turn = Value::Null;
        session.publish(&mut live);
        live.save_checkpoint().map_err(|e| e.to_string())?;
        Ok(json!({"session_id":session.sid,"reverted":true,"raw":result}))
    }
}
