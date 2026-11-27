//! Read the owned CLI's native context estimate, independently of model input.
use super::{
    Binding, Live,
    control::{self, Control},
    host_key, is_control_input,
    transport::ProcessConfig,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(super) fn watch(process: Arc<ProcessConfig>, live: Arc<Live>, shutdown: CancellationToken) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = live.done.cancelled() => break,
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
            }
            if live.events.receiver_count() > 0 {
                refresh(&process, &live).await;
            }
        }
    });
}

pub(super) async fn refresh(process: &ProcessConfig, live: &Live) {
    let _read = live.context_read.lock().await;
    let sequence = live.data.lock().unwrap().seq;
    let result = read(process, live).await;
    let mut data = live.data.lock().unwrap();
    let (context, foreground_idle, errors) = match result {
        Ok((context, idle)) => (context, idle, json!({})),
        // A failed measurement must not keep presenting an old percentage as current.
        Err(error) => (json!({}), false, json!({"context": error})),
    };
    let mut changed = false;
    if data.seq == sequence {
        let consumed = data.foreground_request().map_or_else(
            || data.runtime.is_some() && data.steers.iter().any(|steer| steer.fifo),
            |active| data.consumed(active),
        );
        let pending = data.items.iter().any(|item| {
            is_control_input(item) && item["native_id"].is_null() && item["status"] != "unknown"
        });
        let idle = foreground_idle && consumed && !pending && !data.cancel_pending;
        changed = data.foreground_idle != idle;
        data.foreground_idle = idle;
    }
    if changed || data.context_usage != context || data.usage_errors != errors {
        data.context_usage = context;
        data.usage_errors = errors;
        live.publish(&mut data);
    }
}

async fn read(process: &ProcessConfig, live: &Live) -> Result<(Value, bool), String> {
    let (Some(root), Some(host_dir)) = (&process.adapter_dir, &process.host_dir) else {
        return Ok((json!({}), false));
    };
    let bytes = match tokio::fs::read(root.join(format!("{}.json", live.sid))).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((json!({}), false));
        }
        Err(_) => return Err("无法读取当前 Agy 实例的上下文绑定。".into()),
    };
    let binding: Binding =
        serde_json::from_slice(&bytes).map_err(|_| "无法读取当前 Agy 实例的上下文绑定。")?;
    if binding.sid != live.sid || binding.cwd != live.cwd {
        return Err("Agy 上下文实例身份不匹配。".into());
    }
    let runtime = if binding.turn.runtime_id.is_empty() {
        &binding.turn.id
    } else {
        &binding.turn.runtime_id
    };
    let key = host_key(&live.sid, runtime);
    let host = sdkhost_client::Client::attach(host_dir, &key)
        .await
        .map_err(|_| "无法连接当前 Agy 实例以读取上下文。")?;
    let status = host.status().await.map_err(|_| "无法读取 Agy 实例状态。")?;
    if status.exit.is_some() {
        return Ok((json!({}), false));
    }
    let control = Control::prepare(Some(root), Some((&key, true, 0)))?
        .ok_or("此 Agy 实例没有上下文查询授权。")?;
    let response = control
        .state(status.pid, &live.sid, "sessiondock-context")
        .await?;
    let update = &response["update"];
    if update["conversationId"].as_str() != Some(&live.sid) {
        return Err("Agy 上下文响应的会话身份不匹配。".into());
    }
    let idle = control::foreground_idle(update, &live.sid);
    let context = &update["contextWindowUsage"];
    let number = |value: &Value| value.as_f64().or_else(|| value.as_str()?.parse().ok());
    let (Some(used), Some(max)) = (
        number(&context["estimatedTokensUsed"]),
        number(&context["maxContextTokens"]),
    ) else {
        return Ok((json!({}), idle));
    };
    if !used.is_finite() || !max.is_finite() || used < 0.0 || max <= 0.0 {
        return Ok((json!({}), idle));
    }
    Ok((
        json!({"used_tokens":used,"max_tokens":max,"percent":used/max*100.0,
        "basis":"Agy 原生上下文估计","estimated_after_compaction":context["estimatedAfterCompaction"]}),
        idle,
    ))
}
