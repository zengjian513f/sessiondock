//! View identity helpers shared by `views` (`native_identity`, `history_link`,
//! `inherited_identity`), plus — test-only — the superseded inventory-graph
//! resolver (`Graph`, `rows`, `resolve`) that the on-demand `views` builder
//! is still compared against. Production ownership/fork rules live in
//! `index/graph.rs` over summaries; nothing here is reachable from the
//! facade outside tests.
//!
//! No paths from requests or `history_base` are opened here. Prefixes are
//! reparsed independently: parsing a parent's full tail and filtering offsets
//! would incorrectly retain later rollback/interrupt edits to earlier messages.

use serde_json::{Value, json};

#[cfg(test)]
use super::{Candidate, Event, NativeScope, View, providers};
use super::{Parsed, SessionError, hash};
#[cfg(test)]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
type Inventory = BTreeMap<String, Arc<Parsed>>;
/// Must match the ordinary physical cursor produced by the inventory parser.
/// In particular Codex agent parsing does not need an HTTP `agent` argument.
pub(super) fn native_identity(parsed: &Parsed, agent: &str) -> String {
    let agent = if parsed.meta["_is_subagent"] == true {
        text(&parsed.meta, "sid")
    } else {
        agent
    };
    hash(
        &serde_json::to_vec(&json!([
            "native-view-v1",
            parsed.candidate.source,
            text(&parsed.meta, "uid"),
            agent
        ]))
        .expect("primitive JSON serialization"),
    )
}

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or("")
}

fn unsupported(message: impl Into<String>) -> SessionError {
    SessionError::new(501, message)
}

#[cfg(test)]
fn public_meta(meta: &Value) -> Value {
    let mut meta = meta.clone();
    if let Some(object) = meta.as_object_mut() {
        object.retain(|key, _| !key.starts_with('_'));
    }
    meta
}

#[cfg(test)]
fn mark_unsupported(meta: &mut Value, message: &str) {
    meta["supported"] = json!(false);
    if !meta["migration_warnings"].is_array() {
        meta["migration_warnings"] = json!([]);
    }
    let warnings = meta["migration_warnings"]
        .as_array_mut()
        .expect("array above");
    if !warnings.iter().any(|warning| warning == message) {
        warnings.push(json!(message));
    }
    meta.as_object_mut()
        .expect("provider metadata object")
        .remove("cursor");
}

#[cfg(test)]
struct Agent {
    id: String,
    title: String,
    kind: String,
    owner: Result<String, String>,
}

#[cfg(test)]
struct Graph<'a> {
    inventory: &'a Inventory,
    sids: BTreeMap<(&'a str, &'a str), Vec<String>>,
    agents: BTreeMap<String, Agent>,
}

#[cfg(test)]
struct Selection {
    owner: String,
    selected: String,
    #[cfg_attr(not(test), allow(dead_code))]
    ancestry: Vec<String>,
}

#[cfg(test)]
impl<'a> Graph<'a> {
    fn native_scope(&self, selection: &Selection) -> Result<NativeScope, SessionError> {
        let owner = &self.inventory[&selection.owner];
        let selected = &self.inventory[&selection.selected];
        let source = owner.candidate.source;
        if source != selected.candidate.source || !matches!(source, "claude" | "codex" | "grok") {
            return Err(unsupported("此数据源尚不支持原生操作范围"));
        }
        for parsed in [owner, selected] {
            if let Some(error) = parsed.raw_error.as_ref().or(parsed.unsupported.as_ref()) {
                return Err(unsupported(error.clone()));
            }
        }
        let owner_id = owner.native_id.as_ref().map_err(Clone::clone)?;
        let selected_id = selected.native_id.as_ref().map_err(Clone::clone)?;
        if source == "claude" && owner_id != selected_id {
            return Err(SessionError::new(
                409,
                "Claude 子代理声明的会话 ID 与主会话不一致",
            ));
        }
        let agent_id = if selection.selected == selection.owner {
            None
        } else {
            Some(self.agents[&selection.selected].id.clone())
        };
        Ok(NativeScope {
            source: source.to_owned(),
            uid: selection.owner.clone(),
            session_id: if source == "claude" {
                owner_id
            } else {
                selected_id
            }
            .clone(),
            agent_id,
        })
    }

    fn new(inventory: &'a Inventory) -> Self {
        let mut graph = Self {
            inventory,
            sids: BTreeMap::new(),
            agents: BTreeMap::new(),
        };
        for (uid, parsed) in inventory {
            let sid = text(&parsed.meta, "sid");
            if !sid.is_empty() {
                graph
                    .sids
                    .entry((parsed.candidate.source, sid))
                    .or_default()
                    .push(uid.clone());
            }
        }
        for (uid, parsed) in inventory {
            let claude = claude_agent(parsed);
            if parsed.candidate.source != "codex" && claude.is_none() {
                continue;
            }
            if parsed.candidate.source == "codex" && parsed.meta["_is_subagent"] != true {
                continue;
            }
            let (id, owner) = if let Some((id, owner_path)) = claude {
                let matches: Vec<_> = inventory
                    .iter()
                    .filter(|(_, candidate)| {
                        candidate.candidate.source == "claude"
                            && candidate.candidate.path == owner_path
                    })
                    .map(|(uid, _)| uid.clone())
                    .collect();
                let owner = match matches.as_slice() {
                    [owner] => Ok(owner.clone()),
                    [] => Err("Claude 子代理的主会话不在已配置索引中".to_owned()),
                    _ => Err("Claude 子代理的主会话路径存在歧义".to_owned()),
                };
                (id, owner)
            } else {
                let sid = text(&parsed.meta, "sid").to_owned();
                let owner_sid = text(&parsed.meta, "_parent_thread_id");
                let owner = graph.sid("codex", owner_sid).map_err(|error| error.message);
                (sid, owner)
            };
            let title = text(&parsed.meta, "_agent_title");
            let title = if title.is_empty() {
                format!("子代理 {}", id.chars().take(8).collect::<String>())
            } else {
                title.to_owned()
            };
            let kind = text(&parsed.meta, "_agent_type");
            graph.agents.insert(
                uid.clone(),
                Agent {
                    id,
                    title,
                    kind: if kind.is_empty() {
                        "subagent".to_owned()
                    } else {
                        kind.to_owned()
                    },
                    owner,
                },
            );
        }
        graph
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
        let parsed = &self.inventory[uid];
        parsed.candidate.source == "codex"
            && self.agents.contains_key(uid)
            && self
                .sids
                .get(&("codex", text(&parsed.meta, "sid")))
                .is_some_and(|ids| ids.len() != 1)
    }

