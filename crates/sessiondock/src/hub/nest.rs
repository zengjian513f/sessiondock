//! Validate a display edge against the fleet before routing the write to its child.
use super::{Client, Registry, Target, aggregate, namespace, proxy::ProxyError, registry::Node};
use crate::session_links::Record;
use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap, HashSet},
    time::Duration,
};

pub static WRITER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Keep the inventory and its discovery state for all native reads in this preview.
struct TransferTree<'a> {
    rows: Vec<&'a Value>,
    by_uid: HashMap<&'a str, Vec<usize>>,
    parents: HashMap<&'a str, &'a str>,
    neighbors: HashMap<&'a str, Vec<&'a str>>,
    included: HashSet<&'a str>,
    members: HashMap<String, usize>,
}

impl<'a> TransferTree<'a> {
    fn new(inventory: &'a Value, members: &[Value]) -> Self {
        let rows: Vec<_> = inventory["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .collect();
        let mut by_identity = HashMap::with_capacity(rows.len());
        for &row in &rows {
            // The first inventory row wins, even if its UID is missing.
            by_identity
                .entry((&row["node_id"], &row["source"], &row["sid"]))
                .or_insert(row);
        }
        let mut graph = Self {
            rows,
            by_uid: HashMap::new(),
            parents: HashMap::new(),
            neighbors: HashMap::new(),
            included: HashSet::new(),
            members: HashMap::with_capacity(members.len()),
        };
        for (index, row) in graph.rows.iter().enumerate() {
            let Some(uid) = row["uid"].as_str() else {
                continue;
            };
            graph.by_uid.entry(uid).or_default().push(index);
            let spec = &row["nest_parent"];
            if spec.is_object()
                && let Some(parent) = by_identity
                    .get(&(
                        spec.get("node_id").unwrap_or(&row["node_id"]),
                        &spec["source"],
                        &spec["sid"],
                    ))
                    .and_then(|row| row["uid"].as_str())
            {
                graph.parents.insert(uid, parent);
                graph.neighbors.entry(uid).or_default().push(parent);
                graph.neighbors.entry(parent).or_default().push(uid);
            }
        }
        for (index, member) in members.iter().enumerate() {
            if let Some(uid) = member["uid"].as_str() {
                graph.members.entry(uid.to_owned()).or_insert(index);
            }
        }
        graph
    }

    fn extend(
        &mut self,
        members: &mut Vec<Value>,
        source: &str,
        seeds: Vec<String>,
    ) -> Vec<String> {
        let mut pending: Vec<_> = seeds
            .iter()
            .filter_map(|uid| self.by_uid.get_key_value(uid.as_str()).map(|(&uid, _)| uid))
            .collect();
        let mut connected: Vec<usize> = Vec::new();
        while let Some(uid) = pending.pop() {
            if self.included.insert(uid) {
                connected.extend(self.by_uid[uid].iter().copied());
                pending.extend(self.neighbors.get(uid).into_iter().flatten().copied());
            }
        }

        // Apply newly reached rows in inventory order. Existing native members
        // retain their position and only receive a resolved display parent.
        connected.sort_unstable();
        let mut missing = Vec::new();
        for index in connected {
            let row = self.rows[index];
            let uid = row["uid"].as_str().unwrap();
            let parent = self.parents.get(uid).copied();
            if let Some(&index) = self.members.get(uid) {
                if let Some(parent) = parent {
                    members[index]["parent_uid"] = parent.into();
                }
            } else {
                self.members.insert(uid.to_owned(), members.len());
                let local = row["node_id"] == source;
                if local {
                    missing.push(uid.to_owned());
                }
                members.push(json!({"uid": uid, "sid": row["sid"], "source": row["source"], "title": row["title"],
                    "cwd": row["cwd"], "parent_uid": parent, "selectable": false, "required_uids": [],
                    "native_missing": local,
                    "disabled_reason": if local {"正在核对原生会话"} else {"位于其他机器，请从该机器迁移"},
                    "node_name": row["node_name"]}));
            }
        }
        missing
    }

    fn merge(
        &mut self,
        members: &mut Vec<Value>,
        rows: Vec<Value>,
        requested: Vec<String>,
    ) -> Vec<String> {
        for uid in requested {
            let member = &mut members[self.members[&uid]];
            member["native_missing"] = false.into();
            member["disabled_reason"] = "原生记录当前不可迁移".into();
        }
        let mut new = Vec::new();
        for mut member in rows {
            let Some(uid) = member["uid"].as_str().map(str::to_owned) else {
                continue;
            };
            if let Some(parent) = self.parents.get(uid.as_str()) {
                member["parent_uid"] = (*parent).into();
            }
            let index = *self.members.entry(uid.clone()).or_insert(members.len());
            if index == members.len() {
                members.push(member);
                new.push(uid);
            } else {
                members[index] = member;
            }
        }
        new
    }
}

pub async fn complete_transfer_tree(
    client: &Client,
    target: &Target,
    node: &Node,
    inventory: &Value,
    tree: &mut Value,
) -> Result<(), ProxyError> {
    let Some((source, anchor)) = tree["uid"]
        .as_str()
        .and_then(|uid| namespace::split(uid, true).ok())
    else {
        return Ok(());
    };
    let Some(members) = tree["sessions"].as_array_mut() else {
        return Ok(());
    };
    let mut graph = TransferTree::new(inventory, members);
    let seeds = graph.members.keys().cloned().collect();
    let mut missing = graph.extend(members, &source, seeds);
    let mut batch_supported = true;

    while !missing.is_empty() {
        let local = missing
            .iter()
            .map(|uid| namespace::split(uid, true).map(|(_, uid)| uid))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ProxyError::Invalid(error.to_string()))?;
        let read = |body: Value| async move {
            let result = client
                .json(
                    target,
                    "POST",
                    "/api/session/transfer/tree",
                    Some(&body),
                    Duration::from_secs(300),
                )
                .await;
            result
                .ok()
                .filter(|(status, _)| *status == 200)
                .map(|(_, value)| value)
        };
        let scoped_rows = |value: Value| {
            let mut value = namespace::public_payload(value, node, "/api/session/transfer/tree");
            match value.get_mut("sessions").map(Value::take) {
                Some(Value::Array(rows)) => rows,
                _ => Vec::new(),
            }
        };
        let rows = if batch_supported
            && let Some(value) = read(json!({"uid": anchor, "related_uids": local})).await
            && value["supplemented"] == true
        {
            scoped_rows(value)
        } else {
            // Older nodes ignore related_uids. They still return usable native
            // groups through the existing single-anchor route.
            batch_supported = false;
            futures_util::stream::iter(local.into_iter().map(|uid| read(json!({"uid": uid}))))
                .buffered(aggregate::FAN_OUT)
                .filter_map(|value| async { value.map(scoped_rows) })
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .flatten()
                .collect()
        };
        let seeds = graph.merge(members, rows, missing);
        missing = graph.extend(members, &source, seeds);
    }
    Ok(())
}

