//! Codex physical history cloning into a new private directory. This module
//! deliberately does not publish a CLI home: native database projections and
//! tool executors require separate verification before a clone may be resumed.

use super::{TransferError, group::Group};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Move,
    Clone,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityMap {
    pub threads: BTreeMap<String, String>,
    pub rollouts: BTreeMap<String, String>,
    pub turns: BTreeMap<String, String>,
    /// Native item IDs and call IDs share identity in projected tool items.
    pub records: BTreeMap<String, String>,
    pub tool_calls: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryFile {
    pub source: PathBuf,
    pub relative: PathBuf,
    pub thread: String,
    pub rollout: String,
    pub sha256: String,
    pub bytes: u64,
    pub base: Option<Base>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Base {
    pub rollout: String,
    pub end_byte_offset: u64,
    pub end_ordinal_exclusive: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReferenceIssue {
    pub source: PathBuf,
    pub ordinal: Option<u64>,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClonePlan {
    pub version: u32,
    pub mode: Mode,
    pub group: Group,
    pub identities: IdentityMap,
    pub files: Vec<HistoryFile>,
    pub reference_issues: Vec<ReferenceIssue>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputFile {
    pub source: PathBuf,
    pub relative: PathBuf,
    pub source_sha256: String,
    pub sha256: String,
    pub bytes: u64,
    pub boundaries: BTreeMap<u64, u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StagedClone {
    pub version: u32,
    pub mode: Mode,
    pub publishable: bool,
    pub required_checks: Vec<String>,
    pub identities: IdentityMap,
    pub files: Vec<OutputFile>,
    pub reference_issues: Vec<ReferenceIssue>,
}

fn hash(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}

pub(super) fn uuid() -> Result<String, TransferError> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| TransferError::new("move_identity", e.to_string()))?;
    b[6] = (b[6] & 15) | 64;
    b[8] = (b[8] & 63) | 128;
    let s: String = b.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &s[..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..]
    ))
}

fn rows(raw: &[u8]) -> Result<Vec<(u64, Value)>, TransferError> {
    if !raw.is_empty() && raw.last() != Some(&b'\n') {
        return Err(TransferError::new(
            "move_plan_stale",
            "原生历史末行尚未写完",
        ));
    }
    let mut offset = 0;
    let mut rows = Vec::new();
    for line in raw.split_inclusive(|b| *b == b'\n') {
        offset += line.len() as u64;
        let value: Value = serde_json::from_slice(line)?;
        if !value.is_object() {
            return Err(TransferError::new("move_format", "原生历史记录必须是对象"));
        }
        rows.push((offset, value));
    }
    Ok(rows)
}

fn stable_read(path: &Path) -> Result<Vec<u8>, TransferError> {
    let mut file = fs::File::open(path)?;
    let before = file.metadata()?;
    let mut raw = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut raw)?;
    let after = file.metadata()?;
    let current = fs::metadata(path)?;
    if before.len() != raw.len() as u64
        || before.len() != after.len()
        || before.modified()? != after.modified()?
        || after.modified()? != current.modified()?
        || after.len() != current.len()
    {
        return Err(TransferError::new(
            "move_plan_stale",
            "读取期间历史文件发生变化",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (after.dev(), after.ino()) != (current.dev(), current.ino()) {
            return Err(TransferError::new(
                "move_plan_stale",
                "读取期间历史文件被替换",
            ));
        }
    }
    Ok(raw)
}

pub fn plan(group: Group, mode: Mode) -> Result<ClonePlan, TransferError> {
    if !group.blockers.is_empty() {
        return Err(TransferError::new(
            "move_group_incomplete",
            "整组存在未解析依赖",
        ));
    }
    let mut identities = IdentityMap {
        threads: BTreeMap::new(),
        rollouts: BTreeMap::new(),
        turns: BTreeMap::new(),
        records: BTreeMap::new(),
        tool_calls: BTreeMap::new(),
    };
    let mut files = Vec::new();
    for member in &group.members {
        if member.source != "codex" {
            return Err(TransferError::new(
                "move_group_unsupported",
                "此暂存适配器只处理 Codex；未发布移动或克隆能力",
            ));
        }
        let rollout = member
            .rollout_id
            .as_ref()
            .ok_or_else(|| TransferError::new("move_identity", "无法确定原生 rollout 身份"))?;
        if !identities.threads.contains_key(&member.sid) {
            identities.threads.insert(
                member.sid.clone(),
                if mode == Mode::Move {
                    member.sid.clone()
                } else {
                    uuid()?
                },
            );
        }
        if identities.rollouts.contains_key(rollout) {
            return Err(TransferError::new(
                "move_conflict",
                "同一 rollout 身份对应多份文件",
            ));
        }
        identities.rollouts.insert(
            rollout.clone(),
            if rollout == &member.sid {
                identities.threads[&member.sid].clone()
            } else if mode == Mode::Move {
                rollout.clone()
            } else {
                uuid()?
            },
        );
        let raw = stable_read(&member.path)?;
        let parsed = rows(&raw)?;
        let meta = parsed
            .iter()
            .find(|(_, v)| v["type"] == "session_meta")
            .map(|(_, v)| &v["payload"])
            .ok_or_else(|| TransferError::new("move_format", "历史缺少 session_meta"))?;
        if meta["id"].as_str() != Some(&member.sid) {
            return Err(TransferError::new(
                "move_plan_stale",
                "历史身份与索引快照不同",
            ));
        }
        let base = if meta["history_base"].is_null() {
            None
        } else {
            let b = &meta["history_base"];
            Some(Base {
                rollout: b["thread_id"]
                    .as_str()
                    .ok_or_else(|| {
                        TransferError::new("move_format", "history_base 缺少 rollout ID")
                    })?
                    .into(),
                end_byte_offset: b["end_byte_offset"].as_u64().ok_or_else(|| {
                    TransferError::new("move_format", "history_base 字节边界无效")
                })?,
                end_ordinal_exclusive: b["end_ordinal_exclusive"].as_u64(),
            })
        };
        for (_, row) in &parsed {
            let p = &row["payload"];
            if row["type"] == "response_item"
                && p["type"] == "function_call"
                && (p["namespace"].is_null() || p["namespace"] == "functions")
                && let (Some(id), Some(name)) = (p["call_id"].as_str(), p["name"].as_str())
            {
                if let Some(previous) = identities.tool_calls.insert(id.into(), name.into())
                    && previous != name
                {
                    return Err(TransferError::new(
                        "move_identity",
                        "工具调用 ID 对应多个工具",
                    ));
                }
            }
            if row["type"]=="response_item" && p["type"]=="custom_tool_call" && p["name"]=="exec" &&
                p["input"].as_str().is_some_and(super::code_mode::agent_call) {
                if let Some(id)=p["call_id"].as_str(){identities.tool_calls.insert(id.into(),"__code_agent".into());}
            }
            super::codex_ids::visit(&mut row.clone(), &mut |kind, id| {
                let map = match kind {
                    super::codex_ids::Identity::Thread => return Ok(id.to_owned()),
                    super::codex_ids::Identity::Turn => &mut identities.turns,
                    super::codex_ids::Identity::Record => &mut identities.records,
                };
                if !map.contains_key(id) {
                    map.insert(
                        id.to_owned(),
                        if mode == Mode::Move {
                            id.to_owned()
                        } else {
                            uuid()?
                        },
                    );
                }
                Ok(id.to_owned())
            })?;
        }
        let relative = member
            .path
            .strip_prefix(&member.root)
            .map_err(|_| TransferError::new("move_path", "原生文件位于配置根目录之外"))?
            .to_owned();
        files.push(HistoryFile {
            source: member.path.clone(),
            relative,
            thread: member.sid.clone(),
            rollout: rollout.clone(),
            sha256: hash(&raw),
            bytes: raw.len() as u64,
            base,
        });
    }
    for file in &files {
        if let Some(base) = &file.base
            && !identities.rollouts.contains_key(&base.rollout)
        {
            return Err(TransferError::new(
                "move_group_incomplete",
                "缺少被引用的物理 rollout",
            ));
        }
    }
    let mut reference_issues = Vec::new();
    if mode == Mode::Clone {
        for file in &files {
            for (_, row) in rows(&stable_read(&file.source)?)? {
                if let Some(reason) = super::codex_tools::audit(&row, &identities) {
                    reference_issues.push(ReferenceIssue {
                        source: file.source.clone(),
                        ordinal: row["ordinal"].as_u64(),
                        reason,
                    });
                }
            }
        }
    }
    Ok(ClonePlan {
        version: 1,
        mode,
        group,
        reference_issues,
        identities,
        files,
    })
}

fn remap(
    value: &mut Value,
    key: &str,
    map: &BTreeMap<String, String>,
) -> Result<(), TransferError> {
    if let Some(id) = value[key].as_str().filter(|v| !v.is_empty()) {
        let next = map.get(id).ok_or_else(|| {
            TransferError::new("move_group_incomplete", format!("未映射的 {key}: {id}"))
        })?;
        value[key] = Value::String(next.clone());
    }
    Ok(())
}

/// Change only known structural identity fields. Arbitrary text, user message
/// bodies, dynamic tool schemas and runtime handles are never string-replaced.
fn rewrite(
    row: &mut Value,
    map: &IdentityMap,
    inherited: Option<(&Base, &OutputFile)>,
) -> Result<(), TransferError> {
    let kind = row["type"].as_str().unwrap_or("").to_owned();
    if !matches!(
        kind.as_str(),
        "session_meta" | "turn_context" | "event_msg" | "response_item"
    ) {
        return Ok(());
    }
    if !row["payload"].is_object() {
        return Err(TransferError::new(
            "move_format",
            "原生历史 payload 必须是对象",
        ));
    }
    let p = &mut row["payload"];
    if kind == "session_meta" {
        for key in ["id", "session_id", "forked_from_id", "parent_thread_id"] {
            remap(p, key, &map.threads)?;
        }
        for source_key in ["subagent", "subAgent"] {
            if let Some(spawn) = p.pointer_mut(&format!("/source/{source_key}/thread_spawn")) {
                remap(spawn, "parent_thread_id", &map.threads)?;
            }
        }
        if !p["history_base"].is_null() {
            let (base, output) = inherited
                .ok_or_else(|| TransferError::new("move_group_incomplete", "未解析的父历史边界"))?;
            let cut = output
                .boundaries
                .get(&base.end_byte_offset)
                .ok_or_else(|| TransferError::new("move_format", "父历史偏移不是完整记录边界"))?;
            p["history_base"]["thread_id"] = Value::String(map.rollouts[&base.rollout].clone());
            p["history_base"]["end_byte_offset"] = Value::from(*cut);
        }
    }
    super::codex_tools::rewrite(row, map)?;
    super::codex_ids::visit(row, &mut |kind, id| {
        let identities = match kind {
            super::codex_ids::Identity::Thread => &map.threads,
            super::codex_ids::Identity::Turn => &map.turns,
            super::codex_ids::Identity::Record => &map.records,
        };
        identities.get(id).cloned().ok_or_else(|| {
            TransferError::new("move_group_incomplete", format!("未映射的 {kind:?}: {id}"))
        })
    })?;
    Ok(())
}

fn relative(file: &HistoryFile, map: &IdentityMap) -> Result<PathBuf, TransferError> {
    let name = file
        .relative
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| TransferError::new("move_path", "文件名无效"))?;
    let prefix = name
        .get(..28)
        .ok_or_else(|| TransferError::new("move_path", "rollout 文件名无效"))?;
    let thread = &map.threads[&file.thread];
    let rollout = &map.rollouts[&file.rollout];
    let suffix = if thread == rollout {
        String::new()
    } else {
        format!("_{rollout}")
    };
    let path = file
        .relative
        .with_file_name(format!("{prefix}{thread}{suffix}.jsonl"));
    if !path
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return Err(TransferError::new("move_path", "暂存文件必须是相对路径"));
    }
    Ok(path)
}

/// Creates a new staging directory; never writes to a CLI root or overwrites
/// files. Callers persist `ClonePlan` for retries. Partial staging is retained
/// for diagnosis, and only a final manifest denotes a completed transformation.
pub fn stage(plan: &ClonePlan, destination: &Path) -> Result<StagedClone, TransferError> {
    if plan.version != 1 {
        return Err(TransferError::new("move_format", "未知的计划版本"));
    }
    validate(plan)?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent)?;
    for member in &plan.group.members {
        if parent.starts_with(fs::canonicalize(&member.root)?) {
            return Err(TransferError::new(
                "move_path",
                "暂存目录不能位于源会话根目录内",
            ));
        }
    }
    let expected: BTreeSet<_> = plan.files.iter().map(|f| &f.rollout).collect();
    if expected.len() != plan.files.len() {
        return Err(TransferError::new("move_conflict", "rollout 身份重复"));
    }
    fs::create_dir(destination)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
    }
    let mut output: BTreeMap<String, OutputFile> = BTreeMap::new();
    while output.len() != plan.files.len() {
        let before = output.len();
        for file in &plan.files {
            if output.contains_key(&file.rollout) {
                continue;
            }
            if file
                .base
                .as_ref()
                .is_some_and(|b| !output.contains_key(&b.rollout))
            {
                continue;
            }
            let raw = stable_read(&file.source)?;
            if raw.len() as u64 != file.bytes || hash(&raw) != file.sha256 {
                return Err(TransferError::new(
                    "move_plan_stale",
                    "历史文件与已确认计划不同",
                ));
            }
            let inherited = file.base.as_ref().map(|b| (b, &output[&b.rollout]));
            if let Some(base) = &file.base {
                let parent = plan
                    .files
                    .iter()
                    .find(|p| p.rollout == base.rollout)
                    .ok_or_else(|| TransferError::new("move_group_incomplete", "父历史缺失"))?;
                let parent_raw = stable_read(&parent.source)?;
                let prefix = parent_raw
                    .get(..base.end_byte_offset as usize)
                    .ok_or_else(|| TransferError::new("move_format", "父历史边界超出文件"))?;
                let prefix_rows = rows(prefix)?;
                if let Some(end) = base.end_ordinal_exclusive
                    && !prefix.is_empty()
                    && prefix_rows
                        .last()
                        .and_then(|(_, row)| row["ordinal"].as_u64())
                        .and_then(|n| n.checked_add(1))
                        != Some(end)
                {
                    return Err(TransferError::new(
                        "move_format",
                        "父历史 ordinal 与字节边界不一致",
                    ));
                }
            }
            let mut bytes = Vec::new();
            let mut boundaries = BTreeMap::from([(0, 0)]);
            for (end, mut row) in rows(&raw)? {
                if plan.mode == Mode::Clone {
                    rewrite(&mut row, &plan.identities, inherited)?;
                    serde_json::to_writer(&mut bytes, &row)?;
                    bytes.push(b'\n');
                    boundaries.insert(end, bytes.len() as u64);
                } else {
                    boundaries.insert(end, end);
                }
            }
            if plan.mode == Mode::Move {
                bytes = raw;
            }
            let path = relative(file, &plan.identities)?;
            let absolute = destination.join(&path);
            if let Some(parent) = absolute.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut target = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&absolute)?;
            target.write_all(&bytes)?;
            target.sync_all()?;
            output.insert(
                file.rollout.clone(),
                OutputFile {
                    source: file.source.clone(),
                    relative: path,
                    source_sha256: file.sha256.clone(),
                    sha256: hash(&bytes),
                    bytes: bytes.len() as u64,
                    boundaries,
                },
            );
        }
        if output.len() == before {
            return Err(TransferError::new(
                "move_group_incomplete",
                "物理历史存在循环或缺失依赖",
            ));
        }
    }
    for file in &plan.files {
        if hash(&stable_read(&file.source)?) != file.sha256 {
            return Err(TransferError::new(
                "move_plan_stale",
                "暂存期间源历史发生变化",
            ));
        }
    }
    let staged = StagedClone {
        version: 1,
        mode: plan.mode,
        publishable: false,
        reference_issues: plan.reference_issues.clone(),
        required_checks: vec![
            "native_metadata_and_projection_import".into(),
            "tool_reference_remapping".into(),
            "native_resume_and_list".into(),
            "group_and_liveness_recheck".into(),
        ],
        identities: plan.identities.clone(),
        files: output.into_values().collect(),
    };
    let mut manifest = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination.join("manifest.json"))?;
    serde_json::to_writer_pretty(&mut manifest, &staged)?;
    manifest.write_all(b"\n")?;
    manifest.sync_all()?;
    Ok(staged)
}

