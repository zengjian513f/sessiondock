//! Cross-node evidence: compare contents, not Git status or machine-local inodes.
//! Local stamps detect writes during a scan and between plan and confirmation.
use super::{TransferError, codex};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// Composer uploads belong to the workspace, not to the native session.
/// Classify lexically: missing files must not require a stat or canonicalize.
pub(super) fn workspace_upload(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str() == "sessiondock_attachments")
}

fn without_workspace_uploads<'de, D, T>(deserializer: D) -> Result<BTreeMap<PathBuf, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let mut entries = BTreeMap::<PathBuf, T>::deserialize(deserializer)?;
    entries.retain(|path, _| !workspace_upload(path));
    Ok(entries)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Content {
    pub kind: String,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub executable: u32,
    pub link: Option<PathBuf>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Stamp {
    pub length: u64,
    pub modified: Option<(u64, u32)>,
    pub device: u64,
    pub inode: u64,
    pub changed: i64,
    pub changed_ns: i64,
    pub mode: u32,
}
fn stamp(meta: &fs::Metadata) -> Stamp {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Stamp {
        length: meta.len(),
        modified: meta
            .modified()
            .ok()
            .and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|v| (v.as_secs(), v.subsec_nanos())),
        #[cfg(unix)]
        device: meta.dev(),
        #[cfg(not(unix))]
        device: 0,
        #[cfg(unix)]
        inode: meta.ino(),
        #[cfg(not(unix))]
        inode: 0,
        #[cfg(unix)]
        changed: meta.ctime(),
        #[cfg(not(unix))]
        changed: 0,
        #[cfg(unix)]
        changed_ns: meta.ctime_nsec(),
        #[cfg(not(unix))]
        changed_ns: 0,
        #[cfg(unix)]
        mode: meta.mode(),
        #[cfg(not(unix))]
        mode: 0,
    }
}
fn stale() -> TransferError {
    TransferError::new("move_plan_stale", "工作目录或外部依赖在核对期间发生变化")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub cwd: PathBuf,
    /// Legacy wire field; new snapshots never enumerate the work tree.
    #[serde(deserialize_with = "without_workspace_uploads")]
    pub entries: BTreeMap<PathBuf, Content>,
    /// Absolute names reached through links or explicitly referenced attachments.
    #[serde(deserialize_with = "without_workspace_uploads")]
    pub dependencies: BTreeMap<PathBuf, Content>,
    #[serde(deserialize_with = "without_workspace_uploads")]
    pub stamps: BTreeMap<PathBuf, Stamp>,
}
impl Snapshot {
    pub fn capture(cwd: &Path, dependencies: &[PathBuf]) -> Result<Self, TransferError> {
        if !cwd.is_absolute() {
            return Err(TransferError::new(
                "move_cwd_missing",
                "工作目录必须是绝对路径",
            ));
        }
        if !cwd.is_dir() {
            return Err(TransferError::new("move_cwd_missing", "工作目录不存在"));
        }
        let mut result = Self {
            cwd: cwd.into(),
            entries: BTreeMap::new(),
            dependencies: BTreeMap::new(),
            stamps: BTreeMap::new(),
        };
        let mut seen = BTreeSet::new();
        // The working directory is an execution location, not migration data.
        // Never enumerate it or follow unrelated project links.

        for path in dependencies {
            result.walk(path, false, &mut seen).map_err(|mut error| {
                error.message = format!("外部历史依赖 {}：{}", path.display(), error.message);
                error
            })?;
        }
        result.recheck()?;
        Ok(result)
    }
    fn walk(
        &mut self,
        path: &Path,
        tree: bool,
        seen: &mut BTreeSet<(PathBuf, bool)>,
    ) -> Result<(), TransferError> {
        super::coordination::check()?;
        if workspace_upload(path) {
            return Ok(());
        }
        if !seen.insert((path.to_owned(), tree)) {
            return Ok(());
        }
        let meta = fs::symlink_metadata(path)?;
        let before = stamp(&meta);
        if self.stamps.get(path).is_some_and(|prior| prior != &before) {
            return Err(stale());
        }
        self.stamps.insert(path.to_owned(), before.clone());
        let mut content = Content {
            kind: String::new(),
            bytes: 0,
            sha256: None,
            executable: before.mode & 0o111,
            link: None,
        };
        if meta.is_symlink() {
            content.kind = "symlink".into();
            let link = fs::read_link(path)?;
            content.link = Some(link);
            // canonicalize detects dangling links and link-only loops. Directory
            // cycles terminate through `seen`; their link target is still recorded.
            let resolved = path.canonicalize()?;
            self.walk(&resolved, false, seen)?;
        } else if meta.is_dir() {
            content.kind = "directory".into();
            let mut children = fs::read_dir(path)?
                .map(|e| e.map(|v| v.path()))
                .collect::<Result<Vec<_>, _>>()?;
            children.sort();
            for child in children {
                if child.file_name().is_some_and(|name| name == ".git") {
                    continue;
                }
                self.walk(&child, tree, seen)?;
            }
        } else if meta.is_file() {
            content.kind = "file".into();
            let mut file = fs::File::open(path)?;
            if stamp(&file.metadata()?) != before {
                return Err(stale());
            }
            let mut hash = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                super::coordination::check()?;
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                content.bytes += count as u64;
                hash.update(&buffer[..count]);
            }
            content.sha256 = Some(format!("{:x}", hash.finalize()));
            if stamp(&file.metadata()?) != before {
                return Err(stale());
            }
        } else {
            // Never open a FIFO/socket/device while hashing a working tree.
            content.kind = format!("special:{:o}", before.mode & 0o170000);
        }
        if stamp(&fs::symlink_metadata(path)?) != before {
            return Err(stale());
        }
        if tree {
            self.entries.insert(
                path.strip_prefix(&self.cwd)
                    .map_err(|_| TransferError::new("move_path", "工作目录扫描越界"))?
                    .to_owned(),
                content,
            );
        } else {
            self.dependencies.insert(path.into(), content);
        }
        Ok(())
    }
    pub fn recheck(&self) -> Result<(), TransferError> {
        super::coordination::check()?;
        if !self.cwd.is_dir() {
            return Err(TransferError::new("move_cwd_missing", "目标工作目录不存在"));
        }
        for (path, before) in &self.stamps {
            let meta = fs::symlink_metadata(path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    stale()
                } else {
                    e.into()
                }
            })?;
            if stamp(&meta) != *before {
                return Err(stale());
            }
        }
        Ok(())
    }
    pub fn compare(&self, target: &Self) -> Result<(), TransferError> {
        if self.cwd != target.cwd {
            return Err(TransferError::new(
                "move_cwd_mismatch",
                "两端工作目录路径不同",
            ));
        }
        if self.entries != target.entries || self.dependencies != target.dependencies {
            return Err(TransferError::new(
                "move_cwd_mismatch",
                "工作目录或外部依赖内容不同",
            ));
        }
        Ok(())
    }
}

