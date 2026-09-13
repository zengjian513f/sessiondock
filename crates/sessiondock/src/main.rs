//! Loopback development binary. No option starts the Web service.
//! `--initialize-delivery` and `--initialize-lifecycle` create their state
//! directories as needed and exit without starting Web or any CLI.
//! `--check-config` validates the environment exactly as startup does, prints
//! the effective paths and exits without opening any ledger, binding or
//! starting a CLI (the cutover preflight). Bind failure and graceful shutdown
//! drain lifecycle, delivery, and audit. There is no implicit CLI or
//! production discovery. With the node identity configured a second listener
//! (`SESSIONDOCK_NODE_BIND`) serves the hub-facing router next to loopback.
//! `claude-hook` is the Claude Code hook command (stdin JSON → question-card
//! file, always exit 0, no output) and `--write-bridge-settings` writes the
//! `--settings` file that installs it; neither reads the service configuration.
use std::{error::Error, path::PathBuf};

use sessiondock::config::Config;

const USAGE: &str = "usage: sessiondock [--check-config | --initialize-delivery DIRECTORY | --initialize-lifecycle DIRECTORY | --write-bridge-settings ABSOLUTE_FILE | claude-hook [--state-dir ABSOLUTE_DIRECTORY]]";

/// The reactor's thread count comes from `SESSIONDOCK_ASYNC_WORKERS` (batch
/// 44 WP-A), so the runtime is built after the environment is read.
fn main() -> Result<(), Box<dyn Error>> {
    let workers = match std::env::var_os("SESSIONDOCK_ASYNC_WORKERS") {
        // Validated again by Config::from_env; an invalid value fails there
        // with its precise message instead of here.
        Some(value) => value
            .to_str()
            .and_then(|text| text.trim().parse::<usize>().ok())
            .filter(|n| *n >= 1)
            .unwrap_or_else(sessiondock::config::Pools::default_async_workers),
        None => sessiondock::config::Pools::default_async_workers(),
    };
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1 && arguments[0] == "--help" {
        println!(
            "sessiondock [--check-config | --initialize-delivery DIRECTORY | --initialize-lifecycle DIRECTORY | --write-bridge-settings ABSOLUTE_FILE | claude-hook [--state-dir ABSOLUTE_DIRECTORY]]\nNo option starts the loopback development Web service.\n--check-config validates SESSIONDOCK_* like startup, prints the effective paths and exits.\nInitialization creates the requested state directory as needed and exits without starting Web or any CLI.\n--write-bridge-settings writes the Claude `--settings` hooks file (0600) that runs this binary as `claude-hook --state-dir $SESSIONDOCK_STATE_DIR`.\nclaude-hook reads one Claude Code hook payload from stdin and records an AskUserQuestion card under <state dir>/claude-prompts; it never prints and always exits 0."
        );
        return Ok(());
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == sessiondock::bridge::HOOK_SUBCOMMAND)
    {
        return claude_hook(&arguments[1..]);
    }
    if arguments.len() == 2 && arguments[0] == "--write-bridge-settings" {
        return write_bridge_settings(PathBuf::from(&arguments[1]));
    }
    let check_config = arguments.len() == 1 && arguments[0] == "--check-config";
    let initialize = arguments.len() == 2 && arguments[0] == "--initialize-delivery";
    let initialize_lifecycle = arguments.len() == 2 && arguments[0] == "--initialize-lifecycle";
    if !arguments.is_empty() && !check_config && !initialize && !initialize_lifecycle {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, USAGE).into());
    }
    let mut config = Config::from_env()?;
    if check_config {
        // Same validation as startup (from_env already validated); nothing is
        // bound or spawned, so this is safe against production paths. The
        // The launcher file is read and its host directory cross-checked
        // exactly like `prepare_app`; legacy cwd-root fields grant nothing.
        match (&config.lifecycle_dir, &config.launcher_config) {
            (Some(_), Some(path)) => {
                let launcher = sessiondock::lifecycle::launcher::read_config(path)
                    .map_err(std::io::Error::other)?;
                config.validate_launcher(&launcher)?;
                sessiondock::lifecycle::launcher::Launcher::new(launcher)
                    .map_err(std::io::Error::other)?;
                println!("launcher=ok");
            }
            (None, None) => {}
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "lifecycle startup requires both an initialized lifecycle directory and an explicit launcher configuration",
                )
                .into());
            }
        }
        print_effective_config(&config);
        return Ok(());
    }
    if initialize_lifecycle {
        config.lifecycle_dir = Some(arguments[1].clone().into());
        config.validate()?;
        let directory = config
            .lifecycle_dir
            .take()
            .expect("explicit initialization path");
        tokio::task::spawn_blocking(move || {
            sessiondock::lifecycle::store::LifecycleStore::initialize(&directory).map(drop)
        })
        .await??;
        println!("Lifecycle ledger initialized. No Web service or CLI was started.");
        return Ok(());
    }
    if initialize {
        config.delivery_dir = Some(arguments[1].clone().into());
        config.validate()?;
        let directory = config
            .delivery_dir
            .take()
            .expect("explicit initialization path");
        tokio::task::spawn_blocking(move || {
            sessiondock::delivery::engine::DeliveryEngine::initialize(&directory).map(drop)
        })
        .await??;
        println!("Delivery ledger initialized. No Web service or CLI was started.");
        return Ok(());
    }
    let bind = config.bind;
    let node_bind = config.node_bind;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let sessiondock::PreparedApp {
        router: app,
        node_router,
        delivery,
        lifecycle,
        audit,
    } = sessiondock::prepare_app(config, shutdown.clone()).await?;
    // Both sockets before serving either: a node bind failure is a startup
    // error, never a loopback-only service that silently lost its hub face.
    let listeners = async {
        let loopback = tokio::net::TcpListener::bind(bind).await?;
        let node = match node_bind {
            Some(address) => Some(tokio::net::TcpListener::bind(address).await?),
            None => None,
        };
        Ok::<_, std::io::Error>((loopback, node))
    };
    let (listener, node_listener) = match listeners.await {
        Ok(listeners) => listeners,
        Err(error) => {
            shutdown.cancel();
            if let Some(service) = &lifecycle {
                let _ = service.shutdown().await;
            }
            if let Some(delivery) = &delivery {
                let _ = delivery.shutdown().await;
            }
            return Err(error.into());
        }
    };
    eprintln!("SessionDock: http://{}", listener.local_addr()?);
    if let Some(node_listener) = &node_listener {
        eprintln!(
            "SessionDock node listener: http://{} (X-AgentHub-Node-Token + X-AgentHub-Protocol: 1 from configured peers only)",
            node_listener.local_addr()?
        );
    }
    eprintln!(
        "Native history is read-only. Terminal transport requires an explicit isolated host directory. No implicit CLI discovery."
    );
    let signal_shutdown = shutdown.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        signal_shutdown.cancel();
    });
    let stop = shutdown.clone();
    let loopback = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move { stop.cancelled().await });
    let served = match (node_listener, node_router) {
        (Some(node_listener), Some(node_router)) => {
            let stop = shutdown.clone();
            let node = axum::serve(
                node_listener,
                node_router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async move { stop.cancelled().await });
            tokio::try_join!(loopback, node).map(drop)
        }
        _ => loopback.await,
    };
    shutdown.cancel();
    // Always drain both coordinators, even when one reports a worker failure.
    let lifecycle_result = match lifecycle {
        Some(service) => service.shutdown().await,
        None => Ok(()),
    };
    let delivery_result = match delivery {
        Some(service) => service.shutdown().await,
        None => Ok(()),
    };
    // Diagnostics are best-effort: a drain past the deadline is not a startup/exit error.
    if let Some(service) = audit {
        service.shutdown().await;
    }
    lifecycle_result?;
    delivery_result?;
    served?;
    Ok(())
}

