//! Session resource views preserve execution nodes and incomplete observations.
//! The UI UID is an opaque path identity: resolve its published native SID before
//! joining the shared resource protocol, never infer SID from the UID's suffix.
use super::{Client, Registry, namespace};
use futures_util::{StreamExt, stream};
use process_links::{Session, agent::Resources};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Remember verified participants while this Hub runs, including during outages.
#[derive(Default)]
pub struct RelatedNodes(Mutex<BTreeMap<(String, String, String), BTreeSet<String>>>);
impl RelatedNodes {
    fn filter(&self, session: &Session, rows: Vec<Value>) -> Vec<Value> {
        let mut cache = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let known = cache
            .entry((
                session.node_id.clone(),
                session.source.clone(),
                session.sid.clone(),
            ))
            .or_default();
        known.insert(session.node_id.clone());
        for row in &rows {
            if row["session_related"] == true
                && let Some(id) = row["node_id"].as_str()
            {
                known.insert(id.to_owned());
            }
        }
        rows.into_iter()
            .filter(|row| row["node_id"].as_str().is_some_and(|id| known.contains(id)))
            .collect()
    }
}
fn same_session(a: &Session, b: &Session) -> bool {
    a.node_id == b.node_id && a.source == b.source && a.sid == b.sid
}

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

pub fn inclusive(scope: Option<&str>) -> Result<bool, String> {
    match scope.unwrap_or("direct") {
        "direct" => Ok(false),
        "inclusive" => Ok(true),
        _ => Err("scope 必须为 direct 或 inclusive".into()),
    }
}

pub fn session_from_list(document: &Value, uid: &str, node_id: &str) -> Option<Session> {
    let row = document
        .get("sessions")?
        .as_array()?
        .iter()
        .find(|row| row["uid"] == uid)?;
    let source = row.get("source")?.as_str()?.to_owned();
    let sid = row.get("sid")?.as_str()?.to_owned();
    if source.is_empty() || sid.is_empty() {
        return None;
    }
    Some(Session {
        node_id: node_id.to_owned(),
        source,
        sid,
        title: row.get("title").and_then(Value::as_str).map(str::to_owned),
        created: None,
    })
}

pub fn unavailable(node_id: &str, node_name: &str, status: &str, reason: &str) -> Value {
    json!({"node_id":node_id,"node_name":node_name,"status":status,"reason":reason,"metrics":{}})
}

pub fn node_row(
    node_id: &str,
    node_name: &str,
    value: Value,
    session: &Session,
    inclusive: bool,
) -> Value {
    if value["availability"] == "unavailable" || value["supported"] == false {
        return unavailable(
            node_id,
            node_name,
            "unsupported",
            value["reason"].as_str().unwrap_or("collector_unavailable"),
        );
    }
    let diagnostic = value
        .get("diagnostic")
        .cloned()
        .unwrap_or_else(|| json!({"state":"unsupported","remaining_seconds":0}));
    let Ok(resources) = serde_json::from_value::<Resources>(value) else {
        return unavailable(
            node_id,
            node_name,
            "unsupported",
            "invalid_resource_protocol",
        );
    };
    if resources.version != 1 || resources.node_id != node_id || resources.boot_id.is_empty() {
        return unavailable(
            node_id,
            node_name,
            "unsupported",
            "resource_identity_mismatch",
        );
    }
    // Scope changes the sum, not which machines participated in the session.
    let related = resources.bindings.iter().any(|b| {
        same_session(&b.session, session)
            || b.initiator
                .as_ref()
                .is_some_and(|s| same_session(s, session))
            || b.launch_chain
                .iter()
                .any(|launch| same_session(&launch.session, session))
    });
    let age = now() - resources.sampled_at;
    if !resources.sampled_at.is_finite() || !(-5.0..=15.0).contains(&age) {
        let mut row = unavailable(
            node_id,
            node_name,
            "stale",
            "resource_sample_stale_or_clock_skew",
        );
        row["session_related"] = json!(related);
        row["diagnostic"] = diagnostic.clone();
        return row;
    }
    json!({"node_id":node_id,"node_name":node_name,"status":"ok", "sampled_at":resources.sampled_at,"session_related":related,
        "diagnostic":diagnostic,"metrics": process_links::resource_summary::metrics(&resources, session, inclusive)})
}

pub fn response(rows: Vec<Value>) -> Value {
    let partial = rows.iter().any(|row| row["status"] != "ok");
    let totals = process_links::resource_summary::totals(&rows);
    json!({"sampled_at":now(),"partial":partial,"totals":{"metrics":totals,"partial":partial},"nodes":rows})
}

