use super::{FileError, ListOptions, MAX_DIRECTORY_ENTRIES, MAX_PATH_BYTES};
use cap_fs_ext::{DirExt, FollowSymlinks, MetadataExt, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, Metadata, OpenOptions},
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs::File,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Identity(u64, u64);
pub(super) fn identity(metadata: &Metadata) -> Identity {
    Identity(metadata.dev(), metadata.ino())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    identity: Identity,
    size: u64,
    modified: Option<cap_std::time::SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
}
impl Stamp {
    pub fn new(metadata: &Metadata) -> Self {
        Self {
            identity: identity(metadata),
            size: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            changed: (
                cap_std::fs::MetadataExt::ctime(metadata),
                cap_std::fs::MetadataExt::ctime_nsec(metadata),
            ),
        }
    }
}

/// Opaque checked identity: no caller can manufacture a filesystem version.
/// Parent identities make replacing a directory visible even if its leaf inode
/// is moved unchanged into the replacement directory.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FileVersion {
    path: PathBuf,
    stamp: Stamp,
    root_ancestors: Vec<Identity>,
    parents: Vec<Identity>,
}

fn link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.is_symlink()
}

/// Read-side entry check: no symlink/reparse point, no device/pipe/socket,
/// readable. A regular file with several hard links is an ordinary file for
/// reading (Python's file manager and bug-report attachments link files into
/// the project tree); the write side additionally requires `unshared`.
pub(super) fn ordinary(metadata: &Metadata) -> Result<(), FileError> {
    if link(metadata) {
        return Err(FileError::new(
            403,
            "file_symlink_forbidden",
            "开发文件服务不跟随符号链接或重解析点",
        ));
    }
    if !metadata.is_dir() && !metadata.is_file() {
        return Err(FileError::new(
            403,
            "file_special_forbidden",
            "只允许普通文件与目录，不读取设备、管道或套接字",
        ));
    }
    #[cfg(unix)]
    {
        let mode = cap_std::fs::MetadataExt::mode(metadata);
        if mode & 0o444 == 0 || (metadata.is_dir() && mode & 0o111 == 0) {
            return Err(FileError::new(
                403,
                "file_forbidden",
                "文件缺少读取权限，或目录缺少访问权限",
            ));
        }
    }
    Ok(())
}
/// Write-side entry check: `ordinary` plus exactly one link for regular
/// files, so a rename/move/delete never detaches one alias of shared data.
pub(super) fn unshared(metadata: &Metadata) -> Result<(), FileError> {
    ordinary(metadata)?;
    if metadata.is_file() && metadata.nlink() != 1 {
        return Err(FileError::new(
            403,
            "file_hardlink_forbidden",
            "写入服务不移动、改名或删除具有多个硬链接的文件",
        ));
    }
    Ok(())
}

struct Edge {
    parent: Arc<Dir>,
    name: OsString,
    identity: Identity,
}
impl Edge {
    fn verify(&self) -> Result<(), FileError> {
        let now = self
            .parent
            .open_dir_nofollow(&self.name)
            .map_err(|_| FileError::changed())?;
        let metadata = now.dir_metadata().map_err(FileError::io)?;
        ordinary(&metadata)?;
        if identity(&metadata) != self.identity {
            return Err(FileError::changed());
        }
        Ok(())
    }
}

pub(super) struct Root {
    pub path: PathBuf,
    pub(super) directory: Arc<Dir>,
    ancestors: Vec<Edge>,
}
impl Root {
    pub fn open(raw: &Path) -> Result<Self, FileError> {
        let text = raw.to_str().ok_or_else(|| {
            FileError::new(
                400,
                "file_path_encoding",
                "开发文件目录必须能表示为 UTF-8 路径",
            )
        })?;
        let path = absolute_navigation(text)?;
        if path.parent().is_none() {
            return Err(FileError::new(
                403,
                "file_root_too_broad",
                "文件系统根目录不能作为开发文件授权目录",
            ));
        }
        let mut base = PathBuf::new();
        let mut names = Vec::new();
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => base.push(component.as_os_str()),
                Component::Normal(name) => names.push(name.to_os_string()),
                _ => {
                    return Err(FileError::new(
                        400,
                        "file_path_invalid",
                        "开发文件目录需要标准绝对路径",
                    ));
                }
            }
        }
        // Ambient authority is used only for the explicit absolute path's volume
        // root. Every actual path component is opened relative and no-follow.
        let mut directory =
            Arc::new(Dir::open_ambient_dir(base, ambient_authority()).map_err(FileError::io)?);
        let mut ancestors = Vec::new();
        for name in names {
            let before = directory.symlink_metadata(&name).map_err(FileError::io)?;
            ordinary(&before)?;
            if !before.is_dir() {
                return Err(FileError::new(
                    400,
                    "file_root_not_directory",
                    "开发文件授权入口必须是既有目录",
                ));
            }
            let child = Arc::new(directory.open_dir_nofollow(&name).map_err(FileError::io)?);
            let metadata = child.dir_metadata().map_err(FileError::io)?;
            if identity(&before) != identity(&metadata) {
                return Err(FileError::changed());
            }
            ancestors.push(Edge {
                parent: directory,
                name,
                identity: identity(&metadata),
            });
            directory = child;
        }
        let result = Self {
            path,
            directory,
            ancestors,
        };
        result.verify()?;
        Ok(result)
    }
    pub(super) fn verify(&self) -> Result<(), FileError> {
        for edge in &self.ancestors {
            edge.verify()?;
        }
        Ok(())
    }
}

