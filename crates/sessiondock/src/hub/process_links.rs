//! One fleet coordinator shared by session nesting and external CPU consumers.
use super::{
    Client, Registry, Target,
    client::{JSON_LIMIT, Request},
};
use crate::runtime::process_links::sync;
use futures_util::{StreamExt, stream};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

pub fn spawn(
    registry: Arc<Registry>,
    client: Arc<Client>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        let mut sent = BTreeMap::new();
        let mut baselines = BTreeMap::new();
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                next = tick(&registry, &client, &sent, &mut baselines) => { sent = next; },
            }
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(2)) => {},
            }
        }
    });
}

async fn tick(
    registry: &Registry,
    client: &Client,
    sent: &BTreeMap<String, serde_json::Value>,
    baselines: &mut BTreeMap<String, (Target, sync::Remote)>,
) -> BTreeMap<String, serde_json::Value> {
    let targets: Vec<_> = registry
        .all()
        .into_iter()
        .filter_map(|n| registry.target(&n).ok().map(|t| (n.id, t)))
        .collect();
    baselines.retain(|id, _| targets.iter().any(|(current, _)| current == id));
    let fetched: Vec<_> = stream::iter(targets.clone())
        .map(|(id, target)| {
            let baseline = baselines
                .remove(&id)
                .filter(|(old_target, _)| *old_target == target)
                .map(|(_, cache)| cache);
            async move {
                let (baseline, report) = fetch(client, &id, &target, baseline).await;
                (id, target, baseline, report)
            }
        })
        .buffer_unordered(8)
        .collect()
        .await;
    let mut reports = Vec::new();
    for (id, target, baseline, report) in fetched {
        if let Some(baseline) = baseline {
            baselines.insert(id, (target, baseline));
        }
        if let Some(report) = report {
            reports.push(report);
        }
    }
    let mut links = process_links::correlate(&reports);
    let reports: BTreeMap<_, _> = reports.iter().map(|r| (r.node_id.as_str(), r)).collect();
    stream::iter(targets)
        .map(|(id, target)| {
            let published = links.remove(&id).filter(|p| !p.links.is_empty());
            let report = reports.get(id.as_str()).copied();
            async move {
                let published = published?;
                let mut identity = published.clone();
                for link in &mut identity.links {
                    // Observation time advances every poll, but an established
                    // launch keeps its original time at the destination.
                    link.observed_at = 0.0;
                }
                let identity = serde_json::to_value(identity).ok()?;
                // Both a prior successful send and current destination evidence
                // are required. An inherited binding alone does not prove that
                // this process's own durable link was ever published.
                if sent.get(&id) == Some(&identity)
                    && report.is_some_and(|report| confirmed(report, &published))
                {
                    return Some((id, identity));
                }
                let body = serde_json::to_value(published).ok()?;
                let (status, _) = client
                    .json(
                        &target,
                        "POST",
                        "/api/process-links",
                        Some(&body),
                        Duration::from_secs(3),
                    )
                    .await
                    .ok()?;
                (status == 200).then_some((id, identity))
            }
        })
        .buffer_unordered(8)
        .filter_map(|result| async { result })
        .collect()
        .await
}

async fn fetch(
    client: &Client,
    id: &str,
    target: &Target,
    mut baseline: Option<sync::Remote>,
) -> (Option<sync::Remote>, Option<process_links::Report>) {
    let mut path = "/api/process-links?connection_sync=1".to_owned();
    if let Some(cache) = &baseline {
        // Revisions are locally generated hexadecimal tokens.
        path.push_str("&since=");
        path.push_str(&cache.revision);
    }
    let received = async {
        let response = client
            .open(
                target,
                Request {
                    method: "GET",
                    target: &path,
                    headers: &[],
                    body: None,
                    connect: Duration::from_secs(3),
                    idle: Duration::from_secs(3),
                },
            )
            .await
            .ok()?;
        if response.status != 200 {
            return None;
        }
        let incremental = response.header(sync::HEADER) == Some("1");
        let raw = response.into_body().read_to_end(JSON_LIMIT).await.ok()?;
        Some((incremental, raw))
    }
    .await;
    let Some((incremental, raw)) = received else {
        // A failed poll is a gap, never a fresh observation of cached evidence.
        return (baseline, None);
    };
    let id = id.to_owned();
    client
        .work(move || {
            let report = if incremental {
                match serde_json::from_slice::<sync::Update>(&raw) {
                    Ok(update)
                        if update.report.node_id == id
                            && update.revision.len() == 32
                            && update.revision.bytes().all(|b| b.is_ascii_hexdigit()) =>
                    {
                        sync::Remote::apply(&mut baseline, update)
                    }
                    _ => {
                        baseline = None;
                        None
                    }
                }
            } else {
                // Old nodes and temporary collector fallbacks return a normal full
                // report. Keep the transport baseline for collector recovery only.
                serde_json::from_slice::<process_links::Report>(&raw)
                    .ok()
                    .filter(|report| report.node_id == id)
            };
            (baseline, report)
        })
        .await
        .unwrap_or((None, None))
}

fn confirmed(report: &process_links::Report, published: &process_links::Published) -> bool {
    let bindings: BTreeMap<_, _> = report
        .bindings
        .iter()
        .map(|binding| (&binding.process, binding))
        .collect();
    published.links.iter().all(|link| {
        bindings.get(&link.process).is_some_and(|binding| {
            binding.initiator.as_ref().is_some_and(|initiator| {
                initiator.node_id == link.session.node_id
                    && initiator.source == link.session.source
                    && initiator.sid == link.session.sid
            }) && link.launch_chain.iter().all(|launch| {
                binding
                    .launch_chain
                    .iter()
                    .any(|known| known.process == launch.process)
            })
        })
    })
}
