//! Private bridge evidence. Native session files are never used as a journal.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Rpc {
    pub key: String,
    pub id: Option<u64>,
    pub write_id: String,
    pub frame: Value,
    pub delivery: Option<String>,
    pub response: Option<Value>,
    pub turn: Option<u64>,
    pub request_key: Option<String>,
}

pub(super) fn path(host_dir: &Path, key: &str) -> PathBuf {
    host_dir
        .parent()
        .unwrap_or(host_dir)
        .join("sdk-adapters")
        .join("kiro")
        .join(format!("{key}.json"))
}

pub(super) fn save(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let result = (|| -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("checkpoint has no parent"))?;
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        let temp = path.with_extension("tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        serde_json::to_writer(&mut file, value)?;
        file.flush()?;
        file.sync_all()?;
        std::fs::rename(temp, path)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    result.map_err(|e| format!("kiro_sdk_checkpoint: {e}"))
}
