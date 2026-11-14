//! Durable outbox shared by native protocol adapters. Identity selection stays in the adapter.
use super::save_atomic;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone)]
pub(crate) struct Sender {
    pub(crate) entries: Arc<Mutex<Vec<Entry>>>,
    pub(crate) path: PathBuf,
    pub(crate) wake: Arc<tokio::sync::Notify>,
    pub(crate) closed: Arc<AtomicBool>,
    pub(crate) identity: fn(&Value) -> Option<String>,
    pub(crate) source: &'static str,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) frame: Value,
}
impl Sender {
    pub fn send(&self, frame: Value) -> Result<(), String> {
        if self.closed.load(Ordering::Acquire) {
            return Err(format!("{}_sdk_disconnected", self.source));
        }
        let mut entries = lock(&self.entries);
        let id = (self.identity)(&frame)
            .map(Ok)
            .unwrap_or_else(|| super::uuid_v4().map_err(|e| e.to_string()))?;
        if entries.iter().any(|entry| entry.id == id) {
            return Ok(());
        }
        entries.push(Entry { id, frame });
        if let Err(error) = save_atomic(&self.path, &*entries) {
            // The writer reads this same queue: never expose an entry whose
            // persistence failed, or let its retry look already accepted.
            entries.pop();
            return Err(format!("{}_sdk_outbox: {error}", self.source));
        }
        self.wake.notify_one();
        Ok(())
    }
}
