//! Droid's session-owned history and shared, content-addressed rewind snapshots.
//! Moving preserves transcript/settings/manifest bytes. Shared index additions
//! patch only their native fields; ordinary publication owns rollback evidence.
use super::{
    TransferError,
    group::Member,
    service::prefix::Original,
    service::{Operation, Publication, TransferService},
};
use serde_json::{Value, json, value::RawValue};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, Write},
    path::{Path, PathBuf},
};

fn unsupported(message: &str) -> TransferError {
    TransferError::new("move_native_unsupported", format!("Droid 快照：{message}"))
}
pub fn root(sessions: &Path) -> Result<PathBuf, TransferError> {
    sessions
        .parent()
        .map(Path::to_owned)
        .ok_or_else(|| unsupported("缺少原生根目录"))
}
fn hash_name(value: &Value) -> Result<&str, TransferError> {
    value
        .as_str()
        .filter(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or_else(|| unsupported("未知内容地址格式"))
}
fn content(root: &Path, hash: &str) -> PathBuf {
    root.join("snapshots/content")
        .join(&hash[..2])
        .join(&hash[2..])
}
fn manifest(root: &Path, sid: &str) -> PathBuf {
    root.join("snapshots/manifests")
        .join(format!("{sid}.snapshots.json"))
}
fn boundaries(value: &Value, sid: &str) -> Result<Vec<Value>, TransferError> {
    if value["version"] != 1 || value["sessionId"] != sid {
        return Err(unsupported("manifest 版本或会话身份不符"));
    }
    let rows = value["boundaries"]
        .as_array()
        .ok_or_else(|| unsupported("缺少边界记录"))?;
    let start = value["oldestAvailableBoundaryIndex"]
        .as_u64()
        .ok_or_else(|| unsupported("缺少有效边界位置"))? as usize;
    if start > rows.len() {
        return Err(unsupported("有效边界越界"));
    }
    rows.iter().enumerate().skip(start).map(|(index,row)| {
        let files=row["files"].as_array().ok_or_else(|| unsupported("缺少文件快照"))?;
        let hashes=files.iter().map(|file| {
            if file["size"].as_u64().is_none() { return Err(unsupported("文件快照大小无效")); }
            Ok(Value::String(hash_name(&file["contentHash"])?.into()))
        }).collect::<Result<Vec<_>,TransferError>>()?;
        if row["timestamp"].as_u64().is_none() || row["totalSize"].as_u64().is_none() {
            return Err(unsupported("边界时间或大小无效"));
        }
        Ok(json!({"sessionId":sid,"boundaryIndex":index,"timestamp":row["timestamp"],"sizeBytes":row["totalSize"],"contentHashes":hashes}))
    }).collect()
}

pub fn owned(member: &Member) -> Result<Vec<PathBuf>, TransferError> {
    let root = root(&member.root)?;
    let mut line = String::new();
    std::io::BufReader::new(fs::File::open(&member.path)?).read_line(&mut line)?;
    let header: Value = serde_json::from_str(&line)?;
    if header["decompMissionId"]
        .as_str()
        .is_some_and(|id| !id.is_empty())
    {
        return Err(unsupported(
            "此会话引用尚未适配的 mission 持久状态，未移动任何原生文件",
        ));
    }
    let mut paths = vec![member.path.clone()];
    let settings = member.path.with_extension("settings.json");
    if settings.try_exists()? {
        let value: Value = serde_json::from_slice(&fs::read(&settings)?)?;
        if value["tags"].as_array().into_iter().flatten().any(|tag| {
            tag["metadata"]["missionId"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
        }) {
            return Err(unsupported(
                "会话设置引用尚未适配的 mission 持久状态，未移动任何原生文件",
            ));
        }
        paths.push(settings);
    }
    let path = manifest(&root, &member.sid);
    if path.try_exists()? {
        let value: Value = serde_json::from_slice(&fs::read(&path)?)?;
        let entries = boundaries(&value, &member.sid)?;
        paths.push(path);
        for entry in entries {
            for hash in entry["contentHashes"].as_array().unwrap() {
                let path = content(&root, hash_name(hash)?);
                if !path.is_file() {
                    return Err(unsupported("有效边界的内容文件缺失"));
                }
                paths.push(path);
            }
        }
    }
    Ok(paths)
}
pub fn shared_content(root: &Path, path: &Path) -> bool {
    path.starts_with(root.join("snapshots/content"))
}
pub fn has_snapshots<N>(op: &Operation<N>) -> bool {
    op.file_plan.as_ref().is_some_and(|plan| {
        plan.files
            .iter()
            .any(|f| f.provider == "droid" && f.relative.starts_with("snapshots/manifests"))
    })
}
pub fn allowed(root: &Path, member: &Member, path: &Path) -> bool {
    path == member.path
        || path == member.path.with_extension("settings.json")
        || path == manifest(root, &member.sid)
        || path
            .strip_prefix(root.join("snapshots/content"))
            .ok()
            .is_some_and(|p| {
                let parts = p.iter().filter_map(|p| p.to_str()).collect::<Vec<_>>();
                parts.len() == 2
                    && parts[0].len() == 2
                    && hash_name(&Value::String(parts.concat())).is_ok()
            })
}

// Replace an existing value or append a new field without reserializing the
// containing object. Unrelated spelling, whitespace and key order survive.
fn field(raw: &[u8], key: &str, value: &[u8]) -> Result<Vec<u8>, TransferError> {
    let fields: BTreeMap<String, &RawValue> = serde_json::from_slice(raw)?;
    let mut result = raw.to_vec();
    if let Some(old) = fields.get(key) {
        let start = old.get().as_ptr() as usize - raw.as_ptr() as usize;
        result.splice(start..start + old.get().len(), value.iter().copied());
    } else {
        let end = raw
            .iter()
            .rposition(|b| *b == b'}')
            .ok_or_else(|| unsupported("共享索引不是对象"))?;
        let insertion = format!(
            "{}{}:{}",
            if fields.is_empty() { "" } else { "," },
            serde_json::to_string(key)?,
            std::str::from_utf8(value).map_err(|_| unsupported("无效JSON字段"))?
        );
        result.splice(end..end, insertion.bytes());
    }
    Ok(result)
}
fn get_field<'a>(raw: &'a [u8], key: &str) -> Result<&'a [u8], TransferError> {
    let fields: BTreeMap<String, &RawValue> = serde_json::from_slice(raw)?;
    fields
        .get(key)
        .map(|v| v.get().as_bytes())
        .ok_or_else(|| unsupported("共享索引字段缺失"))
}
fn append_queue(raw: &[u8], additions: &[Value]) -> Result<Vec<u8>, TransferError> {
    let originals: Vec<&RawValue> = serde_json::from_slice(raw)?;
    let mut inserts = BTreeMap::<usize, Vec<&Value>>::new();
    for value in additions {
        let timestamp = value["timestamp"].as_u64().unwrap();
        let at = originals.iter().position(|row| {
            serde_json::from_str::<Value>(row.get())
                .ok()
                .and_then(|v| v["timestamp"].as_u64())
                .is_some_and(|t| t > timestamp)
        });
        let offset = at
            .map(|i| originals[i].get().as_ptr() as usize - raw.as_ptr() as usize)
            .unwrap_or(
                raw.iter()
                    .rposition(|b| *b == b']')
                    .ok_or_else(|| unsupported("边界队列不是数组"))?,
            );
        inserts.entry(offset).or_default().push(value);
    }
    let mut result = raw.to_vec();
    for (offset, mut values) in inserts.into_iter().rev() {
        values.sort_by_key(|v| v["timestamp"].as_u64());
        let end = raw[offset] == b']';
        let encoded = values
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?
            .join(",");
        let insertion = if end {
            format!("{}{encoded}", if originals.is_empty() { "" } else { "," })
        } else {
            format!("{encoded},")
        };
        result.splice(offset..offset, insertion.bytes());
    }
    Ok(result)
}
fn read_or(path: &Path, default: Value) -> Result<Vec<u8>, TransferError> {
    match fs::read(path) {
        Ok(raw) => Ok(raw),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::to_vec(&default)?),
        Err(e) => Err(e.into()),
    }
}
fn store(
    service: &TransferService,
    op: &mut Operation,
    target: PathBuf,
    raw: Vec<u8>,
    slot: &str,
) -> Result<(), TransferError> {
    let directory = service.directory.join(&op.id).join("droid-shared");
    fs::create_dir_all(&directory)?;
    let staging = directory.join(slot);
    let previous = fs::read(&target).ok();
    if previous.as_deref() == Some(raw.as_slice()) {
        return Ok(());
    }
    if let Some(previous) = previous {
        let backup = directory.join(format!("{slot}.before"));
        fs::write(&backup, &previous)?;
        fs::File::open(&backup)?.sync_all()?;
        let digest = super::service::hash(&backup)?;
        if super::service::hash(&target)? != digest {
            return Err(TransferError::new(
                "move_plan_stale",
                "Droid 共享索引在备份期间变化",
            ));
        }
        op.replaced_files.insert(
            target.clone(),
            Original {
                backup,
                sha256: digest,
            },
        );
    }
    let mut out = fs::File::create(&staging)?;
    out.write_all(&raw)?;
    out.sync_all()?;
    op.droid_publications.push(Publication {
        source: target.clone(),
        target,
        sha256: super::service::hash(&staging)?,
        staging,
        symlink: false,
    });
    Ok(())
}

