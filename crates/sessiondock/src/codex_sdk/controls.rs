//! Explicit user controls on the already shared app-server thread.
use serde_json::{Value, json};

use super::{CodexSdk, RpcError};

impl CodexSdk {
    pub async fn controls_settings(
        &self,
        thread_id: &str,
        mode: Option<&str>,
        approval_policy: Option<Value>,
        sandbox_policy: Option<Value>,
        service_tier: Option<String>,
        model_effort: (Option<String>, Option<String>),
    ) -> Result<Value, RpcError> {
        let thread = self.inner.ensure(thread_id).await?;
        let connection = self.inner.thread_connection(thread_id).await?;
        let mut params = json!({"threadId":thread_id});
        if let Some(model) = model_effort.0 {
            params["model"] = json!(model);
        }
        if let Some(effort) = model_effort.1 {
            params["effort"] = json!(effort);
        }
        if let Some(mode) = mode {
            let summary = thread.summary();
            let model = summary["settings"]["model"]
                .as_str()
                .or_else(|| summary["resume"]["model"].as_str())
                .ok_or(RpcError::Disconnected)?;
            let effort = summary["settings"]
                .get("effort")
                .unwrap_or(&summary["resume"]["reasoningEffort"]);
            params["collaborationMode"] = json!({"mode":mode,"settings":{
                "model":model,"reasoning_effort":effort,"developer_instructions":null}});
        }
        if let Some(policy) = approval_policy {
            params["approvalPolicy"] = policy;
        }
        if let Some(policy) = sandbox_policy {
            params["sandboxPolicy"] = policy;
        }
        if let Some(tier) = service_tier {
            params["serviceTier"] = json!(tier);
        }
        // Match the TUI: thread settings can change while a turn is running.
        // turn/settings/update is a separate, feature-gated experiment; calling
        // it after this succeeds can report failure for an already applied change.
        connection.request("thread/settings/update", params).await
    }

    /// Native CLI `!command` semantics: evaluated by the configured shell,
    /// unsandboxed. Output arrives as ordinary commandExecution thread items.
    pub async fn controls_shell(&self, thread_id: &str, command: &str) -> Result<Value, RpcError> {
        self.inner.ensure(thread_id).await?;
        let connection = self.inner.thread_connection(thread_id).await?;
        connection
            .request(
                "thread/shellCommand",
                json!({"threadId":thread_id,"command":command}),
            )
            .await
    }
}
