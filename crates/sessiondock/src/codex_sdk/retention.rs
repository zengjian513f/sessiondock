//! Persist subscription ownership, not native history or input. A closed page
//! must not start the daemon's last-subscriber inactivity countdown.
use super::{Inner, RpcError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default, Serialize, Deserialize)]
struct Record {
    socket: PathBuf,
    threads: HashSet<String>,
}

pub(super) struct Retention {
    path: Option<PathBuf>,
    record: Mutex<Record>,
}

impl Retention {
    pub fn open(directory: Option<PathBuf>, socket: &Path) -> io::Result<Self> {
        let path = directory.map(|dir| dir.join("codex-sdk-subscriptions.json"));
        let mut record: Record = match path.as_ref().map(fs::read) {
            Some(Ok(bytes)) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Some(Err(error)) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => Record::default(),
        };
        if record.socket != socket {
            record = Record {
                socket: socket.to_owned(),
                threads: HashSet::new(),
            };
        }
        Ok(Self {
            path,
            record: Mutex::new(record),
        })
    }

    fn ids(&self) -> HashSet<String> {
        self.record.lock().expect("subscriptions").threads.clone()
    }

    pub fn set(&self, id: &str, retain: bool) -> Result<(), RpcError> {
        let mut record = self.record.lock().expect("subscriptions");
        if record.threads.contains(id) == retain {
            return Ok(());
        }
        let mut next = Record {
            socket: record.socket.clone(),
            threads: record.threads.clone(),
        };
        if retain {
            next.threads.insert(id.to_owned());
        } else {
            next.threads.remove(id);
        }
        if let Some(path) = &self.path {
            let save = || -> io::Result<()> {
                let temporary = path.with_extension("tmp");
                let mut options = fs::OpenOptions::new();
                options.create(true).truncate(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&temporary)?;
                file.write_all(&serde_json::to_vec(&next)?)?;
                file.sync_all()?;
                fs::rename(temporary, path)?;
                #[cfg(unix)]
                fs::File::open(path.parent().expect("subscription directory"))?.sync_all()?;
                Ok(())
            };
            save().map_err(|error| RpcError::Storage(format!("Codex 订阅保存失败：{error}")))?;
        }
        *record = next;
        Ok(())
    }
}

impl Inner {
    pub(super) async fn restore_subscriptions(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => return,
                _ = tick.tick() => {},
            }
            tokio::select! {
                _ = self.shutdown.cancelled() => return,
                _ = self.restore_loaded_subscriptions() => {},
            }
        }
    }

    async fn restore_loaded_subscriptions(self: &Arc<Self>) {
        let ids = self.retention.ids();
        if ids.is_empty() {
            return;
        }
        // Recovery does not start a daemon, or load arbitrary historical threads.
        for connection in self.observed_connections().await {
            self.restore_on(&connection, &ids).await;
        }
    }

    async fn restore_on(
        self: &Arc<Self>,
        connection: &Arc<super::Connection>,
        ids: &HashSet<String>,
    ) {
        let missing: Vec<_> = {
            let threads = self.threads.lock().expect("threads");
            ids.iter()
                .cloned()
                .filter(|id| {
                    !threads
                        .get(id)
                        .is_some_and(|t| t.loaded_on() == Some(connection.generation()))
                })
                .collect()
        };
        if missing.is_empty() {
            return;
        }
        let Ok(result) = connection.request("thread/loaded/list", json!({})).await else {
            return;
        };
        let Some(loaded) = result["data"].as_array() else {
            return;
        };
        for id in missing {
            let lock = self.queue.lock(&id);
            let _dispatch = lock.lock().await;
            // Serialize with explicit stop/discard, and recheck after waiting.
            if !self.retention.ids().contains(&id) || connection.is_closed() {
                continue;
            }
            if !loaded
                .iter()
                .any(|value| value.as_str() == Some(id.as_str()))
            {
                continue;
            }
            // ensure restores metadata/receipts only; it never replays input.
            if let Err(error) = self.ensure_on(&connection, &id).await {
                eprintln!("Codex subscription recovery: {error}");
            }
        }
    }
}
