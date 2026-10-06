//! Whole connected groups share one recoverable entry and one durable receipt.
//! Receipts are private files in the existing trash directory, not another index.
use super::{
    Deleted, Plan, TrashError, TrashService,
    manifest::{FileRole, RunStateNote, Stamp},
    plan::PlannedFile,
};
use crate::transfer::group::Group;
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, io::Write};

pub fn members(group: &Group) -> BTreeSet<String> {
    group.members.iter().map(|m| m.uid.clone()).collect()
}
impl TrashService {
    pub fn plan_tree(&self, group: &Group) -> Result<Plan, TrashError> {
        if let Some(blocker) = group.blockers.first() {
            return Err(TrashError::new(409, "tree_incomplete", &blocker.message));
        }
        let mut combined: Option<Plan> = None;
        let mut sessions = Vec::new();
        let counter = crate::transfer::progress::Task::new(
            "检查会话文件",
            "份历史",
            Some(group.members.len() as u64),
        );
        for member in &group.members {
            let path = if member.source == "grok" {
                member.path.parent().unwrap_or(&member.path)
            } else {
                &member.path
            };
            let row = json!({"uid":member.uid,"source":member.source,"sid":member.sid,
                "title":member.title,"cwd":member.cwd,"path":path});
            let mut plan = self.plan_for(&row)?;
            if member.source == "claude" {
                let mut extra = vec![];
                if member.agent {
                    extra.push((member.path.with_extension("meta.json"), FileRole::AgentMeta));
                } else {
                    extra.push((member.path.with_extension(""), FileRole::Directory));
                    let home = if member.root.file_name().is_some_and(|s| s == "projects") {
                        member.root.parent().unwrap_or(&member.root)
                    } else {
                        &member.root
                    };
                    extra.push((
                        home.join("file-history").join(&member.sid),
                        FileRole::Directory,
                    ));
                }
                for (origin, role) in extra {
                    match fs::symlink_metadata(&origin) {
                        Ok(_) => {
                            let stamp = if role == FileRole::Directory {
                                Stamp::capture_directory(&origin)?
                            } else {
                                Stamp::capture(&origin)?
                            };
                            plan.files.push(PlannedFile {
                                origin,
                                role,
                                stamp,
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => {
                            return Err(TrashError::new(
                                503,
                                "stat_failed",
                                "会话附属文件暂时无法检查",
                            ));
                        }
                    }
                }
            }
            sessions.push(json!({"uid":member.uid,"source":member.source,"sid":member.sid,
                "title":member.title,"cwd":member.cwd,"path":path,"agent":member.agent,
                "file_count":plan.files.len(),"bytes":plan.bytes(),
                "relations":group.edges.iter().filter(|e| e.from == member.uid || e.to == member.uid).map(|e| &e.kind).collect::<Vec<_>>() }));
            if let Some(existing) = combined.as_mut() {
                if member.uid == group.selected {
                    std::mem::swap(existing, &mut plan);
                }
                existing.files.extend(plan.files);
            } else {
                combined = Some(plan);
            }
            counter.add(1);
        }
        let mut plan = combined.ok_or_else(|| TrashError::new(404, "not_found", "会话不存在"))?;
        plan.files.sort_by(|a, b| a.origin.cmp(&b.origin));
        plan.files.dedup_by(|a, b| a.origin == b.origin);
        let directories: Vec<_> = plan
            .files
            .iter()
            .filter(|f| f.role == FileRole::Directory)
            .map(|f| f.origin.clone())
            .collect();
        plan.files.retain(|f| {
            !directories
                .iter()
                .any(|d| f.origin != *d && f.origin.starts_with(d))
        });
        plan.sessions = sessions;
        Ok(plan)
    }

    fn tree_receipt(&self, id: &str) -> Result<std::path::PathBuf, TrashError> {
        if !super::entry_id_is_valid(id) {
            return Err(TrashError::new(
                400,
                "invalid_request_id",
                "删除请求标识无效",
            ));
        }
        Ok(self.directory.join(format!(".tree-delete-{id}.json")))
    }
    fn write_tree_receipt(&self, id: &str, value: &Value) -> Result<(), TrashError> {
        let path = self.tree_receipt(id)?;
        let temp = path.with_extension("tmp");
        let result = (|| -> std::io::Result<()> {
            let mut options = fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&serde_json::to_vec(value)?)?;
            file.sync_all()?;
            fs::rename(temp, path)?;
            fs::File::open(&self.directory)?.sync_all()
        })();
        result.map_err(|_| TrashError::new(500, "tree_receipt_failed", "无法保存删除记录"))
    }
    /// Only the first caller starts work. Repeated requests never delete restored files.
    pub fn start_tree(
        &self,
        id: &str,
        uid: &str,
        uids: &BTreeSet<String>,
    ) -> Result<bool, TrashError> {
        let mut jobs = self.tree_jobs.lock().unwrap_or_else(|e| e.into_inner());
        let path = self.tree_receipt(id)?;
        let old = jobs.get(id).cloned().or_else(|| {
            fs::read(&path)
                .ok()
                .and_then(|v| serde_json::from_slice::<Value>(&v).ok())
        });
        if let Some(old) = old {
            if old["uid"] != uid || old["members"] != json!(uids) {
                return Err(TrashError::new(
                    409,
                    "tree_request_conflict",
                    "此删除请求已用于另一份清单",
                ));
            }
            return Ok(false);
        }
        if path.exists() {
            return Err(TrashError::new(
                500,
                "tree_receipt_failed",
                "已有删除记录无法读取，请检查回收站",
            ));
        }
        let value = json!({"request_id":id,"uid":uid,"members":uids,"phase":"checking"});
        self.write_tree_receipt(id, &value)?;
        jobs.insert(id.into(), value);
        Ok(true)
    }
    pub fn tree_status(&self, id: &str, uid: &str) -> Result<Value, TrashError> {
        let jobs = self.tree_jobs.lock().unwrap_or_else(|e| e.into_inner());
        let mut value = if let Some(value) = jobs.get(id) {
            value.clone()
        } else {
            let mut value: Value = serde_json::from_slice(
                &fs::read(self.tree_receipt(id)?)
                    .map_err(|_| TrashError::new(404, "not_found", "删除记录不存在"))?,
            )
            .map_err(|_| TrashError::new(500, "tree_receipt_failed", "删除记录无法读取"))?;
            if !matches!(value["phase"].as_str(), Some("complete" | "failed")) {
                let entry = self.directory.join(format!("tree-{id}"));
                if let Ok(manifest) = super::Manifest::read(&entry)
                    && manifest.state == super::EntryState::Trashed
                {
                    value["phase"] = json!("complete");
                    value["deleted"] = json!(manifest.sessions);
                } else {
                    value["phase"] = json!("failed");
                    value["error"] = json!(
                        "服务曾中断；请检查会话列表和回收站后重新预览，原请求不会再次删除文件。"
                    );
                }
            }
            value
        };
        if value["uid"] != uid {
            return Err(TrashError::new(
                409,
                "tree_request_conflict",
                "删除请求与会话不一致",
            ));
        }
        value.as_object_mut().unwrap().remove("members");
        value["work"] = self.tree_progress.snapshot(id);
        Ok(value)
    }
    pub fn finish_tree(
        &self,
        id: &str,
        result: Result<Vec<Deleted>, String>,
    ) -> Result<(), TrashError> {
        let mut jobs = self.tree_jobs.lock().unwrap_or_else(|e| e.into_inner());
        let value = jobs
            .get_mut(id)
            .ok_or_else(|| TrashError::new(404, "not_found", "删除记录不存在"))?;
        match result {
            Ok(deleted) => {
                value["phase"] = json!("complete");
                value["deleted"] = json!(deleted);
            }
            Err(error) => {
                value["phase"] = json!("failed");
                value["error"] = json!(error);
            }
        }
        self.write_tree_receipt(id, value)?;
        jobs.remove(id);
        Ok(())
    }
    pub fn move_tree(&self, id: &str, plan: &Plan) -> Result<Vec<Deleted>, TrashError> {
        let _guard = self.guard();
        let deleted = self.move_into_entry(
            plan,
            RunStateNote {
                state: "unknown".into(),
                detail: "tree: fresh running-state check passed".into(),
            },
            false,
            Some(&format!("tree-{id}")),
        )?;
        Ok(plan
            .sessions
            .iter()
            .map(|row| Deleted {
                uid: row["uid"].as_str().unwrap_or_default().into(),
                title: row["title"].as_str().unwrap_or_default().into(),
                ..deleted.clone()
            })
            .collect())
    }
}

pub fn preview(plan: &Plan) -> Value {
    let count = plan
        .sessions
        .iter()
        .map(|m| (m["source"].as_str(), m["sid"].as_str()))
        .collect::<BTreeSet<_>>()
        .len();
    json!({"uid":plan.uid,"sessions":plan.sessions,"session_count":count,"file_count":plan.files.len(),"bytes":plan.bytes()})
}