/// A random, exclusively-created witness, not an inode comparison across hosts.
/// A destination looks for this exact nonce before any bundle has been copied.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StorageProbe {
    pub name: String,
    pub nonce: String,
}
impl StorageProbe {
    fn path(&self, root: &Path) -> Result<PathBuf, TransferError> {
        if self.name.len() != 36
            || !self
                .name
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b'-')
        {
            return Err(TransferError::new("move_path", "存储核对标识无效"));
        }
        Ok(root.join(format!(".sessiondock-transfer-probe-{}", self.name)))
    }
    pub fn create(root: &Path) -> Result<Self, TransferError> {
        let probe = Self {
            name: codex::uuid()?,
            nonce: codex::uuid()?,
        };
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut out = options.open(probe.path(root)?)?;
        out.write_all(probe.nonce.as_bytes())?;
        out.sync_all()?;
        fs::File::open(root)?.sync_all()?;
        Ok(probe)
    }
    pub fn shared(&self, root: &Path) -> Result<bool, TransferError> {
        let path = self.path(root)?;
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        if !meta.is_file() || meta.len() != self.nonce.len() as u64 {
            return Ok(false);
        }
        Ok(fs::read(path)? == self.nonce.as_bytes())
    }
    pub fn remove(&self, root: &Path) -> Result<(), TransferError> {
        if !self.shared(root)? {
            return Err(TransferError::new(
                "move_recovery_required",
                "存储核对文件已变化，保留现场",
            ));
        }
        fs::remove_file(self.path(root)?)?;
        fs::File::open(root)?.sync_all()?;
        Ok(())
    }
}
