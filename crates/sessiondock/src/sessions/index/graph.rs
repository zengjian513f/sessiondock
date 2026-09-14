//! Ownership and fork graph over row summaries.
//!
//! Summaries are the only input: Claude sidecars belong to the main
//! transcript named by their path, Codex subagent rollouts to the thread
//! their header declares. Two Codex fork relations are kept apart:
//!
//! - the *physical* chain (`history_base`): the fixed prefix an open reads
//!   and the catalog validates; its cut must end on a line boundary of an
//!   indexed main file (KEEP: 501/503/413 mark the row unsupported);
//! - the *logical* lineage (`forked_from_id`): the row is decorated
//!   with (`root_sid`, `fork_depth`, `created`, `title`, `size`); a
//!   missing parent just ends it, nothing here is an error.
//!
//! An agent whose owner is not indexed is no row at all;
//! ambiguous ids, ownership cycles and depth overflow stay visible
//! `supported:false` rows with the same warning text as today. Healthy
//! agents are reachable only as `agent_items` of their owner, each with
//! `active` (Codex from its own turn state, Claude from the turn
//! state and the owner's stop notices in `agent_stops`).
//!
//! A Claude main row whose tail names a `continued-in` session carries
//! `continued_in` = the uid of the main Claude row with that sid (`by_sid`
//! over the rows in path order,
//! the last one wins, a self-reference is dropped); absent when unindexed.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::CandidateRef;
use super::agent_stops::{Stops, claude_active};
use super::summary::claude::owner_path;
use crate::sessions::{NativeScope, SessionError, path_text};

/// What the index learned about a parent's fixed-prefix cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutCheck {
    /// `cut == 0` or the byte before `cut` is a LF.
    Boundary,
    NotBoundary,
    /// The parent file is shorter than `cut`.
    BeyondEnd,
    /// The parent could not be read at that offset (changed or vanished).
    Unreadable,
}

/// Native-scope provenance of one physical entry for the runtime catalog.
#[derive(Clone, Debug)]
pub struct CatalogSeed {
    pub uid: String,
    pub source: &'static str,
    pub declared_ids: Vec<String>,
    pub scope: Result<NativeScope, SessionError>,
    pub subagent: bool,
}

pub struct Built {
    /// One row per top-level entry (owned agents are folded into
    /// `agent_items`, orphan agents are absent), unsorted.
    pub rows: Vec<Value>,
    /// Agent uid → root owner uid, for healthy agents only.
    pub owners: BTreeMap<String, String>,
    /// Agent uid → why it has no owner (409 ambiguous id, 501 broken or
    /// missing relation, 413 depth); the code an open of that uid answers
    /// with. Orphans (owner not indexed) are only here, never in `rows`.
    pub agent_errors: BTreeMap<String, SessionError>,
    pub catalog: Vec<CatalogSeed>,
}

fn unsupported(message: impl Into<String>) -> SessionError {
    SessionError::new(501, message)
}

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or("")
}

pub fn mark_unsupported(row: &mut Value, message: &str) {
    row["supported"] = json!(false);
    if !row["migration_warnings"].is_array() {
        row["migration_warnings"] = json!([]);
    }
    let warnings = row["migration_warnings"]
        .as_array_mut()
        .expect("array above");
    if !warnings.iter().any(|warning| warning == message) {
        warnings.push(json!(message));
    }
    if let Some(object) = row.as_object_mut() {
        object.remove("cursor");
    }
}

/// Why an agent has no owner.
#[derive(Clone)]
enum Unowned {
    /// The owner is not indexed at all: such an agent is never listed,
    /// so it is no row, only the typed error an open by uid answers with.
    Missing(SessionError),
    /// Ambiguous owner, cycle or depth: a visible `supported:false` row.
    Broken(SessionError),
}

impl Unowned {
    fn into_error(self) -> SessionError {
        match self {
            Self::Missing(error) | Self::Broken(error) => error,
        }
    }
}

struct Agent {
    id: String,
    title: String,
    kind: String,
    owner: Result<String, Unowned>,
}

/// Non-subagent Codex entries by native sid.
pub(super) fn mains_by_sid(
    entries: &BTreeMap<String, CandidateRef>,
) -> BTreeMap<&str, Vec<&CandidateRef>> {
    let mut mains: BTreeMap<&str, Vec<&CandidateRef>> = BTreeMap::new();
    for entry in entries
        .values()
        .filter(|entry| entry.source == "codex" && entry.summary.agent.is_none())
    {
        mains
            .entry(entry.summary.sid.as_str())
            .or_default()
            .push(entry);
    }
    mains
}