pub(super) enum OpenTarget {
    Directory(Arc<Dir>),
    File(File),
}
/// Unforgeable transport handle: its path is display-only, never reopen it.
pub struct ResolvedTarget {
    pub(super) root: Arc<Root>,
    path: PathBuf,
    pub(super) opened: OpenTarget,
    chain: Vec<Edge>,
    location: Option<(Arc<Dir>, OsString)>,
    pub(super) stamp: Stamp,
    pub(super) metadata: Metadata,
}
impl ResolvedTarget {
    pub(super) fn version(&self) -> FileVersion {
        FileVersion {
            path: self.path.clone(),
            stamp: self.stamp.clone(),
            root_ancestors: self
                .root
                .ancestors
                .iter()
                .map(|edge| edge.identity)
                .collect(),
            parents: self.chain.iter().map(|edge| edge.identity).collect(),
        }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn kind(&self) -> &'static str {
        if self.metadata.is_dir() {
            "directory"
        } else {
            "file"
        }
    }
    pub(super) fn verify(&self) -> Result<(), FileError> {
        self.root.verify()?;
        for edge in &self.chain {
            edge.verify()?;
        }
        let opened = match &self.opened {
            OpenTarget::Directory(directory) => directory.dir_metadata(),
            OpenTarget::File(file) => Metadata::from_file(file),
        }
        .map_err(FileError::io)?;
        ordinary(&opened)?;
        if Stamp::new(&opened) != self.stamp {
            return Err(FileError::changed());
        }
        if let Some((parent, name)) = &self.location {
            let current = parent
                .symlink_metadata(name)
                .map_err(|_| FileError::changed())?;
            ordinary(&current)?;
            if Stamp::new(&current) != self.stamp {
                return Err(FileError::changed());
            }
        }
        Ok(())
    }
    pub(super) fn file(&self) -> Result<&File, FileError> {
        match &self.opened {
            OpenTarget::File(file) => Ok(file),
            _ => Err(FileError::new(400, "file_required", "此操作需要普通文件")),
        }
    }
    /// Identity-only recheck for a directory that the caller is about to
    /// change: root ancestors, every intermediate component and the opened
    /// directory must still be the same inodes reached without following a
    /// link. Size/mtime are intentionally not compared, because the write
    /// itself changes them.
    pub(super) fn verify_identity(&self) -> Result<Identity, FileError> {
        self.root.verify()?;
        for edge in &self.chain {
            edge.verify()?;
        }
        let opened = match &self.opened {
            OpenTarget::Directory(directory) => directory.dir_metadata(),
            OpenTarget::File(file) => Metadata::from_file(file),
        }
        .map_err(FileError::io)?;
        if identity(&opened) != self.stamp.identity {
            return Err(FileError::changed());
        }
        ordinary(&opened)?;
        if let Some((parent, name)) = &self.location {
            // A swapped-in link or replacement is a different inode: report
            // the race itself, not the type of the replacement.
            let current = parent
                .symlink_metadata(name)
                .map_err(|_| FileError::changed())?;
            if identity(&current) != self.stamp.identity {
                return Err(FileError::changed());
            }
            ordinary(&current)?;
        }
        Ok(self.stamp.identity)
    }
    pub(super) fn directory(&self) -> Result<&Arc<Dir>, FileError> {
        match &self.opened {
            OpenTarget::Directory(directory) => Ok(directory),
            _ => Err(FileError::new(
                400,
                "file_directory_required",
                "此操作需要目录",
            )),
        }
    }
}

