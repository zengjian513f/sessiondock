//! A manifest-first tar stream. Archive paths are numbered slots, never native
//! absolute paths. Only validated, configured native roots can receive files.
#[path = "dependencies.rs"]
mod dependencies;

use super::{
    TransferError,
    environment::Snapshot,
    native,
    service::{Operation, TransferService, hash},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct File {
    pub provider: String,
    pub relative: PathBuf,
    pub bytes: u64,
    pub sha256: String,
    pub symlink: bool,
    pub executable: u32,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub roots: BTreeMap<String, PathBuf>,
    pub operation: Operation,
    pub environment: Vec<Snapshot>,
    pub files: Vec<File>,
}
fn invalid(message: &str) -> TransferError {
    TransferError::new("move_format", message)
}
pub(super) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn relative(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)))
}
fn id_valid(id: &str) -> bool {
    id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}
fn append<W: Write>(
    tar: &mut tar::Builder<W>,
    name: &str,
    bytes: u64,
    input: impl Read,
) -> Result<(), TransferError> {
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o600);
    header.set_size(bytes);
    header.set_entry_type(tar::EntryType::Regular);
    tar.append_data(&mut header, name, input)?;
    Ok(())
}
fn private_file(path: &Path) -> Result<fs::File, TransferError> {
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
fn private_dir(path: &Path) -> Result<(), TransferError> {
    fs::create_dir(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
impl TransferService {
    pub fn reserve_export(
        &self,
        id: &str,
    ) -> Result<(Operation, crate::sessions::SessionSnapshot), TransferError> {
        let mut op = self.load(id)?;
        if !matches!(op.phase.as_str(), "planned" | "exporting") || op.incoming_digest.is_some() {
            return Err(invalid("此操作不能导出"));
        }
        let snapshot = self.inventory_snapshot()?;
        self.recheck_in(&op, &snapshot)?;
        op.phase = "exporting".into();
        op.export_lease_until = now() + 3600;
        self.save(&op)?;
        Ok((op, snapshot))
    }
    pub fn release_export(&self, id: &str, completed: bool) -> Result<Operation, TransferError> {
        let mut op = self.load(id)?;
        if op.phase == "exported" && completed {
            return Ok(op);
        }
        if !(op.phase == "exporting" || (completed && op.phase == "planned"))
            || op.incoming_digest.is_some()
        {
            return Err(invalid("此操作未在导出"));
        }
        op.phase = if completed { "exported" } else { "planned" }.into();
        op.export_lease_until = 0;
        self.save(&op)?;
        Ok(op)
    }
    pub fn bundle_roots(&self, op: &Operation) -> Result<BTreeMap<String, PathBuf>, TransferError> {
        let mut roots = BTreeMap::new();
        for member in &op.group().members {
            let root = match member.source.as_str() {
                "codex" => self.roots.codex.as_ref().map(|_| self.home.clone()),
                "claude" => self.roots.claude.as_ref().map(|p| {
                    if p.file_name().is_some_and(|n| n == "projects") {
                        p.parent().unwrap().to_owned()
                    } else {
                        p.clone()
                    }
                }),
                "grok" => self.roots.grok.clone(),
                _ => None,
            }
            .ok_or_else(|| {
                TransferError::new("move_group_unsupported", "目标未配置本组所需的会话来源")
            })?;
            roots.insert(member.source.clone(), root);
        }
        Ok(roots)
    }
    /// Build an immutable export after source rechecks. Rewrite happens in the
    /// existing private stager; source histories are never rewritten in place.
    pub fn bundle_manifest(&self, op: &Operation) -> Result<Manifest, TransferError> {
        self.build_manifest(op, true)
    }
    pub(crate) fn build_manifest(
        &self,
        op: &Operation,
        recheck: bool,
    ) -> Result<Manifest, TransferError> {
        if !cfg!(target_os = "linux") {
            return Err(TransferError::new(
                "move_platform",
                "跨机器迁移目前只支持 Linux",
            ));
        }
        if !matches!(op.phase.as_str(), "planned" | "exporting") || op.incoming_digest.is_some() {
            return Err(invalid("此操作不能作为迁移源"));
        }
        if recheck {
            self.recheck(op)?;
        }
        let mut prepared = op.clone();
        self.prepare(&mut prepared)?;
        let op = &prepared;
        let roots = self.bundle_roots(op)?;
        let cache = self.directory.join(&op.id).join("export-manifest.json");
        if cache.is_file() {
            let manifest: Manifest = serde_json::from_slice(&fs::read(&cache)?)?;
            for snapshot in &manifest.environment {
                snapshot.recheck()?;
            }
            return Ok(manifest);
        }
        let dependencies = dependencies::collect(self, op)?;
        let mut environment = Vec::new();
        for cwd in op
            .group()
            .members
            .iter()
            .map(|m| m.cwd.as_str())
            .filter(|s| !s.is_empty())
            .collect::<BTreeSet<_>>()
        {
            let external = dependencies
                .get(Path::new(cwd))
                .map(|paths| paths.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            environment.push(Snapshot::capture(Path::new(cwd), &external)?);
        }
        let publications = self.publications(op);
        let mut files = Vec::new();
        for (index, file) in publications.iter().enumerate() {
            let provider = if index < op.file_publications.len() {
                op.file_plan
                    .as_ref()
                    .ok_or_else(|| invalid("缺少文件计划"))?
                    .files[index]
                    .provider
                    .clone()
            } else {
                "codex".into()
            };
            let path = file
                .target
                .strip_prefix(&roots[&provider])
                .map_err(|_| invalid("目标文件不在配置根内"))?
                .to_owned();
            if !relative(&path) {
                return Err(invalid("无效的目标相对路径"));
            }
            let meta = fs::symlink_metadata(&file.staging)?;
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                meta.permissions().mode() & 0o111
            };
            #[cfg(not(unix))]
            let executable = 0;
            let bytes = if file.symlink {
                fs::read_link(&file.staging)?.to_string_lossy().len() as u64
            } else {
                meta.len()
            };
            if hash(&file.staging)? != file.sha256 {
                return Err(TransferError::new("move_plan_stale", "暂存文件已变化"));
            }
            files.push(File {
                provider,
                relative: path,
                bytes,
                sha256: file.sha256.clone(),
                symlink: file.symlink,
                executable,
            });
        }
        let mut exported = op.clone();
        exported.phase = "planned".into();
        exported.export_lease_until = 0;
        let manifest = Manifest {
            version: 1,
            roots,
            operation: exported,
            environment,
            files,
        };
        super::service::persist(&cache, &manifest)?;
        Ok(manifest)
    }
    pub fn export_bundle(
        &self,
        op: &Operation,
        destination: &Path,
    ) -> Result<(Manifest, crate::sessions::SessionSnapshot), TransferError> {
        // The API holds the operation gate and has just reserved/rechecked
        // this export. Keep the post-archive recheck below, instead of scanning
        // the whole source group twice before reading any archive bytes.
        let manifest = self.build_manifest(op, false)?;
        let publications = self.publications(&manifest.operation);
        let raw = serde_json::to_vec(&manifest)?;
        let mut archive = tar::Builder::new(private_file(destination)?);
        let result = (|| {
            append(
                &mut archive,
                "manifest.json",
                raw.len() as u64,
                raw.as_slice(),
            )?;
            for (i, (file, wire)) in publications.iter().zip(&manifest.files).enumerate() {
                let name = format!("files/{i}");
                if file.symlink {
                    let link = fs::read_link(&file.staging)?
                        .to_string_lossy()
                        .as_bytes()
                        .to_vec();
                    append(&mut archive, &name, wire.bytes, link.as_slice())?;
                } else {
                    append(
                        &mut archive,
                        &name,
                        wire.bytes,
                        fs::File::open(&file.staging)?,
                    )?;
                }
                if hash(&file.staging)? != wire.sha256 {
                    return Err(TransferError::new(
                        "move_plan_stale",
                        "导出期间暂存文件发生变化",
                    ));
                }
            }
            archive.finish()?;
            archive.get_mut().sync_all()?;
            let snapshot = self.inventory_snapshot()?;
            self.recheck_in(op, &snapshot)?;
            for snapshot in &manifest.environment {
                snapshot.recheck()?;
            }
            Ok::<_, TransferError>(snapshot)
        })();
        drop(archive);
        match result {
            Ok(snapshot) => Ok((manifest, snapshot)),
            Err(error) => {
                let _ = fs::remove_file(destination);
                Err(error)
            }
        }
    }
    pub fn verify_environment(&self, manifest: &Manifest) -> Result<Vec<Snapshot>, TransferError> {
        let source_hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&manifest.environment)?)
        );
        let path = self
            .directory
            .join(format!("environment-{}.json", manifest.operation.id));
        if path.is_file() {
            let (saved_hash, saved): (String, Vec<Snapshot>) =
                serde_json::from_slice(&fs::read(&path)?)?;
            if saved_hash == source_hash && saved.iter().all(|snapshot| snapshot.recheck().is_ok())
            {
                return Ok(saved);
            }
        }
        let mut snapshots = Vec::new();
        for snapshot in &manifest.environment {
            super::coordination::check()?;
            let dependencies = snapshot.dependencies.keys().cloned().collect::<Vec<_>>();
            let target = Snapshot::capture(&snapshot.cwd, &dependencies)?;
            snapshot.compare(&target)?;
            snapshots.push(target);
        }
        super::service::persist(&path, &(source_hash, &snapshots))?;
        Ok(snapshots)
    }
    pub fn validate_bundle(&self, manifest: &Manifest) -> Result<(), TransferError> {
        if !cfg!(target_os = "linux") {
            return Err(TransferError::new(
                "move_platform",
                "跨机器迁移目前只支持 Linux",
            ));
        }
        let op = &manifest.operation;
        if manifest.version != 1
            || !id_valid(&op.id)
            || op.phase != "planned"
            || op.incoming_digest.is_some()
            || !op.reused_files.is_empty()
            || !op.replaced_files.is_empty()
            || op.native_before.is_some()
            || !op.metadata_replaced.is_empty()
            || !op.reclaimed_by.is_empty()
            || op.ownership_sequence != 0
        {
            return Err(invalid("迁移清单或操作标识无效"));
        }
        if self.bundle_roots(op)? != manifest.roots {
            return Err(TransferError::new(
                "move_root_mismatch",
                "两端 CLI 根目录路径不同",
            ));
        }
        if op.moving {
            if op.storage_probes.keys().collect::<Vec<_>>()
                != manifest.roots.keys().collect::<Vec<_>>()
            {
                return Err(invalid("缺少会话存储独立性核对"));
            }
            for (provider, probe) in &op.storage_probes {
                if probe.shared(&manifest.roots[provider])? {
                    return Err(TransferError::new(
                        "move_shared_storage",
                        "两台机器共享会话存储，不能移动文件",
                    ));
                }
            }
        }
        let cwds = op
            .group()
            .members
            .iter()
            .filter(|m| !m.cwd.is_empty())
            .map(|m| PathBuf::from(&m.cwd))
            .collect::<BTreeSet<_>>();
        let checked = manifest
            .environment
            .iter()
            .map(|s| s.cwd.clone())
            .collect::<BTreeSet<_>>();
        if cwds != checked || checked.len() != manifest.environment.len() {
            return Err(invalid("工作目录核对清单不完整"));
        }
        let expected = self.publications(op);
        if manifest.files.len() != expected.len() {
            return Err(invalid("迁移文件清单数量不符"));
        }
        let mut paths = BTreeSet::new();
        for (file, publication) in manifest.files.iter().zip(&expected) {
            let root = manifest
                .roots
                .get(&file.provider)
                .ok_or_else(|| invalid("迁移来源未配置"))?;
            if !relative(&file.relative)
                || root.join(&file.relative) != publication.target
                || file.sha256 != publication.sha256
                || file.symlink != publication.symlink
                || !paths.insert(publication.target.clone())
            {
                return Err(invalid("迁移文件路径或摘要不符"));
            }
        }
        // The receiver validates native paths and schema before any publication.
        let native = op
            .rewritten
            .as_ref()
            .ok_or_else(|| invalid("缺少原生元数据计划"))?;
        for db in &native.databases {
            if db.path.parent() != Some(self.home.as_path())
                || db.path.extension().is_none_or(|e| e != "sqlite")
            {
                return Err(invalid("原生数据库不在配置根内"));
            }
            for table in &db.tables {
                if !matches!(
                    table.name.as_str(),
                    "threads"
                        | "thread_dynamic_tools"
                        | "thread_spawn_edges"
                        | "thread_attachments"
                        | "thread_artifacts"
                        | "thread_turns"
                        | "thread_items"
                        | "thread_history_projection_state"
                        | "thread_realtime_items"
                        | "thread_goals"
                        | "thread_goal_continuation_deferrals"
                        | "projects"
                        | "project_roots"
                        | "thread_sections"
                ) {
                    return Err(invalid("未识别的原生元数据表"));
                }
            }
        }
        let staged = op
            .staged
            .as_ref()
            .ok_or_else(|| invalid("缺少历史暂存计划"))?;
        for f in &staged.files {
            if !relative(&f.relative)
                || !matches!(
                    f.relative
                        .components()
                        .next()
                        .and_then(|c| c.as_os_str().to_str()),
                    Some("sessions" | "archived_sessions")
                )
                || !f
                    .relative
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
            {
                return Err(invalid("历史路径越界"));
            }
        }
        if let Some(files) = &op.file_plan {
            if files
                .roots
                .iter()
                .any(|(provider, root)| manifest.roots.get(provider) != Some(root))
            {
                return Err(invalid("文件根目录不符"));
            }
            if files.files.len() != op.file_publications.len() {
                return Err(invalid("附属文件清单不符"));
            }
            for (file, publication) in files.files.iter().zip(&op.file_publications) {
                if !relative(&file.target) {
                    return Err(invalid("附属文件路径越界"));
                }
                let root = manifest
                    .roots
                    .get(&file.provider)
                    .ok_or_else(|| invalid("文件来源不符"))?;
                if root.join(&file.target) != publication.target {
                    return Err(invalid("附属文件目标不符"));
                }
                let mut allowed = false;
                for member in op
                    .group()
                    .members
                    .iter()
                    .filter(|m| m.source == file.provider)
                {
                    let mapped = files.member_target(member)?;
                    if !mapped.starts_with(root) {
                        return Err(invalid("会话目标越界"));
                    }
                    let id = op
                        .mapped_session(&member.source, &member.sid)
                        .ok_or_else(|| invalid("缺少会话身份"))?;
                    if member.source == "claude" {
                        let filename = mapped.file_name().and_then(|v| v.to_str()).unwrap_or("");
                        if filename != format!("{id}.jsonl")
                            && filename != format!("agent-{id}.jsonl")
                        {
                            return Err(invalid("Claude 会话路径不符"));
                        }
                        allowed |= publication.target == mapped
                            || publication.target == mapped.with_extension("meta.json")
                            || publication.target.starts_with(mapped.with_extension(""))
                            || publication
                                .target
                                .starts_with(root.join("file-history").join(id));
                    } else if member.source == "grok" {
                        if mapped.file_name().and_then(|v| v.to_str()) != Some(id.as_str()) {
                            return Err(invalid("Grok 会话路径不符"));
                        }
                        allowed |= publication.target.starts_with(&mapped);
                    }
                }
                if !allowed {
                    return Err(invalid("附属文件不属于迁移会话"));
                }
            }
        } else if !op.file_publications.is_empty() {
            return Err(invalid("缺少附属文件计划"));
        }
        let targets = op
            .group()
            .members
            .iter()
            .map(|m| self.member_target_uid(op, m, staged))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if op.metadata_after.keys().any(|uid| !targets.contains(uid))
            || !op
                .target_uid
                .as_ref()
                .is_some_and(|uid| targets.contains(uid))
        {
            return Err(invalid("目标会话身份不符"));
        }
        Ok(())
    }
    /// Stream into private numbered files. No tar entry is ever unpacked by its
    /// supplied path, and symlinks are materialized only after content checks.
    pub fn receive_bundle(&self, reader: impl Read) -> Result<Operation, TransferError> {
        let mut archive = tar::Archive::new(reader);
        let mut entries = archive.entries()?;
        let mut first = entries.next().ok_or_else(|| invalid("缺少迁移清单"))??;
        if first.path()?.as_ref() != Path::new("manifest.json")
            || !first.header().entry_type().is_file()
        {
            return Err(invalid("首个成员必须是迁移清单"));
        }
        let mut raw = Vec::new();
        first.read_to_end(&mut raw)?;
        drop(first);
        let manifest: Manifest = serde_json::from_slice(&raw)?;
        let _guard = self
            .locks
            .blocking(self.operation_keys(&manifest.operation));
        let _scope =
            super::coordination::Scope::enter(self.interrupts.flag(&manifest.operation.id));
        super::coordination::check()?;
        self.validate_bundle(&manifest)?;
        let digest = format!("{:x}", Sha256::digest(&raw));
        let directory = self.directory.join(&manifest.operation.id);
        if directory.exists() {
            let previous = self.load(&manifest.operation.id)?;
            if previous.incoming_digest.as_ref() == Some(&digest) {
                return Ok(previous);
            }
            return Err(TransferError::new(
                "move_conflict",
                "目标已存在不同的迁移操作",
            ));
        }
        let target_environment = self.verify_environment(&manifest)?;
        if manifest.operation.new_ids() {
            native::preflight_copy(manifest.operation.rewritten.as_ref().unwrap(), false)?;
        }
        // Preserved identities need the verified incoming bytes before target
        // prefix/rollout proofs and native row comparisons can be completed.
        let scratch = self
            .directory
            .join(format!("incoming-{}", super::codex::uuid()?));
        private_dir(&scratch)?;
        let receiving = scratch.join("receiving");
        private_dir(&receiving)?;
        let result = (|| {
            for (index, file) in manifest.files.iter().enumerate() {
                let mut entry = entries.next().ok_or_else(|| invalid("迁移数据不完整"))??;
                if entry.path()?.as_ref() != Path::new(&format!("files/{index}"))
                    || !entry.header().entry_type().is_file()
                    || entry.size() != file.bytes
                {
                    return Err(invalid("迁移文件成员不符"));
                }
                let mut output = private_file(&receiving.join(index.to_string()))?;
                let mut digest = Sha256::new();
                let mut bytes = 0u64;
                let mut buffer = [0u8; 65536];
                loop {
                    let n = entry.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    output.write_all(&buffer[..n])?;
                    digest.update(&buffer[..n]);
                    bytes += n as u64;
                }
                output.sync_all()?;
                if bytes != file.bytes || format!("{:x}", digest.finalize()) != file.sha256 {
                    return Err(invalid("迁移文件校验失败"));
                }
            }
            if entries.next().is_some() {
                return Err(invalid("迁移包包含未声明的成员"));
            }
            let mut op = manifest.operation.clone();
            op.incoming_digest = Some(digest);
            op.metadata_before.clear();
            for publication in &mut op.file_publications {
                // File-plan target paths were checked against configured roots.
                let file = manifest
                    .files
                    .iter()
                    .find(|f| manifest.roots[&f.provider].join(&f.relative) == publication.target)
                    .ok_or_else(|| invalid("目标文件未声明"))?;
                publication.staging = directory
                    .join("staging-files")
                    .join(&file.provider)
                    .join(&file.relative);
            }
            for (index, (wire, publication)) in manifest
                .files
                .iter()
                .zip(self.publications(&op))
                .enumerate()
            {
                let staged = scratch.join(
                    publication
                        .staging
                        .strip_prefix(&directory)
                        .map_err(|_| invalid("暂存路径越界"))?,
                );
                fs::create_dir_all(staged.parent().unwrap())?;
                let slot = receiving.join(index.to_string());
                if wire.symlink {
                    let link = String::from_utf8(fs::read(&slot)?)
                        .map_err(|_| invalid("链接目标不是 UTF-8"))?;
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(link, &staged)?;
                    #[cfg(not(unix))]
                    return Err(TransferError::new("move_platform", "迁移链接要求 Unix"));
                } else {
                    fs::rename(&slot, &staged)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(
                            &staged,
                            fs::Permissions::from_mode(0o600 | (wire.executable & 0o111)),
                        )?;
                    }
                }
            }
            fs::write(scratch.join("bundle-manifest.json"), &raw)?;
            for snapshot in &target_environment {
                snapshot.recheck()?;
            }
            fs::write(
                scratch.join("incoming-environment.json"),
                serde_json::to_vec(&target_environment)?,
            )?;
            fs::write(scratch.join("operation.json"), serde_json::to_vec(&op)?)?;
            sync_tree(&scratch)?;
            #[cfg(target_os = "linux")]
            rustix::fs::renameat_with(
                rustix::fs::CWD,
                &scratch,
                rustix::fs::CWD,
                &directory,
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(std::io::Error::from)?;
            #[cfg(not(target_os = "linux"))]
            return Err(TransferError::new(
                "move_platform",
                "跨机器迁移目前只支持 Linux",
            ));
            fs::File::open(&self.directory)?.sync_all()?;
            Ok(op)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&scratch);
        }
        result
    }
}

fn sync_tree(path: &Path) -> Result<(), TransferError> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            sync_tree(&entry.path())?;
        } else if kind.is_file() {
            fs::File::open(entry.path())?.sync_all()?;
        }
    }
    fs::File::open(path)?.sync_all()?;
    Ok(())
}
