//! Nodes own group catalogs. The Hub caches their union and sends it back;
//! assignments never leave the session's node.
use super::{Client, Registry};
use crate::metadata::{GroupCatalog, MetadataSnapshot};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use std::{fs, io::Write, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub struct Groups {
    catalog: Mutex<GroupCatalog>,
    cache: PathBuf,
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
        })
    }
    fn save(&self, catalog: &GroupCatalog) -> std::io::Result<()> {
        let temporary = self.cache.with_extension("tmp");
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
        fs::rename(&temporary, &self.cache)?;
        #[cfg(unix)]
        fs::File::open(self.cache.parent().unwrap())?.sync_all()?;
        Ok(())
    }
    pub async fn sync(
        &self,
        registry: &Registry,
        client: &Client,
        incoming: Option<GroupCatalog>,
    ) -> Value {
        let mut catalog = self.catalog.lock().await;
        if let Some(incoming) = incoming {
            // Same normalization and validation as nodes, before any write.
            match MetadataSnapshot::empty().with_group_catalog(&incoming) {
                Ok(snapshot) => {
                    let incoming = snapshot.group_catalog();
                    catalog.groups.extend(incoming.groups);
                }
                Err(error) => return json!({"ok": false, "error": error.message}),
            }
        }
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
        let mut ready = Vec::new();
        let mut errors = Vec::new();
        for (node, result) in reads {
            if let Ok((200, value)) = result
                && let Ok(remote) = serde_json::from_value::<GroupCatalog>(value)
            {
                catalog.groups.extend(remote.groups);
                ready.push(node);
                continue;
            }
            errors
                .push(json!({"node_id": node.id, "name": node.name, "error": "分组集合暂未同步"}));
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
        if let Err(error) = self.save(&catalog) {
            errors.push(json!({"error": format!("分组缓存保存失败：{error}")}));
        }
        json!({"ok": synced > 0, "groups": catalog.groups,
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
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tick.tick() => { self.sync(&registry, &client, None).await; }
                }
            }
        });
    }
}
