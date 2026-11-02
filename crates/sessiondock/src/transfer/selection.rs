//! Select movable native units while keeping display-only nesting portable.
use super::{
    TransferError,
    group::{Group, Member},
    service::TransferService,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Serialize, Deserialize)]
pub struct Selection {
    pub uids: BTreeSet<String>,
    pub source_node: String,
    pub target_node: String,
}

pub fn supported(member: &Member) -> bool {
    matches!(member.source.as_str(), "claude" | "codex" | "grok")
}

pub fn required(group: &Group, uid: &str) -> BTreeSet<String> {
    let mut uids = BTreeSet::from([uid.to_owned()]);
    loop {
        let before = uids.len();
        for edge in &group.edges {
            if !matches!(edge.kind.as_str(), "nest_parent" | "fork" | "continuation")
                && (uids.contains(&edge.from) || uids.contains(&edge.to))
            {
                uids.insert(edge.from.clone());
                uids.insert(edge.to.clone());
            }
        }
        if before == uids.len() {
            return uids;
        }
    }
}

pub fn subset(group: &Group, requested: &BTreeSet<String>) -> Result<Group, TransferError> {
    if requested.is_empty() {
        return Err(TransferError::new(
            "move_selection_empty",
            "请至少选择一个会话",
        ));
    }
    let mut uids = BTreeSet::new();
    for uid in requested {
        let member = group
            .members
            .iter()
            .find(|member| member.uid == *uid)
            .ok_or_else(|| TransferError::new("move_plan_stale", "所选会话不在当前迁移树中"))?;
        if !supported(member) {
            return Err(TransferError::new(
                "move_group_unsupported",
                "所选会话来源尚不支持迁移",
            ));
        }
        uids.extend(required(group, uid));
    }
    let members: Vec<_> = group
        .members
        .iter()
        .filter(|member| uids.contains(&member.uid))
        .cloned()
        .collect();
    if members.iter().any(|member| !supported(member)) {
        return Err(TransferError::new(
            "move_group_unsupported",
            "原生历史依赖包含不支持迁移的来源",
        ));
    }
    Ok(Group {
        selected: if uids.contains(&group.selected) {
            group.selected.clone()
        } else {
            members[0].uid.clone()
        },
        members,
        edges: group
            .edges
            .iter()
            .filter(|edge| uids.contains(&edge.from) && uids.contains(&edge.to))
            .cloned()
            .collect(),
        blockers: group
            .blockers
            .iter()
            .filter(|blocker| uids.contains(&blocker.uid) && !display_blocker(blocker))
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
        let rows = snapshot.list["sessions"].as_array();
        let index = snapshot.index();
        let entries: std::collections::BTreeMap<_, _> = index
            .candidates()
            .map(|entry| (entry.uid.as_str(), entry))
            .collect();
        json!({"selection":true,"uid":group.selected,"sessions":group.members.iter().map(|member| {
            let row = rows.and_then(|rows| rows.iter().find(|row| row["uid"] == member.uid));
            let nest = row.and_then(|row| row.get("nest_parent"));
            let resolve = |source: &str, sid: &str| group.members.iter()
                .find(|candidate| candidate.source == source && candidate.sid == sid && !sid.is_empty())
                .map(|parent| parent.uid.clone());
            let parent = if let Some(parent) = nest.filter(|parent| parent.is_object()) {
                if parent["node_id"].is_null() {resolve(parent["source"].as_str().unwrap_or(""),parent["sid"].as_str().unwrap_or(""))} else {None}
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
                "required_uids":required(group,&member.uid)})
        }).collect::<Vec<_>>()})
    }
}
