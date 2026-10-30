//! Terminal receipts retain session/file identities, not conversation snapshots.
//! Completion is durable before compaction, so interrupted cleanup can retry.
use super::{Operation, TransferError, TransferService, codex::IdentityMap, native, persist};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Default, Serialize, Deserialize)]
struct DatabasePath {
    path: PathBuf,
}
#[derive(Default, Serialize, Deserialize)]
struct NativePaths {
    databases: Vec<DatabasePath>,
}
trait Databases {
    fn paths(&self) -> Vec<PathBuf>;
}
impl Databases for NativePaths {
    fn paths(&self) -> Vec<PathBuf> {
        self.databases.iter().map(|d| d.path.clone()).collect()
    }
}
impl Databases for native::Native {
    fn paths(&self) -> Vec<PathBuf> {
        self.databases.iter().map(|d| d.path.clone()).collect()
    }
}
fn session_ids(ids: &mut IdentityMap) {
    ids.rollouts.clear();
    ids.turns.clear();
    ids.records.clear();
    ids.tool_calls.clear();
}
impl TransferService {
    pub(in crate::transfer) fn compact_after_finish(&self, op: &mut Operation) {
        if let Err(error) = self.compact_finished(op) {
            // The operation is already committed. Retry maintenance in the
            // background instead of reporting a successful move as failed.
            crate::log::warn(
                "transfer.compact.pending",
                serde_json::json!({"operation_id":op.id,"error":error.message}),
            );
        }
    }

    // Parse only native database paths for old receipts, never reconstruct SQL
    // rows just to discard them. Take the same operation/session locks as writes
    // and reload after waiting, including a concurrent ownership reclamation.
    pub(in crate::transfer) fn compact_archived(&self, id: &str) -> Result<(), TransferError> {
        let op: Operation<NativePaths> = self.load_journal(id)?;
        let _guard = self.locks.blocking(self.operation_keys(&op));
        drop(op);
        let mut op: Operation<NativePaths> = self.load_journal(id)?;
        self.compact_finished(&mut op)
    }

    fn compact_finished<N: Databases + Default + Serialize>(
        &self,
        op: &mut Operation<N>,
    ) -> Result<(), TransferError> {
        if op.compacted
            || !matches!(
                op.phase.as_str(),
                "complete" | "aborted" | "exported" | "retired"
            )
        {
            return Ok(());
        }
        let started = std::time::Instant::now();
        let path = self.directory.join(&op.id).join("operation.json");
        let before = fs::metadata(&path)?.len();
        if op.phase == "complete" {
            self.cleanup_markers(op)?;
            self.reclaim_prior_moves(op)?;
        }
        if matches!(op.phase.as_str(), "complete" | "aborted")
            && (op.incoming_digest.is_some() || !op.moving)
            && let Some(native) = &op.rewritten
        {
            native::forget_receipts(&native.paths(), &op.id)?;
        }
        if matches!(op.phase.as_str(), "aborted" | "retired") {
            for (provider, root) in self.bundle_roots(op)? {
                if let Some(probe) = op.storage_probes.get(&provider)
                    && probe.shared(&root)?
                {
                    probe.remove(&root)?;
                }
            }
        }
        self.cleanup_staging(op)?;
        op.native = N::default();
        op.rewritten = None;
        op.native_before = None;
        op.codex_names = None;
        op.metadata_before.clear();
        op.metadata_after.clear();
        op.metadata_replaced.clear();
        op.replaced_files.clear();
        op.reused_files.clear();
        op.storage_probes.clear();
        session_ids(&mut op.plan.identities);
        if let Some(staged) = &mut op.staged {
            session_ids(&mut staged.identities);
            for file in &mut staged.files {
                file.boundaries.clear();
            }
        }
        if let Some(plan) = &mut op.file_plan {
            plan.records.clear();
            plan.tool_names.clear();
            plan.owner_tools.clear();
        }
        op.compacted = true;
        // All native payload fields have been emptied before this generic save;
        // the remaining shape is readable by the normal Operation loader.
        persist(&path, op)?;
        self.remember_links(op);
        self.save_journal_index(op);
        crate::log::info(
            "transfer.compact",
            serde_json::json!({"operation_id":op.id,"phase":op.phase,
            "before_bytes":before,"after_bytes":fs::metadata(&path)?.len(),"ms":started.elapsed().as_millis()}),
        );
        Ok(())
    }
}