fn validate(plan: &ClonePlan) -> Result<(), TransferError> {
    let invalid = || TransferError::new("move_identity", "计划身份映射缺失、冲突或格式无效");
    let uuid = |s: &str| {
        s.len() == 36
            && s.bytes().enumerate().all(|(i, b)| {
                if matches!(i, 8 | 13 | 18 | 23) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
    };
    for map in [
        &plan.identities.threads,
        &plan.identities.rollouts,
        &plan.identities.turns,
        &plan.identities.records,
    ] {
        let values: BTreeSet<_> = map.values().collect();
        if values.len() != map.len()
            || map.iter().any(|(old, new)| {
                (plan.mode == Mode::Clone && !uuid(new))
                    || (plan.mode == Mode::Move && old != new)
                    || (plan.mode == Mode::Clone && map.contains_key(new))
            })
        {
            return Err(invalid());
        }
    }
    for file in &plan.files {
        if crate::sessions::codex_rollout_id(&file.relative, &file.thread)
            != Some(file.rollout.as_str())
        {
            return Err(invalid());
        }
        let thread = plan
            .identities
            .threads
            .get(&file.thread)
            .ok_or_else(invalid)?;
        let rollout = plan
            .identities
            .rollouts
            .get(&file.rollout)
            .ok_or_else(invalid)?;
        if (file.thread == file.rollout) != (thread == rollout) {
            return Err(invalid());
        }
        if file.thread != file.rollout && plan.identities.threads.values().any(|id| id == rollout) {
            return Err(invalid());
        }
        if let Some(base) = &file.base
            && !plan.identities.rollouts.contains_key(&base.rollout)
        {
            return Err(invalid());
        }
        relative(file, &plan.identities)?;
    }
    if plan.mode == Mode::Clone {
        let old: BTreeSet<_> = plan
            .identities
            .threads
            .keys()
            .chain(plan.identities.rollouts.keys())
            .collect();
        if plan
            .identities
            .threads
            .values()
            .chain(plan.identities.rollouts.values())
            .any(|id| old.contains(id))
        {
            return Err(invalid());
        }
    }
    Ok(())
}
