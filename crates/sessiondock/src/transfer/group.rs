//! Undirected connected components over logical and physical history edges.
//! A logical thread may own several physical candidates. Never collapse those
//! files to the row currently visible in the sidebar before walking dependencies.

#[path = "claude_tools.rs"]
pub(super) mod claude_tools;
#[path = "grok_tools.rs"]
pub(super) mod grok_tools;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use super::TransferError;
use crate::sessions::{CandidateRef, SessionSnapshot};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Member {
    pub uid: String,
    pub source: String,
    pub sid: String,
    pub title: String,
    pub cwd: String,
    pub agent: bool,
    pub path: std::path::PathBuf,
    pub root: std::path::PathBuf,
    pub rollout_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Blocker {
    pub uid: String,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Group {
    pub selected: String,
    pub members: Vec<Member>,
    pub edges: Vec<Edge>,
    pub blockers: Vec<Blocker>,
}

pub fn derive(snapshot: &SessionSnapshot, selected: &str) -> Result<Group, TransferError> {
    derive_cached(snapshot, selected, &super::references::Cache::default())
}

pub(super) fn derive_cached(
    snapshot: &SessionSnapshot,
    selected: &str,
    cache: &super::references::Cache,
) -> Result<Group, TransferError> {
    Inventory::default().group(snapshot, selected, cache)
}

/// Publication verifies the delivered graph, not a new migration of the target
/// machine's entire connected component. Source discovery still uses all rows.
pub(super) fn derive_published(
    snapshot: &SessionSnapshot,
    selected: &str,
    cache: &super::references::Cache,
    uids: &BTreeSet<String>,
) -> Result<Group, TransferError> {
    build(snapshot, cache, Some(uids))?.select(selected)
}

fn build(
    snapshot: &SessionSnapshot,
    cache: &super::references::Cache,
    scope: Option<&BTreeSet<String>>,
) -> Result<Graph, TransferError> {
    let index = snapshot.index();
    let entries: BTreeMap<_, _> = index
        .candidates()
        .filter(|e| scope.is_none_or(|uids| uids.contains(&e.uid)))
        .map(|e| (e.uid.clone(), e))
        .collect();
    if scope.is_none() {
        cache.retain(&entries.values().map(|e| e.data.clone()).collect());
    }
    let links = refresh_links(&entries, cache)?;
    build_links(snapshot, entries, links)
}

fn build_links(
    snapshot: &SessionSnapshot,
    entries: BTreeMap<String, &CandidateRef>,
    links: BTreeMap<String, Arc<super::references::Links>>,
) -> Result<Graph, TransferError> {
    let index = snapshot.index();
    let grok_aliases: Vec<_> = entries
        .values()
        .filter(|e| e.source == "grok")
        .filter_map(|e| {
            links[&e.uid]
                .alias
                .as_ref()
                .map(|alias| (alias.clone(), e.summary.sid.clone()))
        })
        .collect();
    let mut identities: BTreeMap<(&str, &str), Vec<&str>> = BTreeMap::new();
    for e in entries.values() {
        if !e.summary.sid.is_empty() {
            identities
                .entry((e.source, &e.summary.sid))
                .or_default()
                .push(&e.uid);
        }
    }
    for (alias, sid) in &grok_aliases {
        if alias != sid
            && let Some(targets) = identities.get(&("grok", sid.as_str())).cloned()
        {
            identities
                .entry(("grok", alias.as_str()))
                .or_default()
                .extend(targets);
        }
    }
    let mut edges = BTreeSet::new();
    let mut blockers = Vec::new();
    let mut connect = |from: &str, to: &str, kind: &str| {
        if from != to {
            edges.insert(Edge {
                from: from.into(),
                to: to.into(),
                kind: kind.into(),
            });
        }
    };
    // Every version of a thread belongs to the same component, even if a
    // corrupt generation prevents the read model from hiding it in the list.
    for ids in identities.values() {
        for uid in ids.iter().skip(1) {
            connect(ids[0], uid, "same_thread");
        }
    }
    for e in entries.values() {
        let mut relate = |source: &str, sid: &str, kind: &str| {
            if sid.is_empty() {
                return;
            }
            match identities.get(&(source, sid)) {
                Some(ids) => {
                    for uid in ids {
                        connect(&e.uid, uid, kind);
                    }
                }
                None => blockers.push(Blocker {
                    uid: e.uid.clone(),
                    code: "move_group_incomplete".into(),
                    message: format!("缺少 {kind} 关联的 {source} 会话 {sid}"),
                }),
            }
        };
        if let Some(c) = &e.summary.codex {
            relate("codex", &c.forked_from_id, "fork");
            relate("codex", &c.parent_thread_id, "subagent");
        }
        if let Some(sid) = &e.summary.continued_in_sid {
            relate(e.source, sid, "continuation");
        }
        if let Some(owner) = &e.owner {
            connect(&e.uid, owner, "subagent");
        }
        if let Some(error) = &e.owner_error {
            blockers.push(Blocker {
                uid: e.uid.clone(),
                code: "move_group_incomplete".into(),
                message: error.message.clone(),
            });
        }
        if let Some(c) = &e.summary.codex
            && !c.history_base.is_null()
        {
            let physical = c.history_base["thread_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(&c.forked_from_id);
            // Resolve each immediate edge even when a deeper ancestor is
            // broken. Losing the whole chain on that error would omit known
            // members of the component instead of displaying its blocker.
            match index.thread_from("codex", physical, &e.uid, &c.history_base) {
                Ok(parent) => connect(&e.uid, &parent.uid, "history_base"),
                Err(error) => blockers.push(Blocker {
                    uid: e.uid.clone(),
                    code: "unsupported_history".into(),
                    message: error.message,
                }),
            }
        }
        if let Some(reason) = &e.summary.unsupported {
            blockers.push(Blocker {
                uid: e.uid.clone(),
                code: "unsupported_history".into(),
                message: reason.clone(),
            });
        }
        if !matches!(e.source, "claude" | "codex" | "grok") {
            blockers.push(Blocker {
                uid: e.uid.clone(),
                code: "move_group_unsupported".into(),
                message: format!("整组包含尚不支持的来源 {}", e.source),
            });
        }
    }
    // Sidebar attachments connect the whole family across providers. A known
    // parent cannot be silently omitted from a single-source operation.
    for row in snapshot.list["sessions"].as_array().into_iter().flatten() {
        let uid = row["uid"].as_str().unwrap_or("");
        if !entries.contains_key(uid) {
            continue;
        }
        let parent = &row["nest_parent"];
        if !parent.is_object() {
            continue;
        }
        let source = parent["source"].as_str().unwrap_or("");
        let sid = parent["sid"].as_str().unwrap_or("");
        if parent["node_id"]
            .as_str()
            .is_some_and(|node| !node.is_empty())
        {
            blockers.push(Blocker {
                uid: uid.into(),
                code: "move_group_incomplete".into(),
                message: format!("nest_parent 关联的 {source} 会话 {sid} 在另一节点，不能拆分整组"),
            });
        } else if let Some(ids) = identities.get(&(source, sid)) {
            for id in ids {
                connect(uid, id, "nest_parent");
            }
        } else {
            blockers.push(Blocker {
                uid: uid.into(),
                code: "move_group_incomplete".into(),
                message: format!("缺少 nest_parent 关联的 {source} 会话 {sid}"),
            });
        }
    }
    // Merge call names across rollouts, then resolve cached output identities.
    let mut calls_by_thread = BTreeMap::<String, BTreeMap<String, String>>::new();
    for e in entries.values().filter(|e| e.source == "codex") {
        calls_by_thread
            .entry(e.summary.sid.clone())
            .or_default()
            .extend(links[&e.uid].calls.clone());
    }
    let mut claude_messages = BTreeMap::<&str, &str>::new();
    for e in entries.values() {
        let native = &links[&e.uid];
        blockers.extend(native.errors.iter().map(|(code, message)| Blocker {
            uid: e.uid.clone(),
            code: code.clone(),
            message: message.clone(),
        }));
        let mut references = native.required.clone();
        references.extend(
            native
                .optional
                .iter()
                .filter(|id| identities.contains_key(&(e.source, id.as_str())))
                .map(|id| (id.clone(), "agent_tool".into())),
        );
        if e.source == "codex" {
            for (call, ids) in &native.outputs {
                if calls_by_thread[&e.summary.sid]
                    .get(call)
                    .is_some_and(|name| {
                        matches!(name.as_str(), "__code_agent" | "spawn_agent" | "wait")
                    })
                {
                    references.extend(ids.iter().map(|id| (id.clone(), "agent_tool".into())));
                }
            }
        }
        if e.source == "claude" && !e.is_agent() {
            for id in &native.messages {
                if let Some(other) = claude_messages.get(id.as_str()) {
                    if *other != e.uid {
                        edges.insert(Edge {
                            from: e.uid.clone(),
                            to: (*other).into(),
                            kind: "fork".into(),
                        });
                    }
                } else {
                    claude_messages.insert(id, &e.uid);
                }
            }
        }
        for (sid, kind) in references {
            if let Some(targets) = identities.get(&(e.source, sid.as_str())) {
                for target in targets {
                    if e.uid != *target {
                        edges.insert(Edge {
                            from: e.uid.clone(),
                            to: (*target).into(),
                            kind: kind.clone(),
                        });
                    }
                }
            } else {
                blockers.push(Blocker {
                    uid: e.uid.clone(),
                    code: "move_group_incomplete".into(),
                    message: format!("缺少 {kind} 关联的 {} 会话 {sid}", e.source),
                });
            }
        }
    }
    let mut adjacency: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for edge in &edges {
        if !entries.contains_key(&edge.from) || !entries.contains_key(&edge.to) {
            return Err(TransferError::new(
                "move_verify",
                "目标历史引用了迁移清单以外的会话",
            ));
        }
        adjacency
            .entry(edge.from.clone())
            .or_default()
            .push(edge.to.clone());
        adjacency
            .entry(edge.to.clone())
            .or_default()
            .push(edge.from.clone());
    }
    let titles: BTreeMap<_, _> = index
        .sessions()
        .iter()
        .filter_map(|row| Some((row["uid"].as_str()?, row["title"].as_str()?)))
        .collect();
    let members = entries
        .values()
        .map(|e| {
            (
                e.uid.clone(),
                Member {
                    uid: e.uid.clone(),
                    source: e.source.into(),
                    sid: e.summary.sid.clone(),
                    title: titles
                        .get(e.uid.as_str())
                        .copied()
                        .unwrap_or(&e.summary.title)
                        .to_owned(),
                    cwd: e.summary.cwd.clone(),
                    agent: e.is_agent(),
                    path: e.data.clone(),
                    root: e.root.clone(),
                    rollout_id: e.codex_rollout_id().map(str::to_owned),
                },
            )
        })
        .collect();
    Ok(Graph {
        members,
        edges,
        blockers,
        adjacency,
    })
}

struct Graph {
    members: BTreeMap<String, Member>,
    edges: BTreeSet<Edge>,
    blockers: Vec<Blocker>,
    adjacency: BTreeMap<String, Vec<String>>,
}
impl Graph {
    fn select(&self, selected: &str) -> Result<Group, TransferError> {
        if !self.members.contains_key(selected) {
            return Err(TransferError::new("not_found", "会话不在当前索引中"));
        }
        let mut seen = BTreeSet::new();
        let mut pending = vec![selected];
        while let Some(uid) = pending.pop() {
            if seen.insert(uid) {
                pending.extend(
                    self.adjacency
                        .get(uid)
                        .into_iter()
                        .flatten()
                        .map(String::as_str),
                );
            }
        }
        Ok(Group {
            selected: selected.into(),
            members: seen.iter().map(|uid| self.members[*uid].clone()).collect(),
            edges: seen
                .iter()
                .flat_map(|uid| {
                    self.edges
                        .range(
                            Edge {
                                from: (*uid).into(),
                                to: String::new(),
                                kind: String::new(),
                            }..,
                        )
                        .take_while(move |edge| edge.from == *uid)
                        .cloned()
                })
                .collect(),
            blockers: self
                .blockers
                .iter()
                .filter(|b| seen.contains(b.uid.as_str()))
                .cloned()
                .collect(),
        })
    }
}

fn refresh_links(
    entries: &BTreeMap<String, &CandidateRef>,
    cache: &super::references::Cache,
) -> Result<BTreeMap<String, Arc<super::references::Links>>, TransferError> {
    let mut links = BTreeMap::new();
    let mut pending = Vec::new();
    for e in entries.values() {
        if let Some(link) = cache.cached_indexed(e) {
            links.insert(e.uid.clone(), link);
        } else {
            pending.push(*e);
        }
    }
    let progress = super::progress::Task::new("更新关联索引", "份历史", Some(pending.len() as u64));
    for e in pending {
        links.insert(e.uid.clone(), cache.get(e)?);
        progress.add(1);
    }
    cache.save();
    Ok(links)
}

/// One reusable reverse graph. Native catalog discovery provides versions;
/// only changed transcripts are read, and an unchanged graph is queried by UID.
#[derive(Default)]
pub(super) struct Inventory {
    rows: BTreeMap<String, CandidateRef>,
    links: BTreeMap<String, Arc<super::references::Links>>,
    signature: serde_json::Value,
    graph: Option<Graph>,
}
impl Inventory {
    pub fn group(
        &mut self,
        snapshot: &SessionSnapshot,
        selected: &str,
        cache: &super::references::Cache,
    ) -> Result<Group, TransferError> {
        let entries: BTreeMap<_, _> = snapshot
            .index()
            .candidates()
            .map(|e| (e.uid.clone(), e))
            .collect();
        if !entries.contains_key(selected) {
            return Err(TransferError::new("not_found", "会话不在当前索引中"));
        }
        cache.retain(&entries.values().map(|e| e.data.clone()).collect());
        let links = refresh_links(&entries, cache)?;
        let unchanged = self.graph.is_some()
            && self.signature == snapshot.list["sig"]
            && entries.len() == self.rows.len()
            && entries.iter().all(|(uid, e)| {
                self.rows.get(uid).is_some_and(|old| {
                    old.stamp == e.stamp
                        && old.summary_stamp == e.summary_stamp
                        && old.owner == e.owner
                        && old.owner_error.as_ref().map(|e| (e.status, &e.message))
                            == e.owner_error.as_ref().map(|e| (e.status, &e.message))
                        && Arc::ptr_eq(&old.summary, &e.summary)
                        && self
                            .links
                            .get(uid)
                            .is_some_and(|old| Arc::ptr_eq(old, &links[uid]))
                })
            });
        crate::log::info(
            if unchanged {
                "transfer.relationship_index.query"
            } else {
                "transfer.relationship_index.update"
            },
            serde_json::json!({
                "reused": unchanged, "candidates": entries.len(),
                "updated": links.iter().filter(|(uid, link)| self.links.get(*uid).is_none_or(|old| !Arc::ptr_eq(old, link))).count()
            }),
        );
        if !unchanged {
            let graph = build_links(snapshot, entries.clone(), links.clone())?;
            self.rows = entries
                .into_iter()
                .map(|(uid, e)| (uid, e.clone()))
                .collect();
            self.links = links;
            self.signature = snapshot.list["sig"].clone();
            self.graph = Some(graph);
        }
        self.graph.as_ref().unwrap().select(selected)
    }
}