/// Fork chain, nearest parent first:
/// `forked_from_id` resolved among the main Codex rows by sid (exactly one
/// match), ending at a missing or ambiguous parent or a sid already seen.
pub(super) fn lineage<'a>(
    entry: &'a CandidateRef,
    mains: &BTreeMap<&str, Vec<&'a CandidateRef>>,
) -> Vec<&'a CandidateRef> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::from([entry.summary.sid.as_str()]);
    let mut current = entry;
    loop {
        let parent_sid = current
            .summary
            .codex
            .as_ref()
            .map(|codex| codex.forked_from_id.as_str())
            .unwrap_or("");
        if parent_sid.is_empty() {
            return chain;
        }
        let Some([parent]) = mains.get(parent_sid).map(Vec::as_slice) else {
            return chain;
        };
        if !seen.insert(parent.summary.sid.as_str()) {
            return chain;
        }
        chain.push(*parent);
        current = parent;
    }
}

/// The logical size: Σ over the chain of
/// `min(cur.history_base.end_byte_offset or 0, parent.size)` with `cur`
/// starting at the row and moving to each parent in turn (a null
/// `history_base` adds 0 and the walk goes on).
fn inherited_size(entry: &CandidateRef, chain: &[&CandidateRef]) -> u64 {
    let mut total = 0;
    let mut current = entry;
    for parent in chain {
        let limit = current
            .summary
            .codex
            .as_ref()
            .and_then(|codex| codex.history_base["end_byte_offset"].as_u64())
            .unwrap_or(0);
        total += limit.min(parent.summary.size);
        current = parent;
    }
    total
}

struct Graph<'a> {
    entries: &'a BTreeMap<String, CandidateRef>,
    sids: BTreeMap<(&'a str, &'a str), Vec<String>>,
    mains: BTreeMap<&'a str, Vec<&'a CandidateRef>>,
    /// Claude `by_sid`: main transcripts by sid, the last in path
    /// order winning (`continued_in` targets).
    claude_by_sid: BTreeMap<&'a str, &'a str>,
    agents: BTreeMap<String, Agent>,
    /// (uid, cut) → validation outcome, resolved once per build.
    cuts: BTreeMap<(String, u64), CutCheck>,
    /// Owner uid → Claude subagent stop notices (only owners the index
    /// scanned: those with a sidecar whose turn is open).
    stops: &'a BTreeMap<String, Stops>,
}

impl<'a> Graph<'a> {
    fn new(
        entries: &'a BTreeMap<String, CandidateRef>,
        check_cut: &mut dyn FnMut(&CandidateRef, u64) -> CutCheck,
        stops: &'a BTreeMap<String, Stops>,
    ) -> Self {
        let mut graph = Self {
            entries,
            sids: BTreeMap::new(),
            mains: mains_by_sid(entries),
            claude_by_sid: BTreeMap::new(),
            agents: BTreeMap::new(),
            cuts: BTreeMap::new(),
            stops,
        };
        for (uid, entry) in entries {
            let sid = entry.summary.sid.as_str();
            if !sid.is_empty() {
                graph
                    .sids
                    .entry((entry.source, sid))
                    .or_default()
                    .push(uid.clone());
            }
        }
        let mut claude_mains: Vec<&CandidateRef> = entries
            .values()
            .filter(|entry| entry.source == "claude" && entry.summary.agent.is_none())
            .collect();
        // Owners are sorted as path strings (`sorted(owners)`).
        claude_mains.sort_by(|left, right| left.path.as_os_str().cmp(right.path.as_os_str()));
        for entry in claude_mains {
            if !entry.summary.sid.is_empty() {
                graph
                    .claude_by_sid
                    .insert(entry.summary.sid.as_str(), entry.uid.as_str());
            }
        }
        for (uid, entry) in entries {
            let Some(agent) = &entry.summary.agent else {
                continue;
            };
            let owner = if entry.source == "claude" {
                let owner_path = owner_path(&entry.path);
                let matches: Vec<_> = entries
                    .iter()
                    .filter(|(_, candidate)| {
                        candidate.source == "claude"
                            && owner_path
                                .as_ref()
                                .is_some_and(|owner| candidate.path == *owner)
                    })
                    .map(|(uid, _)| uid.clone())
                    .collect();
                match matches.as_slice() {
                    [owner] => Ok(owner.clone()),
                    [] => Err(Unowned::Missing(unsupported(
                        "Claude 子代理的主会话不在已配置索引中",
                    ))),
                    _ => Err(Unowned::Broken(unsupported(
                        "Claude 子代理的主会话路径存在歧义",
                    ))),
                }
            } else {
                graph.agent_owner(entry)
            };
            let title = if agent.title.is_empty() {
                format!("子代理 {}", agent.id.chars().take(8).collect::<String>())
            } else {
                agent.title.clone()
            };
            let kind = if agent.kind.is_empty() {
                "subagent".to_owned()
            } else {
                agent.kind.clone()
            };
            graph.agents.insert(
                uid.clone(),
                Agent {
                    id: agent.id.clone(),
                    title,
                    kind,
                    owner,
                },
            );
        }
        // Resolve every declared fork cut once, in uid order.
        for entry in entries.values() {
            if let Ok(Some((sid, cut))) = history_link(entry)
                && let Ok(parent) = graph.history_parent(sid)
                && !graph.cuts.contains_key(&(parent.clone(), cut))
            {
                let outcome = check_cut(&entries[&parent], cut);
                graph.cuts.insert((parent, cut), outcome);
            }
        }
        graph
    }

