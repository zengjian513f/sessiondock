//! Validate a display edge against the fleet before routing the write to its child.
use super::{Client, Registry, aggregate, proxy::ProxyError};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

pub static WRITER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn transfer_tree(tree: &mut Value, inventory: &Value) {
    let source = tree["uid"]
        .as_str()
        .and_then(|uid| super::namespace::split(uid, true).ok())
        .map(|(node, _)| node)
        .unwrap_or_default();
    let Some(rows) = inventory["sessions"].as_array() else {
        return;
    };
    let parent_uid = |row: &Value| {
        let parent = &row["nest_parent"];
        rows.iter()
            .find(|candidate| {
                parent.is_object()
                    && candidate["node_id"] == *parent.get("node_id").unwrap_or(&row["node_id"])
                    && candidate["source"] == parent["source"]
                    && candidate["sid"] == parent["sid"]
            })
            .and_then(|parent| parent["uid"].as_str())
            .map(str::to_owned)
    };
    let Some(members) = tree["sessions"].as_array_mut() else {
        return;
    };
    let mut included: BTreeSet<String> = members
        .iter()
        .filter_map(|row| row["uid"].as_str().map(str::to_owned))
        .collect();
    loop {
        let before = included.len();
        for row in rows {
            if let (Some(uid), Some(parent)) = (row["uid"].as_str(), parent_uid(row))
                && (included.contains(uid) || included.contains(&parent))
            {
                included.insert(uid.to_owned());
                included.insert(parent);
            }
        }
        if before == included.len() {
            break;
        }
    }
    for row in rows.iter().filter(|row| {
        row["uid"]
            .as_str()
            .is_some_and(|uid| included.contains(uid))
    }) {
        if let Some(member) = members
            .iter_mut()
            .find(|member| member["uid"] == row["uid"])
        {
            if let Some(parent) = parent_uid(row) {
                member["parent_uid"] = parent.into();
            }
        } else {
            members.push(json!({"uid":row["uid"],"sid":row["sid"],"source":row["source"],"title":row["title"],
                "cwd":row["cwd"],"parent_uid":parent_uid(row),"selectable":false,"required_uids":[],
                "native_missing":row["node_id"] == source,
                "disabled_reason":if row["node_id"] == source {"正在核对原生会话"} else {"位于其他机器，请从该机器迁移"},"node_name":row["node_name"]}));
        }
    }
}

impl Registry {
    /// Third-node children retain their saved parent identity. Resolve its
    /// location through durable, completed moves, including offline children
    /// returning after the move and successive moves back to the original node.
    pub(crate) fn relocate_nesting(&self, payload: &mut Value, node: &str) {
        let Some(links) = self.transfer_links.get() else {
            return;
        };
        let mut records: Vec<_> = links
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .filter(|record| record.mode == "move" && record.phase == "complete")
            .cloned()
            .collect();
        records.sort_by(|a, b| (a.created_ms, &a.id).cmp(&(b.created_ms, &b.id)));
        fn walk(value: &mut Value, node: &str, records: &[crate::session_links::Record]) {
            match value {
                Value::Array(rows) => {
                    for row in rows {
                        walk(row, node, records);
                    }
                }
                Value::Object(row) => {
                    let owner = row
                        .get("node_id")
                        .and_then(Value::as_str)
                        .unwrap_or(node)
                        .to_owned();
                    if let Some(counts) = row.get_mut("child_counts").and_then(Value::as_array_mut)
                    {
                        for count in counts {
                            let mut location = count["node_id"].as_str().unwrap_or(node).to_owned();
                            let identity = format!(
                                "{}:{}",
                                count["source"].as_str().unwrap_or(""),
                                count["sid"].as_str().unwrap_or("")
                            );
                            for record in records {
                                if record.source_node == location
                                    && record.mappings.contains_key(&identity)
                                {
                                    location = record.target_node.clone();
                                }
                            }
                            count["node_id"] = location.into();
                        }
                    }
                    if let Some(parent) = row
                        .get_mut("nest_parent")
                        .filter(|parent| parent.is_object())
                    {
                        let mut location = parent["node_id"].as_str().unwrap_or(&owner).to_owned();
                        let mut identity = format!(
                            "{}:{}",
                            parent["source"].as_str().unwrap_or(""),
                            parent["sid"].as_str().unwrap_or("")
                        );
                        for record in records {
                            if record.source_node == location
                                && let Some(next) = record.mappings.get(&identity)
                            {
                                location = record.target_node.clone();
                                identity = next.clone();
                            }
                        }
                        if let Some((source, sid)) = identity.split_once(':') {
                            parent["source"] = source.into();
                            parent["sid"] = sid.into();
                            if location == owner {
                                parent.as_object_mut().unwrap().remove("node_id");
                            } else {
                                parent["node_id"] = location.into();
                            }
                        }
                    }
                    for key in ["sessions", "results", "session", "meta"] {
                        if let Some(value) = row.get_mut(key) {
                            walk(value, &owner, records);
                        }
                    }
                }
                _ => {}
            }
        }
        walk(payload, node, &records);
    }
}

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
