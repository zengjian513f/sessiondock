//! Node adapter for the shared process-links protocol. Remote links are held
//! for the lifetime of an observed process incarnation, not a socket or PID.
use crate::{
    error::ApiError,
    runtime::procscan::{SessionRow, parse_created},
    state::AppState,
};
use process_links::{
    Binding, Incoming, Link, Outgoing, Process, Published, Report, Session, linux,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Mutex, OnceLock},
    time::Instant,
};

#[derive(Default)]
struct Cache {
    boot_id: String,
    links: BTreeMap<Process, Link>,
    local: BTreeMap<Process, (String, Session)>,
    incoming: BTreeMap<Process, process_links::Connection>,
    report: Option<(Instant, Report)>,
    first_seen: BTreeMap<Process, f64>,
}
static CACHES: OnceLock<Mutex<HashMap<String, Cache>>> = OnceLock::new();
fn caches() -> &'static Mutex<HashMap<String, Cache>> {
    CACHES.get_or_init(Default::default)
}
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

async fn legacy_report(state: &AppState) -> Result<Report, ApiError> {
    let node_id = state
        .node
        .as_ref()
        .map(|n| n.node_id.clone())
        .unwrap_or_default();
    {
        let caches = caches().lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, report)) = caches.get(&node_id).and_then(|c| c.report.as_ref())
            && at.elapsed().as_secs_f64() < 1.0
        {
            return Ok(report.clone());
        }
    }
    let Some(scanner) = &state.proc_scan else {
        return Ok(Report {
            version: 1,
            node_id,
            boot_id: String::new(),
            supported: false,
            sampled_at: now(),
            outgoing: vec![],
            incoming: vec![],
            bindings: vec![],
            collector: None,
        });
    };
    let document = state
        .reader
        .run_wait(&state.shutdown, |store| store.list_recent())
        .await?;
    let rows = SessionRow::from_list(&document);
    let scanned = scanner.snapshot(false).await.map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "process_scan_failed",
            "进程采样失败",
        )
    })?;
    let root = scanner.root().to_path_buf();
    let disk = state
        .metadata
        .as_ref()
        .map(|m| m.directory().join("process-links.json"));
    let scan = scanned.scan;
    tokio::task::spawn_blocking(move || {
        let snapshot = linux::collect(&root);
        let owned = scan.active_processes(&rows).owned;
        let titles: HashMap<_, _> = document["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| Some((v["uid"].as_str()?, v["title"].as_str()?)))
            .collect();
        let refs: HashMap<_, _> = rows
            .iter()
            .map(|r| {
                let title = titles.get(r.uid.as_str()).map(|title| (*title).to_string());
                (
                    r.uid.clone(),
                    Session {
                        node_id: node_id.clone(),
                        source: r.source.clone(),
                        sid: r.sid.clone(),
                        title,
                        created: parse_created(&r.created),
                    },
                )
            })
            .collect();
        let mut owners = BTreeMap::new();
        for (uid, pids) in owned {
            for pid in pids.into_iter().filter(|p| *p > 0) {
                if let Some(session) = refs.get(&uid) {
                    owners.insert(pid as u32, (uid.clone(), session.clone()));
                }
            }
        }
        let by_sid: HashMap<_, _> = rows
            .iter()
            .filter_map(|r| {
                refs.get(&r.uid).map(|s| {
                    (
                        (r.source.clone(), r.sid.to_lowercase()),
                        (r.uid.clone(), s.clone()),
                    )
                })
            })
            .collect();
        let mut caches = caches().lock().unwrap_or_else(|e| e.into_inner());
        let cache = caches.entry(node_id.clone()).or_default();
        if cache.boot_id != snapshot.boot_id {
            *cache = Cache::default();
            cache.boot_id = snapshot.boot_id.clone();
            if let Some(saved) = disk
                .as_ref()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|raw| serde_json::from_slice::<Published>(&raw).ok())
                && saved.boot_id == snapshot.boot_id
                && !saved.boot_id.is_empty()
            {
                cache.links = saved
                    .links
                    .into_iter()
                    .map(|v| (v.process.clone(), v))
                    .collect();
            }
        }
        cache.links.retain(|p, _| {
            snapshot
                .entries
                .get(&p.pid)
                .is_some_and(|e| e.process == *p)
        });
        cache.first_seen.retain(|p, _| {
            snapshot
                .entries
                .get(&p.pid)
                .is_some_and(|e| e.process == *p)
        });
        let sampled_at = now();
        let resolve = |pid: u32| {
            let mut current = pid;
            let mut seen = HashSet::new();
            while current > 1 && seen.insert(current) {
                if let Some(owner) = owners.get(&current) {
                    return Some(owner.clone());
                }
                let entry = snapshot.entries.get(&current)?;
                if entry.shared_parent {
                    return None;
                }
                for key in &entry.identities {
                    if let Some(owner) = by_sid.get(key) {
                        return Some(owner.clone());
                    }
                }
                current = entry.parent;
            }
            None
        };
        cache.local = snapshot
            .entries
            .values()
            .filter_map(|e| resolve(e.process.pid).map(|s| (e.process.clone(), s)))
            .collect();
        cache.incoming.clear();
        let mut outgoing = Vec::new();
        let mut incoming = Vec::new();
        let mut bindings = Vec::new();
        for entry in snapshot.entries.values() {
            let mut current = entry.process.pid;
            let mut seen = HashSet::new();
            let mut remote = cache.links.get(&entry.process).cloned();
            while current > 1 && seen.insert(current) {
                let Some(ancestor) = snapshot.entries.get(&current) else {
                    break;
                };
                if remote.is_none() {
                    remote = cache.links.get(&ancestor.process).cloned();
                }
                if let Some(connection) = &ancestor.connection {
                    incoming.push(Incoming {
                        process: entry.process.clone(),
                        started_at: entry.started_at,
                        connection: connection.clone(),
                    });
                    cache
                        .incoming
                        .insert(entry.process.clone(), connection.clone());
                    break;
                }
                current = ancestor.parent;
            }
            let local = cache.local.get(&entry.process).map(|(_, s)| s.clone());
            if let Some(session) = local.or_else(|| remote.as_ref().map(|v| v.session.clone())) {
                let previous = cache
                    .links
                    .get(&entry.process)
                    .map(|link| link.observed_at)
                    .unwrap_or(sampled_at);
                let first_observed_at = *cache
                    .first_seen
                    .entry(entry.process.clone())
                    .or_insert(previous);
                let launch_chain = remote
                    .as_ref()
                    .map(|v| v.launch_chain.clone())
                    .unwrap_or_default();
                bindings.push(Binding {
                    process: entry.process.clone(),
                    session: session.clone(),
                    initiator: remote.map(|v| v.session),
                    first_observed_at,
                    launch_chain: launch_chain.clone(),
                });
                if !entry.multiplexed && entry.sockets.len() == 1 {
                    outgoing.push(Outgoing {
                        launch_chain,
                        process: entry.process.clone(),
                        started_at: entry.started_at,
                        connection: entry.sockets[0].clone(),
                        session,
                    });
                }
            }
        }
        let report = Report {
            version: 1,
            node_id,
            boot_id: snapshot.boot_id,
            supported: true,
            sampled_at,
            outgoing,
            incoming,
            bindings,
            collector: None,
        };
        cache.report = Some((Instant::now(), report.clone()));
        report
    })
    .await
    .map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "process_links_failed",
            "进程关联失败",
        )
    })
}