    /// A Codex subagent's owner by its declared parent thread id.
    /// Looked up by sid; dropped when nothing matches.
    fn agent_owner(&self, entry: &CandidateRef) -> Result<String, Unowned> {
        let parent = entry
            .summary
            .codex
            .as_ref()
            .map(|codex| codex.parent_thread_id.as_str())
            .unwrap_or("");
        match self.sid("codex", parent) {
            Ok(uid) => Ok(uid),
            Err(error) if error.status == 409 => Err(Unowned::Broken(error)),
            Err(error) => Err(Unowned::Missing(error)),
        }
    }

    fn sid(&self, source: &str, sid: &str) -> Result<String, SessionError> {
        if sid.is_empty() {
            return Err(unsupported("历史依赖或子代理缺少父线程 ID"));
        }
        match self.sids.get(&(source, sid)).map(Vec::as_slice) {
            Some([uid]) => Ok(uid.clone()),
            Some(_) => Err(SessionError::new(409, "父线程 ID 在已配置索引中存在歧义")),
            None => Err(unsupported(
                "父线程不在已配置索引中，请确认显式数据源包含其原生文件",
            )),
        }
    }

    fn history_parent(&self, sid: &str) -> Result<String, SessionError> {
        let uid = self.sid("codex", sid)?;
        if self.agents.contains_key(&uid) {
            return Err(unsupported("分叉历史不能把子代理文件当作主线程父历史"));
        }
        Ok(uid)
    }

    fn ambiguous_agent(&self, uid: &str) -> bool {
        let entry = &self.entries[uid];
        entry.source == "codex"
            && self.agents.contains_key(uid)
            && self
                .sids
                .get(&("codex", entry.summary.sid.as_str()))
                .is_some_and(|ids| ids.len() != 1)
    }