pub(super) fn open_target(root: Arc<Root>, path: PathBuf) -> Result<ResolvedTarget, FileError> {
    root.verify()?;
    let relative = path
        .strip_prefix(&root.path)
        .map_err(|_| FileError::new(403, "file_outside_roots", "路径越出授权目录"))?;
    let names: Vec<_> = relative
        .components()
        .map(|component| match component {
            Component::Normal(name) => Ok(name.to_os_string()),
            _ => Err(FileError::new(400, "file_path_invalid", "路径组件无效")),
        })
        .collect::<Result<_, _>>()?;
    let mut directory = root.directory.clone();
    let mut chain = Vec::new();
    let mut location = None;
    let opened;
    let metadata;
    if names.is_empty() {
        metadata = directory.dir_metadata().map_err(FileError::io)?;
        opened = OpenTarget::Directory(directory);
    } else {
        for name in &names[..names.len() - 1] {
            let before = directory.symlink_metadata(name).map_err(FileError::io)?;
            ordinary(&before)?;
            if !before.is_dir() {
                return Err(FileError::new(
                    400,
                    "file_parent_not_directory",
                    "路径的父组件不是目录",
                ));
            }
            let child = Arc::new(directory.open_dir_nofollow(name).map_err(FileError::io)?);
            let current = child.dir_metadata().map_err(FileError::io)?;
            if identity(&before) != identity(&current) {
                return Err(FileError::changed());
            }
            chain.push(Edge {
                parent: directory,
                name: name.clone(),
                identity: identity(&current),
            });
            directory = child;
        }
        let name = names.last().unwrap();
        let before = directory.symlink_metadata(name).map_err(FileError::io)?;
        ordinary(&before)?;
        location = Some((directory.clone(), name.clone()));
        if before.is_dir() {
            let child = Arc::new(directory.open_dir_nofollow(name).map_err(FileError::io)?);
            metadata = child.dir_metadata().map_err(FileError::io)?;
            opened = OpenTarget::Directory(child);
        } else {
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No).nonblock(true);
            let file = directory.open_with(name, &options).map_err(FileError::io)?;
            metadata = file.metadata().map_err(FileError::io)?;
            ordinary(&metadata)?;
            opened = OpenTarget::File(file.into_std());
        }
        if Stamp::new(&before) != Stamp::new(&metadata) {
            return Err(FileError::changed());
        }
    }
    let stamp = Stamp::new(&metadata);
    let result = ResolvedTarget {
        root,
        path,
        opened,
        chain,
        location,
        stamp,
        metadata,
    };
    result.verify()?;
    Ok(result)
}

pub(super) fn validate_path_text(raw: &str) -> Result<(), FileError> {
    if raw.is_empty()
        || raw.len() > MAX_PATH_BYTES
        || raw.chars().any(char::is_control)
        || raw.contains("://")
        || raw.starts_with('~')
    {
        return Err(FileError::new(
            400,
            "file_path_invalid",
            "路径为空、过长或含不支持的字符；不展开 HOME 或 URL",
        ));
    }
    #[cfg(not(windows))]
    if raw.contains('\\')
        || (raw.as_bytes().get(1) == Some(&b':') && raw.as_bytes()[0].is_ascii_alphabetic())
    {
        return Err(FileError::new(
            400,
            "file_foreign_path",
            "当前平台不接受 Windows 分隔符或驱动器路径",
        ));
    }
    #[cfg(windows)]
    {
        if raw.starts_with(['/', '\\']) {
            return Err(FileError::new(
                400,
                "file_path_invalid",
                "Windows 仅接受显式盘符绝对路径，不接受 UNC/设备/隐式当前盘路径",
            ));
        }
        for (index, component) in raw.split(['/', '\\']).enumerate() {
            if index == 0 && component.len() == 2 && component.as_bytes()[1] == b':' {
                continue;
            }
            if component.contains(':')
                || (!matches!(component, "." | "..") && component.ends_with(['.', ' ']))
            {
                return Err(FileError::new(
                    400,
                    "file_path_invalid",
                    "不接受 Windows 数据流或歧义路径组件",
                ));
            }
            let stem = component
                .split('.')
                .next()
                .unwrap_or("")
                .to_ascii_uppercase();
            if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.as_bytes()[3].is_ascii_digit())
            {
                return Err(FileError::new(
                    400,
                    "file_path_invalid",
                    "不接受 Windows 保留设备名称",
                ));
            }
        }
    }
    Ok(())
}
fn normalized(path: &Path) -> Result<PathBuf, FileError> {
    let mut result = PathBuf::new();
    let mut depth = 0;
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    return Err(FileError::new(
                        403,
                        "file_outside_roots",
                        "路径越出文件系统根目录",
                    ));
                }
            }
            _ => {
                result.push(component.as_os_str());
                depth += 1;
            }
        }
        if depth > 64 {
            return Err(FileError::new(
                413,
                "file_path_depth",
                "路径超过 64 层解析预算",
            ));
        }
    }
    Ok(result)
}
pub(super) fn absolute_navigation(raw: &str) -> Result<PathBuf, FileError> {
    validate_path_text(raw)?;
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(FileError::new(
            400,
            "file_absolute_path_required",
            "目录导航需要绝对路径",
        ));
    }
    normalized(path)
}
pub(super) fn absolute_path(raw: &str, cwd: &str) -> Result<PathBuf, FileError> {
    validate_path_text(raw)?;
    let path = Path::new(raw);
    if path.is_absolute() {
        return normalized(path);
    }
    let base = absolute_navigation(cwd).map_err(|_| {
        FileError::new(
            400,
            "file_cwd_unavailable",
            "相对引用需要有效的所选会话工作目录；不会使用服务进程目录",
        )
    })?;
    normalized(&base.join(path))
}
pub(super) fn wire_path(path: &Path) -> Result<String, FileError> {
    let value = path
        .to_str()
        .ok_or_else(|| FileError::new(400, "file_path_encoding", "路径无法表示为 UTF-8"))?;
    #[cfg(windows)]
    return Ok(value.replace('\\', "/"));
    #[cfg(not(windows))]
    Ok(value.into())
}
pub(super) fn modified(metadata: &Metadata) -> Option<f64> {
    metadata
        .modified()
        .ok()?
        .into_std()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs_f64())
}