async fn legacy_publish(state: &AppState, published: Published) -> Result<usize, ApiError> {
    let current = legacy_report(state).await?;
    let Some(scanner) = &state.proc_scan else {
        return Ok(0);
    };
    let root = scanner.root().to_path_buf();
    let metadata = state.metadata.clone();
    let node_id = current.node_id;
    tokio::task::spawn_blocking(move || {
        let mut caches = caches().lock().unwrap_or_else(|e| e.into_inner());
        let Some(cache) = caches.get_mut(&node_id) else {
            return Ok(0);
        };
        if cache.boot_id != published.boot_id || cache.boot_id.is_empty() {
            return Ok(0);
        }
        let mut count = 0;
        for link in published.links {
            if !crate::hub::identity::is_node_id(&link.session.node_id)
                || link.session.source.is_empty()
                || link.session.sid.is_empty()
                || cache.incoming.get(&link.process) != Some(&link.connection)
                || linux::identity(&root, link.process.pid).map(|v| v.0)
                    != Some(link.process.clone())
            {
                continue;
            }
            let link = if let Some(saved) = cache.links.get_mut(&link.process) {
                // The owner is stable, but a later coordinator round may learn
                // earlier SSH hops that were not yet observed on the source node.
                if saved.launcher == link.launcher {
                    for launch in link.launch_chain {
                        if !saved
                            .launch_chain
                            .iter()
                            .any(|v| v.process == launch.process)
                        {
                            saved.launch_chain.push(launch);
                        }
                    }
                }
                saved.clone()
            } else {
                link
            };
            // First verified launch is stable for this incarnation even if the
            // connection tuple later gets reused. Never reassign old work.
            cache.links.entry(link.process.clone()).or_insert(link);
            count += 1;
        }
        cache.report = None;
        if let Some(metadata) = &metadata {
            let body = Published {
                boot_id: cache.boot_id.clone(),
                links: cache.links.values().cloned().collect(),
            };
            let path = metadata.directory().join("process-links.json");
            let bytes = serde_json::to_vec(&body).expect("process links serialize");
            if std::fs::read(&path).ok().as_ref() != Some(&bytes) {
                use std::io::Write;
                let temp = path.with_extension(format!("tmp-{}", std::process::id()));
                let result = (|| -> std::io::Result<()> {
                    let mut options = std::fs::OpenOptions::new();
                    options.write(true).create(true).truncate(true);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.mode(0o600);
                    }
                    let mut file = options.open(&temp)?;
                    file.write_all(&bytes)?;
                    file.sync_all()?;
                    std::fs::rename(&temp, &path)?;
                    Ok(())
                })();
                if result.is_err() {
                    let _ = std::fs::remove_file(temp);
                }
                result.map_err(|_| {
                    ApiError::new(
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        "process_links_save_failed",
                        "进程关联保存失败",
                    )
                })?;
            }
        }
        drop(caches);
        Ok(count)
    })
    .await
    .map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "process_links_failed",
            "进程关联失败",
        )
    })?
}