/// `sessiondock claude-hook [--state-dir DIR]`: Claude Code runs this as its
/// AskUserQuestion / SessionStart / SessionEnd hook with the cleared launcher
/// environment, so the state directory travels as an argument (the settings
/// file `--write-bridge-settings` writes carries it); `SESSIONDOCK_STATE_DIR`
/// is the fallback. Like Python's `claude_bridge.main`, every failure is
/// silent and the exit status is 0: a bridge problem must never block the
/// CLI's tool call.
fn claude_hook(arguments: &[std::ffi::OsString]) -> Result<(), Box<dyn Error>> {
    let state_dir = match arguments {
        [] => std::env::var_os("SESSIONDOCK_STATE_DIR").map(PathBuf::from),
        [flag, directory] if flag == "--state-dir" => Some(PathBuf::from(directory)),
        _ => None,
    };
    if let Some(directory) = state_dir {
        let mut stdin = std::io::stdin().lock();
        let _ = sessiondock::bridge::claude::run_hook(&directory, &mut stdin);
    }
    Ok(())
}

/// `sessiondock --write-bridge-settings FILE`: the hooks-only Claude settings
/// document pointing at this very binary and `SESSIONDOCK_STATE_DIR` (both
/// absolute). Pass the file to the Claude launch profile as `--settings FILE`.
fn write_bridge_settings(path: PathBuf) -> Result<(), Box<dyn Error>> {
    let invalid = |message: &str| {
        Box::<dyn Error>::from(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            message.to_owned(),
        ))
    };
    let path = std::path::absolute(path)?;
    let state_dir = std::env::var_os("SESSIONDOCK_STATE_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| invalid("SESSIONDOCK_STATE_DIR is required for bridge settings"))?;
    let state_dir = std::path::absolute(state_dir)?;
    let command = std::env::current_exe()?;
    sessiondock::bridge::claude::write_settings(&path, &command, &state_dir)?;
    println!(
        "Claude bridge settings written: {} (hooks run `{} claude-hook --state-dir {}`). Add `--settings {}` to the Claude launch profile args.",
        path.display(),
        command.display(),
        state_dir.display(),
        path.display()
    );
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// One `key=value` line per setting so an operator or preflight script can
/// diff it against the intended environment. Paths only; no directory is read.
fn print_effective_config(config: &Config) {
    fn path(value: &Option<std::path::PathBuf>) -> String {
        value
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "(unset)".into())
    }
    fn list(values: &[std::path::PathBuf]) -> String {
        if values.is_empty() {
            return "(unset)".into();
        }
        values
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(":")
    }
    println!("config=ok");
    println!("bind={}", config.bind);
    println!("web_dir={}", config.web_dir.display());
    println!("claude_root={}", path(&config.roots.claude));
    println!("codex_root={}", path(&config.roots.codex));
    println!("grok_root={}", path(&config.roots.grok));
    println!("codex_index={}", path(&config.codex_index));
    println!("ptyhost_dir={}", path(&config.ptyhost_dir));
    println!("state_dir={}", path(&config.state_dir));
    println!(
        "debug_runs={}",
        path(
            &config
                .state_dir
                .as_ref()
                .map(|dir| dir.join(sessiondock::sessions::DEBUG_RUNS_FILENAME))
        )
    );
    println!("delivery_dir={}", path(&config.delivery_dir));
    println!("lifecycle_dir={}", path(&config.lifecycle_dir));
    println!("launcher_config={}", path(&config.launcher_config));
    println!("file_roots={}", list(&config.file_roots));
    println!("file_write_roots={}", list(&config.file_write_roots));
    println!("audit_dir={}", path(&config.audit_dir));
    println!("trash_dir={}", path(&config.trash_dir));
    println!("proc_root={}", config.proc_root.display());
    println!("grok_active={}", path(&config.grok_active));
    println!(
        "node_bind={}",
        config
            .node_bind
            .map(|address| address.to_string())
            .unwrap_or_else(|| "(unset)".into())
    );
    println!("node_token_file={}", path(&config.node_token_file));
    println!("node_id_file={}", path(&config.node_id_file));
    println!(
        "node_peers={}",
        if config.node_peers.is_empty() {
            "(unset)".into()
        } else {
            config
                .node_peers
                .iter()
                .map(|network| network.to_string())
                .collect::<Vec<_>>()
                .join(",")
        }
    );
    println!("bug_report_dir={}", path(&config.bug_report_dir));
    println!("bug_report_repo={}", path(&config.bug_report_repo));
    println!("read_workers={}", config.pools.read_workers);
    println!("runtime_probes={}", config.pools.runtime_probes());
    println!("response_permits={}", config.pools.responses());
    println!("history_page_events={}", config.pools.history_page_events);
    println!("async_workers={}", config.pools.async_workers);
    println!("cache_entries={}", config.pools.cache_entries);
    println!("view_cache_mb={}", config.pools.view_cache_mb);
    println!("ast_cache_mb={}", config.pools.ast_cache_mb);
    println!(
        "public_hosts={}",
        if config.public_hosts.is_empty() {
            "(unset)".into()
        } else {
            config.public_hosts.join(",")
        }
    );
    println!("hostname={}", config.hostname);
    println!("search_cache_dir={}", path(&config.search_cache_dir));
    println!("search_cache_bytes={}", config.search_cache_bytes);
    println!("search_workers={}", config.search_workers);
    println!("search_warmup={}", config.search_warmup_secs);
}
