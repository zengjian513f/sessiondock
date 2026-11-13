//! Stop the native spawned tree, without treating forks or display nesting as children.
use super::{CodexSdk, Connection, RpcError};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

impl CodexSdk {
    /// Interrupt the root first, then stop its spawned descendants. Re-read the
    /// native relation metadata to catch children spawned while stop was arriving.
    /// A final root unsubscribe releases auto-subscriptions caused by child events.
    pub async fn stop_thread(&self, thread_id: &str) -> Result<(), RpcError> {
        let connection = match self.inner.writing_connection(thread_id).await {
            Ok(connection) => connection,
            Err(error) => {
                // The parent can unload before a still-running child. Locate
                // that child's existing listener without resuming the parent.
                let mut owner = None;
                let mut observer = None;
                for connection in self.inner.observed_connections().await {
                    let descendants = spawned_descendants(&connection, thread_id).await?;
                    let loaded = connection.request("thread/loaded/list", json!({})).await?;
                    if loaded["data"].as_array().is_some_and(|ids| {
                        ids.iter()
                            .filter_map(|id| id.as_str())
                            .any(|id| descendants.contains(id))
                    }) {
                        owner = Some(connection);
                        break;
                    }
                    // With no loaded root or descendants, the existing daemon
                    // can acknowledge notLoaded. Never resume missing history
                    // just to stop it; keep transport/RPC failures retryable.
                    observer = Some(connection);
                }
                owner.or(observer).ok_or(error)?
            }
        };
        let mut stopped = BTreeSet::from([thread_id.to_owned()]);
        let mut errors = Vec::new();
        if let Err(error) = self.stop_one(&connection, thread_id).await {
            errors.push(format!("{thread_id}: {error}"));
        }
        loop {
            let descendants = match spawned_descendants(&connection, thread_id).await {
                Ok(ids) => ids,
                Err(error) => {
                    errors.push(error.to_string());
                    break;
                }
            };
            let next: Vec<_> = descendants.difference(&stopped).cloned().collect();
            if next.is_empty() {
                break;
            }
            for id in next {
                if let Err(error) = self.stop_one(&connection, &id).await {
                    errors.push(format!("{id}: {error}"));
                }
                stopped.insert(id);
            }
        }
        if stopped.len() > 1 {
            if let Err(error) = self.stop_one(&connection, thread_id).await {
                errors.push(format!("{thread_id}: {error}"));
            }
        }
        if errors.is_empty() {
            return Ok(());
        }
        // Keep the root action retryable when any descendant could not stop.
        self.inner
            .stopped
            .lock()
            .expect("stopped")
            .remove(thread_id);
        Err(RpcError::Remote {
            code: -32000,
            message: format!("停止原生会话树未完成：{}", errors.join("；")),
        })
    }

    async fn stop_one(
        &self,
        connection: &Arc<Connection>,
        thread_id: &str,
    ) -> Result<(), RpcError> {
        let lock = self.inner.queue.lock(thread_id);
        let _dispatch = lock.lock().await;
        self.inner.retention.set(thread_id, false)?;
        // Stopping must not resume or parse history. The latest rollout may
        // already be in trash while the daemon still reports the thread loaded.
        // The native empty-turn interrupt cancels the exact loaded thread's
        // startup/current work without requiring a recovered turn or history.
        let interrupted = connection
            .request("turn/interrupt", json!({"threadId":thread_id,"turnId":""}))
            .await;
        let result = connection
            .request("thread/unsubscribe", json!({"threadId":thread_id}))
            .await?;
        if result["status"] != "notLoaded" {
            interrupted?;
        }
        self.inner
            .stopped
            .lock()
            .expect("stopped")
            .insert(thread_id.to_owned());
        if let Some(thread) = self
            .inner
            .threads
            .lock()
            .expect("threads")
            .remove(thread_id)
        {
            thread.apply("bridge/stopped", json!({}));
        }
        Ok(())
    }
}

async fn spawned_descendants(
    connection: &Arc<Connection>,
    root: &str,
) -> Result<BTreeSet<String>, RpcError> {
    let mut parents = BTreeMap::new();
    for archived in [false, true] {
        let mut cursor = None::<String>;
        loop {
            let result = connection
                .request(
                    "thread/list",
                    json!({
                        "ancestorThreadId":root, "sourceKinds":["subAgent"], "modelProviders":[],
                        "useStateDbOnly":true, "limit":100, "cursor":cursor, "archived":archived,
                    }),
                )
                .await?;
            let rows = result["data"].as_array().ok_or_else(|| RpcError::Remote {
                code: -32603,
                message: "无法读取 Codex 原生子代理列表".into(),
            })?;
            for row in rows {
                if let (Some(id), Some(parent)) =
                    (row["id"].as_str(), row["parentThreadId"].as_str())
                {
                    parents.insert(id.to_owned(), parent.to_owned());
                }
            }
            cursor = result["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
    }
    // Verify native ancestry even if an older server ignores the filter. A
    // self-contained fork, manual nest, sibling or unrelated daemon thread is
    // never authorized by stopping this root.
    let mut ids = BTreeSet::from([root.to_owned()]);
    loop {
        let before = ids.len();
        for (id, parent) in &parents {
            if ids.contains(parent) {
                ids.insert(id.clone());
            }
        }
        if ids.len() == before {
            break;
        }
    }
    ids.remove(root);
    Ok(ids)
}