fn agent_socket(state: &AppState) -> Option<std::path::PathBuf> {
    let configured = std::env::var_os("SESSIONDOCK_RESOURCE_AGENT_SOCKET");
    let path = configured
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(process_links::agent::socket_path);
    // Private proc fixtures must never talk to a production collector.
    if state.proc_scan.as_ref()?.root() != std::path::Path::new("/proc")
        && configured.is_none()
        && std::env::var_os("RESOURCE_AGENT_SOCKET").is_none()
    {
        return None;
    }
    path.exists().then_some(path)
}
async fn agent_catalog(
    state: &AppState,
) -> Result<(process_links::agent::Catalog, Vec<SessionRow>), ApiError> {
    use process_links::agent::{Catalog, Owner};
    let node_id = state
        .node
        .as_ref()
        .map(|n| n.node_id.clone())
        .unwrap_or_default();
    let document = state
        .reader
        .run_wait(&state.shutdown, |store| store.list_recent())
        .await?;
    let rows = SessionRow::from_list(&document);
    let scanner = state.proc_scan.as_ref().ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "collector_unavailable",
            "采集器不可用",
        )
    })?;
    let scanned = scanner.snapshot(false).await.map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "process_scan_failed",
            "进程采样失败",
        )
    })?;
    let owned = scanned.scan.active_processes(&rows).owned;
    let titles: HashMap<_, _> = document["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| Some((v["uid"].as_str()?, v["title"].as_str()?)))
        .collect();
    let sessions: HashMap<_, _> = rows
        .iter()
        .map(|r| {
            (
                r.uid.clone(),
                Session {
                    node_id: node_id.clone(),
                    source: r.source.clone(),
                    sid: r.sid.clone(),
                    title: titles.get(r.uid.as_str()).map(|s| (*s).to_owned()),
                    created: parse_created(&r.created),
                },
            )
        })
        .collect();
    let mut owners = Vec::new();
    for (uid, pids) in owned {
        if let Some(session) = sessions.get(&uid) {
            for pid in pids {
                if let Ok(pid) = u32::try_from(pid)
                    && let Some((process, _)) = linux::identity(scanner.root(), pid)
                {
                    owners.push(Owner {
                        process,
                        session: session.clone(),
                    });
                }
            }
        }
    }
    let boot_id = std::fs::read_to_string(scanner.root().join("sys/kernel/random/boot_id"))
        .unwrap_or_default()
        .trim()
        .to_owned();
    owners.sort_by(|a, b| a.process.cmp(&b.process));
    let mut sessions: Vec<_> = sessions.into_values().collect();
    sessions.sort_by(|a, b| (&a.source, &a.sid).cmp(&(&b.source, &b.sid)));
    let catalog = Catalog {
        node_id,
        boot_id,
        owners,
        sessions,
    };
    Ok((catalog, rows))
}
async fn remember_agent_parents(
    state: &AppState,
    report: &Report,
    rows: Option<&[SessionRow]>,
) -> Result<(), ApiError> {
    let Some(metadata) = &state.metadata else {
        return Ok(());
    };
    if !report
        .bindings
        .iter()
        .any(|binding| binding.initiator.is_some())
    {
        return Ok(());
    }
    let loaded;
    let rows = if let Some(rows) = rows {
        rows
    } else {
        let document = state
            .reader
            .run_wait(&state.shutdown, |store| store.list_recent())
            .await?;
        loaded = SessionRow::from_list(&document);
        &loaded
    };
    let uids: BTreeMap<_, _> = rows
        .iter()
        .map(|r| ((r.source.clone(), r.sid.clone()), r.uid.clone()))
        .collect();
    let parents: Vec<_> = report
        .bindings
        .iter()
        .filter_map(|binding| {
            let parent = binding.initiator.as_ref()?;
            let child = &binding.session;
            if child.node_id != report.node_id
                || (parent.node_id == child.node_id
                    && parent.source == child.source
                    && parent.sid == child.sid)
                || parent.created? > child.created?
            {
                return None;
            }
            Some((
                uids.get(&(child.source.clone(), child.sid.clone()))?
                    .to_string(),
                crate::metadata::NestParent {
                    source: parent.source.clone(),
                    sid: parent.sid.clone(),
                    node_id: (parent.node_id != child.node_id).then(|| parent.node_id.clone()),
                },
            ))
        })
        .collect();
    let metadata = metadata.clone();
    tokio::task::spawn_blocking(move || metadata.initialize_nest_parents(&parents, &uids))
        .await
        .map_err(|_| {
            ApiError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "collector_metadata_failed",
                "关联保存失败",
            )
        })?
        .map_err(ApiError::from)?;
    Ok(())
}
pub async fn report(state: &AppState) -> Result<Report, ApiError> {
    if let Some(path) = agent_socket(state) {
        let (catalog, rows) = agent_catalog(state).await?;
        let node_id = catalog.node_id.clone();
        let boot_id = catalog.boot_id.clone();
        if let Ok(Ok(value)) = tokio::task::spawn_blocking(move || {
            process_links::agent::request(&path, &process_links::agent::Request::Catalog(catalog))
        })
        .await
            && let Ok(report) = serde_json::from_value::<Report>(value)
            && report.node_id == node_id
            && report.boot_id == boot_id
        {
            remember_agent_parents(state, &report, Some(&rows)).await?;
            return Ok(report);
        }
    }
    let report = legacy_report(state).await?;
    remember_agent_parents(state, &report, None).await?;
    Ok(report)
}
pub async fn publish(state: &AppState, published: Published) -> Result<usize, ApiError> {
    if let Some(path) = agent_socket(state) {
        let payload = published.clone();
        if let Ok(Ok(value)) = tokio::task::spawn_blocking(move || {
            process_links::agent::request(&path, &process_links::agent::Request::Publish(payload))
        })
        .await
            && let Ok(report) = serde_json::from_value::<Report>(value)
            && Some(report.node_id.as_str()) == state.node.as_ref().map(|n| n.node_id.as_str())
        {
            remember_agent_parents(state, &report, None).await?;
            return Ok(report.bindings.len());
        }
    }
    let count = legacy_publish(state, published).await?;
    report(state).await?;
    Ok(count)
}
pub async fn resources(state: &AppState) -> serde_json::Value {
    if let Some(path) = agent_socket(state)
        && let Ok(Ok(value)) = tokio::task::spawn_blocking(move || {
            process_links::agent::request(&path, &process_links::agent::Request::Resources)
        })
        .await
        && value["node_id"].as_str() == state.node.as_ref().map(|n| n.node_id.as_str())
    {
        return value;
    }
    serde_json::json!({"version":1,"availability":"unavailable","reason":"collector_not_available","samples":null})
}
/// Keep the independent collector's session catalog fresh without requiring an
/// open browser or a running fleet Hub. Failure is isolated to monitoring.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = state.shutdown.cancelled() => break,
                _ = async { if agent_socket(&state).is_some() { let _ = report(&state).await; } } => {},
            }
            tokio::select! { _ = state.shutdown.cancelled() => break, _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {} }
        }
    });
}

pub async fn probe(
    state: &AppState,
    enabled: bool,
    lease_id: Option<String>,
    lease_seconds: Option<u64>,
) -> Result<serde_json::Value, ApiError> {
    let path = agent_socket(state).ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "collector_unavailable",
            "此机器未安装资源采集服务",
        )
    })?;
    tokio::task::spawn_blocking(move || {
        process_links::agent::request(
            &path,
            &process_links::agent::Request::Probe {
                enabled,
                lease_id,
                lease_seconds,
            },
        )
    })
    .await
    .map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "probe_failed",
            "探测请求失败",
        )
    })?
    .map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "probe_unavailable",
            "此机器暂时无法开启探测，请检查采集服务",
        )
    })
}
