//! Durable same-node clone transaction. Plans contain server-derived paths only;
//! clients submit an opaque operation ID. Recovery never overwrites changed data.
#[path = "journal.rs"]
mod journal;
#[path = "prefix.rs"]
mod prefix;
#[path = "tool_requirements.rs"]
mod tool_requirements;
use super::{
    TransferError,
    codex::{self, ClonePlan, StagedClone},
    files, group,
    native::{self, Native},
};
use crate::{
    metadata::MetadataStore,
    sessions::{SessionRoots, SessionSnapshot, SessionStore},
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
    #[serde(default)]
    pub metadata_replaced: BTreeMap<String, Value>,
    #[serde(default)]
    pub dynamic_tools: Option<BTreeSet<String>>,
    #[serde(default)]
    pub full_group: Option<group::Group>,
    #[serde(default)]
    pub file_plan: Option<files::Plan>,
    #[serde(default)]
    pub file_publications: Vec<Publication>,
    #[serde(default)]
    pub incoming_digest: Option<String>,
    #[serde(default)]
    pub export_lease_until: u64,
    #[serde(default)]
    pub reused_files: BTreeSet<PathBuf>,
    #[serde(default)]
    pub replaced_files: BTreeMap<PathBuf, prefix::Original>,
    #[serde(default)]
    pub native_before: Option<Native>,
    #[serde(default)]
    pub moving: bool,
    #[serde(default)]
    pub storage_probes: BTreeMap<String, super::environment::StorageProbe>,
    #[serde(default)]
    pub reclaimed_by: BTreeMap<String, String>,
    #[serde(default)]
    pub ownership_sequence: u64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Publication {
    pub source: PathBuf,
    pub staging: PathBuf,
    pub target: PathBuf,
    pub sha256: String,
    pub symlink: bool,
}
impl Operation {
    pub fn new_ids(&self) -> bool {
        self.plan.mode == codex::Mode::Clone
    }
    pub fn group(&self) -> &group::Group {
        self.full_group.as_ref().unwrap_or(&self.plan.group)
    }
    pub fn mapped_session(&self, source: &str, id: &str) -> Option<&String> {
        if source == "codex" {
            self.plan.identities.threads.get(id)
        } else {
            self.file_plan
                .as_ref()?
                .sessions
                .get(&format!("{source}:{id}"))
        }
    }
}
pub struct TransferService {
    inventory: SessionStore,
    pub(super) references: super::references::Cache,
    relationship_refresh: std::sync::Mutex<()>,
    journals: journal::Cache,
    pub directory: PathBuf,
    pub roots: SessionRoots,
    pub home: PathBuf,
    pub locks: super::coordination::Locks,
    pub interrupts: super::coordination::Interrupts,
    pub(super) foreground: std::sync::Mutex<BTreeMap<String, std::time::Instant>>,
    pub metadata: Option<Arc<MetadataStore>>,
}
pub fn hash(path: &Path) -> Result<String, TransferError> {
    let mut digest = Sha256::new();
    if fs::symlink_metadata(path)?.is_symlink() {
        digest.update(fs::read_link(path)?.to_string_lossy().as_bytes());
    } else {
        use std::io::Read;
        let mut file = fs::File::open(path)?;
        let mut buffer = [0u8; 65536];
        loop {
            super::coordination::check()?;
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}
pub fn uid(path: &Path) -> String {
    format!(
        "codex:{}",
        &format!("{:x}", Sha1::digest(path.to_string_lossy().as_bytes()))[..16]
    )
}
pub(super) fn persist(path: &Path, value: &impl Serialize) -> Result<(), TransferError> {
    let temp = path.with_extension("tmp");
    let mut file = std::io::BufWriter::new(fs::File::create(&temp)?);
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.flush()?;
    file.get_ref().sync_all()?;
    fs::rename(temp, path)?;
    fs::File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
impl TransferService {
    pub fn operation_keys(&self, op: &Operation) -> Vec<String> {
        let mut keys = vec![format!("operation:{}", op.id)];
        for member in &op.group().members {
            keys.push(format!("session:{}", member.uid));
            if let Some(staged) = &op.staged
                && let Ok(uid) = self.member_target_uid(op, member, staged)
            {
                keys.push(format!("session:{uid}"));
            }
        }
        keys
    }
    pub async fn operation_guard(
        &self,
        id: &str,
    ) -> Result<Vec<tokio::sync::OwnedMutexGuard<()>>, TransferError> {
        let op = self.load(id)?;
        Ok(self.locks.acquire(self.operation_keys(&op)).await)
    }
    pub async fn session_guard(&self, uid: &str) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
        self.locks.acquire(vec![format!("session:{uid}")]).await
    }
    pub fn open(
        directory: PathBuf,
        mut roots: SessionRoots,
        metadata: Option<Arc<MetadataStore>>,
    ) -> Result<Self, TransferError> {
        let home = roots
            .codex
            .as_ref()
            .map(|root| {
                if root
                    .file_name()
                    .is_some_and(|s| s == "sessions" || s == "archived_sessions")
                {
                    root.parent().unwrap().to_owned()
                } else {
                    root.clone()
                }
            })
            .unwrap_or_else(|| directory.join("unconfigured-codex"));
        if roots.codex.is_some() {
            roots.codex = Some(home.clone());
        }
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let service = Self {
            inventory: SessionStore::with_metadata(roots.clone(), metadata.clone()),
            references: super::references::Cache::open(directory.join("relationships-v1.json")),
            relationship_refresh: Default::default(),
            journals: Default::default(),
            directory,
            roots,
            home,
            locks: Default::default(),
            interrupts: Default::default(),
            foreground: Default::default(),
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
                let name = path.file_name().unwrap().to_string_lossy();
                let id = name.strip_prefix("incoming-").unwrap_or(&name);
                if id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
                    fs::remove_dir_all(path)?;
                }
                continue;
            }
            let mut operation: Operation = serde_json::from_slice(&fs::read(file)?)?;
            if operation.phase == "aborted" {
                service.cleanup_staging(&operation)?;
                continue;
            }
            if operation.phase == "aborting"
                && !operation.moving
                && operation.incoming_digest.is_none()
            {
                if let Err(error) = service.abort_local(&operation.id) {
                    operation.error = Some(error.message);
                    service.save(&operation)?;
                }
                continue;
            }
            if operation.phase == "aborting" && operation.incoming_digest.is_some() {
                // The source already durably rejected future ownership switches.
                if let Err(error) = service.abort_target(&operation.id) {
                    operation.error = Some(error.message);
                    service.save(&operation)?;
                }
                continue;
            }
            if operation.phase == "complete" {
                service.cleanup_markers(&operation)?;
                service.reclaim_prior_moves(&operation)?;
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
    pub(super) fn save(&self, op: &Operation) -> Result<(), TransferError> {
        persist(&self.directory.join(&op.id).join("operation.json"), op)
    }
    pub fn store(&self) -> &SessionStore {
        &self.inventory
    }
    pub fn inventory_snapshot(&self) -> Result<SessionSnapshot, TransferError> {
        let started = std::time::Instant::now();
        let result = self
            .store()
            .search_snapshot()
            .map_err(|e| TransferError::new("move_inventory", e.message));
        eprintln!(
            "sessiondock transfer inventory: inventory_ms={}",
            started.elapsed().as_millis()
        );
        result
    }
    fn group(&self, selected: &str) -> Result<group::Group, TransferError> {
        self.group_in(selected, &self.inventory_snapshot()?)
    }
    fn group_in(
        &self,
        selected: &str,
        snapshot: &SessionSnapshot,
    ) -> Result<group::Group, TransferError> {
        // Discover and refresh relationships on demand, including reverse links
        // from histories outside the selected group. No background warm-up is needed.
        // Only the relationship inventory is serialized; page, composer and
        // history readers use their own store and never wait on this lock.
        let _guard = self
            .relationship_refresh
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let started = std::time::Instant::now();
        let result = group::derive_cached(snapshot, selected, &self.references);
        eprintln!(
            "sessiondock transfer group: relationships_ms={}",
            started.elapsed().as_millis()
        );
        result
    }
    pub fn locked(&self, uid: &str) -> Result<bool, TransferError> {
        Ok(self.journal_summaries()?.iter().any(|op| {
            op.locked_uids.contains(uid)
                && (op.phase != "exporting" || op.export_lease_until > super::bundle::now())
        }))
    }
    pub fn plan(&self, selected: &str) -> Result<Operation, TransferError> {
        self.plan_copy(selected, true, &self.inventory_snapshot()?)
    }
    pub fn plan_copy(
        &self,
        selected: &str,
        new_ids: bool,
        snapshot: &SessionSnapshot,
    ) -> Result<Operation, TransferError> {
        if self.locked(selected)? {
            return Err(TransferError::new(
                "move_recovery_required",
                "会话组有尚未恢复的复制操作",
            ));
        }
        let group = self.group_in(selected, snapshot)?;
        let mut codex_group = group.clone();
        codex_group.members.retain(|m| m.source == "codex");
        let mut file_group = group.clone();
        file_group.members.retain(|m| m.source != "codex");
        let file_plan = if file_group.members.is_empty() {
            None
        } else {
            Some(files::Plan::build(file_group, new_ids)?)
        };
        let mut plan = codex::plan(
            codex_group,
            if new_ids {
                codex::Mode::Clone
            } else {
                codex::Mode::Move
            },
        )?;
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
        for member in &group.members {
            if !member.cwd.is_empty() && !Path::new(&member.cwd).is_dir() {
                return Err(TransferError::new(
                    "move_cwd_missing",
                    "会话的工作目录不存在",
                ));
            }
        }
        let native = if plan.files.is_empty() {
            Native::default()
        } else {
            native::capture(&self.home, &plan)?
        };
        native::extend_identities(&native, &mut plan)?;
        let dynamic_tools = tool_requirements::collect(&plan, &native)?;
        let id = codex::uuid()?;
        let directory = self.directory.join(&id);
        fs::create_dir(&directory)?;
        let mut planning = super::cleanup::PlanningDirectory(directory.clone(), false);
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
            metadata_replaced: BTreeMap::new(),
            dynamic_tools: Some(dynamic_tools),
            full_group: Some(group),
            file_plan,
            file_publications: Vec::new(),
            incoming_digest: None,
            export_lease_until: 0,
            reused_files: BTreeSet::new(),
            replaced_files: BTreeMap::new(),
            native_before: None,
            moving: false,
            storage_probes: BTreeMap::new(),
            reclaimed_by: BTreeMap::new(),
            ownership_sequence: 0,
        };
        let selected_file = op
            .group()
            .members
            .iter()
            .find(|m| m.uid == selected)
            .unwrap();
        op.target_uid = Some(self.planned_target_uid(&op, selected_file)?);
        if let Some(metadata) = &self.metadata {
            let snapshot = metadata
                .snapshot()
                .map_err(|e| TransferError::new(e.code, e.message))?;
            let members = op.group().members.clone();
            for member in &members {
                let before = snapshot.row(&member.uid);
                op.metadata_before
                    .insert(member.uid.clone(), before.clone());
                let mut after = serde_json::Map::new();
                for key in [
                    "starred",
                    "starred_at",
                    "group",
                    "fork_parent_visible",
                    "nest_initialized",
                ] {
                    if let Some(v) = before.get(key) {
                        after.insert(key.into(), v.clone());
                    }
                }
                if let Some(mut relation) = before.get("nest_parent").cloned() {
                    if relation["node_id"].is_null()
                        && let Some(mapped) = relation["sid"].as_str().and_then(|id| {
                            op.mapped_session(relation["source"].as_str().unwrap_or(""), id)
                        })
                    {
                        relation["sid"] = mapped.clone().into();
                    }
                    after.insert("nest_parent".into(), relation);
                }
                if new_ids && !after.is_empty() {
                    after.insert("clone_operation".into(), op.id.clone().into());
                }
                op.metadata_after
                    .insert(self.planned_target_uid(&op, member)?, after.into());
            }
        }
        self.save(&op)?;
        planning.1 = true;
        self.touch(&op.id);
        Ok(op)
    }
    /// Preparation belongs to execution, never to opening the preview dialog.
    /// Persist only complete staging; interrupted preparation is rebuilt from
    /// the same confirmed identity map on retry.
    pub(super) fn prepare(&self, op: &mut Operation) -> Result<(), TransferError> {
        if op.staged.is_some() {
            return Ok(());
        }
        let directory = self.directory.join(&op.id);
        for name in ["staging", "staging-files"] {
            let path = directory.join(name);
            if path.exists() {
                fs::remove_dir_all(path)?;
            }
        }
        op.file_publications.clear();
        let staged = codex::stage(&op.plan, &directory.join("staging"))?;
        let rewritten = native::rewrite(&op.native, &op.plan, &staged, &self.home)?;
        if op.new_ids() {
            native::preflight(&rewritten)?;
        }
        if let Some(files) = &op.file_plan {
            let staging = directory.join("staging-files");
            files.stage(&staging)?;
            for file in &files.files {
                let stage = staging.join(&file.provider).join(&file.target);
                op.file_publications.push(Publication {
                    source: file.source.clone(),
                    target: files.roots[&file.provider].join(&file.target),
                    sha256: hash(&stage)?,
                    symlink: file.format == "symlink",
                    staging: stage,
                });
            }
        }
        op.rewritten = Some(rewritten);
        op.staged = Some(staged);
        self.save(op)
    }
    fn planned_target_uid(
        &self,
        op: &Operation,
        member: &group::Member,
    ) -> Result<String, TransferError> {
        if member.source == "codex" {
            let file = op
                .plan
                .files
                .iter()
                .find(|f| f.source == member.path)
                .ok_or_else(|| TransferError::new("move_identity", "缺少目标历史"))?;
            Ok(uid(&self
                .home
                .join(codex::relative(file, &op.plan.identities)?)))
        } else {
            Ok(crate::sessions::uid_for(
                &member.source,
                &op.file_plan.as_ref().unwrap().member_target(member)?,
            ))
        }
    }
    pub fn recheck(&self, op: &Operation) -> Result<(), TransferError> {
        self.recheck_in(op, &self.inventory_snapshot()?)
    }
    pub(super) fn recheck_in(
        &self,
        op: &Operation,
        snapshot: &SessionSnapshot,
    ) -> Result<(), TransferError> {
        let current = self.group_in(&op.uid, snapshot)?;
        let old: BTreeSet<_> = op
            .group()
            .members
            .iter()
            .map(|m| (&m.uid, &m.sid))
            .collect();
        let now: BTreeSet<_> = current.members.iter().map(|m| (&m.uid, &m.sid)).collect();
        if old != now || !current.blockers.is_empty() || current.edges != op.group().edges {
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
        if let Some(files) = &op.file_plan {
            let now = files::Plan::build(files.group.clone(), false)?;
            let before = files
                .files
                .iter()
                .map(|f| (&f.source, &f.sha256))
                .collect::<BTreeMap<_, _>>();
            let after = now
                .files
                .iter()
                .map(|f| (&f.source, &f.sha256))
                .collect::<BTreeMap<_, _>>();
            if before != after {
                return Err(TransferError::new("move_plan_stale", "会话附属文件已变化"));
            }
        }
        if !op.plan.files.is_empty() && native::capture(&self.home, &op.plan)? != op.native {
            return Err(TransferError::new(
                "move_plan_stale",
                "原生会话元数据已变化，请重新查看复制清单",
            ));
        }
        Ok(())
    }
    pub(crate) fn member_target_uid(
        &self,
        op: &Operation,
        member: &group::Member,
        staged: &StagedClone,
    ) -> Result<String, TransferError> {
        if member.source == "codex" {
            let file = staged
                .files
                .iter()
                .find(|f| f.source == member.path)
                .ok_or_else(|| TransferError::new("move_identity", "缺少目标历史"))?;
            Ok(uid(&self.home.join(&file.relative)))
        } else {
            let plan = op.file_plan.as_ref().unwrap();
            let path = plan.member_target(member)?;
            Ok(crate::sessions::uid_for(&member.source, &path))
        }
    }
    pub(super) fn publications(&self, op: &Operation) -> Vec<Publication> {
        let mut result = op.file_publications.clone();
        if let Some(staged) = &op.staged {
            result.extend(staged.files.iter().map(|f| {
                Publication {
                    source: f.source.clone(),
                    staging: self
                        .directory
                        .join(&op.id)
                        .join("staging")
                        .join(&f.relative),
                    target: self.home.join(&f.relative),
                    sha256: f.sha256.clone(),
                    symlink: false,
                }
            }));
        }
        result
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
            if let (Ok(a), Ok(b)) = (
                fs::symlink_metadata(target),
                fs::symlink_metadata(Self::marker(id, target)),
            ) {
                return a.dev() == b.dev() && a.ino() == b.ino();
            }
        }
        false
    }
    /// Keep the small durable decision, remove only private transfer payloads.
    pub(super) fn cleanup_staging(&self, op: &Operation) -> Result<(), TransferError> {
        let directory = self.directory.join(&op.id);
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            if entry.file_name() == "operation.json" {
                continue;
            }
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
        fs::File::open(directory)?.sync_all()?;
        Ok(())
    }
    pub(super) fn cleanup_markers(&self, op: &Operation) -> Result<(), TransferError> {
        for file in self.publications(op) {
            let marker = Self::marker(&op.id, &file.target);
            if let Some(old) = op.replaced_files.get(&file.target) {
                for (path, expected) in [
                    (marker.with_extension("publish"), &file.sha256),
                    (marker.with_extension("restore"), &old.sha256),
                ] {
                    if let Ok(metadata) = fs::symlink_metadata(&path) {
                        if !metadata.is_file() || hash(&path)? != *expected {
                            return Err(TransferError::new(
                                "move_recovery_required",
                                "目标合并临时文件已变化，保留现场",
                            ));
                        }
                        fs::remove_file(path)?;
                        fs::File::open(file.target.parent().unwrap())?.sync_all()?;
                    }
                }
            }
            if fs::symlink_metadata(&marker).is_ok() {
                fs::remove_file(marker)?;
                fs::File::open(file.target.parent().unwrap())?.sync_all()?;
            }
        }
        Ok(())
    }
    pub(super) fn rollback(&self, op: &Operation) -> Result<(), TransferError> {
        let files: Vec<_> = self
            .publications(op)
            .into_iter()
            .filter(|f| !op.reused_files.contains(&f.target))
            .collect();
        for file in &files {
            if let Some(old) = op.replaced_files.get(&file.target) {
                prefix::check_restore(op, file, old)?;
                continue;
            }
            if fs::symlink_metadata(&file.target).is_ok()
                && (!Self::owns(&op.id, &file.target) || hash(&file.target)? != file.sha256)
            {
                return Err(TransferError::new(
                    "move_recovery_required",
                    "克隆文件已变化或不属于本次操作，保留现场等待恢复",
                ));
            }
        }
        if let Some(metadata) = &self.metadata {
            metadata
                .transfer_prefix_rows(&op.metadata_after, &op.metadata_replaced, true)
                .map_err(|e| TransferError::new(e.code, e.message))?;
        }
        if let Some(native) = &op.rewritten {
            native::rollback(native, &op.id)?;
        }
        for file in files {
            if let Some(old) = op.replaced_files.get(&file.target) {
                prefix::restore(op, &file, old)?;
                continue;
            }
            if fs::symlink_metadata(&file.target).is_ok() {
                fs::remove_file(&file.target)?;
                fs::File::open(file.target.parent().unwrap())?.sync_all()?;
            }
        }
        self.cleanup_markers(op)
    }
    pub fn execute(
        &self,
        mut op: Operation,
        snapshot: Option<&SessionSnapshot>,
    ) -> Result<Operation, TransferError> {
        if matches!(op.phase.as_str(), "complete" | "ready") {
            return Ok(op);
        }
        let _scope = super::coordination::Scope::enter(self.interrupts.flag(&op.id));
        super::coordination::check()?;
        if op.phase == "failed" && (!op.moving || op.incoming_digest.is_some()) {
            // A failed phase is written only after compensation succeeded.
            // Retry the same identity map; all preflight checks still run below.
            op.phase = "planned".into();
            op.error = None;
            self.save(&op)?;
        }
        if op.phase != "planned" {
            return Err(TransferError::new(
                "move_recovery_required",
                op.error
                    .unwrap_or_else(|| "复制操作需要恢复或重新发起".into()),
            ));
        }
        if op.incoming_digest.is_none() {
            if op.moving || !op.new_ids() {
                return Err(TransferError::new(
                    "move_conflict",
                    "保留身份复制需要另一台机器",
                ));
            }
            if let Some(snapshot) = snapshot {
                self.recheck_in(&op, snapshot)?;
            } else {
                self.recheck(&op)?;
            }
            self.prepare(&mut op)?;
        } else {
            let snapshots: Vec<super::environment::Snapshot> = serde_json::from_slice(&fs::read(
                self.directory
                    .join(&op.id)
                    .join("incoming-environment.json"),
            )?)?;
            for snapshot in snapshots {
                snapshot.recheck()?;
            }
        }
        let mut extended_metadata = BTreeSet::new();
        if !op.new_ids() {
            let (extended, metadata) = prefix::prepare(self, &mut op)?;
            extended_metadata = metadata;
            op.native_before = Some(native::preflight_prefix(
                op.rewritten.as_ref().unwrap(),
                &extended,
            )?);
        } else {
            native::preflight_copy(op.rewritten.as_ref().unwrap(), false)?;
        }
        op.reused_files.clear();
        for file in self.publications(&op) {
            if fs::symlink_metadata(&file.target).is_ok() {
                if op.replaced_files.contains_key(&file.target) {
                    continue;
                }
                if !op.new_ids()
                    && hash(&file.target)? == file.sha256
                    && fs::symlink_metadata(&file.target)?.is_symlink() == file.symlink
                {
                    #[cfg(unix)]
                    if !file.symlink {
                        use std::os::unix::fs::PermissionsExt;
                        if fs::metadata(&file.target)?.permissions().mode() & 0o111
                            != fs::metadata(&file.staging)?.permissions().mode() & 0o111
                        {
                            return Err(TransferError::new(
                                "move_conflict",
                                "目标文件执行权限不同",
                            ));
                        }
                    }
                    op.reused_files.insert(file.target);
                    continue;
                }
                return Err(TransferError::new("move_conflict", "目标文件已存在"));
            }
        }
        if !op.new_ids()
            && let Some(metadata) = &self.metadata
        {
            let current = metadata
                .snapshot()
                .map_err(|e| TransferError::new(e.code, e.message))?;
            op.metadata_replaced.clear();
            let portable = [
                "starred",
                "starred_at",
                "group",
                "fork_parent_visible",
                "nest_parent",
                "nest_initialized",
            ];
            for (uid, row) in &mut op.metadata_after {
                let before = current.row(uid);
                let mut merged = before.as_object().cloned().unwrap_or_default();
                merged.remove("clone_operation");
                for key in portable {
                    merged.remove(key);
                    if let Some(value) = row.get(key) {
                        merged.insert(key.into(), value.clone());
                    }
                }
                let mut comparable = before.clone();
                comparable
                    .as_object_mut()
                    .unwrap()
                    .remove("clone_operation");
                if Value::Object(merged.clone()) == comparable {
                    *row = json!({}); // Already owned by the destination; no receipt.
                    continue;
                }
                if before != json!({}) {
                    if !extended_metadata.contains(uid) {
                        return Err(TransferError::new(
                            "move_conflict",
                            "目标会话显示设置不同且历史没有延长",
                        ));
                    }
                    if !before["nest_parent"].is_null()
                        && before["nest_parent"] != row["nest_parent"]
                    {
                        return Err(TransferError::new("move_conflict", "目标会话附属关系不同"));
                    }
                    op.metadata_replaced.insert(uid.clone(), before);
                }
                if !merged.is_empty() || op.metadata_replaced.contains_key(uid) {
                    merged.insert("clone_operation".into(), op.id.clone().into());
                }
                *row = merged.into();
            }
            op.metadata_after.retain(|_, row| row != &json!({}));
        }
        op.phase = "publishing".into();
        self.save(&op)?;
        let result = (|| {
            for file in self.publications(&op) {
                if op.reused_files.contains(&file.target) {
                    if hash(&file.target)? != file.sha256 {
                        return Err(TransferError::new("move_plan_stale", "目标复用文件已变化"));
                    }
                    continue;
                }
                let target = file.target;
                let parent = target.parent().unwrap();
                fs::create_dir_all(parent)?;
                let staging = file.staging;
                if hash(&staging)? != file.sha256 {
                    return Err(TransferError::new("move_plan_stale", "复制暂存数据已变化"));
                }
                // Copy to an unindexed sibling then link with no-replace semantics;
                // publication is atomic even when state and CLI roots differ in FS.
                let temp = Self::marker(&op.id, &target);
                if file.symlink {
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(fs::read_link(&staging)?, &temp)?;
                    #[cfg(not(unix))]
                    return Err(TransferError::new(
                        "move_platform",
                        "此平台不支持迁移符号链接",
                    ));
                } else {
                    let mut out = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&temp)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let executable = fs::metadata(&staging)?.permissions().mode() & 0o111;
                        out.set_permissions(fs::Permissions::from_mode(0o600 | executable))?;
                    }
                    super::coordination::copy(&mut fs::File::open(staging)?, &mut out)?;
                    out.sync_all()?;
                }
                if let Some(old) = op.replaced_files.get(&target) {
                    if hash(&target)? != old.sha256 {
                        return Err(TransferError::new(
                            "move_plan_stale",
                            "目标前缀在发布前发生变化",
                        ));
                    }
                    // Keep the marker as proof of ownership for compensation.
                    let publication = temp.with_extension("publish");
                    fs::hard_link(&temp, &publication)?;
                    fs::rename(&publication, &target)?;
                } else {
                    fs::hard_link(&temp, &target)?;
                }
                fs::File::open(parent)?.sync_all()?;
            }
            native::insert_with_prefix(
                op.rewritten.as_ref().unwrap(),
                &op.id,
                !op.new_ids(),
                op.native_before.as_ref(),
            )?;
            if let Some(metadata) = &self.metadata {
                metadata
                    .transfer_prefix_rows(&op.metadata_after, &op.metadata_replaced, false)
                    .map_err(|e| TransferError::new(e.code, e.message))?;
            }
            op.phase = "verifying".into();
            self.save(&op)?;
            // Incoming source snapshots are rechecked on the source node by
            // the Hub. The receiver need not contain any original identities.
            if op.incoming_digest.is_none() {
                self.recheck(&op)?;
            }
            let group = self.group(op.target_uid.as_ref().unwrap())?;
            if !group.blockers.is_empty() || group.members.len() != op.group().members.len() {
                return Err(TransferError::new("move_verify", "克隆后的历史关系不完整"));
            }
            let wanted: BTreeSet<_> = op
                .group()
                .members
                .iter()
                .filter_map(|m| {
                    op.mapped_session(&m.source, &m.sid)
                        .map(|id| (m.source.clone(), id.clone()))
                })
                .collect();
            if group
                .members
                .iter()
                .any(|m| !wanted.contains(&(m.source.clone(), m.sid.clone())))
            {
                return Err(TransferError::new("move_verify", "克隆仍引用源组身份"));
            }
            super::coordination::check()?;
            op.phase = if op.moving { "ready" } else { "complete" }.into();
            if op.phase == "complete" {
                op.ownership_sequence = self.next_ownership_sequence()?;
            }
            self.save(&op)?;
            Ok(())
        })();
        if let Err(error) = result {
            op.error = Some(error.message.clone());
            op.phase = "rollback_required".into();
            self.save(&op)?;
            // Compensation must not inherit the interrupted worker's flag.
            drop(_scope);
            if let Err(recovery) = self.rollback(&op) {
                op.error = Some(format!("{}；回滚：{}", error.message, recovery.message));
                self.save(&op)?;
                return Err(recovery);
            }
            op.phase = "failed".into();
            self.save(&op)?;
            return Err(error);
        }
        if op.phase == "complete" {
            self.cleanup_markers(&op)?;
            self.reclaim_prior_moves(&op)?;
        }
        Ok(op)
    }
    pub fn public(op: &Operation) -> Value {
        json!({"confirm_mode":true,"reserve_manifest":true,"operation_id":op.id,"mode":if op.moving {"move"} else {"clone"},"new_ids":op.new_ids(),"phase":op.phase,"uid":op.uid,"target_uid":op.target_uid,
            "dynamic_tools":op.dynamic_tools,
            "sessions":op.group().members.iter().map(|m|json!({"uid":m.uid,"sid":m.sid,"title":m.title,"agent":m.agent,"source":m.source,
                "cwd":m.cwd,"file_count":op.plan.files.iter().filter(|f|f.source==m.path).count()+op.file_plan.as_ref().map_or(0,|p|p.files.iter().filter(|f|f.owner==m.uid).count()),
                "bytes":op.plan.files.iter().filter(|f|f.source==m.path).map(|f|f.bytes).sum::<u64>()+op.file_plan.as_ref().map_or(0,|p|p.files.iter().filter(|f|f.owner==m.uid).map(|f|f.bytes).sum::<u64>()),
                "relations":op.group().edges.iter().filter(|e|e.from==m.uid||e.to==m.uid).map(|e|e.kind.as_str()).collect::<BTreeSet<_>>()
            })).collect::<Vec<_>>(),
            "session_count":op.group().members.iter().map(|m|(&m.source,&m.sid)).collect::<BTreeSet<_>>().len(),"file_count":op.plan.files.len()+op.file_plan.as_ref().map_or(0,|p|p.files.len()),"bytes":op.plan.files.iter().map(|f|f.bytes).sum::<u64>()+op.file_plan.as_ref().map_or(0,|p|p.files.iter().map(|f|f.bytes).sum::<u64>()),"error":op.error,
            "warnings":if op.moving { Vec::<&str>::new() } else { vec!["原会话保留；新旧会话共用工作目录和外部工具。"] }})
    }
}