impl Registry {
    /// Third-node children retain their saved parent identity. Resolve its
    /// location through durable, completed moves, including offline children
    /// returning after the move and successive moves back to the original node.
    pub(crate) fn relocate_nesting(&self, payload: &mut Value, node: &str) {
        let (_, records) = self.nesting_snapshot();
        if let Some(records) = records.as_deref() {
            relocate_payload(payload, node, records);
        }
    }
}

struct ParentLocation<'a> {
    node: &'a str,
    identity: Cow<'a, str>,
}

impl<'a> ParentLocation<'a> {
    fn resolve(spec: &'a Map<String, Value>, owner: &'a str, records: &'a [Record]) -> Self {
        let mut node = spec.get("node_id").and_then(Value::as_str).unwrap_or(owner);
        let binding = spec.get("binding").unwrap_or(&Value::Null);
        let records = match binding["kind"].as_str() {
            Some("local") => &[],
            Some("hub") => {
                let through = binding["through"].as_u64().unwrap_or_default();
                &records[records
                    .partition_point(|record| record.move_seq.unwrap_or_default() <= through)..]
            }
            Some("move") => records
                .iter()
                .position(|record| binding["operation"] == record.id)
                .map_or(&[][..], |i| &records[i + 1..]),
            _ => records,
        };
        let source = spec
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let sid = spec.get("sid").and_then(Value::as_str).unwrap_or_default();
        let mut identity: Cow<'_, str> = Cow::Owned(format!("{source}:{sid}"));
        for record in records {
            if record.source_node == node
                && let Some(next) = record.mappings.get(identity.as_ref())
            {
                node = &record.target_node;
                identity = Cow::Borrowed(next);
            }
        }
        Self { node, identity }
    }

