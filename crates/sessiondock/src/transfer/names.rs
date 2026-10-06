//! Selected Codex rename records. Keep the append-only index's original bytes;
//! only the ID token changes when cloning into a new identity.
use super::super::{TransferError, codex::ClonePlan, json_bytes};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fs, io::Write, path::Path};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Names {
    pub source: BTreeMap<String, String>,
    pub target: BTreeMap<String, String>,
    #[serde(default)]
    pub publication: Option<Append>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Append {
    offset: u64,
    existed: bool,
    bytes: String,
}

fn read(path: &Path) -> Result<Vec<u8>, TransferError> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn records(raw: &[u8]) -> BTreeMap<String, String> {
    let mut rows = BTreeMap::new();
    for line in raw.split_inclusive(|b| *b == b'\n') {
        let Ok(row) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let Some(id) = row["id"].as_str().filter(|id| !id.is_empty()) else {
            continue;
        };
        let name = &row["thread_name"];
        if name.is_null()
            || name == false
            || name == ""
            || name == 0
            || name.as_array().is_some_and(Vec::is_empty)
            || name.as_object().is_some_and(serde_json::Map::is_empty)
        {
            continue;
        }
        if let Ok(line) = std::str::from_utf8(line) {
            rows.insert(id.to_owned(), line.to_owned());
        }
    }
    rows
}

impl Names {
    pub fn capture(path: &Path, plan: &ClonePlan) -> Result<Self, TransferError> {
        let source: BTreeMap<_, _> = records(&read(path)?)
            .into_iter()
            .filter(|(id, _)| plan.identities.threads.contains_key(id))
            .collect();
        let mut target = BTreeMap::new();
        for (id, raw) in &source {
            let next = &plan.identities.threads[id];
            let mut row: Value = serde_json::from_str(raw)?;
            row["id"] = next.clone().into();
            target.insert(next.clone(), json_bytes::rewrite_text(raw, &row)?);
        }
        Ok(Self {
            source,
            target,
            publication: None,
        })
    }

    pub fn recheck(&self, path: &Path, plan: &ClonePlan) -> Result<(), TransferError> {
        if Self::capture(path, plan)?.source != self.source {
            return Err(TransferError::new(
                "move_plan_stale",
                "源会话自定义标题已变化，请重新查看清单",
            ));
        }
        Ok(())
    }

    pub fn valid(&self, plan: &ClonePlan) -> bool {
        self.publication.is_none()
            && self.source.iter().all(|(id, raw)| {
                plan.identities.threads.get(id).is_some_and(|next| {
                    serde_json::from_str::<Value>(raw)
                        .ok()
                        .is_some_and(|mut row| {
                            if row["id"] != *id {
                                return false;
                            }
                            row["id"] = next.clone().into();
                            json_bytes::rewrite_text(raw, &row).ok().as_ref()
                                == self.target.get(next)
                        })
                })
            })
            && self.source.len() == self.target.len()
    }

    // Save the receipt in the operation journal before writing any native bytes.
    pub fn prepare(&mut self, path: &Path) -> Result<Vec<u8>, TransferError> {
        let before = read(path)?;
        let current = records(&before);
        let mut bytes = String::new();
        for (id, raw) in &self.target {
            if current.get(id) == Some(raw) {
                continue;
            }
            if bytes.is_empty() && !before.is_empty() && before.last() != Some(&b'\n') {
                bytes.push('\n');
            }
            bytes.push_str(raw);
            if !bytes.ends_with('\n') {
                bytes.push('\n');
            }
        }
        self.publication = (!bytes.is_empty()).then(|| Append {
            offset: before.len() as u64,
            existed: path.exists(),
            bytes,
        });
        Ok(before)
    }

    pub fn publish(&self, path: &Path, before: &[u8]) -> Result<(), TransferError> {
        let Some(append) = &self.publication else {
            return Ok(());
        };
        if read(path)? != before {
            return Err(TransferError::new(
                "move_plan_stale",
                "目标名称索引在发布前发生变化",
            ));
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(append.bytes.as_bytes())?;
        file.sync_all()?;
        fs::File::open(path.parent().unwrap())?.sync_all()?;
        Ok(())
    }

    pub fn rollback(&self, path: &Path) -> Result<(), TransferError> {
        let Some(append) = &self.publication else {
            return Ok(());
        };
        let current = read(path)?;
        // An absent append means publication never ran (or was already undone).
        if current.len() as u64 <= append.offset {
            return Ok(());
        }
        let tail = &current[append.offset as usize..];
        if !append.bytes.as_bytes().starts_with(tail) {
            return Err(TransferError::new(
                "move_recovery_required",
                "目标名称索引已变化，保留现场等待恢复",
            ));
        }
        let file = fs::OpenOptions::new().write(true).open(path)?;
        file.set_len(append.offset)?;
        file.sync_all()?;
        if !append.existed && append.offset == 0 {
            fs::remove_file(path)?;
        }
        fs::File::open(path.parent().unwrap())?.sync_all()?;
        Ok(())
    }
}
