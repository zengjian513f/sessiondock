//! Cold work runs outside startup and request handlers. No native files,
//! identity receipts, trash payloads or user-saved diagnostic evidence expire.
use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime},
};
use tokio_util::sync::CancellationToken;

pub(crate) fn spawn(directory: Option<PathBuf>, shutdown: CancellationToken) {
    let Some(directory) = directory else { return };
    tokio::spawn(async move {
        loop {
            // Give startup/recovery the first turn; serialize bounded background work.
            tokio::select! { _ = shutdown.cancelled()=>return, _ = tokio::time::sleep(Duration::from_secs(60))=>{} }
            let root = directory.clone();
            let stop = shutdown.clone();
            if let Err(error) = tokio::task::spawn_blocking(move || collect(root, stop)).await {
                crate::log::warn(
                    "storage.maintenance_failed",
                    serde_json::json!({"error":error.to_string()}),
                );
            }
            tokio::select! { _ = shutdown.cancelled()=>return, _ = tokio::time::sleep(Duration::from_secs(3540))=>{} }
        }
    });
}
fn old(path: &std::path::Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|time| SystemTime::now().duration_since(time).ok())
        .is_some_and(|age| age >= Duration::from_secs(86400))
}
fn collect(root: PathBuf, stop: CancellationToken) {
    let Ok(entries) = fs::read_dir(root.join("sdkhost")) else {
        return;
    };
    for entry in entries.flatten() {
        if stop.is_cancelled() {
            break;
        }
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let dir = entry.path();
        let status_path = dir.join("status.json");
        if !old(&status_path) {
            continue;
        }
        let Ok(status) = fs::read(&status_path).and_then(|bytes| {
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(std::io::Error::other)
        }) else {
            continue;
        };
        if status["exit"].is_null() || status["stdout_eof"] != true || status["stderr_eof"] != true
        {
            continue;
        }
        // Migrate old, closed Kiro checkpoints away from monolithic JSON once.
        let checkpoint = root
            .join("sdk-adapters/kiro")
            .join(entry.file_name())
            .with_extension("json");
        if checkpoint.is_file() {
            if let Err(error) = crate::kiro_sdk::migrate_checkpoint(&checkpoint) {
                crate::log::warn(
                    "storage.checkpoint_migration_failed",
                    serde_json::json!({"error":error}),
                );
                continue;
            }
        }
        #[cfg(unix)]
        for name in ["stdout", "stderr"] {
            let path = dir.join(name);
            if fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() >= 65536) && old(&path) {
                if let Err(error) = sdkhost_client::cold::archive(&path) {
                    crate::log::warn(
                        "storage.archive_failed",
                        serde_json::json!({"stream":name,"error":error.to_string()}),
                    );
                }
            }
        }
    }
}
