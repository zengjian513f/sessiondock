//! Periodic cleanup counts only: no candidate identities, rows, or disk state.
use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(crate) struct Counts(Mutex<Value>);

impl Counts {
    pub fn summary(&self) -> Value {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn spawn<F, Fut>(self: &Arc<Self>, shutdown: CancellationToken, read: F)
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(Value, Value), String>> + Send,
    {
        let counts = self.clone();
        // Operational override also lets private browser fixtures exercise the timer.
        let seconds = std::env::var("SESSIONDOCK_CLEANUP_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(10800);
        tokio::spawn(async move {
            loop {
                let checked_at = DateTime::<Utc>::from(std::time::SystemTime::now());
                let result = tokio::select! {
                    _ = shutdown.cancelled() => return,
                    result = read() => result.and_then(|(rows, live)| summarize(&rows, &live, checked_at)),
                };
                {
                    let mut value = counts.0.lock().unwrap_or_else(|e| e.into_inner());
                    match result {
                        Ok(summary) => *value = summary,
                        Err(error) => {
                            if !value.is_object() {
                                *value = json!({"ready": false});
                            }
                            value["error"] = json!(error);
                        }
                    }
                    value["interval_seconds"] = json!(seconds);
                    value["next_check_at"] = json!(
                        DateTime::<Utc>::from(std::time::SystemTime::now()).timestamp_millis()
                            as u64
                            + seconds.saturating_mul(1000)
                    );
                }
                tokio::select! {
                    _ = shutdown.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(seconds)) => {}
                }
            }
        });
    }
}

fn summarize(sessions: &Value, live: &Value, checked_at: DateTime<Utc>) -> Result<Value, String> {
    let rows = sessions["sessions"].as_array().ok_or("会话列表格式错误")?;
    let uids = live["uids"].as_array().ok_or("运行状态格式错误")?;
    if live["known"] == false {
        return Err("运行状态未知".into());
    }
    let running: HashSet<&str> = uids.iter().filter_map(Value::as_str).collect();
    let unavailable: HashSet<&str> = [sessions, live]
        .into_iter()
        .flat_map(|value| {
            value["errors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|error| error["node_id"].as_str())
        })
        .collect();
    let offline: HashSet<&str> = [sessions, live]
        .into_iter()
        .flat_map(|value| {
            value["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|node| node["online"] == false)
                .filter_map(|node| node["id"].as_str())
        })
        .collect();
    if let Some(nodes) = sessions["nodes"].as_array()
        && !nodes.is_empty()
        && nodes.iter().all(|node| {
            node["id"]
                .as_str()
                .is_some_and(|id| unavailable.contains(id) || offline.contains(id))
        })
    {
        return Err("所有机器的状态均无法读取".into());
    }
    let mut days = BTreeMap::<i64, usize>::new();
    for row in rows {
        let uid = row["uid"].as_str().unwrap_or("");
        let node = row["node_id"].as_str().unwrap_or("");
        if !running.contains(uid)
            || row["stale"] == true
            || row["agent_id"].as_str().is_some_and(|s| !s.is_empty())
            || unavailable.contains(node)
            || offline.contains(node)
        {
            continue;
        }
        let Some(updated) = row["updated"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        else {
            continue;
        };
        let age = checked_at.signed_duration_since(updated).num_milliseconds();
        // Strictly older than N days: exactly N days is not eligible yet.
        if age > 86400000 {
            *days.entry((age - 1) / 86400000).or_default() += 1;
        }
    }
    Ok(
        json!({"ready": true, "checked_at": checked_at.timestamp_millis(), "days": days,
        "partial": sessions["partial"] == true || live["partial"] == true || !offline.is_empty(), "error": null}),
    )
}
