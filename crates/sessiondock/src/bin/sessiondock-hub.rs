//! The multi-machine hub (Python `python3 -m sessiondock.hub`), batch 40 H4.
//! Loopback only, behind the authenticated reverse proxy; it owns no session
//! root, host, ledger or state directory — only its registry, cache and page.
//! Registration is a server-side operation like Python's `Registry` class:
//! `register` / `remove` / `list` subcommands, never an HTTP route.
//! `--check-config` validates `SESSIONDOCK_HUB_*` like startup and exits.
use std::{error::Error, ffi::OsString};

use sessiondock::{
    hub::{Client, registry::Registration},
    hub_api,
    hub_config::HubConfig,
};

const USAGE: &str = "sessiondock-hub [--check-config | register --name NAME --url http://IP:PORT --token-file FILE [--color COLOR] [--id NODE_ID] | remove NODE_ID | list]\n\
No option serves the hub on SESSIONDOCK_HUB_BIND (default 127.0.0.1:8742).\n\
Environment: SESSIONDOCK_HUB_NODES (default ~/.local/share/sessiondock/hub-nodes.json), SESSIONDOCK_HUB_CACHE_DIR, SESSIONDOCK_HUB_NETWORKS, SESSIONDOCK_WEB_DIR, SESSIONDOCK_AUDIT_DIR.";

fn usage_error(detail: &str) -> Box<dyn Error> {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!("{detail}\nusage: {USAGE}"),
    )
    .into()
}

/// `--name X --url Y …` into a registration; every value is one argument.
fn parse_registration(arguments: &[OsString]) -> Result<Registration, Box<dyn Error>> {
    let mut registration = Registration::default();
    let mut token_file = None;
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index].to_string_lossy().into_owned();
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| usage_error(&format!("{flag} needs a value")))?
            .to_string_lossy()
            .into_owned();
        match flag.as_str() {
            "--name" => registration.name = value,
            "--url" => registration.url = value,
            "--token-file" => token_file = Some(value),
            "--color" => registration.color = value,
            "--id" => registration.id = Some(value),
            _ => return Err(usage_error(&format!("unknown register option {flag}"))),
        }
        index += 2;
    }
    let token_file = token_file.ok_or_else(|| usage_error("register needs --token-file"))?;
    // The credential is read from a file so it never appears in a process list.
    registration.token = std::fs::read_to_string(&token_file)
        .map_err(|error| format!("{token_file}: {error}"))?
        .trim()
        .to_string();
    if registration.name.is_empty() || registration.url.is_empty() {
        return Err(usage_error("register needs --name and --url"));
    }
    Ok(registration)
}

fn print_effective_config(config: &HubConfig) {
    println!("config=ok");
    println!("hub_bind={}", config.bind);
    println!("hub_nodes={}", config.nodes_file.display());
    println!("hub_cache_dir={}", config.cache_dir.display());
    println!("hub_networks={}", config.networks_text);
    println!("public_hosts={}", config.public_hosts.join(","));
    println!("web_dir={}", config.web_dir.display());
    println!(
        "audit_dir={}",
        config
            .audit_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "(unset)".into())
    );
    println!("hostname=SessionDock");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1 && arguments[0] == "--help" {
        println!("{USAGE}");
        return Ok(());
    }
    let config = HubConfig::from_env()?;
    match arguments.first().and_then(|first| first.to_str()) {
        None => serve(config).await,
        Some("--check-config") if arguments.len() == 1 => {
            print_effective_config(&config);
            Ok(())
        }
        Some("register") => {
            let registration = parse_registration(&arguments[1..])?;
            let registry = hub_api::open_registry(&config)?;
            let row = registry
                .register(&Client::default(), registration)
                .await
                .map_err(|error| error.to_string())?;
            println!(
                "registered {} {} color={}",
                row.id,
                row.name,
                if row.color.is_empty() {
                    "-"
                } else {
                    &row.color
                }
            );
            Ok(())
        }
        Some("remove") if arguments.len() == 2 => {
            let nid = arguments[1].to_string_lossy().into_owned();
            let registry = hub_api::open_registry(&config)?;
            if registry.find(&nid).is_none() {
                return Err(format!("no registered node {nid}").into());
            }
            registry.remove(&nid)?;
            println!("removed {nid}");
            Ok(())
        }
        Some("list") if arguments.len() == 1 => {
            let registry = hub_api::open_registry(&config)?;
            for row in registry.machines() {
                let nid = row["id"].as_str().unwrap_or_default();
                let url = registry.find(nid).map(|node| node.url).unwrap_or_default();
                println!(
                    "{nid} {} {url} color={} enabled={}",
                    row["name"].as_str().unwrap_or_default(),
                    row["color"]
                        .as_str()
                        .filter(|c| !c.is_empty())
                        .unwrap_or("-"),
                    row["enabled"].as_bool().unwrap_or(true)
                );
            }
            Ok(())
        }
        Some(_) => Err(usage_error("unknown arguments")),
    }
}

async fn serve(config: HubConfig) -> Result<(), Box<dyn Error>> {
    let shutdown = tokio_util::sync::CancellationToken::new();
    let hub_api::HubApp {
        router,
        registry,
        client: _client,
        monitor,
    } = hub_api::hub_app(&config, shutdown.clone())?;
    let listener = match tokio::net::TcpListener::bind(config.bind).await {
        Ok(listener) => listener,
        Err(error) => {
            shutdown.cancel();
            monitor.stop().await;
            return Err(error.into());
        }
    };
    eprintln!(
        "SessionDock hub: http://{} ({} nodes)",
        listener.local_addr()?,
        registry.all().len()
    );
    eprintln!(
        "Multi-machine hub behind the authenticated reverse proxy; registration is the `register` subcommand. No session root is read here."
    );
    let signal_shutdown = shutdown.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        signal_shutdown.cancel();
    });
    let stop = shutdown.clone();
    let served = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move { stop.cancelled().await })
    .await;
    shutdown.cancel();
    monitor.stop().await;
    served?;
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
