//! Resolve saved links through completed transfer records, never by guessing IDs.
use super::{Client, Registry, namespace, transfer::Transfers};
use crate::session_links::{Link, Record, Request};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

type Key = (String, String);
struct Resolver<'a> {
    registry: &'a Registry,
    client: &'a Client,
    transfers: &'a Transfers,
    records: Vec<Record>,
    cache: BTreeMap<Key, Value>,
    unavailable: BTreeSet<String>,
    hydrated: BTreeSet<String>,
}
impl Resolver<'_> {
    async fn fetch(&mut self, keys: &[Key]) {
        let mut groups = BTreeMap::<String, BTreeSet<String>>::new();
        for (node, sid) in keys {
            if self.unavailable.contains(node) {
                self.cache
                    .insert((node.clone(), sid.clone()), json!({"status":"unavailable"}));
            }
            if !self.cache.contains_key(&(node.clone(), sid.clone())) {
                groups.entry(node.clone()).or_default().insert(sid.clone());
            }
        }
        let (registry, client) = (self.registry, self.client);
        let replies = futures_util::future::join_all(groups.into_iter().map(|(node,ids)| async move {
            let ids: Vec<_> = ids.into_iter().collect();
            let response = match registry.get(&node) {
                Some(peer) => registry.request(client,&peer,"/api/sessions/resolve","POST",Some(&json!({"links":ids.iter().map(|sid|json!({"sid":sid})).collect::<Vec<_>>()})),Duration::from_secs(5)).await.ok(),
                None => None,
            };
            (node,ids,response)
        })).await;
        for (node, ids, response) in replies {
            let data = response
                .filter(|(status, _)| *status == 200)
                .map(|(_, data)| data);
            if data.is_none() {
                self.unavailable.insert(node.clone());
            }
            for (i, sid) in ids.iter().enumerate() {
                let mut row = data
                    .as_ref()
                    .and_then(|d| d["results"].get(i))
                    .cloned()
                    .unwrap_or_else(|| json!({"status":"unavailable"}));
                if row["status"] == "found" {
                    if let Some(uid) = row["session"]["uid"]
                        .as_str()
                        .and_then(|s| namespace::qualify(&node, s, true).ok())
                    {
                        row["session"]["uid"] = uid.into();
                    }
                    row["session"]["node"] = node.clone().into();
                    row["session"]["node_name"] = self
                        .registry
                        .find(&node)
                        .map(|n| n.name)
                        .unwrap_or_else(|| node.clone())
                        .into();
                }
                self.cache.insert((node.clone(), sid.clone()), row);
            }
            if let Some(data) = data {
                for mut record in serde_json::from_value::<Vec<Record>>(data["transfers"].clone())
                    .unwrap_or_default()
                {
                    record.source_node = node.clone();
                    record.target_node = node.clone();
                    if !self
                        .records
                        .iter()
                        .any(|r| r.id == record.id && r.source_node == node)
                    {
                        self.records.push(record);
                    }
                }
            }
        }
    }
    async fn resolve(&mut self, roots: Vec<Key>) -> Value {
        if roots.is_empty() {
            return json!({"status":"unavailable","alternatives":[],"reason":"无法确认原会话所在机器"});
        }
        let mut frontier: Vec<_> = roots
            .into_iter()
            .map(|k| (k, Vec::<String>::new(), false))
            .collect();
        let mut visited = BTreeSet::new();
        let mut uncertain = false;
        let mut alternatives = Vec::new();
        while !frontier.is_empty() {
            frontier.retain(|(key, _, _)| visited.insert(key.clone()));
            self.fetch(
                &frontier
                    .iter()
                    .map(|(k, _, _)| k.clone())
                    .collect::<Vec<_>>(),
            )
            .await;
            let mut next = Vec::new();
            let mut found = Vec::new();
            for (key, path, copied) in frontier {
                let row = self
                    .cache
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| json!({"status":"unavailable"}));
                match row["status"].as_str() {
                    Some("found") => {
                        let mut session = row["session"].clone();
                        session["via"] = json!(path);
                        session["copied"] = copied.into();
                        found.push(session);
                        continue;
                    }
                    Some("ambiguous") if path.is_empty() => {
                        return json!({"status":"ambiguous","alternatives":[]});
                    }
                    Some("missing") => {}
                    _ => uncertain = true,
                }
                // Old Hub receipts predate identity maps. Backfill only records
                // relevant to this link, under their existing journal gate.
                let legacy: Vec<_> = self
                    .records
                    .iter()
                    .filter(|r| {
                        r.legacy
                            && r.phase == "complete"
                            && r.source_node == key.0
                            && (r
                                .legacy_members
                                .contains(key.1.split("/agent:").next().unwrap_or(&key.1))
                                || r.next(&key.1).is_some())
                    })
                    .map(|r| r.id.clone())
                    .collect();
                for id in legacy {
                    if !self.hydrated.insert(id.clone()) {
                        uncertain |= self
                            .records
                            .iter()
                            .any(|r| r.id == id && r.next(&key.1).is_none());
                        continue;
                    }
                    if let Some(record) = self
                        .transfers
                        .hydrate_links(self.registry, self.client, &id)
                        .await
                    {
                        self.records.retain(|r| r.id != id);
                        self.records.push(record);
                    } else if self
                        .records
                        .iter()
                        .any(|r| r.id == id && r.next(&key.1).is_none())
                    {
                        uncertain = true;
                    }
                }
                let mut edges: Vec<_> = self
                    .records
                    .iter()
                    .filter(|r| r.source_node == key.0)
                    .filter_map(|r| r.next(&key.1).map(|sid| (r, sid)))
                    .collect();
                edges.sort_by(|(a, _), (b, _)| (a.created_ms, &a.id).cmp(&(b.created_ms, &b.id)));
                for (edge, sid) in edges {
                    let mut route = path.clone();
                    route.push(edge.id.clone());
                    next.push((
                        (edge.target_node.clone(), sid),
                        route,
                        copied || edge.mode == "clone",
                        (edge.created_ms, edge.id.clone()),
                    ));
                }
            }
            if !found.is_empty() {
                // Unscoped original IDs must still name one physical conversation.
                if found.len() > 1 && found[0]["via"].as_array().is_some_and(Vec::is_empty) {
                    return json!({"status":"ambiguous","alternatives":found});
                }
                if !uncertain {
                    return json!({"status":"found","session":found.remove(0)});
                }
                alternatives.extend(found);
            }
            next.sort_by(|a, b| a.3.cmp(&b.3));
            frontier = next
                .into_iter()
                .map(|(key, path, copied, _)| (key, path, copied))
                .collect();
        }
        json!({"status":if uncertain{"unavailable"}else{"missing"},"alternatives":alternatives})
    }
}
fn roots(registry: &Registry, link: &Link) -> Vec<Key> {
    if let Ok((node, sid)) = namespace::split(&link.sid, true) {
        return vec![(node, sid)];
    }
    if !link.node.is_empty() {
        return vec![(link.node.clone(), link.sid.clone())];
    }
    let nodes: Vec<_> = registry
        .all()
        .into_iter()
        .filter(|n| link.host.is_empty() || n.name.eq_ignore_ascii_case(&link.host))
        .collect();
    if !link.host.is_empty() && nodes.len() != 1 {
        return Vec::new();
    }
    nodes
        .into_iter()
        .map(|n| (n.id, link.sid.clone()))
        .collect()
}
pub async fn resolve(
    registry: &Registry,
    client: &Client,
    transfers: &Transfers,
    body: Request,
) -> Value {
    let roots: Vec<_> = body.links.iter().map(|l| roots(registry, l)).collect();
    let mut resolver = Resolver {
        registry,
        client,
        transfers,
        records: transfers.link_records(),
        cache: BTreeMap::new(),
        unavailable: BTreeSet::new(),
        hydrated: BTreeSet::new(),
    };
    resolver
        .fetch(&roots.iter().flatten().cloned().collect::<Vec<_>>())
        .await;
    let mut results = Vec::new();
    for (link, roots) in body.links.into_iter().zip(roots) {
        let mut result = resolver.resolve(roots).await;
        result["requested"] = json!(link);
        results.push(result);
    }
    json!({"results":results})
}
