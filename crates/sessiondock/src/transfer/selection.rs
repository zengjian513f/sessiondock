//! Select movable native units while keeping display-only nesting portable.
use super::{
    TransferError,
    group::{Group, Member},
    service::TransferService,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Selection {
    pub uids: BTreeSet<String>,
    pub source_node: String,
    pub target_node: String,
}

pub fn supported(member: &Member) -> bool {
    matches!(
        member.source.as_str(),
        "claude" | "codex" | "grok" | "droid"
    )
}

fn history_units(group: &Group) -> HashMap<&str, Arc<[&str]>> {
    let mut adjacent = HashMap::<&str, Vec<&str>>::new();
    for edge in group
        .edges
        .iter()
        .filter(|edge| !matches!(edge.kind.as_str(), "nest_parent" | "fork" | "continuation"))
    {
        adjacent.entry(&edge.from).or_default().push(&edge.to);
        adjacent.entry(&edge.to).or_default().push(&edge.from);
    }
    let mut visited = HashSet::new();
    let mut units = HashMap::new();
    for member in &group.members {
        let uid = member.uid.as_str();
        if !visited.insert(uid) {
            continue;
        }
        let mut pending = vec![uid];
        let mut unit = Vec::new();
        while let Some(uid) = pending.pop() {
            unit.push(uid);
            for &neighbor in adjacent.get(uid).into_iter().flatten() {
                if visited.insert(neighbor) {
                    pending.push(neighbor);
                }
            }
        }
        unit.sort_unstable();
        let unit = Arc::<[&str]>::from(unit);
        for &uid in unit.iter() {
            units.insert(uid, unit.clone());
        }
    }
    units
}

pub fn subset(group: &Group, requested: &BTreeSet<String>) -> Result<Group, TransferError> {
    if requested.is_empty() {
        return Err(TransferError::new(
            "move_selection_empty",
            "请至少选择一个会话",
        ));
    }
    let units = history_units(group);
    let members: HashMap<_, _> = group
        .members
        .iter()
        .map(|member| (member.uid.as_str(), member))
        .collect();
    let mut uids = BTreeSet::new();
    for uid in requested {
        let member = members
            .get(uid.as_str())
            .ok_or_else(|| TransferError::new("move_plan_stale", "所选会话不在当前迁移树中"))?;
        if !supported(member) {
            return Err(TransferError::new(
                "move_group_unsupported",
                "所选会话来源尚不支持迁移",
            ));
        }
        if !uids.contains(uid.as_str()) {
            uids.extend(units[uid.as_str()].iter().copied());
        }
    }
    let members: Vec<_> = group
        .members
        .iter()
        .filter(|member| uids.contains(member.uid.as_str()))
        .cloned()
        .collect();
    if members.iter().any(|member| !supported(member)) {
        return Err(TransferError::new(
            "move_group_unsupported",
            "原生历史依赖包含不支持迁移的来源",
        ));
    }
    Ok(Group {
        selected: if uids.contains(group.selected.as_str()) {
            group.selected.clone()
        } else {
            members[0].uid.clone()
        },
        members,
        edges: group
            .edges
            .iter()
            .filter(|edge| uids.contains(edge.from.as_str()) && uids.contains(edge.to.as_str()))
            .cloned()
            .collect(),
        blockers: group
            .blockers
            .iter()
            .filter(|blocker| uids.contains(blocker.uid.as_str()) && !display_blocker(blocker))
            .cloned()
            .collect(),
    })
}

pub(super) fn display_blocker(blocker: &super::group::Blocker) -> bool {
    blocker.message.starts_with("nest_parent ")
        || blocker.message.starts_with("缺少 nest_parent 关联")
        || blocker.message.starts_with("缺少 fork 关联")
        || blocker.message.starts_with("缺少 continuation 关联")
}

impl TransferService {
    pub(super) fn selected_group(
        &self,
        anchor: &str,
        uids: &BTreeSet<String>,
        snapshot: &crate::sessions::SessionSnapshot,
    ) -> Result<Group, TransferError> {
        let mut group = self.group_in(anchor, snapshot)?;
        for uid in uids {
            if !group.members.iter().any(|member| member.uid == *uid) {
                let related = self.group_in(uid, snapshot)?;
                for member in related.members {
                    if !group
                        .members
                        .iter()
                        .any(|existing| existing.uid == member.uid)
                    {
                        group.members.push(member);
                    }
                }
                group.edges.extend(related.edges);
                group.blockers.extend(related.blockers);
            }
        }
        group.edges.sort();
        group.edges.dedup();
        subset(&group, uids)
    }
    pub fn selection_tree(
        &self,
        group: &Group,
        snapshot: &crate::sessions::SessionSnapshot,
    ) -> Value {
        let mut rows = HashMap::new();
        for row in snapshot.list["sessions"].as_array().into_iter().flatten() {
            if let Some(uid) = row["uid"].as_str() {
                rows.entry(uid).or_insert(row);
            }
        }
        let mut identities = HashMap::new();
        for member in &group.members {
            identities
                .entry((member.source.as_str(), member.sid.as_str()))
                .or_insert(member.uid.as_str());
        }
        let units = history_units(group);
        let index = snapshot.index();
        let entries: HashMap<_, _> = index
            .candidates()
            .map(|entry| (entry.uid.as_str(), entry))
            .collect();
        let resolve = |source: &str, sid: &str| {
            if sid.is_empty() {
                return None;
            }
            identities.get(&(source, sid)).map(|uid| (*uid).to_owned())
        };
        let sessions: Vec<_> = group.members.iter().map(|member| {
            let nest = rows.get(member.uid.as_str()).and_then(|row| row.get("nest_parent"));
            let parent = if let Some(parent) = nest.filter(|parent| parent.is_object()) {
                if parent["node_id"].is_null() {
                    resolve(parent["source"].as_str().unwrap_or(""), parent["sid"].as_str().unwrap_or(""))
                } else {
                    None
                }
            } else {
                entries.get(member.uid.as_str()).and_then(|entry| {
                    entry.owner.clone().or_else(|| entry.summary.codex.as_ref().and_then(|codex|
                        resolve("codex", &codex.parent_thread_id).or_else(|| resolve("codex", &codex.forked_from_id))))
                        .or_else(|| entry.summary.forked_from_id.as_ref().and_then(|sid| resolve(&member.source, sid)))
                })
            };
            json!({"uid":member.uid,"sid":member.sid,"title":member.title,"source":member.source,
                "cwd":member.cwd,"agent":member.agent,"parent_uid":parent,"nest_parent":nest,
                "selectable":supported(member),"disabled_reason":if supported(member){""}else{"此来源尚不支持迁移"},
                "required_uids":units[member.uid.as_str()].as_ref()})
        }).collect();
        json!({"selection":true,"uid":group.selected,"sessions":sessions})
    }
}
