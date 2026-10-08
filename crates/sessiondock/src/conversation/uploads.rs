//! Resumable private staging. Only a fully received, synced request advances
//! the checkpoint; interrupted bytes are truncated before the next chunk.
use super::{Conversations, store::Upload};
use crate::delivery::target::Failure;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

fn storage(_: impl std::fmt::Display) -> Failure {
    Failure::new(503, "attachment_storage", "附件暂存失败，请重试续传")
}
fn conflict() -> Failure {
    Failure::new(
        409,
        "attachment_offset",
        "附件续传位置已变化，请重新查询进度",
    )
}

#[derive(Serialize, Deserialize)]
pub struct Checkpoint {
    name: String,
    mime: String,
    total: u64,
    offset: u64,
}
async fn checkpoint(path: &Path) -> Result<Option<Checkpoint>, Failure> {
    match tokio::fs::read(path.with_extension("resume")).await {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(storage),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(storage(e)),
    }
}
async fn save_checkpoint(path: &Path, value: &Checkpoint) -> Result<(), Failure> {
    let bytes = serde_json::to_vec(value).map_err(storage)?;
    let temporary = path.with_extension(format!("{}.upload", super::random_id()?));
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary).await.map_err(storage)?;
    file.write_all(&bytes).await.map_err(storage)?;
    file.sync_all().await.map_err(storage)?;
    drop(file);
    tokio::fs::rename(&temporary, path.with_extension("resume"))
        .await
        .map_err(storage)?;
    sync_directory(path).await
}
async fn sync_directory(path: &Path) -> Result<(), Failure> {
    #[cfg(unix)]
    {
        let directory = path.parent().unwrap().to_owned();
        tokio::task::spawn_blocking(move || std::fs::File::open(directory)?.sync_all())
            .await
            .map_err(storage)?
            .map_err(storage)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn complete(upload: &Upload) -> Value {
    json!({"ok":true,"resumable":true,"offset":upload.size,"total":upload.size,
        "upload_id":upload.id,"name":upload.name,"size":upload.size})
}

// Callers hold the existing per-(draft, upload id) lock throughout these
// operations. Native session files are never touched by staging.
pub async fn status(service: &Conversations, key: &str, id: &str) -> Result<Value, Failure> {
    if let Ok(upload) = service.store.upload(key, id) {
        return Ok(complete(&upload));
    }
    let path = service.upload_path(key, id);
    match checkpoint(&path).await? {
        Some(value) if value.offset == value.total => finish(service, key, id, &path, &value).await,
        Some(value) => Ok(json!({"ok":true,"resumable":true,"offset":value.offset,
            "total":value.total,"name":value.name})),
        None => Ok(json!({"ok":true,"resumable":true,"offset":0})),
    }
}

pub struct Receiving {
    path: PathBuf,
    value: Checkpoint,
    file: tokio::fs::File,
    received: u64,
}
impl Receiving {
    pub async fn begin(
        path: PathBuf,
        name: &str,
        mime: String,
        total: u64,
        offset: u64,
    ) -> Result<Self, Failure> {
        if total == 0 {
            return Err(Failure::new(400, "file_upload_empty", "附件为空"));
        }
        if total > crate::files::WriteService::BUG_REPORT_ATTACHMENT_MAX_BYTES as u64 {
            return Err(Failure::new(
                413,
                "file_upload_too_large",
                "单个附件不能超过 512 MiB",
            ));
        }
        let name = crate::files::WriteService::attachment_name(name);
        let directory = path.parent().unwrap();
        tokio::fs::create_dir_all(directory)
            .await
            .map_err(storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                .await
                .map_err(storage)?;
        }
        let value = if let Some(value) = checkpoint(&path).await? {
            if value.name != name
                || value.mime != mime
                || value.total != total
                || value.offset != offset
            {
                return Err(conflict());
            }
            value
        } else {
            if offset != 0 {
                return Err(conflict());
            }
            let value = Checkpoint {
                name,
                mime,
                total,
                offset: 0,
            };
            save_checkpoint(&path, &value).await?;
            value
        };
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create(true).truncate(false);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(path.with_extension("part"))
            .await
            .map_err(storage)?;
        if file.metadata().await.map_err(storage)?.len() < offset {
            return Err(conflict());
        }
        file.set_len(offset).await.map_err(storage)?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(storage)?;
        Ok(Self {
            path,
            value,
            file,
            received: 0,
        })
    }
    pub async fn append(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        let end = self.value.offset + self.received + bytes.len() as u64;
        if end > self.value.total {
            return Err(conflict());
        }
        self.file.write_all(bytes).await.map_err(storage)?;
        self.received += bytes.len() as u64;
        Ok(())
    }
    pub async fn commit(
        mut self,
        service: &Conversations,
        key: &str,
        id: &str,
    ) -> Result<Value, Failure> {
        if self.received == 0 {
            return Err(conflict());
        }
        self.file.sync_all().await.map_err(storage)?;
        drop(self.file);
        self.value.offset += self.received;
        save_checkpoint(&self.path, &self.value).await?;
        if self.value.offset == self.value.total {
            finish(service, key, id, &self.path, &self.value).await
        } else {
            Ok(
                json!({"ok":true,"resumable":true,"offset":self.value.offset,"total":self.value.total}),
            )
        }
    }
}

async fn finish(
    service: &Conversations,
    key: &str,
    id: &str,
    path: &Path,
    value: &Checkpoint,
) -> Result<Value, Failure> {
    // If the process stopped between rename and ledger commit, the complete
    // file is already at `path`. Status finishes the same staging transaction.
    let partial = path.with_extension("part");
    let (mut file, needs_rename) = match tokio::fs::File::open(&partial).await {
        Ok(file) => (file, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (tokio::fs::File::open(path).await.map_err(storage)?, false)
        }
        Err(e) => return Err(storage(e)),
    };
    if file.metadata().await.map_err(storage)?.len() != value.total {
        return Err(conflict());
    }
    // Preserve the existing upload ledger's digest, computed once on completion.
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; crate::files::STREAM_CHUNK_BYTES];
    loop {
        let count = file.read(&mut buffer).await.map_err(storage)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    drop(file);
    if needs_rename {
        tokio::fs::rename(&partial, path).await.map_err(storage)?;
    }
    sync_directory(path).await?;
    let upload = Upload {
        key: key.to_owned(),
        id: id.to_owned(),
        name: value.name.clone(),
        mime: value.mime.clone(),
        size: value.total,
        published: None,
        sha256: format!("{:x}", hash.finalize()),
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    service.store.note_upload(upload.clone())?;
    let _ = tokio::fs::remove_file(path.with_extension("resume")).await;
    Ok(complete(&upload))
}