pub async fn get(
    related: &RelatedNodes,
    registry: &Registry,
    client: &Client,
    uid: &str,
    inclusive: bool,
) -> Result<Value, String> {
    let (node_id, _) = namespace::split(uid, true).map_err(|error| error.to_string())?;
    let source = registry
        .get(&node_id)
        .ok_or_else(|| "会话所属机器未注册或已停用".to_owned())?;
    let inventory = tokio::time::timeout(
        Duration::from_secs(3),
        registry.fetch(client, &source, "/api/sessions", &Vec::new(), None),
    )
    .await
    .map_err(|_| "暂时无法解析会话身份，请稍后重试".to_owned())?;
    let session = session_from_list(&inventory.data, uid, &node_id)
        .ok_or_else(|| "会话不存在或尚无缓存的会话身份".to_owned())?;
    let mut rows: Vec<(usize, Value)> = stream::iter(registry.all().into_iter().enumerate())
        .map(|(index, node)| {
            let session = &session;
            async move {
                let missing = |status, reason| unavailable(&node.id, &node.name, status, reason);
                if registry.offline(&node.id) {
                    return (index, missing("offline", "node_offline"));
                }
                let Ok(target) = registry.target(&node) else {
                    return (index, missing("offline", "node_target_unavailable"));
                };
                let value = match tokio::time::timeout(
                    Duration::from_secs(3),
                    client.json(
                        &target,
                        "GET",
                        "/api/resources",
                        None,
                        Duration::from_secs(3),
                    ),
                )
                .await
                {
                    Ok(Ok((200, value))) => {
                        node_row(&node.id, &node.name, value, session, inclusive)
                    }
                    Ok(Ok((404 | 501, _))) => {
                        missing("unsupported", "resource_endpoint_unavailable")
                    }
                    _ => missing("offline", "resource_request_failed"),
                };
                (index, value)
            }
        })
        .buffer_unordered(8)
        .collect()
        .await;
    rows.sort_by_key(|(index, _)| *index);
    Ok(response(related.filter(
        &session,
        rows.into_iter().map(|(_, row)| row).collect(),
    )))
}

/// Only the owner and verified execution participants can be targeted by a session action.
#[allow(clippy::too_many_arguments)] // Mirrors the session resource action fields.
pub async fn probe(
    related: &RelatedNodes,
    registry: &Registry,
    client: &Client,
    uid: &str,
    inclusive: bool,
    enabled: bool,
    lease_id: Option<&str>,
    lease_seconds: Option<u64>,
) -> Result<Value, String> {
    let view = get(related, registry, client, uid, inclusive).await?;
    let rows = view["nodes"].as_array().cloned().unwrap_or_default();
    let results: Vec<Value> = stream::iter(rows).map(|row| async move {
        let id = row["node_id"].as_str().unwrap_or_default();
        let error = |message: &str| json!({"node_id":id,"node_name":row["node_name"],"ok":false,"error":message});
        if row["status"] != "ok" && row["status"] != "stale" { return error("机器离线或不支持临时探测"); }
        let Some(node) = registry.get(id) else { return error("机器未注册"); };
        let Ok(target) = registry.target(&node) else { return error("机器暂时无法连接"); };
        match client.json(&target, "POST", "/api/resources/probe", Some(&json!({"enabled":enabled,"lease_id":lease_id,"lease_seconds":lease_seconds})), Duration::from_secs(3)).await {
            Ok((200, value)) => json!({"node_id":id,"node_name":row["node_name"],"ok":true,"diagnostic":value}),
            _ => error("机器探测请求失败，请检查采集服务"),
        }
    }).buffer_unordered(8).collect().await;
    Ok(json!({"partial":results.iter().any(|r| r["ok"] != true),"nodes":results}))
}

/// One cached collector snapshot per node for the entire sidebar, not a fleet
/// request for each historical session. Rows use native identities, never UIDs.
pub fn list_summary(documents: Vec<(String, Value)>, mut partial: bool) -> Value {
    let mut sessions: BTreeMap<(String, String, String), (Session, Vec<Value>)> = BTreeMap::new();
    for (node_id, document) in documents {
        let Ok(resources) = serde_json::from_value::<Resources>(document) else {
            partial = true;
            continue;
        };
        let age = now() - resources.sampled_at;
        if resources.version != 1
            || resources.node_id != node_id
            || resources.boot_id.is_empty()
            || !age.is_finite()
            || !(-5.0..=15.0).contains(&age)
        {
            partial = true;
            continue;
        }
        for row in process_links::resource_summary::sessions(&resources) {
            let Ok(session) = serde_json::from_value::<Session>(row["session"].clone()) else {
                continue;
            };
            sessions
                .entry((
                    session.node_id.clone(),
                    session.source.clone(),
                    session.sid.clone(),
                ))
                .or_insert_with(|| (session, Vec::new()))
                .1
                .push(row);
        }
    }
    let rows: Vec<_> = sessions
        .into_values()
        .map(|(session, rows)| {
            let all = process_links::resource_summary::totals(&rows);
            let metrics: serde_json::Map<_, _> = [
                "cpu_cores",
                "process_count",
                "memory_pss_bytes",
                "gpu_count",
                "proc_storage_read_bytes_per_second",
                "proc_storage_write_bytes_per_second",
            ]
            .into_iter()
            .map(|key| (key.to_owned(), all[key].clone()))
            .collect();
            json!({"session":session,"metrics":metrics})
        })
        .collect();
    json!({"sampled_at":now(),"scope":"direct","partial":partial,"sessions":rows})
}

pub async fn get_list_summary(registry: &Registry, client: &Client) -> Value {
    let results: Vec<_> = stream::iter(registry.all())
        .map(|node| async move {
            if registry.offline(&node.id) {
                return None;
            }
            let target = registry.target(&node).ok()?;
            match tokio::time::timeout(
                Duration::from_secs(3),
                client.json(
                    &target,
                    "GET",
                    "/api/resources",
                    None,
                    Duration::from_secs(3),
                ),
            )
            .await
            {
                Ok(Ok((200, value))) => Some((node.id.clone(), value)),
                _ => None,
            }
        })
        .buffer_unordered(8)
        .collect()
        .await;
    let partial = results.iter().any(Option::is_none);
    list_summary(results.into_iter().flatten().collect(), partial)
}
