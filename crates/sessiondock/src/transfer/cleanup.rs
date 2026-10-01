//! Node-owned cleanup for abandoned previews and standalone foreground copies.
use super::service::TransferService;
use std::{
    fs,
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) struct PlanningDirectory(pub std::path::PathBuf, pub bool);
impl Drop for PlanningDirectory {
    fn drop(&mut self) {
        if !self.1 {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
impl TransferService {
    pub fn touch(&self, id: &str) {
        self.foreground
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.into(), Instant::now());
    }
    pub fn housekeeping(self: Arc<Self>, shutdown: tokio_util::sync::CancellationToken) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            let mut finished = std::collections::BTreeSet::new();
            loop {
                tokio::select! { _ = shutdown.cancelled() => break, _ = interval.tick() => {} }
                let Ok(entries) = fs::read_dir(&self.directory) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                        continue;
                    };
                    if finished.contains(&id) {
                        continue;
                    }
                    let Ok(op) = self.load(&id) else { continue };
                    if matches!(
                        op.phase.as_str(),
                        "complete" | "aborted" | "exported" | "retired"
                    ) {
                        self.foreground
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        finished.insert(id);
                        continue;
                    }
                    if op.incoming_digest.is_some()
                        || !matches!(
                            op.phase.as_str(),
                            "planned" | "failed" | "publishing" | "verifying" | "rollback_required"
                        )
                    {
                        continue;
                    }
                    let age = self
                        .foreground
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(&id)
                        .map(Instant::elapsed)
                        .or_else(|| {
                            fs::metadata(entry.path().join("operation.json"))
                                .ok()?
                                .modified()
                                .ok()?
                                .elapsed()
                                .ok()
                        });
                    if age.is_none_or(|age| age < Duration::from_secs(60)) {
                        continue;
                    }
                    self.interrupts.cancel(&id);
                    let Ok(guard) = self.operation_guard(&id).await else {
                        continue;
                    };
                    let service = self.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        let _guard = guard;
                        // Re-read after taking the worker's gate: export/commit may have won.
                        let op = service.load(&id)?;
                        if op.moving {
                            if op.phase != "planned" {
                                return Ok(());
                            }
                            service.abort_source(&id, false)?;
                            service.abort_source(&id, true)?;
                        } else if op.phase != "exporting" && op.phase != "exported" {
                            service.abort_local(&id)?;
                        }
                        service
                            .foreground
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        Ok::<_, super::TransferError>(())
                    })
                    .await;
                }
            }
        });
    }
}
