//! Native conversation mutations. The CLI owns all database and transcript writes.
use super::*;

pub(super) fn history_busy(data: &Data) -> String {
    if data.cancelling {
        "agy_sdk_history_busy: Agy 已接受中断请求。请等本回合的结束回执到达后再操作会话历史。"
            .into()
    } else {
        "agy_sdk_history_busy: 请等待当前任务结束后操作会话历史。".into()
    }
}

impl Live {
    fn check_history_idle(&self) -> Result<(), String> {
        let data = self.data.lock().unwrap();
        if data.outstanding > 0 || data.compact["state"] == "compacting" {
            return Err(history_busy(&data));
        }
        Ok(())
    }
}

impl Bridge {
    pub(super) async fn history_action(
        self: &Arc<Self>,
        method: &str,
        target: Value,
        input: Value,
    ) -> Result<Value, String> {
        let request = text(&input, "request_id")?;
        let sid = text(&target, "session_id")?;
        let key = format!("{sid}:{method}:{request}");
        let mut actions = self.history_actions.lock().await;
        let path = self
            .process
            .adapter_dir
            .as_ref()
            .map(|p| p.join("history-actions.json"));
        if let Some(path) = &path {
            crate::sdk_common::private_dir(path.parent().unwrap()).map_err(|e| e.to_string())?;
        }
        if actions.is_empty()
            && let Some(path) = &path
        {
            match std::fs::read(path) {
                Ok(bytes) => {
                    *actions = serde_json::from_slice(&bytes)
                        .map_err(|e| format!("agy_sdk_action_receipt: {e}"))?
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(format!("agy_sdk_action_receipt: {e}")),
            }
        }
        if let Some(previous) = actions.get(&key) {
            if previous["input"] != input {
                return Err("agy_sdk_action_request_reused: 请求 ID 已用于不同操作参数。".into());
            }
            if let Some(receipt) = previous.get("receipt") {
                return Ok(receipt.clone());
            }
            return Err(previous["error"].as_str().unwrap_or("agy_sdk_action_unknown: 操作已尝试但结果未确认；请核对原生会话，不能自动重试。").to_owned());
        }
        let live = self.open(&target).await?;
        let _compaction = if method == "compact" {
            Some(
                live.compact_operation
                    .try_lock()
                    .map_err(|_| "agy_sdk_compact_busy: 此会话已有压缩操作正在进行。")?,
            )
        } else {
            None
        };
        self.process.check_available(live.sid.clone()).await?;
        live.check_history_idle()?;
        actions.insert(key.clone(), json!({"input":input}));
        if let Some(path) = &path {
            crate::sdk_common::save_atomic(path, &*actions)
                .map_err(|e| format!("agy_sdk_action_receipt: {e}"))?;
        }
        // Native summary generation can take a full model round. Keep other
        // sessions' history actions independent once the attempt is recorded.
        let actions = (method != "compact").then_some(actions);
        let outcome = async {
            let (sender, receiver) = oneshot::channel();
            live.commands
                .send(Command::History(method.to_owned(), input.clone(), sender))
                .map_err(|_| "agy_sdk_shutdown")?;
            let mut result = receiver.await.map_err(|_| "agy_sdk_shutdown")??;
            if method == "fork" {
                let sid = text(&result, "session_id")?;
                transport::validate_sid(&sid)?;
                let path = self.mirror.join("cli").join(&sid);
                let uid = crate::sessions::uid_for("agy", &path);
                let (model, observed, effort) = {
                    let data = live.data.lock().unwrap();
                    (
                        data.model.clone(),
                        data.observed_model.clone(),
                        data.effort.clone(),
                    )
                };
                let observed_model = super::transcript::model(&self.process, &sid)
                    .await
                    .or(observed);
                let forked = self.install(
                    uid.clone(),
                    sid.clone(),
                    live.cwd.clone(),
                    model.clone(),
                    effort,
                )?;
                forked.observe_model(observed_model.clone());
                result["uid"] = json!(uid);
                result["session"] = json!({"uid":uid,"source":"agy","sid":sid,"cwd":live.cwd,
                    "path":path,"title":"分支会话","model":model.or(observed_model)});
            }
            Ok::<_, String>(result)
        }
        .await;
        let mut actions = match actions {
            Some(actions) => actions,
            None => self.history_actions.lock().await,
        };
        match &outcome {
            Ok(receipt) => actions.get_mut(&key).unwrap()["receipt"] = receipt.clone(),
            Err(error) => actions.get_mut(&key).unwrap()["error"] = json!(error),
        }
        if let Some(path) = &path {
            crate::sdk_common::save_atomic(path, &*actions)
                .map_err(|e| format!("agy_sdk_action_receipt: {e}"))?;
        }
        outcome
    }
}

pub(super) async fn execute(
    process: &Arc<ProcessConfig>,
    live: &Arc<Live>,
    runtime: &mut Runtime,
    method: &str,
    input: &Value,
    shutdown: CancellationToken,
) -> Result<Value, String> {
    live.check_history_idle()?;
    if method == "compact" {
        let native = runtime
            .native
            .as_mut()
            .ok_or("agy_sdk_compact_unavailable: 此会话还没有 Agy SDK 运行实例。")?;
        return compact(process, live, native, input).await;
    }
    let before = input
        .get(if method == "fork" {
            "before"
        } else {
            "message_id"
        })
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .and_then(|s| s.strip_prefix("step:"))
                .and_then(|s| s.parse::<i64>().ok())
                .ok_or("agy_sdk_step: 请选择原生用户消息。")
        })
        .transpose()?;
    if method == "revert" && before.is_none() {
        return Err("agy_sdk_step: 请选择原生用户消息。".into());
    }
    if let Some(index) = before {
        let bytes = crate::sessions::agy::active_transcript(
            &process.native_path(&live.sid),
            &tokio::fs::read(process.transcript_path(&live.sid))
                .await
                .map_err(|e| e.to_string())?,
        );
        let selected = bytes
            .split(|b| *b == b'\n')
            .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
            .any(|row| row["step_index"].as_i64() == Some(index) && row["type"] == "USER_INPUT");
        if !selected {
            return Err("agy_sdk_step: 原生用户消息已不存在，请刷新历史。".into());
        }
    }
    let (model, effort, mode) = {
        let data = live.data.lock().unwrap();
        (data.model.clone(), data.effort.clone(), data.mode.clone())
    };
    // -1 means the *entire* source in ForkConversation, not an empty prefix.
    if method == "fork" && before == Some(0) {
        let native = Native::start(
            process.clone(),
            None,
            std::path::Path::new(&live.cwd),
            model.as_deref(),
            effort.as_deref(),
            mode.as_deref(),
            shutdown.clone(),
            None,
        )
        .await?;
        let sid = native.sid.clone();
        live.diagnostic(native.close(shutdown).await?);
        return Ok(json!({"session_id":sid,"cwd":live.cwd,"empty":true}));
    }
    let mut temporary = if runtime.native.is_none() {
        Some(
            Native::start(
                process.clone(),
                Some(&live.sid),
                std::path::Path::new(&live.cwd),
                model.as_deref(),
                effort.as_deref(),
                mode.as_deref(),
                shutdown.clone(),
                None,
            )
            .await?,
        )
    } else {
        None
    };
    let native = runtime.native.as_mut().or(temporary.as_mut()).unwrap();
    let result = if method == "fork" {
        native
            .history_action(
                "ForkConversation",
                json!({"sourceCascadeId":live.sid,
            "forkAtStepIndex":before.map(|i| i - 1).unwrap_or(-1),
            "targetForkWorkspace":"FORK_TARGET_CURRENT_WORKSPACE"}),
            )
            .await
            .and_then(|raw| {
                Ok(json!({"session_id":text(&raw,"newCascadeId")?,"cwd":live.cwd,"raw":raw}))
            })
    } else {
        native
            .history_action(
                "RevertToCascadeStep",
                json!({"cascadeId":live.sid,
            "stepIndex":before.unwrap()-1,"conversationOnly":true}),
            )
            .await
            .map(|raw| json!({"reverted":true,"session_id":live.sid,"raw":raw}))
    };
    if result.is_ok() && method == "revert" {
        runtime.transcript = transcript::Pending::default();
        let mut data = live.data.lock().unwrap();
        data.items.clear();
        data.steers.clear();
        data.error = None;
        data.error_kind = None;
        data.last_turn = Value::Null;
        if let Some(turn) = &mut data.runtime {
            turn.stdout_offset = native.stdout_offset;
            turn.idle_checkpoint = true;
            turn.text.clear();
            write_binding(
                process.adapter_dir.as_deref(),
                &live.sid,
                &live.cwd,
                turn,
                &[],
            )?;
        }
        live.publish(&mut data);
    }
    if let Some(native) = temporary.take() {
        let closed = native.close(shutdown).await;
        if let Ok(diagnostics) = closed {
            live.diagnostic(diagnostics);
        }
    }
    result
}

async fn compact(
    process: &ProcessConfig,
    live: &Live,
    native: &mut Native,
    input: &Value,
) -> Result<Value, String> {
    let request = text(input, "request_id")?;
    let first = echo::first_native_step(process, live)
        .await
        .ok_or("agy_sdk_compact_unavailable: 无法读取原生会话的当前检查点。")?;
    {
        let mut data = live.data.lock().unwrap();
        data.compact = json!({"state":"compacting"});
        live.publish(&mut data);
    }
    let result = async {
        native.compact(&request).await?;
        if !echo::compacted_since(process, live, first).await {
            return Err("agy_sdk_compact_no_checkpoint: Agy 未写入新的压缩检查点。".into());
        }
        context::refresh(process, live).await;
        Ok(json!({"session_id":live.sid,"compact":{"state":"success"}}))
    }
    .await;
    let mut data = live.data.lock().unwrap();
    data.compact = match &result {
        Ok(_) => json!({"state":"success"}),
        Err(error) => json!({"state":"failed","error":error}),
    };
    live.publish(&mut data);
    result
}
