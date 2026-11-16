//! Session resource views preserve execution nodes and incomplete observations.
//! The UI UID is an opaque path identity: resolve its published native SID before
//! joining the shared resource protocol, never infer SID from the UID's suffix.
use super::{Client, ClientError, Registry, Target, namespace};
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
                    compact_session(client, &target, &node.id, &node.name, session, inclusive),
                )
                .await
                {
                    Ok(Ok((200, mut value))) => {
                        value["node_id"] = json!(node.id);
                        value["node_name"] = json!(node.name);
                        value
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
    // An unassigned remote workload cannot establish participation, but it
    // must not disappear from coverage when unrelated node rows are hidden.
    let gpu_incomplete = rows
        .iter()
        .any(|(_, row)| row["metrics"]["gpu_count"]["attribution_incomplete"] == true);
    let mut result =
        response(related.filter(&session, rows.into_iter().map(|(_, row)| row).collect()));
    if gpu_incomplete {
        process_links::resource_summary::mark_gpu_attribution_incomplete(
            &mut result["totals"]["metrics"],
        );
        result["partial"] = json!(true);
        result["totals"]["partial"] = json!(true);
    }
    Ok(result)
}

/// Inspect the status before parsing: older routers may return an empty or
/// non-JSON 404/405 for this endpoint.
async fn fetch_compact(
    client: &Client,
    target: &Target,
    body: Option<&Value>,
) -> Result<(u16, Value), ClientError> {
    let encoded = body
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|_| ClientError::Invalid("invalid resource request"))?;
    let response = client
        .open(
            target,
            super::client::Request {
                method: if body.is_some() { "POST" } else { "GET" },
                target: "/api/resources/compact",
                headers: &[("Content-Type", "application/json")],
                body: encoded.as_deref(),
                connect: Duration::from_secs(3),
                idle: Duration::from_secs(3),
            },
        )
        .await?;
    let status = response.status;
    if status != 200 {
        return Ok((status, Value::Null));
    }
    let raw = response.into_body().read_to_end(super::JSON_LIMIT).await?;
    let value = client
        .work(move || {
            serde_json::from_slice(&raw).map_err(|_| ClientError::Invalid("body is not JSON"))
        })
        .await??;
    Ok((status, value))
}

/// Old nodes keep their read path during a rolling upgrade. A transient failure
/// must not trigger another, much larger request.
async fn compact_session(
    client: &Client,
    target: &Target,
    node_id: &str,
    node_name: &str,
    session: &Session,
    inclusive: bool,
) -> Result<(u16, Value), ClientError> {
    let body = json!({"session":session,"inclusive":inclusive});
    let (status, value) = fetch_compact(client, target, Some(&body)).await?;
    if matches!(status, 404 | 405 | 501) {
        let (status, value) = client
            .json(
                target,
                "GET",
                "/api/resources",
                None,
                Duration::from_secs(3),
            )
            .await?;
        return Ok((
            status,
            if status == 200 {
                node_row(node_id, node_name, value, session, inclusive)
            } else {
                value
            },
        ));
    }
    Ok((status, value))
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
