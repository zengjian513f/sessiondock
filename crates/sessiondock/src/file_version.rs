//! Filesystem-only versions for read caches; never reads or hashes file contents.
use std::{fs, path::Path, time::SystemTime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileVersion {
    size: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64, u32),
    #[cfg(windows)]
    attributes: u32,
}
impl FileVersion {
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn same_file(&self, other: &Self) -> bool {
        #[cfg(unix)]
        {
            (self.identity.0, self.identity.1) == (other.identity.0, other.identity.1)
        }
        #[cfg(not(unix))]
        {
            self.created == other.created
        }
    }
    pub fn read(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        Some(Self {
            size: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                    metadata.mode(),
                )
            },
            #[cfg(windows)]
            attributes: {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes()
            },
        })
    }
}
