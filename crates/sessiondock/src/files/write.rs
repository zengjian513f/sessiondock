//! Authenticated operator file mutations through checked parent handles.
//! Reads/writes resolve symlink parents; rename/trash preserve the named leaf.
//! Configured roots enable writes without imposing a directory jail. Default
//! conflicts never overwrite; explicit replace first preserves old data in the
//! scoped private trash. See `docs/files.md`.

use super::{
    FileError, FileScope,
    boundary::{self, Root, identity, ordinary, unshared},
};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, DirBuilder, OpenOptions};
use serde_json::{Value, json};
use std::{
    ffi::{OsStr, OsString},
    io::{self, ErrorKind, Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub const MAX_NAME_BYTES: usize = 255;

pub struct WriteService {
    private_root: Option<PathBuf>,
}
impl WriteService {
    /// `state_dir` is private: attachments and recycled entries never land in it.
    pub fn open(_roots: Vec<PathBuf>, state_dir: Option<PathBuf>) -> Result<Self, FileError> {
        let private_root = state_dir
            .as_ref()
            .map(|path| path.canonicalize().map_err(FileError::io))
            .transpose()?;
        Ok(Self { private_root })
    }

    // ---- authorization -------------------------------------------------

    fn root_for(&self, path: &Path) -> Result<Arc<Root>, FileError> {
        boundary::volume_root(path)
    }

    /// Protect the filesystem root, home itself and the
    /// private file-manager state, including its ancestors and descendants.
    fn guard(&self, path: &Path) -> Result<(), FileError> {
        let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        if path.parent().is_none()
            || home.as_deref() == Some(path)
            || self.private_root.as_ref().is_some_and(|private| {
                resolved.starts_with(private) || private.starts_with(&resolved)
            })
        {
            return Err(FileError::new(
                403,
                "file_root_immutable",
                "不能修改根目录、用户主目录或文件管理器的数据目录",
            ));
        }
        Ok(())
    }
}

// ---- helpers -------------------------------------------------------------

/// One component: no separators, no `.`/`..`, no reserved prefix, platform
/// path rules (Windows reserved names, foreign separators) included.
pub fn validate_name(name: &str) -> Result<(), FileError> {
    let invalid = || {
        FileError::new(
            400,
            "file_name_invalid",
            "名称无效：须为单个路径组件，不能包含 /、\\、控制字符或为 . 与 ..",
        )
    };
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(invalid());
    }
    if name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(invalid());
    }
    if Path::new(name).components().count() != 1
        || !matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err(invalid());
    }
    Ok(())
}
fn random_id() -> Result<String, FileError> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| {
        FileError::new(
            503,
            "file_random_unavailable",
            "系统随机源不可用，不能分配任务编号",
        )
    })?;
    Ok(hex(&random))
}
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Ensures `<root>/<name>` exists as a private (0700) real directory owned by
/// this service and returns its handle. Links and lax permissions are refused.
pub(super) fn private_subdir(root: &Arc<Dir>, name: &str) -> Result<Arc<Dir>, FileError> {
    if root.symlink_metadata(name).is_err() {
        #[allow(unused_mut)]
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        cap_std::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match root.create_dir_with(name, &builder) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(FileError::io(error)),
        }
    }
    let before = root.symlink_metadata(name).map_err(FileError::io)?;
    ordinary(&before)?;
    if !before.is_dir() {
        return Err(FileError::new(
            403,
            "file_private_dir_invalid",
            format!("{name} 必须是目录"),
        ));
    }
    let dir = Arc::new(root.open_dir_nofollow(name).map_err(FileError::io)?);
    let metadata = dir.dir_metadata().map_err(FileError::io)?;
    if identity(&metadata) != identity(&before) {
        return Err(FileError::changed());
    }
    #[cfg(unix)]
    if cap_std::fs::MetadataExt::mode(&metadata) & 0o077 != 0 {
        return Err(FileError::new(
            403,
            "file_private_dir_permissions",
            format!("{name} 目录必须仅所有者可访问 (0700)"),
        ));
    }
    Ok(dir)
}
fn fresh_private_child(parent: &Dir) -> Result<(String, Dir), FileError> {
    for _ in 0..4 {
        let id = random_id()?;
        #[allow(unused_mut)]
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        cap_std::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match parent.create_dir_with(&id, &builder) {
            Ok(()) => {
                let dir = parent.open_dir_nofollow(&id).map_err(FileError::io)?;
                return Ok((id, dir));
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(FileError::io(error)),
        }
    }
    Err(FileError::new(503, "file_trash_id", "无法分配回收目录编号"))
}

fn hard_links_unavailable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::CrossesDevices | ErrorKind::PermissionDenied | ErrorKind::Unsupported
    ) || error.raw_os_error().is_some_and(|code| {
        // EPERM (1), EXDEV (18), EMLINK (31), ENOTSUP/EOPNOTSUPP (95).
        matches!(code, 1 | 18 | 31 | 95)
    })
}
/// Snapshot of the copied source. Retained directory handles and no-follow leaf
/// operations keep recursive copying/removal from walking through substituted links.
struct CopiedEntry {
    stamp: boundary::Stamp,
    metadata: cap_std::fs::Metadata,
    /// A real directory whose children were copied and must be verified and
    /// removed recursively. Directory symlinks are deliberately false.
    recurse_directory: bool,
    /// Windows removes a directory symlink with directory removal semantics,
    /// even though the link itself must never be traversed.
    remove_as_directory: bool,
    children: Vec<(OsString, CopiedEntry)>,
}
impl CopiedEntry {
    fn verify(&self, parent: &Dir, name: &OsStr) -> Result<(), FileError> {
        let current = parent.symlink_metadata(name).map_err(FileError::io)?;
        if boundary::Stamp::new(&current) != self.stamp {
            return Err(FileError::changed());
        }
        if self.recurse_directory {
            let directory = parent.open_dir_nofollow(name).map_err(FileError::io)?;
            if boundary::Stamp::new(&directory.dir_metadata().map_err(FileError::io)?) != self.stamp
            {
                return Err(FileError::changed());
            }
            for (name, child) in &self.children {
                child.verify(&directory, name)?;
            }
        }
        Ok(())
    }
    fn remove_verified(&self, parent: &Dir, name: &OsStr) -> Result<(), FileError> {
        let current = parent.symlink_metadata(name).map_err(FileError::io)?;
        // Removing another hard-link alias changes ctime/nlink on this inode.
        // Recheck content and identity without rejecting our own prior unlink.
        if identity(&current) != identity(&self.metadata)
            || current.len() != self.metadata.len()
            || current.modified().ok() != self.metadata.modified().ok()
        {
            return Err(FileError::changed());
        }
        if self.recurse_directory {
            let directory = parent.open_dir_nofollow(name).map_err(FileError::io)?;
            let expected = identity(&directory.dir_metadata().map_err(FileError::io)?);
            for (name, child) in &self.children {
                child.remove_verified(&directory, name)?;
            }
            if identity(&parent.symlink_metadata(name).map_err(FileError::io)?) != expected {
                return Err(FileError::changed());
            }
            parent.remove_dir(name).map_err(FileError::io)
        } else if self.remove_as_directory {
            parent.remove_dir(name).map_err(FileError::io)
        } else {
            parent.remove_file(name).map_err(FileError::io)
        }
    }
}

