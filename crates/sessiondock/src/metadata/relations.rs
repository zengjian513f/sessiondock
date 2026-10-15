//! Work handoffs between top-level sessions. Native parent/fork semantics stay separate.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Predecessor {
    /// Exact local record UID, deliberately not a namespace-rewritten `uid` field.
    pub session_uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub sid: String,
    #[serde(default = "continues")]
    pub kind: String,
    #[serde(default = "confirmed")]
    pub status: String,
    #[serde(default)]
    pub artifact: String,
    #[serde(default)]
    pub evidence: String,
}
fn continues() -> String {
    "continues".into()
}
fn confirmed() -> String {
    "confirmed".into()
}

pub fn top_level(row: &Value) -> bool {
    row["pending"] != true
        && row["source"] != "shell"
        && !row["agent_id"].as_str().is_some_and(|s| !s.is_empty())
        && !row["nest_parent"].is_object()
}

pub fn descriptor(row: &Value, owner: &Value) -> Predecessor {
    let uid = row["uid"].as_str().unwrap_or("");
    let session_uid = crate::hub::namespace::split(uid, true)
        .map(|(_, uid)| uid)
        .unwrap_or_else(|_| uid.into());
    Predecessor {
        session_uid,
        node_id: (row["node_id"] != owner["node_id"])
            .then(|| row["node_id"].as_str().unwrap_or("").to_owned()),
        title: row["title"].as_str().unwrap_or("").into(),
        source: row["source"].as_str().unwrap_or("").into(),
        sid: row["sid"].as_str().unwrap_or("").into(),
        kind: continues(),
        status: confirmed(),
        ..Default::default()
    }
}

pub fn resolve(owner: &Value, relation: &Predecessor) -> String {
    let node = relation
        .node_id
        .as_deref()
        .or_else(|| owner["node_id"].as_str())
        .unwrap_or("");
    if node.is_empty() {
        return relation.session_uid.clone();
    }
    crate::hub::namespace::qualify(node, &relation.session_uid, true).unwrap_or_default()
}

/// Reverse adjacency is derived, so an edge has exactly one durable writer.
pub fn decorate(rows: &mut [Value]) {
    let known: BTreeMap<_, _> = rows
        .iter()
        .map(|r| (r["uid"].as_str().unwrap_or("").to_owned(), r))
        .collect();
    let mut reverse: BTreeMap<String, Vec<Predecessor>> = BTreeMap::new();
    for owner in rows.iter().filter(|row| top_level(row)) {
        for raw in owner["predecessors"].as_array().into_iter().flatten() {
            let Ok(edge) = serde_json::from_value::<Predecessor>(raw.clone()) else {
                continue;
            };
            let uid = resolve(owner, &edge);
            if let Some(source) = known.get(&uid).filter(|row| top_level(row)) {
                let mut back = descriptor(owner, source);
                back.kind = edge.kind;
                back.status = edge.status;
                back.artifact = edge.artifact;
                back.evidence = edge.evidence;
                reverse.entry(uid).or_default().push(back);
            }
        }
    }
    for row in rows {
        row["successors"] = json!(
            reverse
                .remove(row["uid"].as_str().unwrap_or(""))
                .unwrap_or_default()
        );
        if !row["predecessors"].is_array() {
            row["predecessors"] = json!([]);
        }
        row["relations_top_level"] = json!(top_level(row));
    }
}

fn entry(
    owner: &Value,
    edge: &Predecessor,
    known: &BTreeMap<String, &Value>,
    distance: usize,
) -> Value {
    let uid = resolve(owner, edge);
    let found = known.get(&uid).filter(|row| top_level(row));
    let mut value = json!(edge);
    value["uid"] = json!(uid);
    value["available"] = json!(found.is_some_and(|row| row["stale"] != true));
    value["distance"] = json!(distance);
    if let Some(row) = found {
        for key in [
            "title",
            "source",
            "sid",
            "cwd",
            "node_id",
            "node_name",
            "created",
            "updated",
        ] {
            if let Some(v) = row.get(key) {
                value[key] = v.clone();
            }
        }
    }
    value
}

pub fn info(inventory: &Value, uid: &str) -> Option<Value> {
    let mut rows = inventory["sessions"].as_array()?.clone();
    decorate(&mut rows);
    let known: BTreeMap<_, _> = rows
        .iter()
        .filter_map(|row| Some((row["uid"].as_str()?.to_owned(), row)))
        .collect();
    let row = *known.get(uid)?;
    let walk = |key: &str, recursive: bool| {
        let mut seen = BTreeSet::from([uid.to_owned()]);
        let mut queue = VecDeque::from([(row, 1, false)]);
        let mut output = Vec::new();
        while let Some((owner, distance, uncertain)) = queue.pop_front() {
            if !top_level(owner) {
                continue;
            }
            for raw in owner[key].as_array().into_iter().flatten() {
                let Ok(edge) = serde_json::from_value::<Predecessor>(raw.clone()) else {
                    continue;
                };
                let target = resolve(owner, &edge);
                if !seen.insert(target.clone()) {
                    continue;
                }
                let mut item = entry(owner, &edge, &known, distance);
                let uncertain = uncertain || edge.status == "candidate";
                item["path_status"] = json!(if uncertain { "candidate" } else { "confirmed" });
                if recursive && let Some(next) = known.get(&target) {
                    queue.push_back((next, distance + 1, uncertain));
                }
                output.push(item);
            }
        }
        output
    };
    Some(
        json!({"session": row, "predecessors": walk("predecessors", false),
        "successors": walk("successors", false), "ancestors": walk("predecessors", true),
        "descendants": walk("successors", true), "partial": inventory["partial"].as_bool().unwrap_or(false),
        "errors": inventory.get("errors").cloned().unwrap_or_else(|| json!([]))}),
    )
}
