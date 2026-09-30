//! File proofs and retained originals for identity-preserving prefix imports.
use super::{Operation, Publication, TransferService, hash};
use crate::transfer::TransferError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::PathBuf,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Original {
    pub backup: PathBuf,
    pub sha256: String,
}

fn conflict() -> TransferError {
    TransferError::new("move_conflict", "目标历史不是同一物理历史的完整前缀")
}

fn owner<'a>(op: &'a Operation, file: &Publication) -> Option<&'a crate::transfer::group::Member> {
    op.group()
        .members
        .iter()
        .find(|m| m.path == file.source)
        .or_else(|| {
            let history = op.plan.files.iter().find(|f| f.source == file.source)?;
            op.group()
                .members
                .iter()
                .find(|m| m.source == "codex" && m.sid == history.thread)
        })
        .or_else(|| {
            let item = op
                .file_plan
                .as_ref()?
                .files
                .iter()
                .find(|f| f.source == file.source)?;
            op.group().members.iter().find(|m| m.uid == item.owner)
        })
}

fn json_lines(raw: &[u8]) -> bool {
    let mut found = false;
    for line in raw
        .split(|b| *b == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
    {
        if serde_json::from_slice::<Value>(line).is_err() {
            return false;
        }
        found = true;
    }
    found
}

/// Native summaries are rewritten as whole JSON objects, even when their
/// owning history only appended. Identity/ownership fields must agree.
fn summary(file: &Publication, old: &[u8], new: &[u8]) -> bool {
    let name = file
        .target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if name != "summary.json" && !name.ends_with(".meta.json") {
        return false;
    }
    let (Ok(old), Ok(new)) = (
        serde_json::from_slice::<Value>(old),
        serde_json::from_slice::<Value>(new),
    ) else {
        return false;
    };
    old.is_object()
        && new.is_object()
        && [
            "/info/id",
            "/info/cwd",
            "/agentId",
            "/agent_id",
            "/subagent_id",
            "/parentSessionId",
            "/parent_session_id",
            "/child_session_id",
            "/session_kind",
        ]
        .iter()
        .all(|path| old.pointer(path) == new.pointer(path))
}

pub fn prepare(
    service: &TransferService,
    op: &mut Operation,
) -> Result<(BTreeSet<String>, BTreeSet<String>), TransferError> {
    let files = service.publications(op);
    let mut proven = BTreeSet::new();
    let mut extended = BTreeSet::new();
    let mut metadata = BTreeSet::new();
    // Prove each primary history separately. An auxiliary file cannot bless
    // a divergent primary transcript or a different physical rollout.
    for file in &files {
        let Some(member) = op.group().members.iter().find(|m| {
            m.path == file.source
                || (m.source == "grok" && m.path.join("chat_history.jsonl") == file.source)
        }) else {
            continue;
        };
        if !file.target.exists() {
            continue;
        }
        if file.symlink || fs::symlink_metadata(&file.target)?.is_symlink() {
            return Err(conflict());
        }
        let old = fs::read(&file.target)?;
        let new = fs::read(&file.staging)?;
        if !new.starts_with(&old) || !json_lines(&old) {
            return Err(conflict());
        }
        proven.insert(member.uid.clone());
        if old != new {
            metadata.insert(service.member_target_uid(op, member, op.staged.as_ref().unwrap())?);
            if member.source == "codex" {
                extended.insert(member.sid.clone());
            }
        }
    }
    let mut originals = BTreeMap::new();
    for (index, file) in files.iter().enumerate() {
        if fs::symlink_metadata(&file.target).is_err() {
            continue;
        }
        if hash(&file.target)? == file.sha256 {
            continue;
        }
        let member = owner(op, file).ok_or_else(conflict)?;
        if file.symlink
            || fs::symlink_metadata(&file.target)?.is_symlink()
            || !proven.contains(&member.uid)
        {
            return Err(conflict());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(&file.target)?.permissions().mode() & 0o111
                != fs::metadata(&file.staging)?.permissions().mode() & 0o111
            {
                return Err(conflict());
            }
        }
        let old = fs::read(&file.target)?;
        let new = fs::read(&file.staging)?;
        let is_prefix = new.starts_with(&old)
            && (file.target.extension().is_none_or(|e| e != "jsonl") || json_lines(&old));
        if !is_prefix && !summary(file, &old, &new) {
            return Err(conflict());
        }
        let digest = hash(&file.target)?;
        let directory = service.directory.join(&op.id).join("prefix-backups");
        fs::create_dir_all(&directory)?;
        let backup = directory.join(format!("{index}-{digest}"));
        if !backup.exists() {
            let mut out = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&backup)?;
            out.write_all(&old)?;
            out.set_permissions(fs::metadata(&file.target)?.permissions())?;
            out.sync_all()?;
            fs::File::open(&directory)?.sync_all()?;
        }
        if hash(&backup)? != digest || hash(&file.target)? != digest {
            return Err(TransferError::new(
                "move_plan_stale",
                "目标前缀在备份时发生变化",
            ));
        }
        originals.insert(
            file.target.clone(),
            Original {
                backup,
                sha256: digest,
            },
        );
    }
    op.replaced_files = originals;
    Ok((extended, metadata))
}

pub fn check_restore(
    op: &Operation,
    file: &Publication,
    old: &Original,
) -> Result<(), TransferError> {
    if hash(&old.backup)? != old.sha256 {
        return Err(TransferError::new(
            "move_recovery_required",
            "目标原文件备份发生变化",
        ));
    }
    if !file.target.exists() {
        return Ok(());
    }
    let digest = hash(&file.target)?;
    if digest == old.sha256
        || (digest == file.sha256 && TransferService::owns(&op.id, &file.target))
    {
        return Ok(());
    }
    Err(TransferError::new(
        "move_recovery_required",
        "合并后的目标文件已变化，保留现场",
    ))
}

pub fn restore(op: &Operation, file: &Publication, old: &Original) -> Result<(), TransferError> {
    check_restore(op, file, old)?;
    if file.target.exists() && hash(&file.target)? == old.sha256 {
        return Ok(());
    }
    let temporary = TransferService::marker(&op.id, &file.target).with_extension("restore");
    if !temporary.exists() {
        fs::copy(&old.backup, &temporary)?;
        fs::File::open(&temporary)?.sync_all()?;
    }
    if hash(&temporary)? != old.sha256 {
        return Err(TransferError::new(
            "move_recovery_required",
            "目标恢复文件已变化",
        ));
    }
    fs::rename(&temporary, &file.target)?;
    fs::File::open(file.target.parent().unwrap())?.sync_all()?;
    Ok(())
}
