//! Cross-node evidence: compare contents, not Git status or machine-local inodes.
//! Local stamps detect writes during a scan and between plan and confirmation.
use super::{TransferError, codex};
#[cfg(unix)]
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

/// Composer uploads belong to the workspace, not to the native session.
/// Classify lexically: missing files must not require a stat or canonicalize.
pub(super) fn workspace_upload(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str() == "sessiondock_attachments")
}

// Keep UTF-8 paths in the original JSON format, including map keys. A leading
// NUL is reserved for encoded Unix bytes because it cannot occur in a path.
struct WirePath<'a>(&'a Path);

impl Serialize for WirePath<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[cfg(unix)]
        if self.0.to_str().is_none() {
            return serializer.serialize_str(&format!(
                "\0{}",
                STANDARD.encode(self.0.as_os_str().as_encoded_bytes())
            ));
        }
        self.0.serialize(serializer)
    }
}

fn decode_path<E: serde::de::Error>(path: String) -> Result<PathBuf, E> {
    if let Some(encoded) = path.strip_prefix('\0') {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let bytes = STANDARD.decode(encoded).map_err(E::custom)?;
            return Ok(std::ffi::OsString::from_vec(bytes).into());
        }
        #[cfg(not(unix))]
        {
            let _ = encoded;
            return Err(E::custom("迁移路径字节要求 Unix"));
        }
    }
    Ok(path.into())
}

fn serialize_paths<S, T>(entries: &BTreeMap<PathBuf, T>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    T: Serialize,
{
    serializer.collect_map(entries.iter().map(|(path, value)| (WirePath(path), value)))
}

fn without_workspace_uploads<'de, D, T>(deserializer: D) -> Result<BTreeMap<PathBuf, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let mut entries = BTreeMap::new();
    for (path, value) in BTreeMap::<String, T>::deserialize(deserializer)? {
        let path = decode_path(path)?;
        if !workspace_upload(&path) {
            entries.insert(path, value);
        }
    }
    Ok(entries)
}

mod link_path {
    use super::{WirePath, decode_path};
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    pub(super) fn serialize<S: serde::Serializer>(
        link: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        link.as_deref().map(WirePath).serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(decode_path)
            .transpose()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Content {
    pub kind: String,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub executable: u32,
    #[serde(default, with = "link_path")]
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
pub(super) fn stamp(meta: &fs::Metadata) -> Stamp {
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
    #[serde(
        serialize_with = "serialize_paths",
        deserialize_with = "without_workspace_uploads"
    )]
    pub entries: BTreeMap<PathBuf, Content>,
    /// Absolute names reached through links or explicitly referenced attachments.
    #[serde(
        serialize_with = "serialize_paths",
        deserialize_with = "without_workspace_uploads"
    )]
    pub dependencies: BTreeMap<PathBuf, Content>,
    #[serde(
        serialize_with = "serialize_paths",
        deserialize_with = "without_workspace_uploads"
    )]
    pub stamps: BTreeMap<PathBuf, Stamp>,
    /// Publication may create these directories before a relative link is used.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    bypassed_directories: BTreeSet<PathBuf>,
}
impl Snapshot {
    pub fn capture(cwd: &Path, dependencies: &[PathBuf]) -> Result<Self, TransferError> {
        Self::capture_with_directories(cwd, dependencies, &BTreeSet::new())
    }
    pub(super) fn capture_with_directories(
        cwd: &Path,
        dependencies: &[PathBuf],
        planned_directories: &BTreeSet<PathBuf>,
    ) -> Result<Self, TransferError> {
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
            bypassed_directories: BTreeSet::new(),
        };
        let mut seen = BTreeSet::new();
        // The working directory is an execution location, not migration data.
        // Never enumerate it or follow unrelated project links.