    fn changed(&self, spec: &Map<String, Value>, local: Option<&str>) -> bool {
        self.identity.split_once(':').is_some_and(|(source, sid)| {
            spec.get("source").and_then(Value::as_str) != Some(source)
                || spec.get("sid").and_then(Value::as_str) != Some(sid)
                || if local == Some(self.node) {
                    spec.contains_key("node_id")
                } else {
                    spec.get("node_id").and_then(Value::as_str) != Some(self.node)
                }
        })
    }

    fn update(spec: &mut Map<String, Value>, owner: &str, local: Option<&str>, records: &[Record]) {
        let location = ParentLocation::resolve(spec, owner, records);
        if !location.changed(spec, local) {
            return;
        }
        if let Some((source, sid)) = location.identity.split_once(':') {
            let (node, source, sid) = (location.node.to_owned(), source.to_owned(), sid.to_owned());
            spec.insert("source".into(), source.into());
            spec.insert("sid".into(), sid.into());
            if local == Some(node.as_str()) {
                spec.remove("node_id");
            } else {
                spec.insert("node_id".into(), node.into());
            }
        }
    }
}

const NESTED_PAYLOADS: [&str; 4] = ["sessions", "results", "session", "meta"];

pub(super) fn relocate_payload(value: &mut Value, node: &str, records: &[Record]) {
    match value {
        Value::Array(rows) => {
            for row in rows {
                relocate_payload(row, node, records);
            }
        }
        Value::Object(row) => {
            let owner = row
                .get("node_id")
                .and_then(Value::as_str)
                .unwrap_or(node)
                .to_owned();
            if let Some(counts) = row.get_mut("child_counts").and_then(Value::as_array_mut) {
                for count in counts.iter_mut().filter_map(Value::as_object_mut) {
                    ParentLocation::update(count, node, None, records);
                }
            }
            if let Some(parent) = row.get_mut("nest_parent").and_then(Value::as_object_mut) {
                ParentLocation::update(parent, &owner, Some(&owner), records);
            }
            for key in NESTED_PAYLOADS {
                if let Some(value) = row.get_mut(key) {
                    relocate_payload(value, &owner, records);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn needs_relocation(value: &Value, node: &str, records: &[Record]) -> bool {
    match value {
        Value::Array(rows) => rows.iter().any(|row| needs_relocation(row, node, records)),
        Value::Object(row) => {
            let owner = row.get("node_id").and_then(Value::as_str).unwrap_or(node);
            row.get("child_counts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .any(|count| ParentLocation::resolve(count, node, records).changed(count, None))
                || row
                    .get("nest_parent")
                    .and_then(Value::as_object)
                    .is_some_and(|parent| {
                        ParentLocation::resolve(parent, owner, records).changed(parent, Some(owner))
                    })
                || NESTED_PAYLOADS.iter().any(|key| {
                    row.get(*key)
                        .is_some_and(|value| needs_relocation(value, owner, records))
                })
        }
        _ => false,
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
    let move_fence = registry
        .nesting_snapshot()
        .1
        .as_deref()
        .and_then(|records| records.last())
        .and_then(|record| record.move_seq)
        .unwrap_or_default();
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
    body.insert("move_fence".into(), move_fence.into());
    if child["node_id"] != parent["node_id"] {
        body.insert(
            "remote_parent".into(),
            json!({"node_id": parent["node_id"], "source": parent["source"], "sid": parent["sid"]}),
        );
        body.remove("parent_uid");
    }
    Ok(())
}