    fn ownership(&self, uid: &str) -> Result<(String, Vec<String>), SessionError> {
        let mut current = uid.to_owned();
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(current.clone()) {
                return Err(unsupported("子代理归属关系存在循环"));
            }
            chain.push(current.clone());
            let Some(agent) = self.agents.get(&current) else {
                return Ok((current, chain));
            };
            current = agent
                .owner
                .as_ref()
                .map_err(|error| unsupported(error.clone()))?
                .clone();
        }
    }

    fn select(&self, uid: &str, agent: &str) -> Result<Selection, SessionError> {
        if !self.inventory.contains_key(uid) {
            return Err(SessionError::new(404, "会话不存在"));
        }
        if self.agents.contains_key(uid) {
            // Broken agents remain visible as unsupported rows; healthy ones
            // are accessible only through their actual root's agent selector.
            if self.ambiguous_agent(uid) {
                return Err(SessionError::new(409, "Codex 子代理 ID 在索引中存在歧义"));
            }
            self.ownership(uid)?;
            return Err(SessionError::new(404, "子代理必须通过所属主会话访问"));
        }
        if agent.is_empty() {
            return Ok(Selection {
                owner: uid.to_owned(),
                selected: uid.to_owned(),
                ancestry: vec![uid.to_owned()],
            });
        }
        let mut matches = Vec::new();
        for (candidate, info) in &self.agents {
            if info.id == agent
                && let Ok((owner, chain)) = self.ownership(candidate)
                && owner == uid
            {
                matches.push((candidate.clone(), chain));
            }
        }
        match matches.as_slice() {
            [(selected, _)] if self.ambiguous_agent(selected) => {
                Err(SessionError::new(409, "Codex 子代理 ID 在索引中存在歧义"))
            }
            [(selected, ancestry)] => Ok(Selection {
                owner: uid.to_owned(),
                selected: selected.clone(),
                ancestry: ancestry.clone(),
            }),
            [] => Err(SessionError::new(404, "子代理不存在或不属于此主会话")),
            _ => Err(SessionError::new(409, "主会话中子代理 ID 存在歧义")),
        }
    }

    /// Relations only, deliberately ignoring unsupported providers and invalid
    /// offsets. The caller uses this before restamping and reparsing changed
    /// dependencies, so an old unsupported snapshot must not block recovery.
    #[cfg(test)]
    fn dependencies(&self, selection: &Selection) -> Result<Vec<String>, SessionError> {
        let mut dependencies: BTreeSet<_> = selection.ancestry.iter().cloned().collect();
        dependencies.insert(selection.owner.clone());
        dependencies.insert(selection.selected.clone());
        let mut current = selection.selected.clone();
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(current.clone()) {
                return Err(unsupported("分叉历史依赖存在循环"));
            }
            dependencies.insert(current.clone());
            let parsed = &self.inventory[&current];
            let Some(sid) = relation_sid(parsed.candidate.source, &parsed.meta) else {
                return Ok(dependencies.into_iter().collect());
            };
            current = self.history_parent(sid)?;
        }
    }

    /// Metadata-only validation. No parser call or event copy for every row.
    fn chain(&self, uid: &str) -> Result<Vec<(String, usize)>, SessionError> {
        let mut current = uid.to_owned();
        let mut chain = Vec::new();
        let mut seen = BTreeSet::from([current.clone()]);
        loop {
            let parsed = &self.inventory[&current];
            let Some((sid, cut)) = history_link(parsed.candidate.source, &parsed.meta)? else {
                return Ok(chain);
            };
            let parent = self.history_parent(sid)?;
            if !seen.insert(parent.clone()) {
                return Err(unsupported("分叉历史依赖存在循环"));
            }
            check_cut(&self.inventory[&parent], cut)?;
            chain.push((parent.clone(), cut));
            current = parent;
        }
    }

    fn row(&self, uid: &str) -> Value {
        let parsed = &self.inventory[uid];
        let mut row = public_meta(&parsed.meta);
        if let Some(reason) = &parsed.unsupported {
            mark_unsupported(&mut row, reason);
        }
        match self.chain(uid) {
            Err(error) => mark_unsupported(&mut row, &error.message),
            Ok(chain) if !chain.is_empty() => {
                let mut digests = Vec::new();
                for (parent, cut) in &chain {
                    digests.push(json!([
                        parent,
                        cut,
                        self.inventory[parent].prefix_hash(*cut)
                    ]));
                    if *cut == 0 {
                        break;
                    }
                }
                digests.reverse();
                let identity = inherited_identity(native_identity(parsed, ""), digests);
                if row["supported"] != false {
                    row["cursor"] = json!({"end": parsed.committed,
                        "head": parsed.head(parsed.committed),
                        "anchor": super::semantic_anchor(&identity, parsed, parsed.committed)});
                }
                let root = &self.inventory[&chain.last().expect("nonempty").0];
                row["root_sid"] = root.meta["sid"].clone();
                row["fork_depth"] = json!(chain.len());
                row["created"] = root.meta["created"].clone();
                if parsed.meta["_named"] != true {
                    let named = chain
                        .iter()
                        .find(|(uid, _)| self.inventory[uid].meta["_named"] == true)
                        .map(|(uid, _)| &self.inventory[uid])
                        .unwrap_or(root);
                    row["title"] = named.meta["title"].clone();
                }
                let inherited: usize = chain
                    .iter()
                    .take_while(|(_, cut)| *cut > 0)
                    .map(|(_, cut)| cut)
                    .sum();
                row["size"] = json!(parsed.raw_index.length() + inherited as u64);
            }
            Ok(_) => {}
        }
        row
    }

    fn agent_item(&self, uid: &str) -> Value {
        let info = &self.agents[uid];
        let row = self.row(uid);
        let mut item = json!({"id": info.id, "title": info.title, "type": info.kind});
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

    #[cfg(test)]
    fn owner_row(&self, uid: &str) -> Value {
        let mut row = self.row(uid);
        let mut items: Vec<_> = self
            .agents
            .keys()
            .filter(|candidate| {
                !self.ambiguous_agent(candidate)
                    && self
                        .ownership(candidate)
                        .is_ok_and(|(owner, _)| owner == uid)
            })
            .map(|candidate| self.agent_item(candidate))
            .collect();
        items.sort_by(|left, right| {
            (text(left, "created"), text(left, "id"))
                .cmp(&(text(right, "created"), text(right, "id")))
        });
        if !items.is_empty() {
            row["agents"] = json!(items.len());
            row["agent_items"] = json!(items);
        }
        row
    }
}

