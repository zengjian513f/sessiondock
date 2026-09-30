//! Persistent move handoff. A received group stays fenced until the source
//! records its irreversible ownership switch. Cleanup never releases that fence.
use super::{
    TransferError,
    environment::StorageProbe,
    service::{Operation, TransferService},
};
use crate::trash::manifest::{EntryState, FileRecord, FileRole, Manifest, RunStateNote, Stamp};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

fn trash_error(error: crate::trash::TrashError) -> TransferError {
    TransferError::new(error.code, error.message)
}
fn changed() -> TransferError {
    TransferError::new(
        "move_plan_stale",
        "源端内容或剩余引用已变化，保留文件等待清理",
    )
}
fn private_dir(path: &Path) -> Result<(), TransferError> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

impl TransferService {
    pub fn plan_move(&self, uid: &str, new_ids: bool) -> Result<Operation, TransferError> {
        let mut op = self.plan_copy(uid, new_ids)?;
        op.moving = true;
        for (provider, root) in self.bundle_roots(&op)? {
            op.storage_probes
                .insert(provider, StorageProbe::create(&root)?);
        }
        self.save(&op)?;
        Ok(op)
    }
    /// Persist the decision before compensating the receiver. A concurrent or
    /// delayed switch can never succeed after this point, even after restart.
    pub fn abort_source(&self, id: &str, finished: bool) -> Result<Operation, TransferError> {
        let mut op = self.load(id)?;
        if !op.moving
            || op.incoming_digest.is_some()
            || !matches!(
                op.phase.as_str(),
                "planned" | "exporting" | "aborting" | "aborted"
            )
        {
            return Err(TransferError::new(
                "move_recovery_required",
                "执行归属已交接，不能撤回；请继续完成移动",
            ));
        }
        if op.phase == "aborted" {
            return Ok(op);
        }
        if finished && op.phase != "aborting" {
            return Err(TransferError::new(
                "move_recovery_required",
                "源端尚未记录撤回决定",
            ));
        }
        op.phase = if finished { "aborted" } else { "aborting" }.into();
        op.export_lease_until = 0;
        self.save(&op)?;
        if finished {
            for (provider, root) in self.bundle_roots(&op)? {
                if let Some(probe) = op.storage_probes.get(&provider) {
                    let _ = probe.remove(&root);
                }
            }
        }
        Ok(op)
    }
    pub fn abort_target(&self, id: &str) -> Result<Operation, TransferError> {
        let mut op = self.load(id)?;
        if !op.moving
            || op.incoming_digest.is_none()
            || !matches!(
                op.phase.as_str(),
                "planned" | "failed" | "ready" | "rollback_required" | "aborting" | "aborted"
            )
        {
            return Err(TransferError::new(
                "move_recovery_required",
                "目标已开放继续，不能撤回",
            ));
        }
        if op.phase == "aborted" {
            return Ok(op);
        }
        if matches!(
            op.phase.as_str(),
            "ready" | "rollback_required" | "aborting"
        ) {
            op.phase = "aborting".into();
            self.save(&op)?;
            self.rollback(&op)?;
        }
        op.phase = "aborted".into();
        self.save(&op)?;
        Ok(op)
    }
    pub fn switch_source(&self, id: &str) -> Result<Operation, TransferError> {
        let mut op = self.load(id)?;
        if !op.moving || op.incoming_digest.is_some() {
            return Err(TransferError::new("move_plan_stale", "此操作不是迁移源"));
        }
        if matches!(op.phase.as_str(), "moved" | "retiring" | "retired") {
            return Ok(op);
        }
        if op.phase != "exporting" {
            return Err(TransferError::new("move_plan_stale", "迁移源尚未完成导出"));
        }
        self.recheck(&op)?;
        op.phase = "moved".into();
        op.export_lease_until = 0;
        self.save(&op)?;
        Ok(op)
    }
    pub fn activate_target(&self, id: &str) -> Result<Operation, TransferError> {
        let mut op = self.load(id)?;
        if !op.moving || op.incoming_digest.is_none() {
            return Err(TransferError::new(
                "move_plan_stale",
                "此操作不是迁移接收记录",
            ));
        }
        if op.phase == "complete" {
            return Ok(op);
        }
        if op.phase != "ready" {
            return Err(TransferError::new(
                "move_recovery_required",
                "目标历史尚未验证完成",
            ));
        }
        op.phase = "complete".into();
        op.ownership_sequence = self.next_ownership_sequence()?;
        self.save(&op)?;
        self.cleanup_markers(&op)?;
        self.reclaim_prior_moves(&op)?;
        Ok(op)
    }
    /// An explicitly completed import can return a retired identity to this
    /// node. Keep old handoff records, but let the new operation own those UIDs.
    pub(super) fn reclaim_prior_moves(&self, op: &Operation) -> Result<(), TransferError> {
        if op.phase != "complete" || op.incoming_digest.is_none() {
            return Ok(());
        }
        let Some(staged) = &op.staged else {
            return Ok(());
        };
        let active = op
            .group()
            .members
            .iter()
            .map(|m| self.member_target_uid(op, m, staged))
            .collect::<Result<BTreeSet<_>, _>>()?;
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path().join("operation.json");
            if !path.is_file() {
                continue;
            }
            let mut old: Operation = serde_json::from_slice(&fs::read(path)?)?;
            if old.id == op.id
                || old.phase != "retired"
                || !old.moving
                || old.incoming_digest.is_some()
                || op.ownership_sequence <= old.ownership_sequence
            {
                continue;
            }
            let uids: Vec<_> = old
                .group()
                .members
                .iter()
                .filter(|m| active.contains(&m.uid))
                .map(|m| m.uid.clone())
                .collect();
            if !uids.is_empty() {
                for uid in uids {
                    old.reclaimed_by.insert(uid, op.id.clone());
                }
                self.save(&old)?;
            }
        }
        Ok(())
    }
    pub(super) fn next_ownership_sequence(&self) -> Result<u64, TransferError> {
        let mut sequence = 0;
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path().join("operation.json");
            if path.is_file() {
                let op: Operation = serde_json::from_slice(&fs::read(path)?)?;
                sequence = sequence.max(op.ownership_sequence);
            }
        }
        sequence
            .checked_add(1)
            .ok_or_else(|| TransferError::new("move_recovery_required", "迁移序号已耗尽"))
    }

    /// Also works after a partial cleanup: compare explicit references against
    /// captured identities even when the referenced source file is now in trash.
    fn outside_references(&self, op: &Operation) -> Result<(), TransferError> {
        let snapshot = self
            .store()
            .search_snapshot()
            .map_err(|e| TransferError::new("move_inventory", e.message))?;
        let uids: BTreeSet<_> = op.group().members.iter().map(|m| m.uid.as_str()).collect();
        let ids: BTreeSet<_> = op
            .group()
            .members
            .iter()
            .map(|m| (m.source.as_str(), m.sid.as_str()))
            .collect();
        let references = |source: &str, id: &str| !id.is_empty() && ids.contains(&(source, id));
        // A tool call and its result can live in different rollout generations.
        // Resolve call IDs within each outside thread, never across threads.
        let mut calls_by_thread = BTreeMap::<String, BTreeMap<String, String>>::new();
        for e in snapshot
            .index()
            .candidates()
            .filter(|e| e.source == "codex" && !uids.contains(e.uid.as_str()))
        {
            let calls = calls_by_thread.entry(e.summary.sid.clone()).or_default();
            for line in fs::read(&e.data)?.split(|b| *b == b'\n') {
                if let Ok(row) = serde_json::from_slice::<Value>(line) {
                    super::codex_tools::collect_call(&row, calls);
                }
            }
        }
        for e in snapshot
            .index()
            .candidates()
            .filter(|e| !uids.contains(e.uid.as_str()))
        {
            if references(e.source, &e.summary.sid)
                || e.owner
                    .as_ref()
                    .is_some_and(|owner| uids.contains(owner.as_str()))
                || e.summary
                    .continued_in_sid
                    .as_ref()
                    .is_some_and(|id| references(e.source, id))
            {
                return Err(changed());
            }
            if let Some(c) = &e.summary.codex
                && (references("codex", &c.forked_from_id)
                    || references("codex", &c.parent_thread_id)
                    || c.history_base["thread_id"]
                        .as_str()
                        .is_some_and(|id| op.plan.identities.rollouts.contains_key(id)))
            {
                return Err(changed());
            }
            if e.source == "codex" || e.source == "claude" {
                let raw = fs::read(&e.data)?;
                let rows: Vec<Value> = raw
                    .split(|b| *b == b'\n')
                    .filter_map(|line| serde_json::from_slice(line).ok())
                    .collect();
                if e.source == "claude" {
                    let links = super::group::claude_tools::links(&rows);
                    if links
                        .requests
                        .iter()
                        .chain(links.resumed.values())
                        .any(|id| references("claude", id))
                    {
                        return Err(changed());
                    }
                }
                for row in &rows {
                    if e.source == "codex"
                        && super::codex_tools::references(row, &calls_by_thread[&e.summary.sid])
                            .iter()
                            .any(|id| references("codex", id))
                    {
                        return Err(changed());
                    }
                    if e.source == "claude" {
                        if [
                            "parentSessionId",
                            "forkedFromSessionId",
                            "continuedInSessionId",
                        ]
                        .iter()
                        .any(|key| {
                            row[*key]
                                .as_str()
                                .is_some_and(|id| references("claude", id))
                        }) {
                            return Err(changed());
                        }
                        if !e.is_agent()
                            && row.get("parentUuid").is_some()
                            && row["uuid"].as_str().is_some_and(|id| {
                                op.file_plan.as_ref().is_some_and(|p| {
                                    p.records.contains_key(&format!("claude:{id}"))
                                })
                            })
                        {
                            return Err(changed());
                        }
                    }
                }
            }
            if e.source == "grok"
                && let Some(path) = &e.summary_path
            {
                let rows = super::group::grok_tools::rows(path.parent().unwrap())?;
                if super::group::grok_tools::references(&rows)
                    .iter()
                    .any(|id| {
                        references("grok", id)
                            || op.file_plan.as_ref().is_some_and(|plan| {
                                plan.sessions.contains_key(&format!("grok:{id}"))
                            })
                    })
                {
                    return Err(changed());
                }
                let mut paths = vec![path.clone()];
                let agents = path.parent().unwrap().join("subagents");
                if agents.is_dir() {
                    for child in fs::read_dir(agents)? {
                        let meta = child?.path().join("meta.json");
                        if meta.is_file() {
                            paths.push(meta);
                        }
                    }
                }
                for path in paths {
                    let row: Value = serde_json::from_slice(&fs::read(path)?)?;
                    if ["parent_session_id", "child_session_id"]
                        .iter()
                        .any(|key| row[*key].as_str().is_some_and(|id| references("grok", id)))
                    {
                        return Err(changed());
                    }
                }
            }
        }
        for row in snapshot.list["sessions"].as_array().into_iter().flatten() {
            if !uids.contains(row["uid"].as_str().unwrap_or("")) {
                let parent = &row["spawned_by"];
                if references(
                    parent["source"].as_str().unwrap_or(""),
                    parent["sid"].as_str().unwrap_or(""),
                ) {
                    return Err(changed());
                }
            }
        }
        Ok(())
    }
    pub fn retire_source(&self, id: &str, trash: &Path) -> Result<Operation, TransferError> {
        let mut op = self.load(id)?;
        if !op.moving
            || op.incoming_digest.is_some()
            || !matches!(op.phase.as_str(), "moved" | "retiring" | "retired")
        {
            return Err(TransferError::new(
                "move_plan_stale",
                "源端执行归属尚未切换",
            ));
        }
        if op.phase == "retired" {
            return Ok(op);
        }
        if op.phase == "moved" {
            self.recheck(&op)?;
        }
        self.outside_references(&op)?;
        super::native::retire(&op.native, false)?;
        op.phase = "retiring".into();
        self.save(&op)?;
        let roots = self.bundle_roots(&op)?;
        let mut files = BTreeMap::<String, BTreeMap<PathBuf, String>>::new();
        for file in &op.plan.files {
            files
                .entry("codex".into())
                .or_default()
                .insert(file.source.clone(), file.sha256.clone());
        }
        if let Some(plan) = &op.file_plan {
            for file in &plan.files {
                files
                    .entry(file.provider.clone())
                    .or_default()
                    .insert(file.source.clone(), file.sha256.clone());
            }
        }
        for (provider, expected) in files {
            let member = op
                .group()
                .members
                .iter()
                .find(|m| m.source == provider)
                .unwrap();
            let entry_id = format!("move-{}-{provider}", op.id);
            let directory = trash.join(&entry_id);
            private_dir(&directory)?;
            private_dir(&directory.join("files"))?;
            let manifest_path = directory.join("manifest.json");
            let mut manifest = if manifest_path.exists() {
                Manifest::read(&directory).map_err(trash_error)?
            } else {
                let now = crate::trash::manifest::now_unix();
                Manifest {
                    version: 1,
                    entry_id,
                    uid: member.uid.clone(),
                    source: provider.clone(),
                    sid: member.sid.clone(),
                    title: member.title.clone(),
                    cwd: member.cwd.clone(),
                    created: String::new(),
                    updated: String::new(),
                    origin: member.path.clone(),
                    root: roots[&provider].clone(),
                    deleted_at: crate::trash::manifest::rfc3339(now),
                    deleted_at_unix: now,
                    state: EntryState::Moving,
                    forced: false,
                    run_state: RunStateNote {
                        state: "exited".into(),
                        detail: format!("move:{}", op.id),
                    },
                    bytes: 0,
                    files: expected
                        .keys()
                        .enumerate()
                        .map(|(i, path)| {
                            Ok(FileRecord {
                                name: i.to_string(),
                                origin: path.clone(),
                                role: FileRole::Data,
                                stamp: Stamp::capture(path).map_err(trash_error)?,
                                in_trash: false,
                            })
                        })
                        .collect::<Result<Vec<_>, TransferError>>()?,
                }
            };
            if manifest
                .files
                .iter()
                .map(|f| &f.origin)
                .collect::<BTreeSet<_>>()
                != expected.keys().collect()
                || manifest.run_state.detail != format!("move:{}", op.id)
            {
                return Err(changed());
            }
            manifest.bytes = manifest.files.iter().map(|f| f.stamp.size).sum();
            manifest.write(&directory).map_err(trash_error)?;
            for index in 0..manifest.files.len() {
                let file = &manifest.files[index];
                let held = directory.join("files").join(&file.name);
                let source_exists = fs::symlink_metadata(&file.origin).is_ok();
                let held_exists = fs::symlink_metadata(&held).is_ok();
                if held_exists && super::service::hash(&held)? != expected[&file.origin] {
                    return Err(changed());
                }
                if source_exists {
                    if super::service::hash(&file.origin)? != expected[&file.origin] {
                        return Err(changed());
                    }
                    if Stamp::capture(&file.origin).map_err(trash_error)? != file.stamp {
                        return Err(changed());
                    }
                    // Reappearance after a recorded removal is a new source
                    // writer/restore, never another cleanup candidate.
                    if file.in_trash {
                        return Err(changed());
                    }
                    if held_exists {
                        fs::remove_file(&file.origin)?;
                    } else {
                        crate::trash::move_entry(&file.origin, &held).map_err(trash_error)?;
                    }
                    fs::File::open(file.origin.parent().unwrap())?.sync_all()?;
                } else if !held_exists {
                    return Err(changed());
                }
                manifest.files[index].in_trash = true;
                manifest.write(&directory).map_err(trash_error)?;
            }
            manifest.state = EntryState::Trashed;
            manifest.write(&directory).map_err(trash_error)?;
        }
        super::native::retire(&op.native, true)?;
        if let Some(metadata) = &self.metadata {
            metadata
                .transfer_rows(&op.metadata_before, true)
                .map_err(|e| TransferError::new(e.code, e.message))?;
        }
        // The API drops launch receipts and drafts before marking retirement
        // complete. Every earlier failure leaves the permanent source fence.
        Ok(op)
    }
    pub fn finish_retirement(&self, mut op: Operation) -> Result<Operation, TransferError> {
        if op.phase == "retired" {
            return Ok(op);
        }
        op.phase = "retired".into();
        op.ownership_sequence = self.next_ownership_sequence()?;
        self.save(&op)?;
        for (provider, root) in self.bundle_roots(&op)? {
            if let Some(probe) = op.storage_probes.get(&provider) {
                let _ = probe.remove(&root);
            }
        }
        Ok(op)
    }
}
