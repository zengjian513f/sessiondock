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

pub async fn report(state: &AppState) -> Result<Report, ApiError> {
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

pub async fn publish(state: &AppState, published: Published) -> Result<usize, ApiError> {
    let current = report(state).await?;
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
        let mut parents = Vec::new();
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
            if let Some((uid, child)) = cache.local.get(&link.process)
                && (child.node_id != link.session.node_id
                    || child.source != link.session.source
                    || child.sid != link.session.sid)
                && let (Some(parent_created), Some(child_created)) =
                    (link.session.created, child.created)
                && parent_created <= child_created
            {
                parents.push((
                    uid.clone(),
                    crate::metadata::SpawnedBy {
                        source: link.session.source.clone(),
                        sid: link.session.sid.clone(),
                        node_id: (node_id != link.session.node_id)
                            .then(|| link.session.node_id.clone()),
                    },
                ));
            }
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
        if let Some(metadata) = metadata {
            metadata
                .record_spawn_parents(&parents)
                .map_err(ApiError::from)?;
        }
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