#[cfg(windows)]
fn directory_handle_path(directory: &Dir) -> Result<PathBuf, FileError> {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let mut buffer = vec![0u16; 260];
    loop {
        let length = unsafe {
            GetFinalPathNameByHandleW(
                directory.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                0,
            )
        };
        if length == 0 {
            return Err(FileError::io(io::Error::last_os_error()));
        }
        if length as usize >= buffer.len() {
            buffer.resize(length as usize + 1, 0);
            continue;
        }
        return Ok(PathBuf::from(std::ffi::OsString::from_wide(
            &buffer[..length as usize],
        )));
    }
}

/// Copy ordinary files/directories and the link itself, never a link's target.
fn copy_entry(
    src: &Dir,
    source: &OsStr,
    dst: &Dir,
    destination: &OsStr,
) -> Result<CopiedEntry, FileError> {
    let before = src.symlink_metadata(source).map_err(FileError::io)?;
    unshared(&before)?;
    #[cfg(windows)]
    let remove_as_directory = {
        use cap_std::fs::MetadataExt;
        before.file_attributes() & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY
            != 0
    };
    #[cfg(not(windows))]
    let remove_as_directory = before.is_dir();
    let mut copied = CopiedEntry {
        stamp: boundary::Stamp::new(&before),
        metadata: before.clone(),
        recurse_directory: !before.is_symlink() && before.is_dir(),
        remove_as_directory,
        children: Vec::new(),
    };
    if before.is_symlink() {
        let target = src.read_link_contents(source).map_err(FileError::io)?;
        #[cfg(not(windows))]
        dst.symlink_contents(target, destination)
            .map_err(FileError::io)?;
        #[cfg(windows)]
        {
            // The link contents are copied even when the target is absolute.
            // cap-fs-ext's symlink methods reject such targets as an escape.
            let destination = directory_handle_path(dst)?.join(destination);
            if remove_as_directory {
                std::os::windows::fs::symlink_dir(target, destination).map_err(FileError::io)?;
            } else {
                std::os::windows::fs::symlink_file(target, destination).map_err(FileError::io)?;
            }
        }
    } else if before.is_dir() {
        let source_dir = src.open_dir_nofollow(source).map_err(FileError::io)?;
        if boundary::Stamp::new(&source_dir.dir_metadata().map_err(FileError::io)?) != copied.stamp
        {
            return Err(FileError::changed());
        }
        dst.create_dir(destination).map_err(FileError::io)?;
        let destination_dir = dst.open_dir_nofollow(destination).map_err(FileError::io)?;
        for entry in source_dir.entries().map_err(FileError::io)? {
            let name = entry.map_err(FileError::io)?.file_name();
            let child = copy_entry(&source_dir, &name, &destination_dir, &name)?;
            copied.children.push((name, child));
        }
        destination_dir
            .set_permissions(".", before.permissions())
            .map_err(FileError::io)?;
    } else {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let mut input = src.open_with(source, &options).map_err(FileError::io)?;
        if boundary::Stamp::new(&input.metadata().map_err(FileError::io)?) != copied.stamp {
            return Err(FileError::changed());
        }
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        let mut output = dst
            .open_with(destination, &options)
            .map_err(FileError::io)?;
        let count = io::copy(
            &mut crate::transfer::progress::Io(
                Read::by_ref(&mut input).take(before.len()),
                crate::transfer::progress::Task::new("复制文件", "bytes", Some(before.len())),
            ),
            &mut output,
        )
        .map_err(FileError::io)?;
        if count != before.len()
            || boundary::Stamp::new(&input.metadata().map_err(FileError::io)?) != copied.stamp
        {
            return Err(FileError::changed());
        }
        output
            .set_permissions(before.permissions())
            .map_err(FileError::io)?;
        output.sync_all().map_err(FileError::io)?;
    }
    dst.set_symlink_times(
        destination,
        before
            .accessed()
            .ok()
            .map(cap_fs_ext::SystemTimeSpec::Absolute),
        before
            .modified()
            .ok()
            .map(cap_fs_ext::SystemTimeSpec::Absolute),
    )
    .map_err(FileError::io)?;
    copied.verify(src, source)?;
    Ok(copied)
}

