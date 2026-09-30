//! Session resource views preserve execution nodes and incomplete observations.
//! The UI UID is an opaque path identity: resolve its published native SID before
//! joining the shared resource protocol, never infer SID from the UID's suffix.
use super::{Client, Registry, namespace};
use futures_util::{StreamExt, stream};
use process_links::{Session, agent::Resources};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    let age = now() - resources.sampled_at;
    if !resources.sampled_at.is_finite() || !(-5.0..=15.0).contains(&age) {
        return unavailable(
            node_id,
            node_name,
            "stale",
            "resource_sample_stale_or_clock_skew",
        );
    }
    json!({"node_id":node_id,"node_name":node_name,"status":"ok", "sampled_at":resources.sampled_at,
        "metrics": process_links::resource_summary::metrics(&resources, session, inclusive)})
}

pub fn response(rows: Vec<Value>) -> Value {
    let partial = rows.iter().any(|row| row["status"] != "ok");
    let totals = process_links::resource_summary::totals(&rows);
    json!({"sampled_at":now(),"partial":partial,"totals":{"metrics":totals,"partial":partial},"nodes":rows})
}

pub async fn get(
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
    Ok(response(rows.into_iter().map(|(_, row)| row).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> Session {
        Session {
            node_id: "node".into(),
            source: "codex".into(),
            sid: "native:sid".into(),
            title: None,
            created: None,
        }
    }
    fn sample() -> Value {
        json!({"version":1,"node_id":"node","boot_id":"boot","sampled_at":now(),
            "availability":"observed","method":"fixture","samples":[],"unavailable":[]})
    }
    #[test]
    fn uid_uses_inventory_native_sid() {
        let list =
            json!({"sessions":[{"uid":"codex:opaque-hash","source":"codex","sid":"native:sid"}]});
        assert_eq!(
            session_from_list(&list, "codex:opaque-hash", "node")
                .unwrap()
                .sid,
            "native:sid"
        );
        assert!(session_from_list(&list, "codex:native:sid", "node").is_none());
    }
    #[test]
    fn stale_and_foreign_node_data_never_become_zero_usage() {
        let mut stale = sample();
        stale["sampled_at"] = json!(now() - 60.0);
        let row = node_row("node", "name", stale, &session(), false);
        assert_eq!(row["status"], "stale");
        assert!(row["metrics"]["cpu_cores"]["value"].is_null());
        let mut wrong = sample();
        wrong["node_id"] = json!("other");
        assert_eq!(
            node_row("node", "name", wrong, &session(), false)["status"],
            "unsupported"
        );
        assert_eq!(response(vec![row])["totals"]["partial"], true);
    }
}
