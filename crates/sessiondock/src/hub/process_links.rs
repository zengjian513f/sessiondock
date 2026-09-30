//! One fleet coordinator shared by session nesting and external CPU consumers.
use super::{Client, Registry};
use futures_util::{StreamExt, stream};
use std::{sync::Arc, time::Duration};

pub fn spawn(
    registry: Arc<Registry>,
    client: Arc<Client>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tick(&registry, &client) => {},
            }
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(2)) => {},
            }
        }
    });
}

async fn tick(registry: &Registry, client: &Client) {
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
    stream::iter(targets)
        .map(|(id, target)| {
            let body = links.remove(&id).and_then(|p| serde_json::to_value(p).ok());
            async move {
                if let Some(body) = body {
                    let _ = client
                        .json(
                            &target,
                            "POST",
                            "/api/process-links",
                            Some(&body),
                            Duration::from_secs(3),
                        )
                        .await;
                }
            }
        })
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
}