/// Move a recycle-bin entry using the same retained-parent operations as the
/// file manager. A complete copy is published before a cross-device source
/// is removed; a failed removal leaves that recoverable copy in place.
pub(crate) fn move_recycle_entry(source: &Path, destination: &Path) -> Result<(), FileError> {
    let parent = |path: &Path| -> Result<Dir, FileError> {
        Dir::open_ambient_dir(
            path.parent().unwrap_or_else(|| Path::new(".")),
            cap_std::ambient_authority(),
        )
        .map_err(FileError::io)
    };
    let name = |path: &Path| -> Result<String, FileError> {
        path.file_name()
            .and_then(OsStr::to_str)
            .map(str::to_owned)
            .ok_or_else(|| FileError::new(400, "file_path_encoding", "路径无法表示为 UTF-8"))
    };
    let src = parent(source)?;
    let dst = parent(destination)?;
    let source = name(source)?;
    let destination = name(destination)?;
    match rename_noreplace(&src, &source, &dst, &destination) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == ErrorKind::CrossesDevices => {}
        Err(error) => return Err(FileError::io(error)),
    }
    let (temporary_name, temporary) = fresh_private_child(&dst)?;
    let result = (|| {
        let copied = copy_entry(&src, source.as_ref(), &temporary, "data".as_ref())?;
        copied.verify(&src, source.as_ref())?;
        rename_noreplace(&temporary, "data", &dst, &destination).map_err(FileError::io)?;
        copied
            .remove_verified(&src, source.as_ref())
            .map_err(|error| {
                FileError::new(
                    error.status,
                    "file_move_source_cleanup",
                    format!("目标已完整发布，但无法移除源名称：{}", error.message),
                )
            })
    })();
    let _ = dst.remove_dir_all(&temporary_name);
    result
}

