//! One bounded publisher per logical view. Subscribers keep their own cursor;
//! the watch slot only retains the latest immutable source version.

use axum::http::StatusCode;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::CancellationToken;

use crate::{error::ApiError, sessions::ViewSnapshot, state::Reader};

type Key = (String, String);

#[derive(Clone)]
enum Published {
    Loading,
    Ready(Arc<ViewSnapshot>),
    Failed(ApiError),
}

struct Entry {
    id: u64,
    subscribers: usize,
    sender: watch::Sender<Published>,
    cancel: CancellationToken,
}

#[derive(Default)]
struct Registry {
    next: u64,
    views: BTreeMap<Key, Entry>,
}

pub struct WatchHub {
    reader: Reader,
    shutdown: CancellationToken,
    registry: Mutex<Registry>,
    workers: Semaphore,
    interval: Duration,
}

impl WatchHub {
    pub fn new(reader: Reader, shutdown: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            reader,
            shutdown,
            registry: Mutex::new(Registry::default()),
            workers: Semaphore::new(2),
            interval: Duration::from_millis(500),
        })
    }

    pub async fn subscribe(
        self: &Arc<Self>,
        uid: String,
        agent: String,
    ) -> Result<Subscription, ApiError> {
        if self.shutdown.is_cancelled() {
            return Err(closed());
        }
        let key = (uid, agent);
        let (id, receiver, launch) = {
            let mut registry = self.registry.lock().map_err(|_| closed())?;
            if let Some(entry) = registry.views.get_mut(&key) {
                entry.subscribers += 1;
                (entry.id, entry.sender.subscribe(), None)
            } else {
                registry.next = registry.next.checked_add(1).ok_or_else(closed)?;
                let id = registry.next;
                let (sender, receiver) = watch::channel(Published::Loading);
                let cancel = self.shutdown.child_token();
                registry.views.insert(
                    key.clone(),
                    Entry {
                        id,
                        subscribers: 1,
                        sender: sender.clone(),
                        cancel: cancel.clone(),
                    },
                );
                (id, receiver, Some((sender, cancel)))
            }
        };
        let mut subscription = Subscription {
            hub: self.clone(),
            key: key.clone(),
            id,
            receiver,
        };
        if let Some((sender, cancel)) = launch {
            let hub = self.clone();
            tokio::spawn(async move {
                hub.publish(key, id, sender, cancel).await;
            });
        }
        subscription.ready().await?;
        Ok(subscription)
    }

    async fn publish(
        self: Arc<Self>,
        key: Key,
        id: u64,
        sender: watch::Sender<Published>,
        cancel: CancellationToken,
    ) {
        let mut revision = String::new();
        loop {
            let admission = tokio::select! {
                _ = cancel.cancelled() => break,
                permit = self.workers.acquire() => match permit {Ok(permit) => permit, Err(_) => break},
            };
            let (uid, agent) = key.clone();
            let result = self
                .reader
                .run_wait(&cancel, move |store| store.snapshot(&uid, &agent))
                .await;
            drop(admission);
            if cancel.is_cancelled() {
                break;
            }
            match result {
                Ok(snapshot) if snapshot.revision() != revision => {
                    revision = snapshot.revision().to_owned();
                    sender.send_replace(Published::Ready(snapshot));
                }
                Ok(_) => {}
                Err(error) => {
                    sender.send_replace(Published::Failed(error));
                    break;
                }
            }
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(self.interval) => {},
            }
        }
        // A failed/expired publisher must not remove a replacement registration.
        if let Ok(mut registry) = self.registry.lock()
            && registry.views.get(&key).is_some_and(|entry| entry.id == id)
        {
            registry.views.remove(&key);
        }
    }
}

pub struct Subscription {
    hub: Arc<WatchHub>,
    key: Key,
    id: u64,
    receiver: watch::Receiver<Published>,
}

impl Subscription {
    async fn ready(&mut self) -> Result<(), ApiError> {
        loop {
            let current = self.receiver.borrow().clone();
            match current {
                Published::Loading => {}
                Published::Ready(_) => return Ok(()),
                Published::Failed(error) => return Err(error),
            }
            tokio::select! {
                _ = self.hub.shutdown.cancelled() => return Err(closed()),
                changed = self.receiver.changed() => changed.map_err(|_| closed())?,
            }
        }
    }

    pub fn current(&mut self) -> Result<Arc<ViewSnapshot>, ApiError> {
        match self.receiver.borrow_and_update().clone() {
            Published::Ready(snapshot) => Ok(snapshot),
            Published::Failed(error) => Err(error),
            Published::Loading => Err(closed()),
        }
    }

    /// Inspect the latest view without acknowledging its history notification.
    /// Side-channel probes must leave it pending for the message consumer.
    pub fn peek(&self) -> Result<Arc<ViewSnapshot>, ApiError> {
        match self.receiver.borrow().clone() {
            Published::Ready(snapshot) => Ok(snapshot),
            Published::Failed(error) => Err(error),
            Published::Loading => Err(closed()),
        }
    }

    pub async fn changed(&mut self) -> Result<(), ApiError> {
        self.receiver.changed().await.map_err(|_| closed())
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Ok(mut registry) = self.hub.registry.lock()
            && let Some(entry) = registry.views.get_mut(&self.key)
            && entry.id == self.id
        {
            entry.subscribers -= 1;
            if entry.subscribers == 0 {
                entry.cancel.cancel();
                registry.views.remove(&self.key);
            }
        }
    }
}

fn closed() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "watch_closed",
        "会话观察已关闭，请重试",
    )
}
