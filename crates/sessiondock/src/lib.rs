//! Loopback development HTTP crate: config, router, and optional isolated services.
//! `app` is synchronous and rejects lifecycle/delivery configuration; `prepare_app`
//! opens existing private ledgers before publishing routes. Caller owns shutdown
//! of delivery, lifecycle, and audit. No home discovery, CLI launch, or ledger init.
//! With a node identity configured the same state also backs a
//! second router for the node listener (`node_auth` gate, no static page).
//! The hub binary has its own configuration and router:
//! `hub_config::HubConfig` + `hub_api::hub_app`.

mod api;
mod assets;
pub mod audit;
pub mod bridge;
pub mod bug_report;
pub mod config;
pub mod conversation;
pub mod delivery;
mod error;
pub mod files;
pub mod fingerprint;
pub mod hub;
pub mod hub_config;
pub mod lifecycle;
pub mod media;
pub mod metadata;
mod native_replay;
mod observe;
pub mod polls;
pub mod runtime;
pub mod search;
mod security;
pub mod sessions;
mod state;
pub mod terminal;
pub mod trash;

use std::{io, sync::Arc};

use axum::{Router, extract::DefaultBodyLimit, middleware};
use tokio::sync::Semaphore;

use config::Config;
use state::{AppState, Reader};

/// The hub's HTTP surface and constructors (`sessiondock-hub` binary).
pub use api::hub as hub_api;

pub fn app(config: Config) -> io::Result<Router> {
    app_with_shutdown(config, tokio_util::sync::CancellationToken::new())
}

/// Both listeners' routers from the synchronous factory: the loopback router
/// and, only with the node identity configured, the node listener's.
pub fn app_pair(config: Config) -> io::Result<(Router, Option<Router>)> {
    app_pair_with_shutdown(config, tokio_util::sync::CancellationToken::new())
}

pub fn app_with_shutdown(
    config: Config,
    shutdown: tokio_util::sync::CancellationToken,
) -> io::Result<Router> {
    app_pair_with_shutdown(config, shutdown).map(|(router, _node)| router)
}

pub fn app_pair_with_shutdown(
    config: Config,
    shutdown: tokio_util::sync::CancellationToken,
) -> io::Result<(Router, Option<Router>)> {
    config.validate()?;
    if config.delivery_dir.is_some()
        || config.lifecycle_dir.is_some()
        || config.launcher_config.is_some()
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "configured lifecycle/delivery services require the asynchronous prepare_app factory",
        ));
    }
    let terminal = prepare_terminal(&config)?;
    let built = build_app(config, shutdown.clone(), None, None, terminal, Vec::new())?;
    // The synchronous factory runs inside test runtimes; without one the
    // `/api/live` handler still records spawners on every call.
    if let Some(start) = built.spawn_watch
        && tokio::runtime::Handle::try_current().is_ok()
    {
        start.watcher.spawn_loop(start.reader, shutdown);
    }
    Ok((built.router, built.node_router))
}

/// The spawner watch loop, started by the caller from an async context.
struct SpawnStart {
    watcher: Arc<runtime::spawn::SpawnWatcher>,
    reader: Reader,
}

/// Startup ownership remains with the caller, which must await delivery
/// shutdown after stopping admission to the HTTP server.
pub struct PreparedApp {
    pub router: Router,
    /// The node listener's router (`SESSIONDOCK_NODE_BIND`); `None` unless the
    /// identity, credential and peer networks are all configured.
    pub node_router: Option<Router>,
    pub delivery: Option<Arc<delivery::service::DeliveryService>>,
    pub lifecycle: Option<Arc<lifecycle::service::LifecycleService>>,
    /// Bounded diagnostics writer; `shutdown()` drains it within its deadline.
    pub audit: Option<Arc<audit::AuditService>>,
}

