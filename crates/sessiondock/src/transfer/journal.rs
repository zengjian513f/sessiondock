//! Disposable, demand-read summaries of the existing operation journals.
//! No extra files or background synchronization; replacement invalidates by stamp.
use super::{Operation, TransferError, TransferService};
use crate::transfer::environment::{Stamp, stamp};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub(in crate::transfer) struct Summary {
    pub id: String,
    pub phase: String,
    pub retired_source: bool,
    pub ownership_sequence: u64,
    pub export_lease_until: u64,
    pub source_uids: BTreeSet<String>,
    pub locked_uids: BTreeSet<String>,
}

#[derive(Default)]
pub(super) struct Cache(Mutex<BTreeMap<PathBuf, (Stamp, Arc<Summary>)>>);

impl TransferService {
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
            let before = stamp(&fs::metadata(&path)?);
            if let Some(summary) = self
                .journals
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&path)
                .filter(|(key, _)| *key == before)
                .map(|(_, row)| row.clone())
            {
                summaries.push(summary);
                continue;
            }
            let op: Operation = serde_json::from_slice(&fs::read(&path)?)?;
            let retired_source = op.phase == "retired" && op.moving && op.incoming_digest.is_none();
            let source_uids: BTreeSet<_> =
                op.group().members.iter().map(|m| m.uid.clone()).collect();
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
                            .filter_map(|member| self.member_target_uid(&op, member, staged).ok()),
                    );
                }
                locked_uids.retain(|uid| !op.reclaimed_by.contains_key(uid));
            }
            let summary = Arc::new(Summary {
                id: op.id,
                phase: op.phase,
                retired_source,
                ownership_sequence: op.ownership_sequence,
                export_lease_until: op.export_lease_until,
                source_uids,
                locked_uids,
            });
            // A concurrent atomic save must not attach an old summary to its new stamp.
            if stamp(&fs::metadata(&path)?) == before {
                self.journals
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(path, (before, summary.clone()));
            }
            summaries.push(summary);
        }
        self.journals
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|path, _| found.contains(path));
        Ok(summaries)
    }
}
