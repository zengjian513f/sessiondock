//! Provider identity belongs to the native process as well as the thread.
//! Keep the user's shared daemon; start additional listeners only on demand.
use super::{Connection, Endpoint, Inner, RpcError};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub(super) fn next_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn provider_name(provider: &str) -> String {
    provider
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl Endpoint {
    pub(super) fn for_provider(&self, provider: &str) -> Option<Self> {
        let home = self.autostart_home.as_ref()?;
        Some(Self {
            socket: home
                .join("sd-providers")
                .join(format!("{}.sock", provider_name(provider))),
            autostart_home: Some(home.clone()),
            explicit: false,
            provider: Some(provider.to_owned()),
        })
    }
}

impl Inner {
    pub(super) async fn connected(&self) -> Vec<Arc<Connection>> {
        let mut connections: Vec<_> = self.connection.lock().await.iter().cloned().collect();
        connections.extend(self.providers.lock().await.values().cloned());
        connections.retain(|c| !c.is_closed());
        connections
    }

    pub(super) async fn provider_connection(
        self: &Arc<Self>,
        provider: &str,
        autostart: bool,
    ) -> Result<Arc<Connection>, RpcError> {
        // Explicit integrations own their listener lifecycle; never silently
        // launch another process/home behind an operator-supplied endpoint.
        if self.endpoint.explicit {
            return self.connection_mode(autostart).await;
        }
        // Reuse a matching shared daemon, including the user's cache provider.
        if let Ok(shared) = self.connection_mode(false).await {
            let config = shared
                .request("config/read", json!({"includeLayers":false}))
                .await?;
            if config["config"]["model_provider"]
                .as_str()
                .unwrap_or("openai")
                == provider
            {
                return Ok(shared);
            }
        }
        let mut pool = self.providers.lock().await;
        if let Some(connection) = pool.get(provider).filter(|c| !c.is_closed()) {
            return Ok(connection.clone());
        }
        let mut endpoint = self
            .endpoint
            .for_provider(provider)
            .ok_or(RpcError::Disconnected)?;
        if !autostart {
            endpoint.autostart_home = None;
        }
        let connection = Connection::open(self.clone(), &endpoint, next_generation()).await?;
        pool.insert(provider.to_owned(), connection.clone());
        Ok(connection)
    }

    pub(super) async fn thread_connection(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<Arc<Connection>, RpcError> {
        let loaded = self
            .threads
            .lock()
            .expect("threads")
            .get(id)
            .and_then(|t| t.loaded_on());
        if let Some(connection) = self
            .connected()
            .await
            .into_iter()
            .find(|c| Some(c.generation()) == loaded)
        {
            return Ok(connection);
        }
        let provider = self.routes.lock().expect("routes").get(id).cloned();
        if let Some(provider) = provider {
            return self.provider_connection(&provider, true).await;
        }
        // A still-loaded native thread owns its writer even if configuration has
        // changed since it started. Discovery never resumes or sends input.
        if let Ok(connection) = self.writing_connection(id).await {
            return Ok(connection);
        }
        let reader = if self.endpoint.explicit {
            self.connection().await?
        } else {
            self.provider_connection("openai", true).await?
        };
        let native = reader
            .request("thread/read", json!({"threadId":id,"includeTurns":false}))
            .await?;
        let provider = native["thread"]["modelProvider"]
            .as_str()
            .filter(|p| !p.is_empty())
            .unwrap_or("openai")
            .to_owned();
        let connection = self.provider_connection(&provider, true).await?;
        self.routes
            .lock()
            .expect("routes")
            .insert(id.to_owned(), provider);
        Ok(connection)
    }

    /// Include listeners from before a service restart, without launching any.
    pub(super) async fn observed_connections(self: &Arc<Self>) -> Vec<Arc<Connection>> {
        let _ = self.connection_mode(false).await;
        if let Some(home) = &self.endpoint.autostart_home {
            let paths: Vec<PathBuf> = std::fs::read_dir(home.join("sd-providers"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .collect();
            for path in paths {
                if path.extension().and_then(|s| s.to_str()) != Some("sock") {
                    continue;
                }
                let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let bytes: Option<Vec<_>> = name
                    .as_bytes()
                    .chunks(2)
                    .map(|pair| {
                        std::str::from_utf8(pair)
                            .ok()
                            .and_then(|s| u8::from_str_radix(s, 16).ok())
                    })
                    .collect();
                if let Some(provider) = bytes.and_then(|b| String::from_utf8(b).ok()) {
                    // Connect the exact persistent endpoint, without a config
                    // read on an unrelated daemon for each observer interval.
                    let mut pool = self.providers.lock().await;
                    if pool.get(&provider).is_some_and(|c| !c.is_closed()) {
                        continue;
                    }
                    let endpoint = Endpoint {
                        socket: path,
                        autostart_home: None,
                        explicit: false,
                        provider: Some(provider.clone()),
                    };
                    if let Ok(c) =
                        Connection::open(self.clone(), &endpoint, next_generation()).await
                    {
                        pool.insert(provider, c);
                    }
                }
            }
        }
        self.connected().await
    }

    pub(super) async fn writing_connection(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<Arc<Connection>, RpcError> {
        for connection in self.observed_connections().await {
            if let Ok(result) = connection.request("thread/loaded/list", json!({})).await {
                if result["data"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|v| v.as_str() == Some(id)))
                {
                    return Ok(connection);
                }
            }
        }
        // Older explicit integrations may not implement loaded/list.
        if self.endpoint.explicit {
            return self.connection_mode(false).await;
        }
        Err(RpcError::Unavailable(
            "会话未在任何 Codex provider 进程中加载".into(),
        ))
    }
}