pub async fn prepare_app(
    config: Config,
    shutdown: tokio_util::sync::CancellationToken,
) -> io::Result<PreparedApp> {
    let (config, launcher, terminal, adapters) = tokio::task::spawn_blocking(move || -> io::Result<_> {
        config.validate()?;
        let launcher = match (&config.lifecycle_dir, &config.launcher_config) {
            (Some(_), Some(path)) => {
                let launcher = lifecycle::launcher::read_config(path).map_err(io::Error::other)?;
                config.validate_launcher(&launcher)?;
                Some(launcher)
            }
            (None, None) => None,
            _ => return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "lifecycle startup requires both an initialized lifecycle directory and an explicit launcher configuration")),
        };
        let adapters = launcher.as_ref().map(|launcher| lifecycle::launcher::entries(launcher).into_iter()
            .map(|entry| state::LaunchAdapter { source:entry.source })
            .collect()).unwrap_or_default();
        let terminal = prepare_terminal(&config)?;
        Ok((config, launcher, terminal, adapters))
    }).await.map_err(io::Error::other)??;
    let delivery = match config.delivery_dir.clone() {
        Some(directory) => Some(Arc::new(
            delivery::service::DeliveryService::open(
                directory,
                Default::default(),
                shutdown.clone(),
            )
            .await
            .map_err(io::Error::other)?,
        )),
        None => None,
    };
    let lifecycle = match (config.lifecycle_dir.clone(), launcher) {
        (Some(directory), Some(launcher)) => {
            let result = lifecycle::service::LifecycleService::open(
                directory,
                launcher,
                terminal.clone().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "lifecycle requires an explicit terminal service",
                    )
                })?,
                Default::default(),
                shutdown.clone(),
            )
            .await;
            match result {
                Ok(service) => Some(Arc::new(service)),
                Err(error) => {
                    if let Some(service) = &delivery {
                        let _ = service.shutdown().await;
                    }
                    return Err(io::Error::other(error));
                }
            }
        }
        _ => None,
    };
    let worker_delivery = delivery.clone();
    let worker_lifecycle = lifecycle.clone();
    let shutdown_for_watch = shutdown.clone();
    let result = tokio::task::spawn_blocking(move || {
        build_app(
            config,
            shutdown,
            worker_delivery,
            worker_lifecycle,
            terminal,
            adapters,
        )
    })
    .await
    .map_err(io::Error::other)
    .and_then(|result| result);
    match result {
        Ok(built) => {
            // Native tracking runs on this runtime, spawned here from
            // the async context rather than the blocking build thread.
            if let Some(executor) = &built.executor {
                executor.spawn_tracker();
            }
            // The 10 s spawner tick, same context.
            if let Some(start) = built.spawn_watch {
                start.watcher.spawn_loop(start.reader, shutdown_for_watch);
            }
            // Process-evidence binding of pending Codex/Grok launches
            // (`new-status` resolution); no-op unless lifecycle,
            // managed runtime and the process scan are all configured.
            lifecycle::autobind::spawn(built.state);
            Ok(PreparedApp {
                router: built.router,
                node_router: built.node_router,
                delivery,
                lifecycle,
                audit: built.audit,
            })
        }
        Err(error) => {
            if let Some(service) = lifecycle {
                let _ = service.shutdown().await;
            }
            if let Some(service) = delivery {
                let _ = service.shutdown().await;
            }
            Err(error)
        }
    }
}

fn prepare_terminal(config: &Config) -> io::Result<Option<Arc<terminal::TerminalService>>> {
    let service = config
        .ptyhost_dir
        .clone()
        .map(terminal::TerminalService::new)
        .transpose()
        .map_err(io::Error::other)?;
    if let Some(service) = &service {
        // Agent hosts no longer record; sweep what earlier hosts left behind.
        match terminal::records::remove_agent_leftovers(service.directory()) {
            Ok(0) => {}
            Ok(removed) => eprintln!("sessiondock: removed {removed} agent session recordings"),
            Err(error) => eprintln!("sessiondock: agent recording sweep failed: {error}"),
        }
    }
    Ok(service.map(Arc::new))
}

/// build_app's outputs the caller drives: the routers plus the services whose
/// shutdown and (for the executor / spawn watcher) background task the caller owns.
struct BuiltApp {
    router: Router,
    node_router: Option<Router>,
    audit: Option<Arc<audit::AuditService>>,
    executor: Option<Arc<delivery::executor::DeliveryExecutor>>,
    spawn_watch: Option<SpawnStart>,
    /// The router's state, for background tasks started from the async context.
    state: AppState,
}

