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
    #[cfg(test)]
    reads: std::sync::atomic::AtomicUsize,
}

impl WatchHub {
    pub fn new(reader: Reader, shutdown: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            reader,
            shutdown,
            registry: Mutex::new(Registry::default()),
            workers: Semaphore::new(2),
            interval: Duration::from_millis(500),
            #[cfg(test)]
            reads: std::sync::atomic::AtomicUsize::new(0),
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
                if registry.views.len() >= 32 {
                    return Err(ApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "view_limit",
                        "同时观察的逻辑视图过多",
                    ));
                }
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
            #[cfg(test)]
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::{MessageQuery, SessionRoots, SessionStore};
    use serde_json::json;
    use std::{fs, io::Write, path::PathBuf, sync::atomic::Ordering};

    fn setup(interval: Duration) -> (tempfile::TempDir, PathBuf, Arc<WatchHub>, String) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("project/session.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            format!(
                "{}\n",
                json!({"type":"user","uuid":"u1","parentUuid":null,
            "message":{"content":"shared initial question"}})
            ),
        )
        .unwrap();
        let store = Arc::new(SessionStore::new(SessionRoots {
            claude: Some(temp.path().into()),
            ..Default::default()
        }));
        let uid = store.list(false).unwrap()["sessions"][0]["uid"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut hub = WatchHub::new(
            Reader {
                store,
                workers: Arc::new(Semaphore::new(4)),
                wait: Duration::from_secs(10),
            },
            CancellationToken::new(),
        );
        Arc::get_mut(&mut hub).unwrap().interval = interval;
        (temp, path, hub, uid)
    }

    #[tokio::test]
    async fn twenty_browsers_share_one_initial_read_and_publisher() {
        let (_temp, _path, hub, uid) = setup(Duration::from_secs(60));
        let mut subscriptions = Vec::new();
        for _ in 0..20 {
            subscriptions.push(hub.subscribe(uid.clone(), String::new()).await.unwrap());
        }
        assert_eq!(hub.reads.load(Ordering::SeqCst), 1);
        assert_eq!(hub.registry.lock().unwrap().views.len(), 1);
        let first = subscriptions[0].current().unwrap();
        for subscription in &mut subscriptions {
            assert!(Arc::ptr_eq(&first, &subscription.current().unwrap()));
        }
        drop(subscriptions);
        assert!(hub.registry.lock().unwrap().views.is_empty());
        let mut replacement = hub.subscribe(uid, String::new()).await.unwrap();
        assert_eq!(hub.reads.load(Ordering::SeqCst), 2);
        assert!(replacement.current().is_ok());
        assert_eq!(hub.registry.lock().unwrap().views.len(), 1);
    }

    #[tokio::test]
    async fn slow_subscriber_gets_latest_snapshot_without_blocking_active_subscriber() {
        let (_temp, path, hub, uid) = setup(Duration::from_millis(20));
        let mut fast = hub.subscribe(uid.clone(), String::new()).await.unwrap();
        let mut slow = hub.subscribe(uid, String::new()).await.unwrap();
        let first = slow
            .current()
            .unwrap()
            .messages(&MessageQuery::default())
            .unwrap();
        fast.current().unwrap();
        for (id, parent, text) in [("a1", "u1", "first append"), ("a2", "a1", "second append")] {
            writeln!(
                fs::OpenOptions::new().append(true).open(&path).unwrap(),
                "{}",
                json!({"type":"assistant","uuid":id,"parentUuid":parent,"message":{"content":text}})
            )
            .unwrap();
            tokio::time::timeout(Duration::from_secs(2), fast.changed())
                .await
                .unwrap()
                .unwrap();
            assert!(
                fast.current()
                    .unwrap()
                    .messages(&MessageQuery::default())
                    .unwrap()["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| message["text"] == text)
            );
        }
        // The slow reader has not consumed the intermediate version at all.
        slow.changed().await.unwrap();
        let current = slow.current().unwrap();
        let delta = current
            .messages(&MessageQuery {
                start: first["end"].as_u64().unwrap(),
                head: first["version"]["head"].as_str().unwrap().into(),
                anchor: first["anchor"].as_str().unwrap().into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(delta["reset"], false);
        assert_eq!(delta["messages"].as_array().unwrap().len(), 2);
        assert_eq!(delta["messages"][1]["text"], "second append");
        assert_eq!(hub.registry.lock().unwrap().views.len(), 1);
    }

    #[tokio::test]
    async fn errors_are_delivered_and_repaired_retry_gets_a_new_publisher() {
        let (_temp, path, hub, uid) = setup(Duration::from_millis(20));
        let original = fs::read(&path).unwrap();
        let mut old = hub.subscribe(uid.clone(), String::new()).await.unwrap();
        old.current().unwrap();
        // A shape the reference adapter cannot read either (scalar `content`);
        // a non-JSON line would only be a note since batch 35.
        writeln!(
            fs::OpenOptions::new().append(true).open(&path).unwrap(),
            "{}",
            json!({"type":"user","uuid":"u2","parentUuid":"u1","message":{"content":42}})
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), old.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            old.current().err().unwrap().status,
            StatusCode::NOT_IMPLEMENTED
        );
        fs::write(&path, original).unwrap();
        let mut retry = hub.subscribe(uid, String::new()).await.unwrap();
        drop(old); // Old cleanup must not remove the newer publisher.
        assert!(retry.current().is_ok());
        assert_eq!(hub.registry.lock().unwrap().views.len(), 1);
        hub.shutdown.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), retry.changed())
                .await
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn cancelled_initial_subscribe_does_not_leave_a_registry_entry() {
        let (_temp, _path, hub, uid) = setup(Duration::from_secs(60));
        let block = hub
            .reader
            .workers
            .clone()
            .acquire_many_owned(4)
            .await
            .unwrap();
        let copy = hub.clone();
        let task = tokio::spawn(async move { copy.subscribe(uid, String::new()).await });
        for _ in 0..100 {
            if !hub.registry.lock().unwrap().views.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(hub.registry.lock().unwrap().views.len(), 1);
        task.abort();
        match task.await {
            Err(error) => assert!(error.is_cancelled()),
            Ok(_) => panic!("initial subscription should still be waiting"),
        }
        assert!(hub.registry.lock().unwrap().views.is_empty());
        drop(block);
    }
}
