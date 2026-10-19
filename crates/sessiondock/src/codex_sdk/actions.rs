//! Native conversation operations. The daemon owns all rollout changes.
use serde_json::{Value, json};

use super::{CodexSdk, RpcError};

impl CodexSdk {
    pub async fn fork_thread(
        &self,
        thread_id: &str,
        last_turn_id: Option<&str>,
    ) -> Result<Value, RpcError> {
        self.inner.ensure(thread_id).await?;
        let connection = self.inner.connection().await?;
        let mut params = json!({"threadId":thread_id,"excludeTurns":true});
        if let Some(id) = last_turn_id.filter(|id| !id.is_empty()) {
            params["lastTurnId"] = json!(id);
        }
        let result = connection.request("thread/fork", params).await?;
        self.inner
            .adopt_result(connection.generation(), result, None)
    }

    pub async fn list_turns(
        &self,
        thread_id: &str,
        cursor: Option<&str>,
    ) -> Result<Value, RpcError> {
        self.inner.ensure(thread_id).await?;
        let connection = self.inner.connection().await?;
        let mut params = json!({"threadId":thread_id,"sortDirection":"desc","itemsView":"summary"});
        if let Some(cursor) = cursor {
            params["cursor"] = json!(cursor);
        }
        connection.request("thread/turns/list", params).await
    }

    pub async fn revert_thread(
        &self,
        thread_id: &str,
        before_turn_id: &str,
    ) -> Result<Value, RpcError> {
        self.inner.ensure(thread_id).await?;
        self.inner
            .connection()
            .await?
            .request(
                "thread/revert",
                json!({"threadId":thread_id,"beforeTurnId":before_turn_id}),
            )
            .await
    }

    pub async fn compact_thread(&self, thread_id: &str) -> Result<Value, RpcError> {
        self.inner.ensure(thread_id).await?;
        self.inner
            .connection()
            .await?
            .request("thread/compact/start", json!({"threadId":thread_id}))
            .await
    }

    pub async fn review_thread(&self, thread_id: &str, target: Value) -> Result<Value, RpcError> {
        self.inner.ensure(thread_id).await?;
        self.inner
            .connection()
            .await?
            .request(
                "review/start",
                json!({"threadId":thread_id,"delivery":"inline","target":target}),
            )
            .await
    }
}