    fn ownership(&self, uid: &str) -> Result<(String, Vec<String>), Unowned> {
        let mut current = uid.to_owned();
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(current.clone()) {
                return Err(Unowned::Broken(unsupported("子代理归属关系存在循环")));
            }
            chain.push(current.clone());
            let Some(agent) = self.agents.get(&current) else {
                return Ok((current, chain));
            };
            current = match &agent.owner {
                Ok(owner) => owner.clone(),
                Err(lost) => return Err(lost.clone()),
            };
        }
    }

    fn check_cut(&self, parent: &str, cut: u64) -> Result<(), SessionError> {
        match self.cuts.get(&(parent.to_owned(), cut)) {
            Some(CutCheck::Boundary) => Ok(()),
            Some(CutCheck::BeyondEnd) => Err(unsupported("父历史固定前缀超出完整原生数据范围")),
            Some(CutCheck::NotBoundary) => Err(unsupported("父历史固定前缀不在完整 JSONL 行边界")),
            Some(CutCheck::Unreadable) | None => Err(SessionError::new(
                503,
                "父历史固定前缀在读取期间变化，请重试",
            )),
        }
    }

    /// Metadata-only validation of the declared physical chain, nearest
    /// parent first: what an open reads.
    fn chain(&self, uid: &str) -> Result<Vec<(String, u64)>, SessionError> {
        let mut current = uid.to_owned();
        let mut chain = Vec::new();
        let mut seen = BTreeSet::from([current.clone()]);
        loop {
            let Some((sid, cut)) = history_link(&self.entries[&current])? else {
                return Ok(chain);
            };
            let parent = self.history_parent(sid)?;
            if !seen.insert(parent.clone()) {
                return Err(unsupported("分叉历史依赖存在循环"));
            }
            self.check_cut(&parent, cut)?;
            chain.push((parent.clone(), cut));
            current = parent;
        }
    }

    fn row(&self, uid: &str) -> Value {
        let entry = &self.entries[uid];
        let mut row = base_row(entry);
        if let Some(reason) = &entry.summary.unsupported {
            mark_unsupported(&mut row, reason);
        }
        if let Err(error) = self.chain(uid) {
            mark_unsupported(&mut row, &error.message);
        }
        // Main rows only; `title` here is the
        // root's base title, the name index overrides it afterwards.
        if entry.summary.agent.is_none() {
            let chain = lineage(entry, &self.mains);
            if let Some(root) = chain.last() {
                row["root_sid"] = json!(root.summary.sid);
                row["fork_depth"] = json!(chain.len());
                row["created"] = json!(root.summary.created);
                row["title"] = json!(root.summary.title);
                row["size"] = json!(entry.summary.size + inherited_size(entry, &chain));
            }
            if entry.source == "claude"
                && let Some(sid) = &entry.summary.continued_in_sid
                && let Some(target) = self.claude_by_sid.get(sid.as_str())
                && *target != uid
            {
                row["continued_in"] = json!(target);
            }
        }
        row
    }

    /// A Codex subagent's own turn state; a
    /// Claude sidecar's turn state against its owner's stop notices.
    fn agent_active(&self, owner: &str, uid: &str) -> bool {
        let entry = &self.entries[uid];
        let open_turn = entry
            .summary
            .agent
            .as_ref()
            .is_some_and(|agent| agent.open_turn);
        if entry.source != "claude" {
            return open_turn;
        }
        let stopped_at = self
            .stops
            .get(owner)
            .and_then(|stops| stops.get(&self.agents[uid].id))
            .map(String::as_str);
        claude_active(open_turn, &entry.summary.updated, stopped_at)
    }

    fn agent_item(&self, owner: &str, uid: &str) -> Value {
        let info = &self.agents[uid];
        let row = self.row(uid);
        let mut item = json!({"id": info.id, "title": info.title, "type": info.kind,
            "active": self.agent_active(owner, uid)});
        for field in [
            "path",
            "cwd",
            "model",
            "created",
            "updated",
            "size",
            "supported",
            "migration_warnings",
            "cursor",
        ] {
            if let Some(value) = row.get(field) {
                item[field] = value.clone();
            }
        }
        item
    }

    fn select_main(&self, uid: &str) -> Result<(), SessionError> {
        if self.agents.contains_key(uid) {
            if self.ambiguous_agent(uid) {
                return Err(SessionError::new(409, "Codex 子代理 ID 在索引中存在歧义"));
            }
            self.ownership(uid).map_err(Unowned::into_error)?;
            return Err(SessionError::new(404, "子代理必须通过所属主会话访问"));
        }
        Ok(())
    }

    fn native_scope(&self, uid: &str) -> Result<NativeScope, SessionError> {
        let entry = &self.entries[uid];
        if !matches!(entry.source, "claude" | "codex" | "grok") {
            return Err(unsupported("此数据源尚不支持原生操作范围"));
        }
        if let Some(error) = &entry.summary.unsupported {
            return Err(unsupported(error.clone()));
        }
        let id = entry.summary.native_id.clone()?;
        Ok(NativeScope {
            source: entry.source.to_owned(),
            uid: uid.to_owned(),
            session_id: id,
            agent_id: None,
        })
    }
}

/// The public fields today's `parse_candidate` + `public_meta` leave on a
/// row before graph decoration (no `cursor`: it needs the projection).
fn base_row(entry: &CandidateRef) -> Value {
    let summary = &entry.summary;
    let mut row = json!({
        "sid": summary.sid, "title": summary.title, "cwd": summary.cwd,
        "created": summary.created, "updated": summary.updated,
        "model": summary.model, "branch": summary.branch,
    });
    if let Some(codex) = &summary.codex
        && codex.has_meta
    {
        row["forked_from_id"] = json!(codex.forked_from_id);
        row["history_base"] = codex.history_base.clone();
    }
    row["migration_warnings"] = json!(summary.migration_warnings());
    row["uid"] = json!(entry.uid);
    row["source"] = json!(entry.source);
    row["path"] = json!(path_text(&entry.path));
    row["size"] = json!(summary.size);
    if let Some(grok) = &summary.grok {
        row["chat_exists"] = json!(grok.chat_exists);
    }
    row["supported"] = json!(summary.supported());
    row
}