/// Atomic no-clobber rename on platforms with the corresponding syscall.
fn rename_noreplace(src: &Dir, source: &str, dst: &Dir, destination: &str) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
    {
        rustix::fs::renameat_with(
            src,
            source,
            dst,
            destination,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::from)
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        };
        use windows_sys::{
            Wdk::Storage::FileSystem::{
                FILE_RENAME_INFORMATION, FileRenameInformation, NtSetInformationFile,
            },
            Win32::{Foundation::RtlNtStatusToDosError, System::IO::IO_STATUS_BLOCK},
        };
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        options.access_mode(DELETE | FILE_READ_ATTRIBUTES);
        // Open the named link itself and allow directory handles. The capability
        // walk still checks its parent; the operation then uses this held handle.
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        let source = src.open_with(source, &options)?;
        let name: Vec<u16> = OsStr::new(destination).encode_wide().collect();
        let name_bytes = name
            .len()
            .checked_mul(2)
            .ok_or_else(|| io::Error::from(ErrorKind::InvalidInput))?;
        let bytes = std::mem::size_of::<FILE_RENAME_INFORMATION>()
            .checked_add(name_bytes)
            .ok_or_else(|| io::Error::from(ErrorKind::InvalidInput))?;
        let length = u32::try_from(bytes).map_err(|_| io::Error::from(ErrorKind::InvalidInput))?;
        // A usize allocation supplies FILE_RENAME_INFORMATION's pointer alignment,
        // including the variable-length UTF-16 tail. Zero means no replacement.
        let mut buffer = vec![
            0usize;
            bytes
                .max(std::mem::size_of::<FILE_RENAME_INFORMATION>())
                .div_ceil(std::mem::size_of::<usize>())
        ];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        // SAFETY: buffer is aligned, initialized and large enough for the header
        // and filename. Both directory/source handles remain alive for the call.
        // RootDirectory resolves only the validated single-component leaf name.
        // Use the native operation: the Win32 wrapper rejects this relative
        // directory-handle form with ERROR_INVALID_PARAMETER on Windows.
        // The source is opened synchronously (no FILE_FLAG_OVERLAPPED).
        let mut io_status = IO_STATUS_BLOCK::default();
        let status = unsafe {
            (*info).RootDirectory = dst.as_raw_handle();
            (*info).FileNameLength = name_bytes as u32;
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                name.len(),
            );
            NtSetInformationFile(
                source.as_raw_handle(),
                &mut io_status,
                info.cast(),
                length,
                FileRenameInformation,
            )
        };
        if status < 0 {
            // SAFETY: translating the returned status has no pointer arguments.
            Err(io::Error::from_raw_os_error(
                unsafe { RtlNtStatusToDosError(status) } as i32,
            ))
        } else {
            Ok(())
        }
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos", target_os = "ios")))]
    {
        src.rename(source, dst, destination)
    }
}

// ---- Bug-report attachments ----------------------------------------

/// The API derives its body cap from this value.
const BUG_REPORT_ATTACHMENT_MAX_BYTES: usize = 512 * 1024 * 1024;

/// Keep a readable file name that can neither
/// take part in path resolution nor exceed filesystem limits.
fn attachment_name(raw: &str) -> String {
    let base = raw
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim_matches([' ', '.'])
        .to_owned();
    let mut cleaned = String::new();
    let mut underscore = false;
    for ch in base.chars() {
        if ch.is_control() || ch == '/' || ch == '\\' {
            if !underscore {
                cleaned.push('_');
                underscore = true;
            }
        } else {
            underscore = false;
            cleaned.push(ch);
        }
    }
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = if collapsed.is_empty() || collapsed == "." || collapsed == ".." {
        "attachment".to_owned()
    } else {
        collapsed
    };
    let (stem, suffix) = match name.rfind('.') {
        Some(index) if index > 0 => {
            let suffix: String = name[index..].chars().take(20).collect();
            (name[..index].to_owned(), suffix)
        }
        _ => (name.clone(), String::new()),
    };
    let mut stem = stem;
    while stem.len() > 150 {
        stem.pop();
    }
    let stem = if stem.is_empty() {
        "attachment".to_owned()
    } else {
        stem
    };
    format!("{stem}{suffix}")
}

