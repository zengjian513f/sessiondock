//! Rebuildable journal indexes. The full operation remains the recovery authority.
//! Reuse the existing file stamp to invalidate both memory and disk caches after
//! atomic journal replacement, including writes by an older server.
use super::{Operation, Publication, TransferError, TransferService, prefix};
use crate::transfer::environment::{Stamp, stamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub(super) const INDEX_FILE: &str = "startup-index-v1.json";

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::transfer) struct Summary {
    pub id: String,
    pub phase: String,
    pub retired_source: bool,
    pub ownership_sequence: u64,
    pub export_lease_until: u64,
    pub source_uids: BTreeSet<String>,
    pub locked_uids: BTreeSet<String>,
    pub reclaimed_by: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
pub(in crate::transfer) struct Index {
    version: u32,
    #[serde(default)]
    pub compacted: bool,
    source: Stamp,
    home: PathBuf,
    pub summary: Summary,
    pub public: Value,
    pub local_link: Option<crate::session_links::Record>,
    pub moving: bool,
    pub incoming: bool,
    pub publications: Vec<Publication>,
    pub replaced_files: BTreeMap<PathBuf, prefix::Original>,
    pub active_uids: BTreeSet<String>,
}

#[derive(Default)]
struct State {
    entries: BTreeMap<PathBuf, (Stamp, Arc<Index>)>,
    journal_bytes_read: u64,
    disk_hits: u64,
}
#[derive(Default)]
pub(super) struct Cache(Mutex<State>);
impl Cache {
    pub fn record_read(&self, bytes: u64) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .journal_bytes_read += bytes;
    }
    pub fn startup_stats(&self) -> (u64, u64) {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        (state.journal_bytes_read, state.disk_hits)
    }
}

impl TransferService {
    fn make_journal_index<N>(
        &self,
        op: &Operation<N>,
        source: Stamp,
    ) -> Result<Index, TransferError> {
        let retired_source = op.phase == "retired" && op.moving && op.incoming_digest.is_none();
        let source_uids: BTreeSet<_> = op.group().members.iter().map(|m| m.uid.clone()).collect();
        let mut locked_uids = BTreeSet::new();
        if matches!(
            op.phase.as_str(),
            "publishing"
                | "verifying"
                | "rollback_required"
                | "ready"
                | "aborting"
                | "moved"
                | "retiring"
                | "retired"
                | "exporting"
        ) {
            locked_uids.clone_from(&source_uids);
            if !matches!(op.phase.as_str(), "moved" | "retiring" | "retired")
                && let Some(staged) = &op.staged
            {
                locked_uids.extend(
                    op.group()
                        .members
                        .iter()
                        .filter_map(|member| self.member_target_uid(op, member, staged).ok()),
                );
            }
            locked_uids.retain(|uid| !op.reclaimed_by.contains_key(uid));
        }

        let mut public = Self::public(op);
        public["incoming"] = op.incoming_digest.is_some().into();
        let local_link = if op.incoming_digest.is_some() || op.moving {
            None
        } else {
            Some(crate::session_links::Record {
                id: op.id.clone(),
                created_ms: op.created_ms,
                mode: "clone".into(),
                phase: op.phase.clone(),
                mappings: Self::link_map(op),
                ..Default::default()
            })
        };
        let active_uids = if op.phase == "complete" && op.incoming_digest.is_some() {
            match &op.staged {
                Some(staged) => op
                    .group()
                    .members
                    .iter()
                    .map(|m| self.member_target_uid(op, m, staged))
                    .collect::<Result<_, _>>()?,
                None => BTreeSet::new(),
            }
        } else {
            BTreeSet::new()
        };
        Ok(Index {
            version: 1,
            compacted: op.compacted,
            source,
            home: self.home.clone(),
            summary: Summary {
                id: op.id.clone(),
                phase: op.phase.clone(),
                retired_source,
                ownership_sequence: op.ownership_sequence,
                export_lease_until: op.export_lease_until,
                source_uids,
                locked_uids,
                reclaimed_by: op.reclaimed_by.clone(),
            },
            public,
            local_link,
            moving: op.moving,
            incoming: op.incoming_digest.is_some(),
            publications: if op.phase == "complete" {
                self.publications(op)
            } else {
                Vec::new()
            },
            replaced_files: if op.phase == "complete" {
                op.replaced_files.clone()
            } else {
                BTreeMap::new()
            },
            active_uids,
        })
    }

    // The cache is optional: failure to write it must not turn a committed
    // operation into a failed operation. Missing or invalid indexes rebuild.
    fn write_journal_index(&self, path: &Path, index: &Index) {
        if let Err(error) = super::persist(&path.with_file_name(INDEX_FILE), index) {
            crate::log::warn(
                "transfer.index.write",
                serde_json::json!({"operation_id":index.summary.id,"error":error.message}),
            );
        }
    }

    pub(in crate::transfer) fn journal_index(
        &self,
        path: &Path,
    ) -> Result<Arc<Index>, TransferError> {
        let mut cache = self.journals.0.lock().unwrap_or_else(|e| e.into_inner());
        let metadata = fs::metadata(path)?;
        let before = stamp(&metadata);
        if let Some((key, row)) = cache.entries.get(path)
            && *key == before
        {
            return Ok(row.clone());
        }
        if let Ok(raw) = fs::read(path.with_file_name(INDEX_FILE))
            && let Ok(index) = serde_json::from_slice::<Index>(&raw)
            && index.version == 1
            && index.source == before
            && index.home == self.home
        {
            let index = Arc::new(index);
            cache.disk_hits += 1;
            cache
                .entries
                .insert(path.to_owned(), (before, index.clone()));
            return Ok(index);
        }
        let raw = fs::read(path)?;
        cache.journal_bytes_read += raw.len() as u64;
        let mut op: Operation<serde::de::IgnoredAny> = serde_json::from_slice(&raw)?;
        drop(raw);
        if op.created_ms == 0 {
            op.created_ms = metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
        }
        let index = Arc::new(self.make_journal_index(&op, before.clone())?);
        // Never associate an old read with a concurrently replaced journal.
        if stamp(&fs::metadata(path)?) == before {
            self.write_journal_index(path, &index);
            cache
                .entries
                .insert(path.to_owned(), (before, index.clone()));
        }
        Ok(index)
    }

    pub(super) fn save_journal_index<N>(&self, op: &Operation<N>) {
        let mut cache = self.journals.0.lock().unwrap_or_else(|e| e.into_inner());
        let path = self.directory.join(&op.id).join("operation.json");
        let result = (|| {
            let source = stamp(&fs::metadata(&path)?);
            let index = Arc::new(self.make_journal_index(op, source.clone())?);
            self.write_journal_index(&path, &index);
            cache.entries.insert(path.clone(), (source, index));
            Ok::<_, TransferError>(())
        })();
        if let Err(error) = result {
            cache.entries.remove(&path);
            crate::log::warn(
                "transfer.index.build",
                serde_json::json!({"operation_id":op.id,"error":error.message}),
            );
        }
    }

    pub(in crate::transfer) fn journal_summaries(
        &self,
    ) -> Result<Vec<Arc<Summary>>, TransferError> {
        let mut summaries = Vec::new();
        let mut found = BTreeSet::new();
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path().join("operation.json");
            if !path.is_file() {
                continue;
            }
            found.insert(path.clone());
            summaries.push(Arc::new(self.journal_index(&path)?.summary.clone()));
        }
        self.journals
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .retain(|path, _| found.contains(path));
        Ok(summaries)
    }
}
