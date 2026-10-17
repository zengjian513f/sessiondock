//! Explicit user controls on the already shared app-server thread.
use serde_json::{Value, json};

use super::{CodexSdk, RpcError};

impl CodexSdk {
    pub async fn controls_goal(
        &self,
        thread_id: &str,
        action: &str,
        objective: Option<&str>,
        status: Option<&str>,
        token_budget: Option<i64>,
    ) -> Result<Value, RpcError> {
        self.inner.ensure(thread_id).await?;
        let connection = self.inner.connection().await?;
        let mut params = json!({"threadId":thread_id});
        let method = match action {
            "set" => {
                params["origin"] = json!("user");
                if let Some(objective) = objective {
                    params["objective"] = json!(objective);
                }
                if let Some(status) = status {
                    params["status"] = json!(status);
                }
                if let Some(budget) = token_budget {
                    params["tokenBudget"] = json!(budget);
                }
                "thread/goal/set"
            }
            "clear" => {
                params["origin"] = json!("user");
                "thread/goal/clear"
            }
            _ => "thread/goal/get",
        };
        connection.request(method, params).await
    }

    pub async fn controls_settings(
        &self,
        thread_id: &str,
        mode: Option<&str>,
        approval_policy: Option<Value>,
        sandbox_policy: Option<Value>,
    ) -> Result<Value, RpcError> {
        let thread = self.inner.ensure(thread_id).await?;
        let connection = self.inner.connection().await?;
        let mut params = json!({"threadId":thread_id});
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
        connection.request("thread/settings/update", params).await
    }

    /// Native CLI `!command` semantics: evaluated by the configured shell,
    /// unsandboxed. Output arrives as ordinary commandExecution thread items.
    pub async fn controls_shell(&self, thread_id: &str, command: &str) -> Result<Value, RpcError> {
        self.inner.ensure(thread_id).await?;
        let connection = self.inner.connection().await?;
        connection
            .request(
                "thread/shellCommand",
                json!({"threadId":thread_id,"command":command}),
            )
            .await
    }
}
