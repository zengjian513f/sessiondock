//! Durable same-node clone transaction. Plans contain server-derived paths only;
//! clients submit an opaque operation ID. Recovery never overwrites changed data.
use super::{
    TransferError,
    codex::{self, ClonePlan, StagedClone},
    group,
    native::{self, Native},
};
use crate::{
    metadata::MetadataStore,
    sessions::{SessionRoots, SessionStore},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    pub phase: String,
    pub uid: String,
    pub target_uid: Option<String>,
    pub plan: ClonePlan,
    pub native: Native,
    pub rewritten: Option<Native>,
    pub staged: Option<StagedClone>,
    pub error: Option<String>,
    pub metadata_before: BTreeMap<String, Value>,
    pub metadata_after: BTreeMap<String, Value>,
}
pub struct TransferService {
    pub directory: PathBuf,
    pub roots: SessionRoots,
    pub home: PathBuf,
    pub gate: Arc<tokio::sync::Mutex<()>>,
    pub metadata: Option<Arc<MetadataStore>>,
}
pub fn hash(path: &Path) -> Result<String, TransferError> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}
pub fn uid(path: &Path) -> String {
    format!(
        "codex:{}",
        &format!("{:x}", Sha1::digest(path.to_string_lossy().as_bytes()))[..16]
    )
}
fn persist(path: &Path, value: &impl Serialize) -> Result<(), TransferError> {
    let temp = path.with_extension("tmp");
    let mut file = fs::File::create(&temp)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(temp, path)?;
    fs::File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
impl TransferService {
    pub fn open(
        directory: PathBuf,
        mut roots: SessionRoots,
        metadata: Option<Arc<MetadataStore>>,
    ) -> Result<Self, TransferError> {
        let root = roots
            .codex
            .as_ref()
            .ok_or_else(|| TransferError::new("move_group_unsupported", "未配置 Codex 历史目录"))?;
        let home = if root
            .file_name()
            .is_some_and(|s| s == "sessions" || s == "archived_sessions")
        {
            root.parent().unwrap().to_owned()
        } else {
            root.clone()
        };
        roots.codex = Some(home.clone());
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let service = Self {
            directory,
            roots,
            home,
            gate: Arc::new(tokio::sync::Mutex::new(())),
            metadata,
        };
        // Recovery is performed before the service starts accepting launches.
        for entry in fs::read_dir(&service.directory)? {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }
            let file = path.join("operation.json");
            if !file.exists() {
                continue;
            }
            let mut operation: Operation = serde_json::from_slice(&fs::read(file)?)?;
            if operation.phase == "complete" {
                service.cleanup_markers(&operation)?;
            }
            if matches!(
                operation.phase.as_str(),
                "publishing" | "verifying" | "rollback_required"
            ) {
                match service.rollback(&operation) {
                    Ok(()) => operation.phase = "failed".into(),
                    Err(e) => {
                        operation.phase = "rollback_required".into();
                        operation.error = Some(e.message);
                    }
                }
                service.save(&operation)?;
            }
        }
        Ok(service)
    }
    pub fn load(&self, id: &str) -> Result<Operation, TransferError> {
        if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            return Err(TransferError::new("move_plan_stale", "复制记录不存在"));
        }
        Ok(serde_json::from_slice(&fs::read(
            self.directory.join(id).join("operation.json"),
        )?)?)
    }
    fn save(&self, op: &Operation) -> Result<(), TransferError> {
        persist(&self.directory.join(&op.id).join("operation.json"), op)
    }
    pub fn store(&self) -> SessionStore {
        SessionStore::with_metadata(self.roots.clone(), self.metadata.clone())
    }
    pub fn locked(&self, uid: &str) -> Result<bool, TransferError> {
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path().join("operation.json");
            if !path.is_file() {
                continue;
            }
            let op: Operation = serde_json::from_slice(&fs::read(path)?)?;
            if matches!(
                op.phase.as_str(),
                "publishing" | "verifying" | "rollback_required"
            ) && (op.plan.group.members.iter().any(|m| m.uid == uid)
                || op.staged.as_ref().is_some_and(|s| {
                    s.files
                        .iter()
                        .any(|f| self::uid(&self.home.join(&f.relative)) == uid)
                }))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn plan(&self, selected: &str) -> Result<Operation, TransferError> {
        if self.locked(selected)? {
            return Err(TransferError::new(
                "move_recovery_required",
                "会话组有尚未恢复的复制操作",
            ));
        }
        let snapshot = self
            .store()
            .search_snapshot()
            .map_err(|e| TransferError::new("move_inventory", e.message))?;
        let group = group::derive(&snapshot, selected)?;
        let mut plan = codex::plan(group, codex::Mode::Clone)?;
        if !plan.reference_issues.is_empty() {
            return Err(TransferError::new(
                "move_reference_unsupported",
                plan.reference_issues
                    .iter()
                    .map(|v| v.reason.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join("；"),
            ));
        }
        for member in &plan.group.members {
            if !member.cwd.is_empty() && !Path::new(&member.cwd).is_dir() {
                return Err(TransferError::new(
                    "move_cwd_missing",
                    "会话的工作目录不存在",
                ));
            }
        }
        let native = native::capture(&self.home, &plan)?;
        native::extend_identities(&native, &mut plan)?;
        let id = codex::uuid()?;
        let directory = self.directory.join(&id);
        fs::create_dir(&directory)?;
        let mut op = Operation {
            id,
            phase: "planned".into(),
            uid: selected.into(),
            target_uid: None,
            plan,
            native,
            rewritten: None,
            staged: None,
            error: None,
            metadata_before: BTreeMap::new(),
            metadata_after: BTreeMap::new(),
        };
        // Stage at planning time: every reference/offset/native row is validated
        // before confirmation, and retry always uses this exact identity map.
        let staged = codex::stage(&op.plan, &directory.join("staging"))?;
        let rewritten = native::rewrite(&op.native, &op.plan, &staged, &self.home)?;
        native::preflight(&rewritten)?;
        let selected_file = op
            .plan
            .group
            .members
            .iter()
            .find(|m| m.uid == selected)
            .unwrap();
        let target = staged
            .files
            .iter()
            .find(|f| f.source == selected_file.path)
            .unwrap();
        op.target_uid = Some(uid(&self.home.join(&target.relative)));
        if let Some(metadata) = &self.metadata {
            let snapshot = metadata
                .snapshot()
                .map_err(|e| TransferError::new(e.code, e.message))?;
            for member in &op.plan.group.members {
                let before = snapshot.row(&member.uid);
                op.metadata_before
                    .insert(member.uid.clone(), before.clone());
                let mut after = serde_json::Map::new();
                for key in [
                    "starred",
                    "starred_at",
                    "fork_parent_visible",
                    "nest_independent",
                ] {
                    if let Some(v) = before.get(key) {
                        after.insert(key.into(), v.clone());
                    }
                }
                for key in ["spawned_by", "nest_parent"] {
                    if let Some(mut relation) = before.get(key).cloned() {
                        if relation["source"] == "codex" && relation["node_id"].is_null() {
                            if let Some(mapped) = relation["sid"]
                                .as_str()
                                .and_then(|id| op.plan.identities.threads.get(id))
                            {
                                relation["sid"] = mapped.clone().into();
                                after.insert(key.into(), relation);
                            }
                        }
                    }
                }
                if !after.is_empty() {
                    after.insert("clone_operation".into(), op.id.clone().into());
                }
                let file = staged
                    .files
                    .iter()
                    .find(|f| f.source == member.path)
                    .unwrap();
                op.metadata_after
                    .insert(uid(&self.home.join(&file.relative)), after.into());
            }
        }
        op.rewritten = Some(rewritten);
        op.staged = Some(staged);
        self.save(&op)?;
        Ok(op)
    }
    pub fn recheck(&self, op: &Operation) -> Result<(), TransferError> {
        let snapshot = self
            .store()
            .search_snapshot()
            .map_err(|e| TransferError::new("move_inventory", e.message))?;
        let current = group::derive(&snapshot, &op.uid)?;
        let old: BTreeSet<_> = op
            .plan
            .group
            .members
            .iter()
            .map(|m| (&m.uid, &m.sid))
            .collect();
        let now: BTreeSet<_> = current.members.iter().map(|m| (&m.uid, &m.sid)).collect();
        if old != now || !current.blockers.is_empty() || current.edges != op.plan.group.edges {
            return Err(TransferError::new(
                "move_plan_stale",
                "会话组关联已变化，请重新查看复制清单",
            ));
        }
        for f in &op.plan.files {
            if hash(&f.source)? != f.sha256 {
                return Err(TransferError::new(
                    "move_plan_stale",
                    "会话历史已变化，请重新查看复制清单",
                ));
            }
        }
        if let Some(metadata) = &self.metadata {
            let snapshot = metadata
                .snapshot()
                .map_err(|e| TransferError::new(e.code, e.message))?;
            for (uid, row) in &op.metadata_before {
                if snapshot.row(uid) != *row {
                    return Err(TransferError::new(
                        "move_plan_stale",
                        "源会话显示设置已变化，请重新查看复制清单",
                    ));
                }
            }
        }
        if native::capture(&self.home, &op.plan)? != op.native {
            return Err(TransferError::new(
                "move_plan_stale",
                "原生会话元数据已变化，请重新查看复制清单",
            ));
        }
        Ok(())
    }
    fn marker(id: &str, target: &Path) -> PathBuf {
        target.with_file_name(format!(
            ".sessiondock-{id}-{}.pending",
            target.file_name().unwrap().to_string_lossy()
        ))
    }
    fn owns(id: &str, target: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(a), Ok(b)) = (fs::metadata(target), fs::metadata(Self::marker(id, target))) {
                return a.dev() == b.dev() && a.ino() == b.ino();
            }
        }
        false
    }
    fn cleanup_markers(&self, op: &Operation) -> Result<(), TransferError> {
        if let Some(staged) = &op.staged {
            for f in &staged.files {
                let target = self.home.join(&f.relative);
                let marker = Self::marker(&op.id, &target);
                if marker.exists() {
                    fs::remove_file(marker)?;
                    fs::File::open(target.parent().unwrap())?.sync_all()?;
                }
            }
        }
        Ok(())
    }
    fn rollback(&self, op: &Operation) -> Result<(), TransferError> {
        // The hidden publication link proves file ownership even if a crash
        // happened immediately after linking and before the next journal write.
        if let Some(staged) = &op.staged {
            for f in &staged.files {
                let path = self.home.join(&f.relative);
                if path.exists() && (!Self::owns(&op.id, &path) || hash(&path)? != f.sha256) {
                    return Err(TransferError::new(
                        "move_recovery_required",
                        "克隆文件已变化或不属于本次操作，保留现场等待恢复",
                    ));
                }
            }
        }
        if let Some(metadata) = &self.metadata {
            metadata
                .transfer_rows(&op.metadata_after, true)
                .map_err(|e| TransferError::new(e.code, e.message))?;
        }
        if let Some(native) = &op.rewritten {
            native::rollback(native, &op.id)?;
        }
        if let Some(staged) = &op.staged {
            for f in &staged.files {
                let path = self.home.join(&f.relative);
                if path.exists() {
                    fs::remove_file(&path)?;
                    fs::File::open(path.parent().unwrap())?.sync_all()?;
                }
            }
        }
        self.cleanup_markers(op)
    }
    pub fn execute(&self, mut op: Operation) -> Result<Operation, TransferError> {
        if op.phase == "complete" {
            return Ok(op);
        }
        if op.phase != "planned" {
            return Err(TransferError::new(
                "move_recovery_required",
                op.error
                    .unwrap_or_else(|| "复制操作需要恢复或重新发起".into()),
            ));
        }
        self.recheck(&op)?;
        native::preflight(op.rewritten.as_ref().unwrap())?;
        let staged = op.staged.as_ref().unwrap();
        for file in &staged.files {
            if self.home.join(&file.relative).exists() {
                return Err(TransferError::new("move_conflict", "目标文件已存在"));
            }
        }
        op.phase = "publishing".into();
        self.save(&op)?;
        let result = (|| {
            for file in &op.staged.as_ref().unwrap().files {
                let target = self.home.join(&file.relative);
                let parent = target.parent().unwrap();
                fs::create_dir_all(parent)?;
                let staging = self
                    .directory
                    .join(&op.id)
                    .join("staging")
                    .join(&file.relative);
                if hash(&staging)? != file.sha256 {
                    return Err(TransferError::new("move_plan_stale", "复制暂存数据已变化"));
                }
                // Copy to an unindexed sibling then link with no-replace semantics;
                // publication is atomic even when state and CLI roots differ in FS.
                let temp = Self::marker(&op.id, &target);
                let mut out = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temp)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    out.set_permissions(fs::Permissions::from_mode(0o600))?;
                }
                std::io::copy(&mut fs::File::open(staging)?, &mut out)?;
                out.sync_all()?;
                fs::hard_link(&temp, &target)?;
                fs::File::open(parent)?.sync_all()?;
            }
            native::insert(op.rewritten.as_ref().unwrap(), &op.id)?;
            if let Some(metadata) = &self.metadata {
                metadata
                    .transfer_rows(&op.metadata_after, false)
                    .map_err(|e| TransferError::new(e.code, e.message))?;
            }
            op.phase = "verifying".into();
            self.save(&op)?;
            // Recheck only source rows and files: new identities cannot join it.
            self.recheck(&op)?;
            let store = self.store();
            store
                .list(true)
                .map_err(|e| TransferError::new("move_verify", e.message))?;
            let snapshot = store
                .search_snapshot()
                .map_err(|e| TransferError::new("move_verify", e.message))?;
            let group = group::derive(&snapshot, op.target_uid.as_ref().unwrap())?;
            if !group.blockers.is_empty() || group.members.len() != op.plan.group.members.len() {
                return Err(TransferError::new("move_verify", "克隆后的历史关系不完整"));
            }
            let wanted: BTreeSet<_> = op.plan.identities.threads.values().cloned().collect();
            if group.members.iter().any(|m| !wanted.contains(&m.sid)) {
                return Err(TransferError::new("move_verify", "克隆仍引用源组身份"));
            }
            op.phase = "complete".into();
            self.save(&op)?;
            Ok(())
        })();
        if let Err(error) = result {
            op.error = Some(error.message.clone());
            op.phase = "rollback_required".into();
            self.save(&op)?;
            if let Err(recovery) = self.rollback(&op) {
                op.error = Some(format!("{}；回滚：{}", error.message, recovery.message));
                self.save(&op)?;
                return Err(recovery);
            }
            op.phase = "failed".into();
            self.save(&op)?;
            return Err(error);
        }
        self.cleanup_markers(&op)?;
        Ok(op)
    }
    pub fn public(op: &Operation) -> Value {
        json!({"operation_id":op.id,"mode":"clone","phase":op.phase,"uid":op.uid,"target_uid":op.target_uid,
            "sessions":op.plan.group.members.iter().map(|m|json!({"uid":m.uid,"sid":m.sid,"title":m.title,"agent":m.agent,"source":m.source,
                "cwd":m.cwd,"file_count":op.plan.files.iter().filter(|f|f.source==m.path).count(),
                "bytes":op.plan.files.iter().filter(|f|f.source==m.path).map(|f|f.bytes).sum::<u64>(),
                "relations":op.plan.group.edges.iter().filter(|e|e.from==m.uid||e.to==m.uid).map(|e|e.kind.as_str()).collect::<BTreeSet<_>>()
            })).collect::<Vec<_>>(),
            "session_count":op.plan.identities.threads.len(),"file_count":op.plan.files.len(),"bytes":op.plan.files.iter().map(|f|f.bytes).sum::<u64>(),"error":op.error,
            "warnings":["原会话保留；新旧会话共用工作目录和外部工具。"]})
    }
}
