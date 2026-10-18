//! Display-only discovery of threads already running on the shared daemon.
//! It must never start the daemon, resume a thread or grant process control.
use std::time::{Duration, Instant};

use serde_json::json;

use super::{CodexSdk, RpcError};

const TTL: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct Sample {
    at: Option<Instant>,
    generation: Option<u64>,
    ids: Vec<String>,
}

impl CodexSdk {
    pub async fn live_threads(&self) -> Vec<String> {
        // One small read per observer interval, shared by concurrent fleet reads.
        let mut sample = self.inner.observation.lock().await;
        let generation = self
            .inner
            .connection
            .lock()
            .await
            .as_ref()
            .filter(|connection| !connection.is_closed())
            .map(|connection| connection.generation());
        if sample.at.is_none_or(|at| at.elapsed() >= TTL) || sample.generation != generation {
            let result = tokio::time::timeout(TTL, async {
                let connection = self.inner.connection_mode(false).await?;
                // No limit means the daemon returns all loaded identities.
                let result = connection.request("thread/loaded/list", json!({})).await?;
                let ids = result["data"]
                    .as_array()
                    .ok_or(RpcError::Disconnected)?
                    .iter()
                    .filter_map(|id| id.as_str().map(str::to_owned))
                    .collect();
                Ok::<_, RpcError>((connection.generation(), ids))
            })
            .await;
            sample.ids.clear();
            sample.generation = self
                .inner
                .connection
                .lock()
                .await
                .as_ref()
                .filter(|connection| !connection.is_closed())
                .map(|connection| connection.generation());
            if let Ok(Ok((generation, ids))) = result
                && sample.generation == Some(generation)
            {
                sample.ids = ids;
            }
            sample.at = Some(Instant::now());
        }
        let mut ids = sample.ids.clone();
        drop(sample);
        // Older daemons may lack loaded/list; exact live subscriptions remain
        // valid evidence. This does not promote discovered threads to subscribers.
        ids.extend(self.loaded_threads().await);
        ids.sort();
        ids.dedup();
        ids
    }
}