/// `history::history_link` over a summary: the declared fixed-prefix parent
/// (`history_base.thread_id or forked_from_id`,
/// `limit = end_byte_offset`). A null `history_base` inherits nothing, so a
/// fork without one is self-contained, and `forked_from_id` may name a
/// different thread than `history_base.thread_id` (a rewind past the
/// parent's own fork point): the physical file is the one to read.
pub(super) fn history_link(entry: &CandidateRef) -> Result<Option<(&str, u64)>, SessionError> {
    let Some(codex) = &entry.summary.codex else {
        return Ok(None);
    };
    let base = &codex.history_base;
    if base.is_null() {
        return Ok(None);
    }
    if !base.is_object() {
        return Err(unsupported("history_base 必须是对象"));
    }
    let declared = text(base, "thread_id");
    let parent = if declared.is_empty() {
        codex.forked_from_id.as_str()
    } else {
        declared
    };
    if parent.is_empty() {
        return Err(unsupported("history_base 缺少父线程 ID"));
    }
    let cut = base["end_byte_offset"]
        .as_u64()
        .ok_or_else(|| unsupported("history_base 前缀偏移必须是非负整数"))?;
    Ok(Some((parent, cut)))
}

/// Build the rows, agent ownership and catalog seeds for one snapshot.
/// `stops` are the Claude subagent stop notices per owner uid the index
/// scanned for this snapshot (owners with an open-turn sidecar).
pub fn build(
    entries: &BTreeMap<String, CandidateRef>,
    check_cut: &mut dyn FnMut(&CandidateRef, u64) -> CutCheck,
    stops: &BTreeMap<String, Stops>,
) -> Built {
    let graph = Graph::new(entries, check_cut, stops);
    let mut rows: BTreeMap<String, Value> = BTreeMap::new();
    let mut owned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut owners = BTreeMap::new();
    let mut agent_errors = BTreeMap::new();
    for uid in entries.keys() {
        if graph.agents.contains_key(uid) {
            if graph.ambiguous_agent(uid) {
                let error = SessionError::new(409, "Codex 子代理 ID 在索引中存在歧义");
                let mut row = graph.row(uid);
                mark_unsupported(&mut row, &error.message);
                rows.insert(uid.clone(), row);
                agent_errors.insert(uid.clone(), error);
                continue;
            }
            match graph.ownership(uid) {
                Ok((owner, _)) => {
                    owned.entry(owner.clone()).or_default().push(uid.clone());
                    owners.insert(uid.clone(), owner);
                }
                Err(Unowned::Missing(error)) => {
                    agent_errors.insert(uid.clone(), error);
                }
                Err(Unowned::Broken(error)) => {
                    let mut row = graph.row(uid);
                    mark_unsupported(&mut row, &error.message);
                    rows.insert(uid.clone(), row);
                    agent_errors.insert(uid.clone(), error);
                }
            }
            continue;
        }
        rows.insert(uid.clone(), graph.row(uid));
    }
    for (uid, mut agents) in owned {
        agents.sort_by(|left, right| {
            (
                entries[left].summary.created.as_str(),
                &graph.agents[left].id,
            )
                .cmp(&(
                    entries[right].summary.created.as_str(),
                    &graph.agents[right].id,
                ))
        });
        if let Some(row) = rows.get_mut(&uid) {
            row["agent_items"] = Value::Array(
                agents
                    .iter()
                    .map(|agent| graph.agent_item(&uid, agent))
                    .collect(),
            );
            row["agents"] = json!(agents.len());
        }
    }
    let catalog = entries
        .iter()
        .map(|(uid, entry)| CatalogSeed {
            uid: uid.clone(),
            source: entry.source,
            declared_ids: entry.summary.declared_ids.clone(),
            scope: graph.select_main(uid).and_then(|()| {
                graph.chain(uid)?;
                graph.native_scope(uid)
            }),
            subagent: graph.agents.contains_key(uid),
        })
        .collect();
    Built {
        rows: rows.into_values().collect(),
        owners,
        agent_errors,
        catalog,
    }
}
