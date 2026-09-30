//! Undirected connected components over logical and physical history edges.
//! A logical thread may own several physical candidates. Never collapse those
//! files to the row currently visible in the sidebar before walking dependencies.

#[path = "claude_tools.rs"]
pub(super) mod claude_tools;
#[path = "grok_tools.rs"]
pub(super) mod grok_tools;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

pub fn derive(snapshot: &SessionSnapshot, selected: &str) -> Result<Group, TransferError> {
    let index = snapshot.index();
    let entries: BTreeMap<_, _> = index.candidates().map(|e| (e.uid.clone(), e)).collect();
    if !entries.contains_key(selected) {
        return Err(TransferError::new("not_found", "会话不在当前索引中"));
    }
    let mut grok_aliases = Vec::new();
    for e in entries.values().filter(|e| e.source == "grok") {
        if let Some(path) = &e.summary_path {
            if let Ok(raw) = std::fs::read(path) {
                if let Ok(row) = serde_json::from_slice::<Value>(&raw) {
                    if let Some(alias) = row["agent_id"].as_str().filter(|s| !s.is_empty()) {
                        grok_aliases.push((alias.to_owned(), e.summary.sid.clone()));
                    }
                }
            }
        }
    }
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
        if alias != sid {
            if let Some(targets) = identities.get(&("grok", sid.as_str())).cloned() {
                identities
                    .entry(("grok", alias.as_str()))
                    .or_default()
                    .extend(targets);
            }
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
    for row in snapshot.list["sessions"].as_array().into_iter().flatten() {
        let uid = string(row, "uid");
        if !entries.contains_key(uid) {
            continue;
        }
        let parent = &row["spawned_by"];
        if parent.is_object() {
            let source = string(parent, "source");
            let sid = string(parent, "sid");
            if let Some(ids) = identities.get(&(source, sid)) {
                for id in ids {
                    connect(uid, id, "spawned_by");
                }
            } else {
                blockers.push(Blocker {
                    uid: uid.into(),
                    code: "move_group_incomplete".into(),
                    message: format!("缺少 spawned_by 关联的 {source} 会话 {sid}"),
                });
            }
        }
        // nest_parent is presentation, not ownership or native dependency.
    }
    // Calls and outputs can fall in different generations of one thread.
    // Scope call IDs to the owning thread, never to unrelated conversations.
    let mut calls_by_thread = BTreeMap::<String, BTreeMap<String, String>>::new();
    for e in entries.values().filter(|e| e.source == "codex") {
        if let Ok(raw) = std::fs::read(&e.data) {
            let calls = calls_by_thread.entry(e.summary.sid.clone()).or_default();
            for line in raw.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
                if let Ok(row) = serde_json::from_slice::<Value>(line) {
                    super::codex_tools::collect_call(&row, calls);
                }
            }
        }
    }
    for e in entries.values().filter(|e| e.source == "codex") {
        if let Ok(raw) = std::fs::read(&e.data) {
            for line in raw.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
                let Ok(row) = serde_json::from_slice::<Value>(line) else {
                    continue;
                };
                for id in super::codex_tools::references(&row, &calls_by_thread[&e.summary.sid]) {
                    if let Some(targets) = identities.get(&("codex", id.as_str())) {
                        for target in targets {
                            if e.uid != *target {
                                edges.insert(Edge {
                                    from: e.uid.clone(),
                                    to: (*target).into(),
                                    kind: "agent_tool".into(),
                                });
                            }
                        }
                    } else {
                        blockers.push(Blocker {
                            uid: e.uid.clone(),
                            code: "move_group_incomplete".into(),
                            message: format!("工具引用的 Codex 会话 {id} 缺失"),
                        });
                    }
                }
            }
        }
    }
    // Native Claude forks can copy message UUIDs without a session-level fork
    // field. Resolve those shared records as well as explicit physical refs.
    // Grok keeps its fork relationship in summary.json, outside chat history.
    let mut claude_messages = BTreeMap::<String, String>::new();
    for e in entries
        .values()
        .filter(|e| matches!(e.source, "claude" | "grok"))
    {
        let mut references = BTreeSet::<(String, String)>::new();
        if e.source == "claude" {
            let raw = match std::fs::read(&e.data) {
                Ok(raw) => raw,
                Err(error) => {
                    blockers.push(Blocker {
                        uid: e.uid.clone(),
                        code: "move_io".into(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            let tool_rows: Vec<Value> = raw
                .split(|b| *b == b'\n')
                .filter_map(|line| serde_json::from_slice(line).ok())
                .collect();
            let links = claude_tools::links(&tool_rows);
            for id in links.requests {
                // Teammate names and failed lookups are not persisted histories.
                if identities.contains_key(&("claude", id.as_str())) {
                    references.insert((id, "agent_tool".into()));
                }
            }
            for id in links.resumed.into_values() {
                references.insert((id, "agent_tool".into()));
            }
            for line in raw.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
                let row: Value = match serde_json::from_slice(line) {
                    Ok(row) => row,
                    Err(error) => {
                        blockers.push(Blocker {
                            uid: e.uid.clone(),
                            code: "move_format".into(),
                            message: error.to_string(),
                        });
                        continue;
                    }
                };
                for key in [
                    "parentSessionId",
                    "forkedFromSessionId",
                    "continuedInSessionId",
                ] {
                    if let Some(id) = row[key].as_str().filter(|id| !id.is_empty()) {
                        references.insert((
                            id.into(),
                            if row["type"] == "fork-context-ref" {
                                "history_base".into()
                            } else {
                                "fork".into()
                            },
                        ));
                    }
                }
                if !e.is_agent() && row.get("parentUuid").is_some() {
                    if let Some(id) = row["uuid"].as_str().filter(|id| !id.is_empty()) {
                        if let Some(other) = claude_messages.get(id) {
                            if other != &e.uid {
                                edges.insert(Edge {
                                    from: e.uid.clone(),
                                    to: other.clone(),
                                    kind: "fork".into(),
                                });
                            }
                        } else {
                            claude_messages.insert(id.into(), e.uid.clone());
                        }
                    }
                }
            }
        } else if let Some(path) = &e.summary_path {
            match grok_tools::rows(path.parent().unwrap()) {
                Ok(rows) => {
                    for id in grok_tools::references(&rows) {
                        // The same tool also accepts shell task IDs and failed lookups.
                        // Only indexed durable agents establish a history relationship.
                        if identities.contains_key(&("grok", id.as_str())) {
                            references.insert((id, "agent_tool".into()));
                        }
                    }
                }
                Err(error) => blockers.push(Blocker {
                    uid: e.uid.clone(),
                    code: error.code,
                    message: error.message,
                }),
            }
            let row: Value = match std::fs::read(path)
                .map_err(TransferError::from)
                .and_then(|raw| Ok(serde_json::from_slice(&raw)?))
            {
                Ok(row) => row,
                Err(error) => {
                    blockers.push(Blocker {
                        uid: e.uid.clone(),
                        code: error.code,
                        message: error.message,
                    });
                    continue;
                }
            };
            if let Some(id) = row["parent_session_id"]
                .as_str()
                .filter(|id| !id.is_empty())
            {
                references.insert((id.into(), "fork".into()));
            }
            // Grok children have independent directories and need not carry a
            // parent_session_id in their own summary. The owner records them.
            let agents = path.parent().unwrap().join("subagents");
            if agents.is_dir() {
                for entry in std::fs::read_dir(agents)? {
                    let metadata = entry?.path().join("meta.json");
                    if !metadata.is_file() {
                        continue;
                    }
                    match std::fs::read(&metadata)
                        .map_err(TransferError::from)
                        .and_then(|raw| Ok(serde_json::from_slice::<Value>(&raw)?))
                    {
                        Ok(meta) => {
                            for name in ["child_session_id", "parent_session_id"] {
                                if let Some(id) = meta[name].as_str().filter(|id| !id.is_empty()) {
                                    references.insert((id.into(), "subagent".into()));
                                }
                            }
                        }
                        Err(error) => blockers.push(Blocker {
                            uid: e.uid.clone(),
                            code: error.code,
                            message: error.message,
                        }),
                    }
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