fn build_app(
    config: Config,
    shutdown: tokio_util::sync::CancellationToken,
    delivery: Option<Arc<delivery::service::DeliveryService>>,
    lifecycle: Option<Arc<lifecycle::service::LifecycleService>>,
    terminal: Option<Arc<terminal::TerminalService>>,
    launch_adapters: Vec<state::LaunchAdapter>,
) -> io::Result<BuiltApp> {
    let audit = config
        .audit_dir
        .clone()
        .map(|directory| {
            audit::AuditService::open(directory, config.audit_limits.clone(), shutdown.clone())
        })
        .transpose()?
        .map(Arc::new);
    let runtime = config
        .ptyhost_dir
        .as_ref()
        .map(|directory| {
            let client = ptyhost_client::HostClient::new(
                directory,
                ptyhost_client::Limits {
                    max_line_bytes: 4 * 1024 * 1024, // ptyhost protocol::MAX_LINE; a 1 MiB send plus its guard envelope
                    operation_timeout: std::time::Duration::from_secs(10),
                    ..Default::default()
                },
            )
            .map_err(io::Error::other)?;
            runtime::ManagedRuntime::new(client, Default::default())
                .map(Arc::new)
                .map_err(io::Error::other)
        })
        .transpose()?;
    let search_cache_dir = config.search_cache_dir.clone();
    let files = Some(Arc::new(
        files::FileService::open(config.file_roots)
            .and_then(|files| files.with_grants(config.state_dir.as_deref()))
            .map_err(io::Error::other)?,
    ));
    let files_write = if terminal.is_some() || !config.file_write_roots.is_empty() {
        Some(Arc::new(
            files::WriteService::open(
                config.file_write_roots,
                config.file_write_limits.clone(),
                config.state_dir.clone(),
            )
            .map_err(io::Error::other)?,
        ))
    } else {
        None
    };
    let metadata = config
        .state_dir
        .as_deref()
        .map(metadata::MetadataStore::open)
        .transpose()
        .map_err(io::Error::other)?
        .map(Arc::new);
    let mut capabilities = state::capabilities();
    capabilities["terminal_transport"] = serde_json::json!(terminal.is_some());
    capabilities["terminal"] = serde_json::json!(terminal.is_some());
    capabilities["terminal_records"] = serde_json::json!(terminal.is_some());
    capabilities["terminal_create"] = serde_json::json!(lifecycle.is_some());
    capabilities["terminal_pending"] = serde_json::json!(lifecycle.is_some());
    capabilities["terminal_bind"] = serde_json::json!(lifecycle.is_some());
    capabilities["terminal_takeover"] = serde_json::json!(lifecycle.is_some());
    capabilities["terminal_complete_dir"] = serde_json::json!(lifecycle.is_some());
    // Stop a managed instance through its host (Ctrl-D → guarded stop); never
    // an external process. Needs the terminal transport and the lifecycle service.
    capabilities["session_stop"] = serde_json::json!(lifecycle.is_some() && terminal.is_some());
    capabilities["terminal_backend"] = serde_json::json!(terminal.is_some());
    // Raw HTTP text/key input under the page's terminal lease; not a send ledger.
    capabilities["terminal_input"] = serde_json::json!(terminal.is_some());
    capabilities["metadata"] = serde_json::json!(metadata.is_some());
    // Display-only Claude timeline pins; never a native rewind of the CLI.
    capabilities["timeline_pin"] = serde_json::json!(metadata.is_some());
    capabilities["files"] = serde_json::json!(files.is_some());
    capabilities["files_jobs"] = serde_json::json!(files_write.is_some());
    capabilities["files_write"] = files_write
        .as_ref()
        .map_or(serde_json::json!(false), |service| service.capabilities());
    capabilities["file_thumbnails"] = serde_json::json!(false);
    capabilities["outbox_read"] = serde_json::json!(delivery.is_some());
    // The legacy composer/outbox needs both the initialized ledger
    // and the terminal transport (managed instances only).
    capabilities["outbox"] = serde_json::json!(delivery.is_some() && terminal.is_some());
    capabilities["audit"] = serde_json::json!(audit.is_some());
    // Explicit private trash directory only; routes stay 501 without it.
    let trash = config
        .trash_dir
        .clone()
        .map(|directory| trash::TrashService::open(directory, config.roots.clone()))
        .transpose()?
        .map(Arc::new);
    capabilities["trash"] = serde_json::json!(trash.is_some());
    // Native CLI liveness is always discovered on supported platforms.
    // Explicit proc paths remain injectable for isolated tests.
    let proc_scan = runtime::procscan::ProcScanner::supported().then(|| {
        let grok_active = config.grok_active.clone().or_else(|| {
            lifecycle::model::expand_user(std::path::Path::new("~/.grok/active_sessions.json"))
        });
        Arc::new(runtime::procscan::ProcScanner::new(
            config.proc_root.clone(),
            grok_active,
            runtime::procscan::SessionRoots::new(
                [
                    &config.roots.claude,
                    &config.roots.codex,
                    &config.roots.grok,
                ]
                .into_iter()
                .flatten()
                .map(std::path::PathBuf::as_path),
            ),
        ))
    });
    capabilities["live"] = serde_json::json!(proc_scan.is_some());
    // True only with the bundle directory, repository, audit log,
    // terminal transport and lifecycle service together; which CLI a worker
    // runs is decided per request from the source (503 when none).
    capabilities["bug_report"] = serde_json::json!(
        config.bug_report_dir.is_some()
            && config.bug_report_repo.is_some()
            && audit.is_some()
            && terminal.is_some()
            && lifecycle.is_some()
            && metadata.is_some()
            && files_write.is_some()
    );
    // Node identity: configuration proved the four settings come
    // together; the id is minted here (O_EXCL 0600) on first start. `hub`
    // stays false — a node is not a hub.
    let node = match (&config.node_token_file, &config.node_id_file) {
        (Some(token), Some(id)) => Some(Arc::new(api::node_auth::NodeIdentity {
            node_id: hub::identity::node_id(id)?,
            token: hub::NodeToken::load(token)?,
            peers: config.node_peers.clone(),
        })),
        _ => None,
    };
    capabilities["conversation_send"] = serde_json::json!(
        metadata.is_some() && terminal.is_some() && lifecycle.is_some() && files_write.is_some()
    );
    let capabilities = Arc::new(capabilities);
    let assets = Arc::new(assets::Assets::load(
        &config.web_dir,
        &config.hostname,
        &capabilities,
    )?);
    // One configurable read pool; the
    // response/probe pools are derived by the documented ratios. The cache
    // budgets are process-wide and fixed by the first app built.
    let pools = config.pools.clone();
    if !sessions::budgets::configure(pools.caches()) {
        eprintln!("sessiondock: cache budgets already fixed by an earlier app; keeping them");
    }
    let reader = Reader {
        store: Arc::new(sessions::SessionStore::with_metadata_and_names(
            config.roots,
            metadata.clone(),
            config.codex_index,
        )),
        workers: Arc::new(Semaphore::new(pools.read_workers)),
    };
    let runtime_probes = Arc::new(Semaphore::new(pools.runtime_probes()));
    // The search-text cache (persistent only with the explicit
    // directory) and its own parse budget; the warm-up thread (persistent
    // cache only) stops with the shutdown token.
    let search = Arc::new(search::service::SearchService::open(
        reader.store.clone(),
        search_cache_dir,
        config.search_cache_bytes,
        config.search_fold_bytes,
        config.search_workers,
        config.search_warmup_secs,
    )?);
    search.spawn_warmup(shutdown.clone());
    let spawn_watch = match (&proc_scan, &metadata) {
        (Some(scanner), Some(metadata)) => Some(Arc::new(runtime::spawn::SpawnWatcher::new(
            scanner.clone(),
            metadata.clone(),
        ))),
        _ => None,
    };
    let spawn_start = spawn_watch.as_ref().map(|watcher| SpawnStart {
        watcher: watcher.clone(),
        reader: reader.clone(),
    });
    // Every dependency of a bug-report worker must be configured —
    // the bundle directory and repository, the audit log (events.jsonl is the
    // core of a report), the terminal transport and the lifecycle service —
    // else the route stays 501 `bug_report_disabled`.
    let bug_report = match (
        &config.bug_report_dir,
        &config.bug_report_repo,
        &audit,
        &terminal,
        &lifecycle,
        config.ptyhost_dir.as_ref(),
    ) {
        (
            Some(directory),
            Some(repository),
            Some(audit),
            Some(terminal),
            Some(lifecycle),
            Some(host),
        ) if config.audit_dir.is_some() => {
            let service = bug_report::BugReportService::open(
                directory.clone(),
                repository.clone(),
                config.audit_dir.clone().expect("checked above"),
                assets.build.clone(),
            )?;
            let client = ptyhost_client::HostClient::new(
                host,
                ptyhost_client::Limits {
                    max_line_bytes: 4 * 1024 * 1024, // ptyhost protocol::MAX_LINE; a 1 MiB send plus its guard envelope
                    operation_timeout: std::time::Duration::from_secs(10),
                    ..Default::default()
                },
            )
            .map_err(io::Error::other)?;
            Some(Arc::new(bug_report::worker::WorkerContext {
                service: Arc::new(service),
                lifecycle: lifecycle.clone(),
                terminal: terminal.clone(),
                client: Arc::new(client),
                reader: reader.clone(),
                audit: audit.clone(),
                shutdown: shutdown.clone(),
            }))
        }
        _ => None,
    };
    // Live question cards (Claude hook files under the state dir) and
    // Codex approvals (read-only screen capture of the managed instance).
    let prompts = Arc::new(
        bridge::LivePrompts::new(config.state_dir.as_deref(), config.ptyhost_dir.as_deref())
            .map_err(io::Error::other)?,
    );
    let conversations = match (&metadata, &terminal, &runtime, &lifecycle, &files_write) {
        (Some(metadata), Some(terminal), Some(runtime), Some(lifecycle), Some(writer)) => {
            let directory = metadata.directory().join("conversations");
            let store = Arc::new(
                conversation::store::Store::open(&directory)
                    .map_err(|e| io::Error::other(e.message))?,
            );
            Some(Arc::new(conversation::Conversations::new(
                store,
                reader.clone(),
                lifecycle.clone(),
                Arc::new(delivery::executor::ManagedResolver {
                    runtime: runtime.clone(),
                    reader: reader.clone(),
                    lifecycle: Some(lifecycle.clone()),
                    probes: runtime_probes.clone(),
                    proc_scan: proc_scan.clone(),
                }),
                Arc::new(delivery::driver::HostTerminalDriver::new(terminal.clone())),
                writer.clone(),
                bug_report.as_ref().map(|ctx| ctx.service.clone()),
            )))
        }
        _ => None,
    };
    if let Some(service) = &conversations {
        service.housekeeping(shutdown.clone());
    }
    let executor = match (&delivery, &terminal, &runtime, conversations.is_none()) {
        (Some(delivery), Some(terminal), Some(runtime), true) => {
            Some(delivery::executor::DeliveryExecutor::start(
                delivery.clone(),
                Arc::new(delivery::driver::HostTerminalDriver::new(terminal.clone())),
                Arc::new(delivery::executor::ManagedResolver {
                    runtime: runtime.clone(),
                    reader: reader.clone(),
                    lifecycle: lifecycle.clone(),
                    probes: runtime_probes.clone(),
                    proc_scan: proc_scan.clone(),
                }),
                reader.clone(),
                Default::default(),
                shutdown.clone(),
            ))
        }
        _ => None,
    };
    let state = AppState {
        conversations,
        assets,
        capabilities,
        terminal,
        metadata,
        delivery,
        executor: executor.clone(),
        lifecycle,
        launch_adapters: Arc::new(launch_adapters),
        lifecycle_http: Arc::new(Semaphore::new(pools.responses())),
        media: Arc::new(media::MediaStore::new()),
        history_pages: Arc::new(sessions::PageStore::with_page_events(
            pools.history_page_events,
        )),
        history_page_http: Arc::new(Semaphore::new(pools.responses())),
        media_http: Arc::new(Semaphore::new(pools.responses())),
        media_jobs: Arc::new(Semaphore::new(2)),
        files,
        file_jobs: Arc::new(Semaphore::new(2)),
        files_write,
        file_write_http: Arc::new(Semaphore::new(pools.responses())),
        runtime,
        runtime_probes,
        proc_scan,
        spawn_watch,
        observations: observe::WatchHub::new(reader.clone(), shutdown.clone()),
        reader,
        watchers: Arc::new(Semaphore::new(32)),
        searches: Arc::new(Semaphore::new(2)),
        search,
        hostname: config.hostname.into(),
        public_hosts: config.public_hosts.into(),
        shutdown,
        audit: audit.clone(),
        trash,
        node,
        bug_report,
        prompts,
        polls: Arc::new(polls::PollCache::default()),
    };
    // Same state, own gate, no static fallback: everything the hub proxies.
    let node_router = state.node.as_ref().map(|_| {
        api::node_router()
            .layer(DefaultBodyLimit::disable())
            .layer(middleware::from_fn_with_state(
                state.clone(),
                api::node_auth::node_auth,
            ))
            .with_state(state.clone())
    });
    let router = Router::new()
        .nest("/api", api::router())
        .fallback(assets::serve)
        .layer(DefaultBodyLimit::disable())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security::local_only,
        ))
        .with_state(state.clone());
    Ok(BuiltApp {
        router,
        node_router,
        audit,
        executor,
        spawn_watch: spawn_start,
        state,
    })
}
