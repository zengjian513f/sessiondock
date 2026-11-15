//! Coalesced restart snapshots; online lists always use the live cache.
use super::registry::write_snapshot_file;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct State {
    pending: Option<(f64, Arc<Value>)>,
    saved_sig: Option<Value>,
    last_attempt: Option<Instant>,
    retired: bool,
}
#[derive(Default)]
struct Node {
    state: Mutex<State>,
    writer: Mutex<()>,
}
pub(super) struct Snapshots {
    directory: PathBuf,
    interval: Duration,
    nodes: Mutex<HashMap<String, Arc<Node>>>,
    wake: Notify,
}
impl Snapshots {
    pub fn new(directory: PathBuf, interval: Duration) -> Self {
        Self {
            directory,
            interval,
            nodes: Default::default(),
            wake: Notify::new(),
        }
    }
    pub fn seed(&self, id: &str, sig: Value) {
        let node = self.node(id);
        node.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .saved_sig = Some(sig);
    }
    fn node(&self, id: &str) -> Arc<Node> {
        self.nodes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.into())
            .or_default()
            .clone()
    }
    pub fn update(&self, id: &str, stamp: f64, data: Arc<Value>) {
        let node = self.node(id);
        let mut state = node.state.lock().unwrap_or_else(|e| e.into_inner());
        let sig = data.get("sig").unwrap_or(&Value::Null);
        if crate::json_compat::truthy(sig)
            && state.saved_sig.as_ref() == Some(sig)
            && state.pending.is_none()
            && self.directory.join(format!("{id}.sessions.json")).exists()
        {
            return;
        }
        // Keep reversions too: an older write may still be in flight.
        // Only the latest full replacement is retained for this node.
        state.pending = Some((stamp, data));
        drop(state);
        self.wake.notify_one();
    }
    pub fn remove(&self, id: &str) {
        let node = self
            .nodes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        if let Some(node) = node {
            let _writer = node.writer.lock().unwrap_or_else(|e| e.into_inner());
            node.state.lock().unwrap_or_else(|e| e.into_inner()).retired = true;
        }
        let _ = std::fs::remove_file(self.directory.join(format!("{id}.sessions.json")));
    }
    pub async fn flush(&self, only: Option<&str>, force: bool) {
        let nodes: Vec<_> = self
            .nodes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(id, _)| only.is_none_or(|wanted| wanted == id.as_str()))
            .map(|(id, node)| (id.clone(), node.clone()))
            .collect();
        let dir = self.directory.clone();
        let interval = self.interval;
        let _ = tokio::task::spawn_blocking(move || {
            for (id, node) in nodes {
                // A cancelled caller cannot leave an older write racing a flush.
                let _writer = node.writer.lock().unwrap_or_else(|e| e.into_inner());
                let pending = {
                    let mut state = node.state.lock().unwrap_or_else(|e| e.into_inner());
                    if state.retired
                        || (!force && state.last_attempt.is_some_and(|at| at.elapsed() < interval))
                    {
                        continue;
                    }
                    let Some(pending) = state.pending.clone() else {
                        continue;
                    };
                    let sig = pending.1.get("sig").unwrap_or(&Value::Null);
                    if crate::json_compat::truthy(sig)
                        && state.saved_sig.as_ref() == Some(sig)
                        && dir.join(format!("{id}.sessions.json")).exists()
                    {
                        state.pending = None;
                        continue;
                    }
                    state.last_attempt = Some(Instant::now());
                    pending
                };
                let (stamp, data) = pending;
                let saved = write_snapshot_file(&dir, &id, stamp, &data).is_ok();
                let mut state = node.state.lock().unwrap_or_else(|e| e.into_inner());
                state.last_attempt = Some(Instant::now());
                if saved {
                    state.saved_sig = Some(data.get("sig").cloned().unwrap_or(Value::Null));
                    if state
                        .pending
                        .as_ref()
                        .is_some_and(|(_, latest)| Arc::ptr_eq(latest, &data))
                    {
                        state.pending = None;
                    }
                }
                // Failures retain the latest pending value and retry at the
                // normal interval, without failing the online request.
            }
        })
        .await;
    }
    fn delay(&self) -> Duration {
        self.nodes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter_map(|node| {
                let state = node.state.lock().unwrap_or_else(|e| e.into_inner());
                state.pending.as_ref()?;
                Some(state.last_attempt.map_or(Duration::ZERO, |at| {
                    self.interval.saturating_sub(at.elapsed())
                }))
            })
            .min()
            .unwrap_or(self.interval)
    }
    pub async fn run(self: Arc<Self>, stop: CancellationToken) {
        loop {
            tokio::select! {
                _ = stop.cancelled() => break,
                _ = self.wake.notified() => {},
                _ = tokio::time::sleep(self.delay()) => {},
            }
            self.flush(None, false).await;
        }
    }
}
