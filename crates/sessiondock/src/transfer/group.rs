//! Undirected connected components over logical and physical history edges.
//! A logical thread may own several physical candidates. Never collapse those
//! files to the row currently visible in the sidebar before walking dependencies.

#[path = "claude_tools.rs"]
pub(super) mod claude_tools;
#[path = "grok_tools.rs"]
pub(super) mod grok_tools;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::TransferError;
use crate::sessions::SessionSnapshot;

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
    let index = snapshot.index();
    let entries: BTreeMap<_, _> = index.candidates().map(|e| (e.uid.clone(), e)).collect();
    if !entries.contains_key(selected) {
        return Err(TransferError::new("not_found", "会话不在当前索引中"));
    }
    cache.retain(&entries.values().map(|e| e.data.clone()).collect());
    let links: BTreeMap<_, _> = entries
        .values()
        .map(|e| Ok((e.uid.clone(), cache.get(e)?)))
        .collect::<Result<_, TransferError>>()?;
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
    // Include existing local sidebar attachments in the selected group. A
    // missing or remote display parent is not a native dependency or blocker.
    for row in snapshot.list["sessions"].as_array().into_iter().flatten() {
        let Some(uid) = row["uid"].as_str().filter(|uid| entries.contains_key(*uid)) else {
            continue;
        };
        let parent = &row["nest_parent"];
        if parent["node_id"].is_null()
            && let (Some(source), Some(sid)) = (parent["source"].as_str(), parent["sid"].as_str())
            && let Some(ids) = identities.get(&(source, sid))
        {
            for id in ids {
                connect(uid, id, "nest_parent");
            }
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
    let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in &edges {
        adjacency.entry(&edge.from).or_default().push(&edge.to);
        adjacency.entry(&edge.to).or_default().push(&edge.from);
    }
    let mut seen = BTreeSet::new();
    let mut pending = vec![selected];
    while let Some(uid) = pending.pop() {
        if seen.insert(uid) {
            pending.extend(adjacency.get(uid).into_iter().flatten().copied());
        }
    }
    let members = seen
        .iter()
        .map(|uid| {
            let e = entries[*uid];
            Member {
                uid: e.uid.clone(),
                source: e.source.into(),
                sid: e.summary.sid.clone(),
                title: e.summary.title.clone(),
                cwd: e.summary.cwd.clone(),
                agent: e.is_agent(),
                path: e.data.clone(),
                root: e.root.clone(),
                rollout_id: e.codex_rollout_id().map(str::to_owned),
            }
        })
        .collect();
    blockers.retain(|b| seen.contains(b.uid.as_str()));
    let edges = edges
        .iter()
        .filter(|e| seen.contains(e.from.as_str()))
        .cloned()
        .collect();
    Ok(Group {
        selected: selected.into(),
        members,
        edges,
        blockers,
    })
}