fn attachment_id_ok(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 9
        && !text.starts_with('0')
        && text.bytes().all(|b| b.is_ascii_digit())
}

/// Windows canonicalization adds a verbatim drive prefix. Keep the ordinary
/// drive spelling for configured-root comparison and returned attachment paths.
#[cfg(windows)]
fn attachment_directory_path(canonical: PathBuf) -> PathBuf {
    let mut components = canonical.components();
    if let Some(Component::Prefix(prefix)) = components.next()
        && let std::path::Prefix::VerbatimDisk(letter) = prefix.kind()
    {
        let mut plain = PathBuf::from(format!("{}:\\", char::from(letter)));
        for component in components {
            if let Component::Normal(name) = component {
                plain.push(name);
            }
        }
        return plain;
    }
    canonical
}

fn same_content(dir: &Dir, name: &str, bytes: &[u8]) -> bool {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No).nonblock(true);
    let Ok(mut file) = dir.open_with(name, &options) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if metadata.len() != bytes.len() as u64 {
        return false;
    }
    let mut existing = Vec::with_capacity(bytes.len());
    file.read_to_end(&mut existing).is_ok() && existing == bytes
}

enum AttachmentBody<'a> {
    Bytes(&'a [u8]),
    File(&'a Path),
}
impl AttachmentBody<'_> {
    fn len(&self) -> Result<u64, FileError> {
        match self {
            Self::Bytes(bytes) => Ok(bytes.len() as u64),
            Self::File(path) => Ok(std::fs::metadata(path).map_err(FileError::io)?.len()),
        }
    }
    fn write_to(&self, writer: &mut impl Write) -> Result<(), FileError> {
        match self {
            Self::Bytes(bytes) => writer.write_all(bytes).map_err(FileError::io),
            Self::File(path) => {
                let mut file = std::fs::File::open(path).map_err(FileError::io)?;
                std::io::copy(&mut file, writer).map_err(FileError::io)?;
                Ok(())
            }
        }
    }
    fn same_content(&self, dir: &Dir, name: &str) -> bool {
        match self {
            Self::Bytes(bytes) => same_content(dir, name, bytes),
            Self::File(path) => {
                let mut options = OpenOptions::new();
                options.read(true).follow(FollowSymlinks::No).nonblock(true);
                let (Ok(mut a), Ok(mut b)) =
                    (dir.open_with(name, &options), std::fs::File::open(path))
                else {
                    return false;
                };
                let (Ok(am), Ok(bm)) = (a.metadata(), b.metadata()) else {
                    return false;
                };
                if !am.is_file() || am.len() != bm.len() {
                    return false;
                }
                let mut left = [0u8; 65536];
                let mut right = [0u8; 65536];
                loop {
                    let Ok(n) = a.read(&mut left) else {
                        return false;
                    };
                    if b.read_exact(&mut right[..n]).is_err() || left[..n] != right[..n] {
                        return false;
                    }
                    if n == 0 {
                        return true;
                    }
                }
            }
        }
    }
}

impl WriteService {
    pub const BUG_REPORT_ATTACHMENT_MAX_BYTES: usize = BUG_REPORT_ATTACHMENT_MAX_BYTES;

