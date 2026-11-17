//! Durable, non-expiring mutation fences, including target pre-publication.
use super::{TransferService, persist};
use crate::transfer::TransferError;
use std::collections::BTreeSet;

impl TransferService {
    pub fn owns_freeze(&self, id: &str) -> bool {
        self.frozen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .is_some_and(|uids| !uids.is_empty())
    }
    pub fn frozen_uids(&self) -> BTreeSet<String> {
        self.frozen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .flatten()
            .cloned()
            .collect()
    }

    pub async fn mutation_guard(
        &self,
        uids: &BTreeSet<String>,
    ) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
        self.locks
            .acquire(uids.iter().map(|uid| format!("mutation:{uid}")).collect())
            .await
    }

    pub fn freeze(
        &self,
        id: &str,
        uids: BTreeSet<String>,
        release: bool,
    ) -> Result<(), TransferError> {
        let mut frozen = self.frozen.lock().unwrap_or_else(|e| e.into_inner());
        if !release {
            if frozen.get(id).is_some_and(BTreeSet::is_empty) {
                return Err(TransferError::new(
                    "move_cancelled",
                    "迁移已结束，不能重新冻结",
                ));
            }
            if frozen
                .iter()
                .any(|(other, held)| other != id && !held.is_disjoint(&uids))
            {
                return Err(TransferError::new(
                    "move_session_locked",
                    "会话已由另一迁移冻结",
                ));
            }
            if frozen.get(id).is_some_and(|held| held != &uids) {
                return Err(TransferError::new(
                    "move_conflict",
                    "迁移冻结范围与原选择不符",
                ));
            }
        }
        let mut next = frozen.clone();
        // Empty tombstones reject late freeze requests after release.
        next.insert(id.into(), if release { BTreeSet::new() } else { uids });
        persist(&self.directory.join("frozen.json"), &next)?;
        *frozen = next;
        Ok(())
    }
}