pub fn prepare_target(service: &TransferService, op: &mut Operation) -> Result<(), TransferError> {
    op.droid_publications.clear();
    if !has_snapshots(op) {
        return Ok(());
    }
    let plan = op.file_plan.as_ref().unwrap();
    let root = plan.roots["droid"].clone();
    let mut incoming = Vec::new();
    for member in plan.group.members.iter().filter(|m| m.source == "droid") {
        let path = manifest(&root, &member.sid);
        if let Some(file) = op.file_publications.iter().find(|f| f.target == path) {
            incoming.extend(boundaries(
                &serde_json::from_slice::<Value>(&fs::read(&file.staging)?)?,
                &member.sid,
            )?);
        }
    }
    let index_path = root.join("snapshots/.index.json");
    let metadata_path = root.join("snapshots/content/.metadata.json");
    if !metadata_path.try_exists()? {
        let has_content = fs::read_dir(root.join("snapshots/content"))
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| {
                let prefix = entry.file_name().to_string_lossy().into_owned();
                prefix.len() == 2
                    && prefix.bytes().all(|b| b.is_ascii_hexdigit())
                    && fs::read_dir(entry.path())
                        .into_iter()
                        .flatten()
                        .flatten()
                        .any(|file| file.path().is_file())
            });
        if index_path.try_exists()? || has_content {
            return Err(unsupported(
                "原生共享元数据不完整，请先由 Droid 完成原生恢复后重试",
            ));
        }
    }
    let mut index = read_or(
        &index_path,
        json!({"version":1,"lastCleanupAt":0,"totalSizeBytes":0,"boundaryQueue":[]}),
    )?;
    let mut metadata = read_or(
        &metadata_path,
        json!({"version":1,"refCounts":{},"totalSizeBytes":0,"lastGCAt":0}),
    )?;
    let index_value: Value = serde_json::from_slice(&index)?;
    let meta_value: Value = serde_json::from_slice(&metadata)?;
    if index_value["version"] != 1 || meta_value["version"] != 1 {
        return Err(unsupported("共享索引版本未知"));
    }
    let existing = index_value["boundaryQueue"]
        .as_array()
        .ok_or_else(|| unsupported("共享队列缺失"))?;
    let mut additions = Vec::new();
    for row in incoming {
        let same = existing.iter().find(|e| {
            e["sessionId"] == row["sessionId"]
                && e["boundaryIndex"] == row["boundaryIndex"]
                && e["timestamp"] == row["timestamp"]
        });
        if let Some(previous) = same {
            if previous != &row {
                return Err(unsupported("已有边界内容冲突"));
            }
        } else {
            additions.push(row);
        }
    }
    let mut counts = get_field(&metadata, "refCounts")?.to_vec();
    let mut sizes = meta_value["totalSizeBytes"]
        .as_u64()
        .ok_or_else(|| unsupported("共享内容大小无效"))?;
    let mut queue_size = index_value["totalSizeBytes"]
        .as_u64()
        .ok_or_else(|| unsupported("共享队列大小无效"))?;
    for row in &additions {
        queue_size += row["sizeBytes"].as_u64().unwrap();
        for value in row["contentHashes"].as_array().unwrap() {
            let hash = hash_name(value)?;
            let map: Value = serde_json::from_slice(&counts)?;
            let prior = map
                .get(hash)
                .map(|v| v.as_u64().ok_or_else(|| unsupported("共享引用计数无效")))
                .transpose()?;
            if prior.is_none() {
                let path = content(&root, hash);
                let file = op
                    .file_publications
                    .iter()
                    .find(|f| f.target == path)
                    .ok_or_else(|| unsupported("内容不在迁移清单中"))?;
                sizes += fs::metadata(&file.staging)?.len();
            }
            counts = field(
                &counts,
                hash,
                serde_json::to_string(&(prior.unwrap_or(0) + 1))?.as_bytes(),
            )?;
        }
    }
    metadata = field(&metadata, "refCounts", &counts)?;
    metadata = field(&metadata, "totalSizeBytes", sizes.to_string().as_bytes())?;
    let queue = append_queue(get_field(&index, "boundaryQueue")?, &additions)?;
    index = field(&index, "boundaryQueue", &queue)?;
    index = field(&index, "totalSizeBytes", queue_size.to_string().as_bytes())?;
    store(service, op, index_path, index, "index.json")?;
    store(service, op, metadata_path, metadata, "metadata.json")?;
    Ok(())
}
