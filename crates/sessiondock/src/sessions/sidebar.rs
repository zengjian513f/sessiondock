//! Compact sidebar projections: child counts are cheap; child rows are opt-in.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct ExpandedParent {
    pub source: String,
    pub sid: String,
    #[serde(default)]
    pub node_id: String,
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

pub(crate) fn project(document: &mut Value, expanded: &[ExpandedParent], node_id: &str) {
    let Some(rows) = document["sessions"].as_array_mut() else {
        return;
    };
    let mut local_counts = BTreeMap::new();
    let mut remote_counts = BTreeMap::new();
    for row in rows.iter() {
        let parent = &row["nest_parent"];
        let (source, sid, node) = (
            text(parent, "source"),
            text(parent, "sid"),
            text(parent, "node_id"),
        );
        if !source.is_empty() && !sid.is_empty() {
            if node.is_empty() || node == node_id {
                *local_counts
                    .entry((source.to_owned(), sid.to_owned()))
                    .or_insert(0usize) += 1;
            } else {
                *remote_counts
                    .entry((node.to_owned(), source.to_owned(), sid.to_owned()))
                    .or_insert(0usize) += 1;
            }
        }
    }
    let is_open = |source: &str, sid: &str, node: &str| {
        expanded.iter().any(|parent| {
            let wanted_node = if parent.node_id.is_empty() {
                node_id
            } else {
                &parent.node_id
            };
            parent.source == source && parent.sid == sid && wanted_node == node
        })
    };
    rows.retain_mut(|row| {
        let parent = &row["nest_parent"];
        if !text(parent, "sid").is_empty() {
            let node = if text(parent, "node_id").is_empty() {
                node_id
            } else {
                text(parent, "node_id")
            };
            if !is_open(text(parent, "source"), text(parent, "sid"), node) {
                return false;
            }
        }
        if !text(row, "agent_id").is_empty() {
            return false;
        }
        let source = text(row, "source");
        let sid = text(row, "sid");
        let count = local_counts
            .get(&(source.to_owned(), sid.to_owned()))
            .copied()
            .unwrap_or(0)
            + row["agent_items"].as_array().map_or(0, Vec::len);
        let open = is_open(source, sid, node_id);
        row["child_count"] = json!(count);
        if !open && let Some(row) = row.as_object_mut() {
            row.remove("agent_items");
        }
        true
    });
    // A cross-node child contributes a count to its remote owner at the hub.
    document["child_counts"] = json!(
        remote_counts
            .into_iter()
            .map(|((node_id, source, sid), count)| {
                json!({"node_id": node_id, "source": source, "sid": sid, "count": count})
            })
            .collect::<Vec<_>>()
    );
}