pub(super) fn list(target: &ResolvedTarget, options: &ListOptions) -> Result<Value, FileError> {
    if !(1..=500).contains(&options.limit)
        || options.offset > MAX_DIRECTORY_ENTRIES
        || !matches!(options.sort.as_str(), "name" | "size" | "modified" | "type")
        || !matches!(options.order.as_str(), "asc" | "desc")
    {
        return Err(FileError::new(
            400,
            "file_list_options",
            "无效的目录分页或排序参数",
        ));
    }
    target.verify()?;
    let OpenTarget::Directory(directory) = &target.opened else {
        return Err(FileError::new(
            400,
            "file_directory_required",
            "请选择目录浏览",
        ));
    };
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    for (count, entry) in directory.entries().map_err(FileError::io)?.enumerate() {
        if count >= MAX_DIRECTORY_ENTRIES {
            return Err(FileError::new(
                413,
                "file_directory_budget",
                "目录超过 10000 项预算；未返回假完整列表，请选择较小目录",
            ));
        }
        let entry = entry.map_err(FileError::io)?;
        let name = entry.file_name().into_string().map_err(|_| {
            FileError::new(
                400,
                "file_path_encoding",
                "目录含非 UTF-8 名称，不能安全导航",
            )
        })?;
        if !options.hidden && name.starts_with('.') {
            continue;
        }
        let path = target.path.join(&name);
        let mut kind = "unavailable";
        let mut size = None;
        let mut timestamp = None;
        let mut symlink = false;
        match directory.symlink_metadata(&name) {
            Ok(metadata) => {
                symlink = link(&metadata);
                match ordinary(&metadata).and_then(|_| validate_path_text(&name)) {
                    Ok(()) => { kind = if metadata.is_dir() { "directory" } else { "file" }; size = metadata.is_file().then_some(metadata.len()); timestamp = modified(&metadata); }
                    Err(error) => errors.push(json!({"name":name,"status":error.status,"code":error.code,"error":error.message})),
                }
            }
            Err(error) => {
                let error = FileError::io(error);
                errors.push(json!({"name":name,"status":error.status,"code":error.code,"error":error.message}));
            }
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_lowercase();
        let file_type = if extension.is_empty() {
            if kind == "directory" {
                "文件夹"
            } else {
                "文件"
            }
        } else {
            &extension
        };
        rows.push(json!({"name":name,"path":wire_path(&path)?,"kind":kind,"size":size,"modified":timestamp,"symlink":symlink,"type":file_type}));
    }
    target.verify()?;
    rows.sort_by(|left, right| {
        let group = (left["kind"] != "directory").cmp(&(right["kind"] != "directory"));
        if !group.is_eq() {
            return group;
        }
        let primary = match options.sort.as_str() {
            "size" | "modified" => left[&options.sort]
                .as_f64()
                .unwrap_or(-1.0)
                .total_cmp(&right[&options.sort].as_f64().unwrap_or(-1.0)),
            "type" => left["type"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&right["type"].as_str().unwrap_or("").to_lowercase()),
            _ => left["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&right["name"].as_str().unwrap_or("").to_lowercase()),
        }
        .then_with(|| left["name"].as_str().cmp(&right["name"].as_str()));
        if options.order == "desc" {
            primary.reverse()
        } else {
            primary
        }
    });
    let total = rows.len();
    let entries: Vec<_> = rows
        .into_iter()
        .skip(options.offset)
        .take(options.limit)
        .collect();
    let end = options.offset.saturating_add(entries.len());
    let parent = if target.path == target.root.path {
        None
    } else {
        target.path.parent().map(wire_path).transpose()?
    };
    Ok(
        json!({"path":wire_path(&target.path)?,"root":wire_path(&target.root.path)?,"parent":parent,"entries":entries,"total":total,"offset":options.offset,"next_offset":if end < total {Some(end)} else {None},"writable":false,"errors":errors,"incomplete":!errors.is_empty()}),
    )
}
