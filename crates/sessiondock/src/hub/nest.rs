//! Validate a display edge against the fleet before routing the write to its child.
use super::{Client, Registry, aggregate, proxy::ProxyError};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

pub static WRITER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn prepare(
    registry: &Registry,
    client: &Client,
    body: &mut Map<String, Value>,
) -> Result<(), ProxyError> {
    // The browser cannot supply an authenticated hub's resolved parent descriptor.
    body.remove("remote_parent");
    let Some(parent_uid) = body
        .get("parent_uid")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
    else {
        return Ok(());
    };
    let uid = body.get("uid").and_then(Value::as_str).unwrap_or("");
    let inventory = aggregate::sessions(registry, client, &[("force".into(), "1".into())])
        .await
        .map_err(|error| ProxyError::Invalid(error.message().into()))?;
    let rows = inventory["sessions"]
        .as_array()
        .ok_or(ProxyError::Upstream)?;
    let find = |uid: &str| rows.iter().find(|row| row["uid"] == uid);
    let child = find(uid).ok_or_else(|| ProxyError::Invalid("会话不存在".into()))?;
    let parent = find(&parent_uid).ok_or_else(|| ProxyError::Invalid("目标会话不存在".into()))?;
    let mut seen = BTreeSet::from([uid.to_owned()]);
    let mut current = Some(parent);
    while let Some(row) = current {
        if !seen.insert(row["uid"].as_str().unwrap_or("").to_owned()) {
            return Err(ProxyError::Invalid(
                "不能附属到自己或自己的子会话下面".into(),
            ));
        }
        let spec = row.get("nest_parent");
        current = spec.and_then(|spec| {
            let node = spec.get("node_id").unwrap_or(&row["node_id"]);
            rows.iter().find(|candidate| {
                candidate["node_id"] == *node
                    && candidate["source"] == spec["source"]
                    && candidate["sid"] == spec["sid"]
            })
        });
    }
    if child["node_id"] != parent["node_id"] {
        body.insert(
            "remote_parent".into(),
            json!({"node_id": parent["node_id"], "source": parent["source"], "sid": parent["sid"]}),
        );
        body.remove("parent_uid");
    }
    Ok(())
}