#[cfg(test)]
/// The path is used solely to match another already indexed candidate. It is
/// never opened, canonicalized, or constructed from an HTTP agent parameter.
fn claude_agent(parsed: &Parsed) -> Option<(String, std::path::PathBuf)> {
    if parsed.candidate.source != "claude" {
        return None;
    }
    let path = &parsed.candidate.path;
    let directory = path.parent()?;
    if directory.file_name()? != "subagents" {
        return None;
    }
    let id = path.file_stem()?.to_str()?.strip_prefix("agent-")?;
    let session_directory = directory.parent()?;
    let main_stem = session_directory.file_name()?.to_str()?;
    let owner = session_directory
        .parent()?
        .join(format!("{main_stem}.jsonl"));
    Some((id.to_owned(), owner))
}

/// The physical parent `history_link` would name, ignoring offset validity:
/// a null `history_base` is a self-contained file with no relation.
#[cfg(test)]
fn relation_sid<'a>(source: &str, meta: &'a Value) -> Option<&'a str> {
    if source != "codex" {
        return None;
    }
    let base = &meta["history_base"];
    if !base.is_object() {
        return None;
    }
    let sid = text(base, "thread_id");
    let sid = if sid.is_empty() {
        text(meta, "forked_from_id")
    } else {
        sid
    };
    (!sid.is_empty()).then_some(sid)
}

/// The fixed-prefix parent a Codex transcript declares: `(thread_id, cut)`.
/// Python `_history_segments`: the physical parent is `history_base.thread_id`
/// (falling back to `forked_from_id`), the cut is `end_byte_offset`. A null
/// `history_base` inherits nothing — old-style forks and subagent rollouts
/// are self-contained files whose `forked_from_id` is only the logical
/// parent, and a rewind past the parent's own fork point legitimately names
/// a `thread_id` other than `forked_from_id`. `index/graph.rs` carries the
/// same function over summaries; the two must agree.
pub(super) fn history_link<'a>(
    source: &str,
    meta: &'a Value,
) -> Result<Option<(&'a str, usize)>, SessionError> {
    if source != "codex" {
        return Ok(None);
    }
    let base = &meta["history_base"];
    if base.is_null() {
        return Ok(None);
    }
    if !base.is_object() {
        return Err(unsupported("history_base 必须是对象"));
    }
    let declared = text(base, "thread_id");
    let parent = if declared.is_empty() {
        text(meta, "forked_from_id")
    } else {
        declared
    };
    if parent.is_empty() {
        return Err(unsupported("history_base 缺少父线程 ID"));
    }
    let cut = base["end_byte_offset"]
        .as_u64()
        .and_then(|cut| usize::try_from(cut).ok())
        .ok_or_else(|| unsupported("history_base 前缀偏移必须是非负整数"))?;
    Ok(Some((parent, cut)))
}

#[cfg(test)]
fn check_cut(parsed: &Parsed, cut: usize) -> Result<(), SessionError> {
    if cut > parsed.committed || cut as u64 > parsed.raw_index.length() {
        return Err(unsupported("父历史固定前缀超出完整原生数据范围"));
    }
    if !parsed.raw_index.is_checkpoint(cut as u64) {
        return Err(unsupported("父历史固定前缀不在完整 JSONL 行边界"));
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn rows(inventory: &Inventory) -> Vec<Value> {
    let graph = Graph::new(inventory);
    let mut rows: BTreeMap<String, Value> = BTreeMap::new();
    let mut owned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for uid in inventory.keys() {
        if graph.agents.contains_key(uid) {
            if graph.ambiguous_agent(uid) {
                let mut row = graph.row(uid);
                mark_unsupported(&mut row, "Codex 子代理 ID 在索引中存在歧义");
                rows.insert(uid.clone(), row);
                continue;
            }
            match graph.ownership(uid) {
                Ok((owner, _)) => {
                    owned.entry(owner).or_default().push(uid.clone());
                    continue;
                }
                Err(error) => {
                    let mut row = graph.row(uid);
                    mark_unsupported(&mut row, &error.message);
                    rows.insert(uid.clone(), row);
                    continue;
                }
            }
        }
        rows.insert(uid.clone(), graph.row(uid));
    }
    for (uid, mut agents) in owned {
        agents.sort_by(|left, right| {
            let left_parsed = &inventory[left];
            let right_parsed = &inventory[right];
            (text(&left_parsed.meta, "created"), &graph.agents[left].id)
                .cmp(&(text(&right_parsed.meta, "created"), &graph.agents[right].id))
        });
        if let Some(row) = rows.get_mut(&uid) {
            row["agent_items"] =
                Value::Array(agents.iter().map(|uid| graph.agent_item(uid)).collect());
            row["agents"] = json!(agents.len());
        }
    }
    rows.into_values().collect()
}

#[cfg(test)]
pub(super) fn dependencies(
    inventory: &Inventory,
    uid: &str,
    agent: &str,
) -> Result<Vec<String>, SessionError> {
    let graph = Graph::new(inventory);
    graph.dependencies(&graph.select(uid, agent)?)
}

/// Superseded inventory-graph resolver (frozen inventory); kept as the
/// reference implementation the on-demand `views` builder is tested against.
#[cfg(test)]
struct Builder<'a> {
    graph: &'a Graph<'a>,
    /// Inherited prefix events only (physical end zero); the leaf's own
    /// events stay in its `Parsed` and are chained at read time.
    events: Vec<Event>,
    event_count: usize,
    raw_bytes: usize,
    event_bytes: usize,
    digests: Vec<Value>,
    sources: Vec<Candidate>,
    seen: BTreeSet<String>,
    actual_dependencies: BTreeSet<String>,
}

#[cfg(test)]
impl Builder<'_> {
    /// Charge `events` against the view budgets; inherited ones are copied
    /// with a zero physical end, the leaf's own are only counted.
    fn push(&mut self, events: &[Event], inherited: bool) -> Result<(), SessionError> {
        self.event_count += events.len();
        for event in events {
            self.event_bytes += serde_json::to_vec(&event.message)
                .map_err(|_| SessionError::new(500, "历史消息序列化失败"))?
                .len();
            self.event_bytes += event
                .media
                .iter()
                .map(crate::media::NativeImage::resident_len)
                .sum::<usize>();
            if inherited {
                self.events.push(Event {
                    end: 0,
                    message: event.message.clone(),
                    media: event.media.clone(),
                });
            }
        }
        Ok(())
    }

    fn inherit(&mut self, source: &str, meta: &Value) -> Result<(), SessionError> {
        let Some((sid, cut)) = history_link(source, meta)? else {
            return Ok(());
        };
        let uid = self.graph.history_parent(sid)?;
        if !self.seen.insert(uid.clone()) {
            return Err(unsupported("分叉历史依赖存在循环"));
        }
        self.actual_dependencies.insert(uid.clone());
        let parsed = &self.graph.inventory[&uid];
        check_cut(parsed, cut)?;
        self.raw_bytes = self
            .raw_bytes
            .checked_add(cut)
            .ok_or_else(|| SessionError::new(413, "继承历史预算溢出"))?;
        // Zero is an empty prefix, not permission to include grandparents.
        if cut > 0 {
            let (prefix_meta, prefix_events) = parse_prefix(parsed, cut)?;
            self.inherit(parsed.candidate.source, &prefix_meta)?;
            self.push(&prefix_events, true)?;
        }
        self.digests
            .push(json!([uid, cut, parsed.prefix_hash(cut)]));
        self.sources.push(parsed.candidate.clone());
        Ok(())
    }
}

