//! One fleet coordinator shared by session nesting and external CPU consumers.
use super::{Client, Registry};
use futures_util::{StreamExt, stream};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

pub fn spawn(
    registry: Arc<Registry>,
    client: Arc<Client>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        let mut sent = BTreeMap::new();
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                next = tick(&registry, &client, &sent) => { sent = next; },
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
) -> BTreeMap<String, serde_json::Value> {
    let targets: Vec<_> = registry
        .all()
        .into_iter()
        .filter_map(|n| registry.target(&n).ok().map(|t| (n.id, t)))
        .collect();
    let reports: Vec<process_links::Report> = stream::iter(targets.clone())
        .map(|(id, target)| async move {
            let (status, value) = client
                .json(
                    &target,
                    "GET",
                    "/api/process-links",
                    None,
                    Duration::from_secs(3),
                )
                .await
                .ok()?;
            if status != 200 {
                return None;
            }
            let report: process_links::Report = serde_json::from_value(value).ok()?;
            (report.node_id == id).then_some(report)
        })
        .buffer_unordered(8)
        .filter_map(|r| async { r })
        .collect()
        .await;
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
