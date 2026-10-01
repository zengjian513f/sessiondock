//! Short registry locks; work is serialized only for overlapping identities.
use super::TransferError;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
#[derive(Default)]
pub struct Locks(Mutex<BTreeMap<String, Weak<AsyncMutex<()>>>>);
impl Locks {
    fn handles(&self, mut keys: Vec<String>) -> Vec<Arc<AsyncMutex<()>>> {
        keys.sort();
        keys.dedup();
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, lock| lock.strong_count() > 0);
        keys.into_iter()
            .map(|key| {
                let lock = map
                    .get(&key)
                    .and_then(Weak::upgrade)
                    .unwrap_or_else(|| Arc::new(AsyncMutex::new(())));
                map.insert(key, Arc::downgrade(&lock));
                lock
            })
            .collect()
    }
    pub async fn acquire(&self, keys: Vec<String>) -> Vec<OwnedMutexGuard<()>> {
        let mut guards = Vec::new();
        for lock in self.handles(keys) {
            guards.push(lock.lock_owned().await);
        }
        guards
    }
    pub fn blocking(&self, keys: Vec<String>) -> Vec<OwnedMutexGuard<()>> {
        self.handles(keys)
            .into_iter()
            .map(|lock| lock.blocking_lock_owned())
            .collect()
    }
}
#[derive(Default)]
pub struct Interrupts(Mutex<BTreeMap<String, Arc<AtomicBool>>>);
impl Interrupts {
    pub fn flag(&self, id: &str) -> Arc<AtomicBool> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.into())
            .or_default()
            .clone()
    }
    pub fn cancel(&self, id: &str) {
        self.flag(id).store(true, Ordering::Release);
    }
    pub fn reset(&self, id: &str) {
        self.flag(id).store(false, Ordering::Release);
    }
}
thread_local! { static CURRENT: std::cell::RefCell<Option<Arc<AtomicBool>>> = const { std::cell::RefCell::new(None) }; }
pub struct Scope(Option<Arc<AtomicBool>>);
impl Scope {
    pub fn enter(flag: Arc<AtomicBool>) -> Self {
        Self(CURRENT.with(|slot| slot.replace(Some(flag))))
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|slot| slot.replace(self.0.take()));
    }
}
pub fn check() -> Result<(), TransferError> {
    if CURRENT.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
    }) {
        return Err(TransferError::new(
            "move_cancelled",
            "操作已取消，源会话保留",
        ));
    }
    Ok(())
}
