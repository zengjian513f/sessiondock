//! Display-only discovery of threads already running on the shared daemon.
//! It must never start the daemon, resume a thread or grant process control.
use std::time::{Duration, Instant};

use serde_json::json;

use super::CodexSdk;

const TTL: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct Sample {
    at: Option<Instant>,
    ids: Vec<String>,
}

impl CodexSdk {
    pub async fn live_threads(&self) -> Vec<String> {
        self.observe_threads(false).await
    }

    /// Unsubscribe is not native unload: destructive operations must still see
    /// stopped threads while the daemon can flush metadata or history.
    pub async fn writing_threads(&self) -> Vec<String> {
        self.observe_threads(true).await
    }

    async fn observe_threads(&self, writers: bool) -> Vec<String> {
        // One small read per observer interval, shared by concurrent fleet reads.
        let mut sample = self.inner.observation.lock().await;
        if writers || sample.at.is_none_or(|at| at.elapsed() >= TTL) {
            sample.ids.clear();
            for connection in self.inner.observed_connections().await {
                let result =
                    tokio::time::timeout(TTL, connection.request("thread/loaded/list", json!({})))
                        .await;
                if let Ok(Ok(result)) = result {
                    if let Some(ids) = result["data"].as_array() {
                        sample
                            .ids
                            .extend(ids.iter().filter_map(|v| v.as_str().map(str::to_owned)));
                    }
                }
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
        if !writers {
            let stopped = self.inner.stopped.lock().expect("stopped");
            ids.retain(|id| !stopped.contains(id));
        }
        ids
    }
}