    /// Conversation upload destination comes from the selected native view,
    /// never from a client-supplied cwd or file-browser navigation grant.
    pub fn session_attachment_upload(
        &self,
        scope: &FileScope<'_>,
        requested_id: Option<&str>,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<Value, FileError> {
        super::references::validate_scope(scope)?;
        let cwd = boundary::absolute_navigation(scope.cwd)?;
        self.guard(&cwd.join(crate::bug_report::ATTACHMENT_DIR))?;
        self.bug_report_upload(&cwd, requested_id, name, mime, bytes)
    }

    /// Publish a private, completely uploaded file without buffering it in RAM.
    pub fn session_attachment_publish(
        &self,
        scope: &FileScope<'_>,
        requested_id: Option<&str>,
        name: &str,
        mime: &str,
        path: &Path,
    ) -> Result<Value, FileError> {
        super::references::validate_scope(scope)?;
        let cwd = boundary::absolute_navigation(scope.cwd)?;
        self.guard(&cwd.join(crate::bug_report::ATTACHMENT_DIR))?;
        self.attachment_upload(&cwd, requested_id, name, mime, AttachmentBody::File(path))
    }

    /// Exposed for the transport's tests.
    pub fn attachment_name(raw: &str) -> String {
        attachment_name(raw)
    }

    /// For the `bug-report` scope: one raw body
    /// into `<repository>/sessiondock_attachments/<id>/<name>` where the
    /// repository lies inside a write root. The batch directory is the
    /// requested id or the next free number; the file keeps its name, reuses
    /// an identical existing file, or takes `stem__N.suffix`; nothing is ever
    /// overwritten. Returns the upload document.
    pub fn bug_report_upload(
        &self,
        repository: &Path,
        requested_id: Option<&str>,
        raw_name: &str,
        supplied_mime: &str,
        bytes: &[u8],
    ) -> Result<Value, FileError> {
        self.attachment_upload(
            repository,
            requested_id,
            raw_name,
            supplied_mime,
            AttachmentBody::Bytes(bytes),
        )
    }
    fn attachment_upload(
        &self,
        repository: &Path,
        requested_id: Option<&str>,
        raw_name: &str,
        supplied_mime: &str,
        body: AttachmentBody<'_>,
    ) -> Result<Value, FileError> {
        let size = body.len()?;
        if size == 0 {
            return Err(FileError::new(
                400,
                "file_upload_empty",
                "附件为空或缺少 Content-Length",
            ));
        }
        if size > BUG_REPORT_ATTACHMENT_MAX_BYTES as u64 {
            return Err(FileError::new(
                413,
                "file_upload_too_large",
                format!(
                    "单个附件不能超过 {} MB",
                    BUG_REPORT_ATTACHMENT_MAX_BYTES / (1024 * 1024)
                ),
            )
            .with_details(json!({"limit": BUG_REPORT_ATTACHMENT_MAX_BYTES})));
        }
        if let Some(id) = requested_id
            && !attachment_id_ok(id)
        {
            return Err(FileError::new(
                400,
                "file_attachment_id",
                "附件目录编号无效",
            ));
        }
        let repository = repository.canonicalize().map_err(FileError::io)?;
        let attachment_dir = crate::bug_report::ATTACHMENT_DIR;
        let attachment_path = repository.join(attachment_dir);
        let root = self.root_for(&attachment_path)?;
        // An operator may authorize only cwd/sessiondock_attachments, without
        // making the rest of a home/project directory writable.
        let (checked_repository, attachments) = if repository.starts_with(&root.path) {
            let checked = boundary::open_target(root.clone(), repository.clone())?;
            checked.verify_identity()?;
            let attachments =
                private_subdir(checked.directory()?, attachment_dir).map_err(|_| {
                    FileError::new(
                        409,
                        "file_attachment_dir",
                        format!("{attachment_dir} 不是安全目录"),
                    )
                })?;
            (checked, attachments)
        } else {
            let checked = boundary::open_target(root.clone(), attachment_path)?;
            checked.verify_identity()?;
            let attachments = checked.directory()?.clone();
            (checked, attachments)
        };
        let (attachment_id, batch) = match requested_id {
            Some(id) => (
                id.to_owned(),
                private_subdir(&attachments, id).map_err(|_| {
                    FileError::new(409, "file_attachment_dir", "附件编号对应的不是安全目录")
                })?,
            ),
            None => {
                let mut used = 0u64;
                for entry in attachments.entries().map_err(FileError::io)?.flatten() {
                    let name = entry.file_name();
                    if let Some(name) = name.to_str()
                        && attachment_id_ok(name)
                        && entry.file_type().is_ok_and(|kind| kind.is_dir())
                    {
                        used = used.max(name.parse().unwrap_or(0));
                    }
                }
                let mut allocated = None;
                for offset in 1..=32u64 {
                    let id = (used + offset).to_string();
                    #[allow(unused_mut)]
                    let mut builder = DirBuilder::new();
                    #[cfg(unix)]
                    cap_std::fs::DirBuilderExt::mode(&mut builder, 0o700);
                    match attachments.create_dir_with(&id, &builder) {
                        Ok(()) => {
                            allocated = Some((id.clone(), private_subdir(&attachments, &id)?));
                            break;
                        }
                        Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                        Err(error) => return Err(FileError::io(error)),
                    }
                }
                allocated.ok_or_else(|| {
                    FileError::new(503, "file_attachment_id", "无法分配附件目录编号")
                })?
            }
        };
        let checked_batch =
            boundary::open_target(root, repository.join(attachment_dir).join(&attachment_id))?;
        if identity(&batch.dir_metadata().map_err(FileError::io)?)
            != checked_batch.verify_identity()?
        {
            return Err(FileError::changed());
        }
        let original = attachment_name(raw_name);
        let stamp = format!(".{}-{}.upload", random_id()?, std::process::id());
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        cap_std::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let written = (|| -> Result<(), FileError> {
            let mut temp = batch.open_with(&stamp, &options).map_err(FileError::io)?;
            body.write_to(&mut temp)?;
            temp.sync_data().map_err(FileError::io)
        })();
        if let Err(error) = written {
            let _ = batch.remove_file(&stamp);
            return Err(error);
        }
        let (stem, suffix) = match original.rfind('.') {
            Some(index) if index > 0 => {
                (original[..index].to_owned(), original[index..].to_owned())
            }
            _ => (original.clone(), String::new()),
        };
        let mut target = None;
        let mut reused = false;
        for number in 0..10_000u32 {
            if let Err(error) = checked_repository
                .verify_identity()
                .and_then(|_| checked_batch.verify_identity())
            {
                let _ = batch.remove_file(&stamp);
                return Err(error);
            }
            let candidate = if number == 0 {
                original.clone()
            } else {
                format!("{stem}__{number}{suffix}")
            };
            match batch.symlink_metadata(&candidate) {
                Ok(metadata) if metadata.is_symlink() => continue,
                Ok(metadata) if metadata.is_file() && body.same_content(&batch, &candidate) => {
                    target = Some(candidate);
                    reused = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => {}
            }
            match batch.hard_link(&stamp, &batch, &candidate) {
                Ok(()) => {
                    target = Some(candidate);
                    break;
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) if hard_links_unavailable(&error) => {
                    // Same O_EXCL publish as the job path on link-less filesystems.
                    let mut create = OpenOptions::new();
                    create
                        .write(true)
                        .create_new(true)
                        .follow(FollowSymlinks::No);
                    #[cfg(unix)]
                    cap_std::fs::OpenOptionsExt::mode(&mut create, 0o600);
                    match batch.open_with(&candidate, &create) {
                        Ok(mut file) => {
                            if let Err(error) = body
                                .write_to(&mut file)
                                .and_then(|()| file.sync_data().map_err(FileError::io))
                            {
                                let _ = batch.remove_file(&candidate);
                                let _ = batch.remove_file(&stamp);
                                return Err(error);
                            }
                            target = Some(candidate);
                            break;
                        }
                        Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                        Err(error) => {
                            let _ = batch.remove_file(&stamp);
                            return Err(FileError::io(error));
                        }
                    }
                }
                Err(error) => {
                    let _ = batch.remove_file(&stamp);
                    return Err(FileError::io(error));
                }
            }
        }
        let _ = batch.remove_file(&stamp);
        let name = target.ok_or_else(|| {
            FileError::new(409, "file_keep_exhausted", "同名附件过多，无法分配文件名")
        })?;
        let guessed = mime_guess::from_path(&original).first_raw();
        let supplied = supplied_mime
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let supplied_ok = supplied.split_once('/').is_some_and(|(kind, sub)| {
            let ok = |text: &str| {
                !text.is_empty()
                    && text
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
            };
            ok(kind) && ok(sub)
        });
        let mime = guessed
            .map(str::to_owned)
            .or_else(|| supplied_ok.then_some(supplied))
            .unwrap_or_else(|| "application/octet-stream".into());
        let kind = match mime.split('/').next().unwrap_or("") {
            kind @ ("image" | "video" | "audio") => kind,
            _ => "file",
        };
        let path = repository
            .join(attachment_dir)
            .join(&attachment_id)
            .join(&name);
        #[cfg(windows)]
        let path = attachment_directory_path(path);
        let relative = Path::new(attachment_dir).join(&attachment_id).join(&name);
        Ok(json!({
            "ok": true, "name": name, "original_name": original, "path": path,
            "relative_path": relative, "attachment_id": attachment_id, "mime": mime,
            "kind": kind, "size": size, "reused": reused, "media": Value::Null,
            "path_style": if cfg!(windows) { "windows" } else { "posix" },
        }))
    }
}
