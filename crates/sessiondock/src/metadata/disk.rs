//! Metadata reads and atomic replacement in the configured directory.

use super::{METADATA_FILENAME, MetadataError, MetadataSnapshot, SCHEMA_VERSION, model::Document};
use sha1::{Digest, Sha1};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub(super) struct Disk {
    directory: PathBuf,
    #[cfg(test)]
    pub(super) failpoint: std::sync::atomic::AtomicU8,
}

fn io_error(error: io::Error) -> MetadataError {
    MetadataError::new(
        if error.kind() == io::ErrorKind::PermissionDenied {
            403
        } else {
            503
        },
        "metadata_io",
        format!("元数据操作失败：{error}"),
    )
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}

impl Disk {
    pub(super) fn open(directory: &Path) -> Result<Self, MetadataError> {
        fs::create_dir_all(directory).map_err(io_error)?;
        Ok(Self {
            directory: directory.canonicalize().map_err(io_error)?,
            #[cfg(test)]
            failpoint: std::sync::atomic::AtomicU8::new(0),
        })
    }
    pub(super) fn directory(&self) -> &Path {
        &self.directory
    }
    pub(super) fn load(&self) -> Result<(MetadataSnapshot, Option<String>), MetadataError> {
        let Ok(bytes) = fs::read(self.directory.join(METADATA_FILENAME)) else {
            return Ok((MetadataSnapshot::empty(), None));
        };
        // An unavailable, malformed or unsupported document is treated
        // as empty. Deserialize through Value so repeated
        // JSON keys have last-value behavior.
        let document = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|value| serde_json::from_value::<Document>(value).ok())
            .filter(|document| document.schema_version == SCHEMA_VERSION);
        let snapshot = document
            .map(|document| MetadataSnapshot { document })
            .unwrap_or_else(MetadataSnapshot::empty);
        Ok((snapshot, Some(hash(&bytes))))
    }
    pub(super) fn persist(&self, snapshot: &MetadataSnapshot) -> Result<String, MetadataError> {
        let mut bytes = serde_json::to_vec(&snapshot.document)
            .map_err(|_| MetadataError::new(503, "metadata_serialize", "无法序列化元数据"))?;
        bytes.push(b'\n');
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|_| MetadataError::new(503, "metadata_entropy", "无法创建元数据暂存文件"))?;
        let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = self.directory.join(format!(".metadata-tmp-{name}"));
        let result = (|| {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&path).map_err(io_error)?;
            if self.fail(1) {
                return Err(io_error(io::Error::other("模拟写入失败")));
            }
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(io_error)?;
            drop(file);
            if self.fail(2) {
                return Err(io_error(io::Error::other("模拟替换失败")));
            }
            fs::rename(&path, self.directory.join(METADATA_FILENAME)).map_err(io_error)?;
            if self.fail(3) {
                return Err(MetadataError::uncertain());
            }
            #[cfg(unix)]
            fs::File::open(&self.directory)
                .and_then(|dir| dir.sync_all())
                .map_err(|_| MetadataError::uncertain())?;
            Ok(hash(&bytes))
        })();
        let _ = fs::remove_file(&path);
        result
    }
    fn fail(&self, point: u8) -> bool {
        #[cfg(test)]
        {
            self.failpoint.load(std::sync::atomic::Ordering::Relaxed) == point
        }
        #[cfg(not(test))]
        {
            let _ = point;
            false
        }
    }
}