#[cfg(test)]
fn parse_prefix(parsed: &Parsed, cut: usize) -> Result<(Value, Vec<Event>), SessionError> {
    check_cut(parsed, cut)?;
    let candidate = &parsed.candidate;
    let expected = candidate
        .data_stamp()
        .ok_or_else(|| unsupported("父历史缺少原生输入"))?;
    let mut reader = super::native_input::CheckedNative::open_prefix(
        &candidate.root,
        &candidate.data,
        expected,
        cut as u64,
    )?;
    let mut decoder = super::records::Decoder::cold();
    let index = super::records::scan_native_records(&mut reader, &mut decoder, None, candidate)?;
    reader.finish()?;
    if index.committed() != cut as u64
        || index.prefix_hash(cut as u64) != Some(parsed.prefix_hash(cut))
    {
        return Err(SessionError::new(
            503,
            "父历史固定前缀在读取期间变化，请重试",
        ));
    }
    let batch = decoder.finish(cut);
    if let Some(error) = batch.error {
        return Err(unsupported(format!("父历史前缀不受支持：{error}")));
    }
    let records = batch.records;
    let (meta, events, error) = providers::parse_with_media(
        parsed.candidate.source,
        &parsed.candidate.path,
        &records,
        None,
        text(&parsed.meta, "created"),
        &batch.sidecars,
    );
    if let Some(error) = error {
        return Err(unsupported(format!("父历史固定前缀不受支持：{error}")));
    }
    if text(&meta, "sid") != text(&parsed.meta, "sid") {
        return Err(unsupported("父历史固定前缀不包含相同线程身份"));
    }
    Ok((meta, events))
}

#[cfg(test)]
pub(super) fn resolve(inventory: &Inventory, uid: &str, agent: &str) -> Result<View, SessionError> {
    let graph = Graph::new(inventory);
    let selection = graph.select(uid, agent)?;
    let native_scope = graph.native_scope(&selection);
    let dependencies = graph.dependencies(&selection)?;
    let parsed = inventory[&selection.selected].clone();
    if let Some(error) = parsed.raw_error.as_ref().or(parsed.unsupported.as_ref()) {
        return Err(unsupported(error.clone()));
    }
    // Validate the whole declared graph even when a zero prefix contributes no
    // events. Its relation dependencies remain stable for caller restamping.
    graph.chain(&selection.selected)?;
    let mut builder = Builder {
        graph: &graph,
        events: Vec::new(),
        event_count: 0,
        raw_bytes: 0,
        event_bytes: 0,
        digests: Vec::new(),
        sources: Vec::new(),
        seen: BTreeSet::from([selection.selected.clone()]),
        actual_dependencies: BTreeSet::new(),
    };
    builder.inherit(parsed.candidate.source, &parsed.meta)?;
    builder.push(&parsed.events, false)?;
    let native = native_identity(&parsed, agent);
    let identity = inherited_identity(native, builder.digests);
    let mut meta = graph.owner_row(&selection.owner);
    if selection.selected != selection.owner {
        let item = graph.agent_item(&selection.selected);
        meta["parent_title"] = meta["title"].clone();
        meta["sid"] = item["id"].clone();
        meta["agent_id"] = item["id"].clone();
        meta["agent_type"] = item["type"].clone();
        meta["title"] = item["title"].clone();
        for field in [
            "path",
            "cwd",
            "model",
            "created",
            "updated",
            "size",
            "supported",
            "migration_warnings",
        ] {
            if let Some(value) = item.get(field) {
                meta[field] = value.clone()
            }
        }
        let object = meta.as_object_mut().expect("metadata object");
        object.remove("cursor");
        // A display pin applies to the main transcript only, never to a
        // subagent view projected from its own sidecar file.
        object.remove("timeline_pin");
    }
    let dependencies: BTreeSet<_> = dependencies
        .into_iter()
        .chain(builder.actual_dependencies)
        .collect();
    let mut sources = vec![parsed.candidate.clone()];
    sources.extend(builder.sources);
    let rename = super::views::rename_event(&meta);
    Ok(View {
        parsed,
        meta,
        inherited: Arc::new(builder.events),
        dependencies: dependencies.into_iter().collect(),
        identity,
        native_scope,
        sources,
        rename,
    })
}

