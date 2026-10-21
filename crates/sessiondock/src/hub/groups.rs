//! Nodes own group catalogs. The Hub caches their union and sends it back;
//! assignments never leave the session's node.
use super::{Client, Registry};
use crate::metadata::{GroupCatalog, GroupCatalogUpdate};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub struct Groups {
    catalog: Mutex<GroupCatalog>,
    cache: PathBuf,
    /// Generation of the latest save request, assigned under `catalog`.
    saves: AtomicU64,
    /// Generation last written to `cache`.
    written: Arc<std::sync::Mutex<u64>>,
}
impl Groups {
    pub fn open(cache: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(cache.parent().unwrap())?;
        let catalog = fs::read(&cache)
            .or_else(|_| fs::read(cache.with_file_name("labels.json")))
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default();
        Ok(Self {
            catalog: Mutex::new(catalog),
            cache,
            saves: AtomicU64::new(0),
            written: Default::default(),
        })
    }
    /// Write the cache off the runtime: `sync_all` and the directory fsync
    /// block for as long as the disk takes. Call with the catalog locked. A
    /// cancelled sync leaves its write running, so writes share one temporary
    /// file under `written`, and one that lost the race to a newer catalog
    /// is skipped instead of overwriting it.
    async fn save(&self, catalog: GroupCatalog) -> std::io::Result<()> {
        let cache = self.cache.clone();
        let written = self.written.clone();
        let generation = self.saves.fetch_add(1, Ordering::Relaxed) + 1;
        tokio::task::spawn_blocking(move || {
            let mut written = written.lock().unwrap_or_else(|e| e.into_inner());
            if *written > generation {
                return Ok(());
            }
            save(&cache, &catalog)?;
            *written = generation;
            Ok(())
        })
        .await
        .map_err(std::io::Error::other)?
    }
    pub async fn sync(
        &self,
        registry: &Registry,
        client: &Client,
        incoming: Option<GroupCatalogUpdate>,
    ) -> Value {
        // Read every node without holding the catalog: a slow or unreachable
        // node must not stall concurrent syncs and user edits for its timeout.
        // Merging is last-writer-wins per group, so reads taken before another
        // sync published its catalog are merged safely afterwards.
        let reads = stream::iter(registry.all().into_iter().map(|node| async move {
            let result = registry
                .request(
                    client,
                    &node,
                    "/api/groups",
                    "GET",
                    None,
                    Duration::from_secs(3),
                )
                .await;
            (node, result)
        }))
        .buffer_unordered(16)
        .collect::<Vec<_>>()
        .await;
        // The catalog lock serializes merge, publication and save, so the
        // cache on disk never regresses to an older catalog.
        let mut catalog = self.catalog.lock().await;
        let mut ready = Vec::new();
        let mut errors = Vec::new();
        for (node, result) in reads {
            if let Ok((200, value)) = result
                && let Ok(remote) = serde_json::from_value::<GroupCatalog>(value)
            {
                if let Err(error) = catalog.merge(&remote) {
                    errors.push(json!({"node_id": node.id, "error": error.message}));
                    continue;
                }
                ready.push(node);
                continue;
            }
            errors
                .push(json!({"node_id": node.id, "name": node.name, "error": "分组集合暂未同步"}));
        }
        if let Some(incoming) = incoming {
            let mut next = catalog.clone();
            if let Err(error) = next.apply(&incoming) {
                return json!({"ok": false, "error": error.message});
            }
            *catalog = next;
        }
        let body = json!(&*catalog);
        let writes = stream::iter(ready.into_iter().map(|node| {
            let body = &body;
            async move {
                let result = registry
                    .request(
                        client,
                        &node,
                        "/api/groups",
                        "POST",
                        Some(body),
                        Duration::from_secs(3),
                    )
                    .await;
                (node, result)
            }
        }))
        .buffer_unordered(16)
        .collect::<Vec<_>>()
        .await;
        let mut synced = 0;
        for (node, result) in writes {
            if matches!(result, Ok((200, _))) {
                synced += 1;
            } else {
                errors.push(
                    json!({"node_id": node.id, "name": node.name, "error": "分组集合下发未完成"}),
                );
            }
        }
        if let Err(error) = self.save(catalog.clone()).await {
            errors.push(json!({"error": format!("分组缓存保存失败：{error}")}));
        }
        json!({"ok": synced > 0, "groups": catalog.groups, "changes": catalog.changes,
            "synced_nodes": synced, "sync_errors": errors})
    }
    pub fn spawn(
        self: Arc<Self>,
        registry: Arc<Registry>,
        client: Arc<Client>,
        shutdown: CancellationToken,
    ) {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(10));
            // A sync slower than the period must not be followed by a burst of
            // catch-up syncs.
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tick.tick() => { self.sync(&registry, &client, None).await; }
                }
            }
        });
    }
}

fn save(cache: &Path, catalog: &GroupCatalog) -> std::io::Result<()> {
    let temporary = cache.with_extension("tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&serde_json::to_vec(catalog)?)?;
    file.sync_all()?;
    fs::rename(&temporary, cache)?;
    #[cfg(unix)]
    fs::File::open(cache.parent().unwrap())?.sync_all()?;
    Ok(())
}