        for path in dependencies {
            result
                .walk(path, &mut seen, planned_directories)
                .map_err(|mut error| {
                    error.message = format!("外部历史依赖 {}：{}", path.display(), error.message);
                    error
                })?;
        }
        result.recheck()?;
        Ok(result)
    }
    fn lookup<'a>(
        &mut self,
        path: &'a Path,
        planned_directories: &BTreeSet<PathBuf>,
    ) -> io::Result<(Cow<'a, Path>, fs::Metadata)> {
        let missing = match fs::symlink_metadata(path) {
            Ok(meta) => return Ok((Cow::Borrowed(path), meta)),
            Err(error)
                if error.kind() == io::ErrorKind::NotFound && !planned_directories.is_empty() =>
            {
                error
            }
            Err(error) => return Err(error),
        };
        // A planned directory can be traversed out of before it exists.
        // Leave ".." intact at existing symlinks and at unplanned paths so the
        // filesystem still decides what those components mean.
        let mut lookup = PathBuf::new();
        for component in path.components() {
            if component == Component::ParentDir
                && planned_directories.contains(&lookup)
                && ordinary_or_missing_directory(&lookup)?
            {
                self.bypassed_directories.insert(lookup.clone());
                lookup.pop();
            } else {
                lookup.push(component.as_os_str());
            }
        }
        if lookup == path {
            return Err(missing);
        }
        // A terminal slash or "/." still requires directory traversal.
        let spelling = path.as_os_str().as_encoded_bytes();
        if spelling.ends_with(b"/") || spelling.ends_with(b"/.") {
            lookup.push(".");
        }
        let meta = fs::symlink_metadata(&lookup)?;
        Ok((Cow::Owned(lookup), meta))
    }
    fn walk(
        &mut self,
        path: &Path,
        seen: &mut BTreeSet<PathBuf>,
        planned_directories: &BTreeSet<PathBuf>,
    ) -> Result<(), TransferError> {
        super::coordination::check()?;
        if workspace_upload(path) {
            return Ok(());
        }
        if !seen.insert(path.to_owned()) {
            return Ok(());
        }
        let (lookup, meta) = self.lookup(path, planned_directories)?;
        let lookup = lookup.as_ref();
        let before = stamp(&meta);
        if self
            .stamps
            .get(lookup)
            .is_some_and(|prior| prior != &before)
        {
            return Err(stale());
        }
        self.stamps.insert(lookup.to_owned(), before.clone());
        let mut content = Content {
            kind: String::new(),
            bytes: 0,
            sha256: None,
            executable: before.mode & 0o111,
            link: None,
        };
        if meta.is_symlink() {
            content.kind = "symlink".into();
            let link = fs::read_link(lookup)?;
            content.link = Some(link);
            // canonicalize detects dangling links and link-only loops. Directory
            // cycles terminate through `seen`; their link target is still recorded.
            let resolved = lookup.canonicalize()?;
            self.walk(&resolved, seen, planned_directories)?;
        } else if meta.is_dir() {
            content.kind = "directory".into();
            let mut children = fs::read_dir(lookup)?
                .map(|e| e.map(|v| v.file_name()))
                .collect::<Result<Vec<_>, _>>()?;
            children.sort();
            for child in children {
                if child == ".git" {
                    continue;
                }
                self.walk(&path.join(child), seen, planned_directories)?;
            }
        } else if meta.is_file() {
            content.kind = "file".into();
            let mut file = fs::File::open(lookup)?;
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
        if stamp(&fs::symlink_metadata(lookup)?) != before {
            return Err(stale());
        }
        self.dependencies.insert(path.into(), content);
        Ok(())
    }
    pub fn recheck(&self) -> Result<(), TransferError> {
        super::coordination::check()?;
        if !self.cwd.is_dir() {
            return Err(TransferError::new("move_cwd_missing", "目标工作目录不存在"));
        }
        for path in &self.bypassed_directories {
            if !ordinary_or_missing_directory(path)? {
                return Err(stale());
            }
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

fn ordinary_or_missing_directory(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            fs::symlink_metadata(path.join(".."))?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
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