pub(super) fn inherited_identity(native: String, digests: Vec<Value>) -> String {
    if digests.is_empty() {
        native
    } else {
        hash(
            &serde_json::to_vec(&json!(["inherited-view-v1", native, digests]))
                .expect("primitive JSON serialization"),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::sessions::Candidate;

    const NOW: &str = "2026-09-11T00:00:00.000Z";

    fn header(sid: &str) -> Value {
        json!({"type":"session_meta", "timestamp":NOW,
            "payload":{"id":sid,"session_id":sid,"timestamp":NOW,"cwd":"/synthetic/project","thread_source":"user"}})
    }

    fn message(value: &str) -> Value {
        json!({"type":"response_item","timestamp":NOW,"payload":{
            "type":"message","role":"user","turn_id":"synthetic-turn",
            "content":[{"type":"input_text","text":value}]}})
    }

    fn fork_header(sid: &str, parent: &str, cut: usize) -> Value {
        let mut row = header(sid);
        row["payload"]["forked_from_id"] = json!(parent);
        row["payload"]["history_base"] = json!({"thread_id":parent,"end_byte_offset":cut});
        row
    }

    fn agent_header(sid: &str, parent: &str) -> Value {
        let mut row = header(sid);
        row["payload"]["session_id"] = json!(parent);
        row["payload"]["thread_source"] = json!("subagent");
        row["payload"]["parent_thread_id"] = json!(parent);
        row["payload"]["forked_from_id"] = json!(parent);
        row["payload"]["agent_path"] = json!(format!("worker/{sid}"));
        row["payload"]["agent_role"] = json!("explorer");
        row
    }

    fn encoded(records: &[Value]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for record in records {
            bytes.extend(serde_json::to_vec(record).unwrap());
            bytes.push(b'\n');
        }
        bytes
    }

    fn parsed(
        uid: &str,
        source: &'static str,
        path: &str,
        records: &[Value],
        agent: &str,
    ) -> Arc<Parsed> {
        let bytes = encoded(records);
        let mut end = 0;
        let records: Vec<_> = records
            .iter()
            .map(|row| {
                end += serde_json::to_vec(row).unwrap().len() as u64 + 1;
                (row.clone(), end)
            })
            .collect();
        let path = PathBuf::from(path);
        let (mut meta, events, unsupported) =
            providers::parse_agent(source, &path, &records, None, NOW, agent);
        meta["uid"] = json!(uid);
        meta["source"] = json!(source);
        meta["path"] = json!(path.to_string_lossy());
        meta["size"] = json!(bytes.len());
        meta["supported"] = json!(unsupported.is_none());
        if unsupported.is_some() || !meta["migration_warnings"].is_array() {
            meta["migration_warnings"] = json!([]);
        }
        meta["cursor"] =
            json!({"end":bytes.len(),"head":"synthetic-head","anchor":"synthetic-anchor"});
        let (native_id, _declared) = super::super::scope::native_identity(source, &records);
        let fixture = Arc::new(tempfile::tempdir().unwrap());
        let root = fixture.path().canonicalize().unwrap();
        let data = root.join("native.jsonl");
        std::fs::write(&data, &bytes).unwrap();
        let actual_stamp = super::super::stamp(&data).unwrap();
        Arc::new(Parsed {
            _fixture: Some(fixture),
            native_id,
            semantic_digest: super::super::projection_digest(&events, bytes.len()),
            raw_index: super::super::native_input::RawIndex::scan(bytes.as_slice()).unwrap(),
            candidate: Candidate {
                source,
                root,
                data,
                path,
                summary: None,
                stamps: vec![actual_stamp],
            },
            committed: bytes.len(),
            meta,
            events,
            unsupported,
            raw_error: None,
            pin: None,
        })
    }

    /// A record the reference adapter cannot read either (scalar `content`).
    const UNREADABLE: &[u8] = b"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":42}}\n";

    fn append_fixture(parsed: &mut Parsed, line: &[u8]) {
        let mut bytes = std::fs::read(&parsed.candidate.data).unwrap();
        bytes.extend_from_slice(line);
        std::fs::write(&parsed.candidate.data, &bytes).unwrap();
        parsed.candidate.stamps = vec![super::super::stamp(&parsed.candidate.data).unwrap()];
        parsed.committed = bytes.len();
        parsed.raw_index = super::super::native_input::RawIndex::scan(bytes.as_slice()).unwrap();
    }

    fn codex(uid: &str, records: &[Value]) -> Arc<Parsed> {
        parsed(
            uid,
            "codex",
            &format!("/synthetic/rollout-{uid}.jsonl"),
            records,
            "",
        )
    }

    fn inventory(entries: &[(&str, Arc<Parsed>)]) -> Inventory {
        entries
            .iter()
            .map(|(uid, parsed)| ((*uid).to_owned(), parsed.clone()))
            .collect()
    }

    fn texts(view: &View) -> Vec<String> {
        view.events()
            .filter(|event| event.message["role"] != "status")
            .map(|event| text(&event.message, "text").to_owned())
            .collect()
    }

    #[test]
    fn ordinary_view_uses_native_identity_and_keeps_list_cursor() {
        let parent = codex("parent", &[header("parent-sid"), message("first")]);
        let inventory = inventory(&[("parent", parent.clone())]);
        let view = resolve(&inventory, "parent", "").unwrap();
        assert_eq!(view.identity, native_identity(&parent, ""));
        assert_eq!(view.dependencies, ["parent"]);
        assert_eq!(texts(&view), ["first"]);
        assert!(rows(&inventory)[0].get("cursor").is_some());
        assert!(rows(&inventory)[0].get("_is_subagent").is_none());
    }

    #[test]
    fn fixed_prefix_excludes_parent_tail_and_marks_inherited_offsets_zero() {
        let prefix = [header("parent-sid"), message("kept")];
        let cut = encoded(&prefix).len();
        let parent = codex(
            "parent",
            &[prefix[0].clone(), prefix[1].clone(), message("old tail")],
        );
        let child = codex(
            "child",
            &[
                fork_header("child-sid", "parent-sid", cut),
                message("new branch"),
            ],
        );
        let inventory = inventory(&[("parent", parent), ("child", child.clone())]);
        let view = resolve(&inventory, "child", "").unwrap();
        assert_eq!(texts(&view), ["kept", "new branch"]);
        assert_eq!(view.all_events()[0].end, 0);
        assert_eq!(view.all_events()[1].end as usize, child.committed);
        assert_eq!(view.dependencies, ["child", "parent"]);
        assert_eq!(
            view.dependencies,
            dependencies(&inventory, "child", "").unwrap()
        );
        let row = rows(&inventory)
            .into_iter()
            .find(|row| row["uid"] == "child")
            .unwrap();
        assert_eq!(row["root_sid"], "parent-sid");
        assert_eq!(row["fork_depth"], 1);
        assert_eq!(row["size"], child.raw_index.length() + cut as u64);
        assert_eq!(row["title"], "kept");
        let checkpoint =
            super::super::ViewSnapshot::new(Arc::new(resolve(&inventory, "child", "").unwrap()));
        assert_eq!(row["cursor"], checkpoint.cursor());
    }

    #[test]
    fn two_level_inheritance_and_empty_prefix_are_not_duplicated() {
        let parent = codex("parent", &[header("parent-sid"), message("root")]);
        let child = codex(
            "child",
            &[
                fork_header("child-sid", "parent-sid", parent.committed),
                message("branch"),
            ],
        );
        let grand = codex(
            "grand",
            &[
                fork_header("grand-sid", "child-sid", child.committed),
                message("leaf"),
            ],
        );
        let empty = codex(
            "empty",
            &[
                fork_header("empty-sid", "child-sid", 0),
                message("only leaf"),
            ],
        );
        let inventory = inventory(&[
            ("parent", parent),
            ("child", child),
            ("grand", grand),
            ("empty", empty),
        ]);
        let view = resolve(&inventory, "grand", "").unwrap();
        assert_eq!(texts(&view), ["root", "branch", "leaf"]);
        assert_eq!(
            view.events()
                .map(|event| event.end == 0)
                .collect::<Vec<_>>(),
            [true, true, false]
        );
        assert_eq!(view.meta["fork_depth"], 2);
        let empty = resolve(&inventory, "empty", "").unwrap();
        assert_eq!(texts(&empty), ["only leaf"]);
        assert_eq!(empty.dependencies, ["child", "empty", "parent"]);
    }

    #[test]
    fn append_after_parent_cut_keeps_identity_but_prefix_rewrite_changes_it() {
        let first = [header("parent-sid"), message("root A")];
        let parent = codex("parent", &first);
        let child = codex(
            "child",
            &[
                fork_header("child-sid", "parent-sid", parent.committed),
                message("leaf"),
            ],
        );
        let mut inventory = inventory(&[("parent", parent), ("child", child)]);
        let original = resolve(&inventory, "child", "").unwrap();
        inventory.insert(
            "parent".to_owned(),
            codex(
                "parent",
                &[first[0].clone(), first[1].clone(), message("later")],
            ),
        );
        let appended = resolve(&inventory, "child", "").unwrap();
        assert_eq!(original.identity, appended.identity);
        assert_eq!(texts(&original), texts(&appended));
        inventory.insert(
            "parent".to_owned(),
            codex(
                "parent",
                &[first[0].clone(), message("root B"), message("later")],
            ),
        );
        let rewritten = resolve(&inventory, "child", "").unwrap();
        assert_ne!(original.identity, rewritten.identity);
        assert_eq!(texts(&rewritten), ["root B", "leaf"]);
    }

    #[test]
    fn unsupported_or_malformed_parent_tail_does_not_poison_fixed_prefix() {
        let records = [header("parent-sid"), message("valid")];
        let parent = codex("parent", &records);
        let child = codex(
            "child",
            &[
                fork_header("child-sid", "parent-sid", parent.committed),
                message("leaf"),
            ],
        );
        let mut inventory = inventory(&[("parent", parent), ("child", child)]);
        let original = resolve(&inventory, "child", "").unwrap();
        // Batch 33: an unknown record kind after the cutoff is skipped with a
        // warning, so the parent stays readable and the child's identity holds.
        let unknown = codex(
            "parent",
            &[
                records[0].clone(),
                records[1].clone(),
                json!({"type":"unknown_future_record"}),
            ],
        );
        assert!(unknown.unsupported.is_none());
        assert_eq!(
            unknown.meta["migration_warnings"],
            json!(["跳过未知的Codex 记录类型：unknown_future_record ×1"])
        );
        inventory.insert("parent".to_owned(), unknown);
        assert_eq!(
            original.identity,
            resolve(&inventory, "child", "").unwrap().identity
        );
        assert!(resolve(&inventory, "parent", "").is_ok());
        // A later session_meta is skipped and counted like Python's
        // `and not meta`; the parent stays readable and the child's fixed
        // prefix is untouched.
        let duplicated = codex(
            "parent",
            &[records[0].clone(), records[1].clone(), header("parent-sid")],
        );
        assert!(duplicated.unsupported.is_none());
        assert_eq!(
            duplicated.meta["migration_warnings"],
            json!(["跳过重复的Codex session_meta ×1"])
        );
        inventory.insert("parent".to_owned(), duplicated);
        assert_eq!(
            original.identity,
            resolve(&inventory, "child", "").unwrap().identity
        );
        assert_eq!(
            texts(&resolve(&inventory, "parent", "").unwrap()),
            ["valid"]
        );
        let mut malformed = codex("parent", &records);
        let parsed = Arc::get_mut(&mut malformed).unwrap();
        append_fixture(parsed, UNREADABLE);
        parsed.raw_error = Some("synthetic invalid tail".to_owned());
        parsed.unsupported = parsed.raw_error.clone();
        inventory.insert("parent".to_owned(), malformed);
        assert_eq!(
            original.identity,
            resolve(&inventory, "child", "").unwrap().identity
        );
        assert_eq!(
            dependencies(&inventory, "child", "").unwrap(),
            ["child", "parent"]
        );
    }

    #[test]
    fn unreadable_record_inside_parent_prefix_fails_closed_but_a_skipped_line_does_not() {
        // Batch 35: a line that is not a JSON object inside the fixed prefix
        // is skipped like Python; a record the reference adapter cannot read
        // either still refuses the inherited prefix.
        for (line, expected) in [(b"not-json\n".as_slice(), None), (UNREADABLE, Some(501))] {
            let mut parent = codex("parent", &[header("parent-sid"), message("valid")]);
            let parsed = Arc::get_mut(&mut parent).unwrap();
            append_fixture(parsed, line);
            let child = codex(
                "child",
                &[
                    fork_header("child-sid", "parent-sid", parent.committed),
                    message("leaf"),
                ],
            );
            let inventory = inventory(&[("parent", parent), ("child", child)]);
            let resolved = resolve(&inventory, "child", "");
            assert_eq!(resolved.as_ref().err().map(|error| error.status), expected);
            if let Ok(resolved) = resolved {
                assert_eq!(texts(&resolved), ["valid", "leaf"]);
            }
        }
    }

    #[test]
    fn invalid_offsets_reject_but_dependencies_still_allow_refresh() {
        let parent = codex("parent", &[header("parent-sid"), message("valid")]);
        for cut in [1, parent.committed + 1] {
            let child = codex("child", &[fork_header("child-sid", "parent-sid", cut)]);
            let inventory = inventory(&[("parent", parent.clone()), ("child", child)]);
            assert_eq!(
                dependencies(&inventory, "child", "").unwrap(),
                ["child", "parent"]
            );
            assert_eq!(resolve(&inventory, "child", "").err().unwrap().status, 501);
            assert_eq!(
                rows(&inventory)
                    .into_iter()
                    .find(|row| row["uid"] == "child")
                    .unwrap()["supported"],
                false
            );
        }
        let mut bad = fork_header("child-sid", "parent-sid", 0);
        bad["payload"]["history_base"]["end_byte_offset"] = json!(-1);
        let inventory = inventory(&[("parent", parent), ("child", codex("child", &[bad]))]);
        assert!(dependencies(&inventory, "child", "").is_ok());
        assert_eq!(resolve(&inventory, "child", "").err().unwrap().status, 501);
    }

    #[test]
    fn missing_duplicate_and_cycle_parents_fail_closed() {
        let child = codex("child", &[fork_header("child-sid", "parent-sid", 0)]);
        let missing = inventory(&[("child", child.clone())]);
        assert_eq!(resolve(&missing, "child", "").err().unwrap().status, 501);
        let duplicate = inventory(&[
            ("child", child),
            ("parent-a", codex("parent-a", &[header("parent-sid")])),
            ("parent-b", codex("parent-b", &[header("parent-sid")])),
        ]);
        assert_eq!(resolve(&duplicate, "child", "").err().unwrap().status, 409);
        let cycle = inventory(&[
            ("a", codex("a", &[fork_header("a-sid", "b-sid", 0)])),
            ("b", codex("b", &[fork_header("b-sid", "a-sid", 0)])),
        ]);
        assert!(
            dependencies(&cycle, "a", "")
                .err()
                .unwrap()
                .message
                .contains("循环")
        );
        assert!(rows(&cycle).iter().all(|row| row["supported"] == false));
    }

    /// Python `_history_segments`: `history_base` null means nothing is
    /// inherited. Old-style forks copy the ancestors' metas and history into
    /// their own file and are read alone; the copied metas are skipped.
    #[test]
    fn fork_without_history_base_is_self_contained_and_copied_metas_are_skipped() {
        let mut fork = header("child-sid");
        fork["payload"]["forked_from_id"] = json!("parent-sid");
        let inventory = inventory(&[
            (
                "parent",
                codex("parent", &[header("parent-sid"), message("parent only")]),
            ),
            (
                "child",
                codex(
                    "child",
                    &[
                        fork,
                        header("parent-sid"),
                        header("grandparent-sid"),
                        message("copied history"),
                        message("own leaf"),
                    ],
                ),
            ),
        ]);
        let view = resolve(&inventory, "child", "").unwrap();
        assert_eq!(texts(&view), ["copied history", "own leaf"]);
        assert!(view.inherited.is_empty());
        assert_eq!(view.identity, native_identity(&view.parsed, ""));
        assert_eq!(view.meta["sid"], "child-sid");
        assert_eq!(view.meta["supported"], true);
        assert_eq!(
            view.meta["migration_warnings"],
            json!(["跳过重复的Codex session_meta ×2"])
        );
        assert_eq!(
            view.parsed.native_id.as_deref().unwrap(),
            "child-sid",
            "copied ancestor metas are not this file's identity"
        );
        // The parent is still resolvable on its own; nothing links the two
        // physically, so a missing parent cannot break the child either.
        assert_eq!(
            texts(&resolve(&inventory, "parent", "").unwrap()),
            ["parent only"]
        );
        let alone = inventory
            .iter()
            .filter(|(uid, _)| uid.as_str() == "child")
            .map(|(uid, parsed)| (uid.clone(), parsed.clone()))
            .collect::<Inventory>();
        assert_eq!(
            texts(&resolve(&alone, "child", "").unwrap()),
            ["copied history", "own leaf"]
        );
    }

    /// Python `_history_segments` table: `base.thread_id or forked_from_id`
    /// is the physical parent, `end_byte_offset` the cut, and only a
    /// non-null `history_base` inherits anything.
    #[test]
    fn history_link_follows_the_reference_segments_rule() {
        let link = |base: Value, fork: &str, subagent: bool| {
            let mut meta = json!({"history_base": base, "forked_from_id": fork});
            if subagent {
                meta["_is_subagent"] = json!(true);
            }
            history_link("codex", &meta).map(|link| link.map(|(sid, cut)| (sid.to_owned(), cut)))
        };
        assert_eq!(link(Value::Null, "", false).unwrap(), None);
        assert_eq!(link(Value::Null, "P", false).unwrap(), None);
        assert_eq!(link(Value::Null, "P", true).unwrap(), None);
        assert_eq!(
            link(json!({"thread_id": "R", "end_byte_offset": 5}), "Q", false).unwrap(),
            Some(("R".to_owned(), 5)),
            "a rewind past the parent's fork point names another physical file"
        );
        assert_eq!(
            link(json!({"end_byte_offset": 5}), "Q", false).unwrap(),
            Some(("Q".to_owned(), 5))
        );
        assert_eq!(
            link(json!({"thread_id": "R", "end_byte_offset": 5}), "Q", true).unwrap(),
            Some(("R".to_owned(), 5))
        );
        let error = link(json!({"thread_id": "R"}), "Q", false).unwrap_err();
        assert_eq!(error.status, 501);
        assert!(error.message.contains("前缀偏移"), "{}", error.message);
        assert_eq!(
            link(json!({"thread_id": "R", "end_byte_offset": -1}), "Q", false)
                .unwrap_err()
                .status,
            501
        );
        assert_eq!(
            link(json!({"end_byte_offset": 5}), "", false)
                .unwrap_err()
                .status,
            501
        );
        assert_eq!(link(json!(42), "Q", false).unwrap_err().status, 501);
        assert_eq!(
            history_link("claude", &json!({"history_base": 42})).unwrap(),
            None
        );
    }

    #[test]
    fn nested_codex_agents_flatten_to_actual_root_not_its_fork() {
        let parent = codex("parent", &[header("parent-sid"), message("root")]);
        let fork = codex(
            "fork",
            &[
                fork_header("fork-sid", "parent-sid", parent.committed),
                message("fork"),
            ],
        );
        let agent = codex(
            "agent",
            &[
                agent_header("agent-sid", "parent-sid"),
                message("agent body"),
            ],
        );
        let nested = codex(
            "nested",
            &[
                agent_header("nested-sid", "agent-sid"),
                message("nested body"),
            ],
        );
        let inventory = inventory(&[
            ("parent", parent),
            ("fork", fork),
            ("agent", agent.clone()),
            ("nested", nested),
        ]);
        let list = rows(&inventory);
        assert_eq!(list.len(), 2);
        let parent = list.iter().find(|row| row["uid"] == "parent").unwrap();
        let fork = list.iter().find(|row| row["uid"] == "fork").unwrap();
        assert_eq!(parent["agents"], 2);
        assert!(fork.get("agent_items").is_none());
        let view = resolve(&inventory, "parent", "agent-sid").unwrap();
        assert_eq!(view.meta["uid"], "parent");
        assert_eq!(view.meta["sid"], "agent-sid");
        assert_eq!(view.meta["agent_id"], "agent-sid");
        assert_eq!(view.meta["agent_type"], "explorer");
        assert_eq!(view.meta["title"], "worker/agent-sid");
        assert_eq!(view.meta["parent_title"], "root");
        assert_eq!(view.meta["agent_items"].as_array().unwrap().len(), 2);
        assert_eq!(texts(&view), ["agent body"]);
        assert_eq!(view.identity, native_identity(&agent, ""));
        assert_eq!(view.dependencies, ["agent", "parent"]);
        assert_eq!(
            resolve(&inventory, "parent", "nested-sid")
                .unwrap()
                .dependencies,
            ["agent", "nested", "parent"]
        );
        assert_eq!(
            resolve(&inventory, "fork", "agent-sid")
                .err()
                .unwrap()
                .status,
            404
        );
        assert_eq!(resolve(&inventory, "agent", "").err().unwrap().status, 404);
        assert_eq!(
            resolve(&inventory, "parent", "../../outside")
                .err()
                .unwrap()
                .status,
            404
        );
        assert!(parent["agent_items"][0].get("cursor").is_some());
    }

    #[test]
    fn orphan_and_cyclic_agents_are_visible_unsupported_not_silently_lost() {
        let inventory = inventory(&[
            (
                "orphan",
                codex("orphan", &[agent_header("orphan-sid", "absent")]),
            ),
            ("a", codex("a", &[agent_header("a-sid", "b-sid")])),
            ("b", codex("b", &[agent_header("b-sid", "a-sid")])),
        ]);
        let list = rows(&inventory);
        assert_eq!(list.len(), 3);
        for row in list {
            assert_eq!(row["supported"], false);
            assert!(!row["migration_warnings"].as_array().unwrap().is_empty());
        }
        assert_eq!(resolve(&inventory, "orphan", "").err().unwrap().status, 501);
        assert_eq!(resolve(&inventory, "a", "").err().unwrap().status, 501);
    }

    #[test]
    fn duplicate_agent_sid_is_visible_unsupported_and_selector_returns_conflict() {
        let inventory = inventory(&[
            ("parent", codex("parent", &[header("parent-sid")])),
            (
                "a",
                codex("a", &[agent_header("duplicate-sid", "parent-sid")]),
            ),
            (
                "b",
                codex("b", &[agent_header("duplicate-sid", "parent-sid")]),
            ),
        ]);
        let list = rows(&inventory);
        assert_eq!(list.len(), 3);
        assert!(
            list.iter()
                .filter(|row| row["uid"] != "parent")
                .all(|row| row["supported"] == false)
        );
        let parent = resolve(&inventory, "parent", "").unwrap();
        assert!(parent.meta.get("agent_items").is_none());
        assert_eq!(
            resolve(&inventory, "parent", "duplicate-sid")
                .err()
                .unwrap()
                .status,
            409
        );
        assert_eq!(resolve(&inventory, "a", "").err().unwrap().status, 409);
    }

    #[test]
    fn claude_agents_use_exact_inventory_path_and_full_agent_id() {
        let root_record = json!({"type":"user","uuid":"root-user","parentUuid":null,
            "sessionId":"parent-session-id","timestamp":NOW,"message":{"role":"user","content":"root"}});
        let child_record = json!({"type":"user","uuid":"agent-user","parentUuid":null,
            "sessionId":"parent-session-id","agentId":"long-agent-id","isSidechain":true,
            "timestamp":NOW,"message":{"role":"user","content":"agent work"}});
        let parent = parsed(
            "parent",
            "claude",
            "/synthetic/project/main.with.dots.jsonl",
            &[root_record],
            "",
        );
        let mut agent = parsed(
            "agent",
            "claude",
            "/synthetic/project/main.with.dots/subagents/agent-long-agent-id.jsonl",
            &[child_record],
            "long-agent-id",
        );
        let metadata = &mut Arc::get_mut(&mut agent).unwrap().meta;
        metadata["_agent_title"] = json!("sidecar description");
        metadata["_agent_type"] = json!("Explore");
        let inventory = inventory(&[("parent", parent), ("agent", agent.clone())]);
        assert_eq!(rows(&inventory).len(), 1);
        let view = resolve(&inventory, "parent", "long-agent-id").unwrap();
        assert_eq!(view.meta["uid"], "parent");
        assert_eq!(view.meta["sid"], "long-agent-id");
        assert_eq!(view.meta["title"], "sidecar description");
        assert_eq!(view.meta["agent_type"], "Explore");
        assert_eq!(texts(&view), ["agent work"]);
        assert_eq!(view.identity, native_identity(&agent, "long-agent-id"));
        assert_eq!(view.dependencies, ["agent", "parent"]);
        assert_eq!(
            resolve(&inventory, "parent", "long-age")
                .err()
                .unwrap()
                .status,
            404
        );
        let orphan = inventory
            .into_iter()
            .filter(|(uid, _)| uid == "agent")
            .collect();
        assert_eq!(rows(&orphan)[0]["supported"], false);
    }

    #[test]
    fn inherited_parent_later_interrupt_does_not_amend_pinned_message() {
        let assistant = json!({"type":"response_item","timestamp":NOW,"payload":{
            "type":"message","role":"assistant","phase":"commentary","turn_id":"synthetic-turn",
            "content":[{"type":"output_text","text":"in progress"}]}});
        let prefix = [header("parent-sid"), assistant];
        let cut = encoded(&prefix).len();
        let parent = codex(
            "parent",
            &[
                prefix[0].clone(),
                prefix[1].clone(),
                json!({"type":"event_msg","timestamp":NOW,"payload":{"type":"turn_aborted","turn_id":"synthetic-turn"}}),
            ],
        );
        assert_eq!(parent.events[0].message["interrupted"], true);
        let child = codex(
            "child",
            &[fork_header("child-sid", "parent-sid", cut), message("leaf")],
        );
        let inventory = inventory(&[("parent", parent), ("child", child)]);
        let view = resolve(&inventory, "child", "").unwrap();
        assert_ne!(view.all_events()[0].message["interrupted"], true);
        assert_eq!(texts(&view), ["in progress", "leaf"]);
    }

    #[test]
    fn deep_history_and_large_output_have_no_service_quota() {
        let mut inventory = Inventory::new();
        inventory.insert("0".to_owned(), codex("0", &[header("sid-0")]));
        for depth in 1..=40 {
            let uid = depth.to_string();
            inventory.insert(
                uid.clone(),
                codex(
                    &uid,
                    &[fork_header(
                        &format!("sid-{depth}"),
                        &format!("sid-{}", depth - 1),
                        0,
                    )],
                ),
            );
        }
        assert!(resolve(&inventory, "40", "").is_ok());
        assert_eq!(dependencies(&inventory, "40", "").unwrap().len(), 41);
        let mut huge = codex("huge", &[header("huge-sid")]);
        Arc::get_mut(&mut huge).unwrap().events = vec![
            Event {
                end: 1,
                message: json!({"role":"user","text":"x"}),
                media: Vec::new(),
            };
            2_000_001
        ];
        let inventory = BTreeMap::from([("huge".to_owned(), huge)]);
        assert!(resolve(&inventory, "huge", "").is_ok());
    }
}
