//! Filesystem boundary for development metadata. Fixed names, exclusive OS lock,
//! bounded reads, same-directory temporary files and no delete-before-rename.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::Deserialize;
use sha1::{Digest, Sha1};

use super::{
    LOCK_FILENAME, MAX_BYTES, METADATA_FILENAME, MetadataError, MetadataSnapshot, SCHEMA_VERSION,
    model::Document,
};

const TEMP_PREFIX: &str = ".metadata-tmp-";
const DEBUG_RUNS_FILENAME: &str = crate::sessions::DEBUG_RUNS_FILENAME;
const DEBUG_RUNS_TEMP_FILENAME: &str = "debug-runs.json.tmp";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity(String);

fn identity(metadata: &fs::Metadata) -> Identity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Identity(format!("{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        // Stable std Windows file-index getters remain nightly. The exclusive
        // lock remains the writer authority; creation time is an additional
        // replacement check, not a claimed hostile same-user race boundary.
        Identity(format!("{:?}", metadata.created().ok()))
    }
}

fn io_error(operation: &'static str, error: io::Error) -> MetadataError {
    MetadataError::new(
        if error.kind() == io::ErrorKind::PermissionDenied {
            403
        } else {
            503
        },
        "metadata_io",
        format!(
            "开发元数据操作失败：{operation}（{:?}）；未忽略错误或重建空数据",
            error.kind()
        ),
    )
}

fn is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT: reject junctions as well as symlinks.
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

fn private_file(metadata: &fs::Metadata) -> Result<(), MetadataError> {
    if !metadata.is_file() || is_link(metadata) {
        return Err(MetadataError::new(
            403,
            "metadata_unsafe_path",
            "元数据文件必须是普通文件，不能是链接或目录",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o777 != 0o600 || metadata.nlink() != 1 {
            return Err(MetadataError::new(
                403,
                "metadata_unsafe_permissions",
                "元数据文件必须为 0600 且不能有硬链接；不会自动更改已有权限",
            ));
        }
    }
    if metadata.permissions().readonly() {
        return Err(MetadataError::new(
            403,
            "metadata_unsafe_permissions",
            "开发元数据文件不可为只读",
        ));
    }
    Ok(())
}

fn trusted_directory(directory: &Path) -> Result<fs::Metadata, MetadataError> {
    for ancestor in directory.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let metadata =
            fs::symlink_metadata(ancestor).map_err(|error| io_error("检查开发目录", error))?;
        if is_link(&metadata) {
            return Err(MetadataError::new(
                403,
                "metadata_unsafe_path",
                "开发状态目录及其父目录不能经过符号链接或重解析点",
            ));
        }
    }
    let metadata =
        fs::symlink_metadata(directory).map_err(|error| io_error("检查开发目录", error))?;
    if !metadata.is_dir() {
        return Err(MetadataError::new(
            400,
            "metadata_not_directory",
            "必须显式指定一个已存在的独立开发目录",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o022 != 0 || metadata.mode() & 0o300 != 0o300 {
            return Err(MetadataError::new(
                403,
                "metadata_unsafe_permissions",
                "开发状态目录须由所有者可写/可访问，且不能允许组或其他用户写入",
            ));
        }
    }
    Ok(metadata)
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

pub(super) struct Disk {
    directory: PathBuf,
    directory_identity: Identity,
    lock: File,
    lock_identity: Identity,
    #[cfg(unix)]
    directory_handle: File,
    #[cfg(test)]
    pub(super) failpoint: std::sync::atomic::AtomicU8,
}

impl Disk {
    #[cfg(test)]
    pub(super) fn duplicate_lock_for_test(&self) -> File {
        self.lock.try_clone().unwrap()
    }
    /// The canonical state directory this store was opened on.
    pub(super) fn directory(&self) -> &Path {
        &self.directory
    }
    pub(super) fn open(directory: &Path) -> Result<Self, MetadataError> {
        // Resolve only the explicitly supplied path, never any home/config default.
        let absolute = if directory.is_absolute() {
            directory.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| io_error("解析显式目录", error))?
                .join(directory)
        };
        trusted_directory(&absolute)?;
        let directory = absolute
            .canonicalize()
            .map_err(|error| io_error("解析显式目录", error))?;
        let directory_identity = identity(&trusted_directory(&directory)?);
        let entries = fs::read_dir(&directory).map_err(|error| io_error("枚举开发目录", error))?;
        for (count, entry) in entries.enumerate() {
            if count >= 128 {
                return Err(MetadataError::new(
                    413,
                    "metadata_directory_limit",
                    "开发状态目录条目过多；不会清理未确认的遗留文件",
                ));
            }
            let entry = entry.map_err(|error| io_error("检查开发目录条目", error))?;
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(|| {
                MetadataError::new(
                    403,
                    "metadata_foreign_directory",
                    "开发状态目录含不属于本模块的条目",
                )
            })?;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| io_error("检查开发目录条目", error))?;
            // Siblings this store tolerates in the state directory: the
            // file manager's private trash directory (`files/write.rs`), the
            // Claude question-card directory (`claude-prompts/`, written by
            // the `claude-hook` subcommand, read by `bridge::live`), and the
            // debug-run registry (`debug-runs.json` in Python's format plus its
            // `.tmp` replace step; read by the session read model, never here —
            // docs/security-model.md "debug_run").
            let tolerated_dir = metadata.is_dir()
                && !is_link(&metadata)
                && (name == crate::files::FILE_TRASH_DIR || name == crate::bridge::PROMPTS_DIRNAME);
            if tolerated_dir {
                continue;
            }
            if !metadata.is_file()
                || is_link(&metadata)
                || !(name == METADATA_FILENAME
                    || name == LOCK_FILENAME
                    || name == DEBUG_RUNS_FILENAME
                    || name == DEBUG_RUNS_TEMP_FILENAME
                    || name.starts_with(TEMP_PREFIX))
            {
                return Err(MetadataError::new(
                    403,
                    "metadata_foreign_directory",
                    "该目录并非独立开发元数据目录；拒绝读取或迁移生产/Python/其他文件",
                ));
            }
        }
        let lock_path = directory.join(LOCK_FILENAME);
        match fs::symlink_metadata(&lock_path) {
            Ok(metadata) => private_file(&metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("检查元数据锁", error)),
        }
        let lock = private_options()
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| io_error("打开元数据锁", error))?;
        let lock_metadata = lock
            .metadata()
            .map_err(|error| io_error("检查元数据锁", error))?;
        private_file(&lock_metadata)?;
        let lock_identity = identity(&lock_metadata);
        if identity(
            &fs::symlink_metadata(&lock_path).map_err(|error| io_error("核对元数据锁", error))?,
        ) != lock_identity
        {
            return Err(MetadataError::new(
                409,
                "metadata_changed",
                "元数据锁在打开期间已被替换",
            ));
        }
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(MetadataError::new(
                    409,
                    "metadata_writer_locked",
                    "开发元数据目录已有写入进程；请勿启动第二个 writer",
                ));
            }
            Err(_) => {
                return Err(MetadataError::new(
                    503,
                    "metadata_lock_failed",
                    "无法取得操作系统元数据独占锁；拒绝无锁写入",
                ));
            }
        }
        let result = Self {
            #[cfg(unix)]
            directory_handle: File::open(&directory)
                .map_err(|error| io_error("打开开发目录同步句柄", error))?,
            directory,
            directory_identity,
            lock,
            lock_identity,
            #[cfg(test)]
            failpoint: std::sync::atomic::AtomicU8::new(0),
        };
        result.check_owner()?;
        Ok(result)
    }

    fn check_owner(&self) -> Result<(), MetadataError> {
        if identity(&trusted_directory(&self.directory)?) != self.directory_identity {
            return Err(MetadataError::new(
                409,
                "metadata_changed",
                "开发状态目录已被替换，拒绝写入新目标",
            ));
        }
        let path = self.directory.join(LOCK_FILENAME);
        let metadata =
            fs::symlink_metadata(path).map_err(|error| io_error("核对元数据锁", error))?;
        private_file(&metadata)?;
        if identity(&metadata) != self.lock_identity
            || identity(
                &self
                    .lock
                    .metadata()
                    .map_err(|error| io_error("核对锁句柄", error))?,
            ) != self.lock_identity
        {
            return Err(MetadataError::new(
                409,
                "metadata_changed",
                "元数据锁文件被替换；原锁不再代表该目录，拒绝写入",
            ));
        }
        Ok(())
    }

    fn read(&self) -> Result<Option<Vec<u8>>, MetadataError> {
        self.check_owner()?;
        let path = self.directory.join(METADATA_FILENAME);
        let before = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error("检查元数据文件", error)),
        };
        private_file(&before)?;
        if before.len() > MAX_BYTES as u64 {
            return Err(MetadataError::new(
                413,
                "metadata_byte_limit",
                "开发元数据文件超过 4 MiB 限制",
            ));
        }
        let mut file = File::open(&path).map_err(|error| io_error("读取元数据文件", error))?;
        let opened = file
            .metadata()
            .map_err(|error| io_error("核对元数据文件句柄", error))?;
        private_file(&opened)?;
        if identity(&opened) != identity(&before) {
            return Err(MetadataError::new(
                409,
                "metadata_changed",
                "元数据在打开期间已被替换",
            ));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| io_error("读取元数据内容", error))?;
        if bytes.len() > MAX_BYTES {
            return Err(MetadataError::new(
                413,
                "metadata_byte_limit",
                "开发元数据文件超过 4 MiB 限制",
            ));
        }
        let after =
            fs::symlink_metadata(&path).map_err(|error| io_error("核对已读元数据", error))?;
        private_file(&after)?;
        if identity(&after) != identity(&before)
            || after.len() != before.len()
            || after.modified().ok() != before.modified().ok()
        {
            return Err(MetadataError::new(
                409,
                "metadata_changed",
                "元数据在读取期间变化；拒绝不一致快照",
            ));
        }
        self.check_owner()?;
        Ok(Some(bytes))
    }

    pub(super) fn load(&self) -> Result<(MetadataSnapshot, Option<String>), MetadataError> {
        let Some(bytes) = self.read()? else {
            return Ok((MetadataSnapshot::empty(), None));
        };
        #[derive(Deserialize)]
        struct Header {
            schema_version: u32,
        }
        let invalid = || {
            MetadataError::new(
                503,
                "metadata_invalid",
                "开发元数据 JSON/schema 损坏或含未知字段；原文件未被覆盖",
            )
        };
        let header: Header = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if header.schema_version != SCHEMA_VERSION {
            return Err(MetadataError::new(
                501,
                "metadata_schema_unsupported",
                "开发元数据 schema_version 尚不受支持；原文件未被覆盖",
            ));
        }
        let document: Document = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let snapshot = MetadataSnapshot { document };
        snapshot.validate().map_err(|_| invalid())?;
        Ok((snapshot, Some(hash(&bytes))))
    }

    pub(super) fn verify(&self, expected: Option<&str>) -> Result<(), MetadataError> {
        let actual = self.read()?.map(|bytes| hash(&bytes));
        if actual.as_deref() != expected {
            return Err(MetadataError::new(
                409,
                "metadata_changed",
                "磁盘元数据已被外部修改；拒绝覆盖，请重启核验",
            ));
        }
        Ok(())
    }

    pub(super) fn persist(
        &self,
        snapshot: &MetadataSnapshot,
        expected: Option<&str>,
    ) -> Result<String, MetadataError> {
        snapshot.validate()?;
        let mut bytes = serde_json::to_vec(&snapshot.document)
            .map_err(|_| MetadataError::new(503, "metadata_serialize", "无法序列化有效元数据"))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_BYTES {
            return Err(MetadataError::new(
                413,
                "metadata_byte_limit",
                "开发元数据更新超过 4 MiB 限制，未写入",
            ));
        }
        self.verify(expected)?;
        let mut temp = self.create_temp()?;
        if self.fail(1) {
            return Err(MetadataError::new(
                503,
                "metadata_test_failure",
                "模拟替换前写入失败",
            ));
        }
        temp.file
            .as_mut()
            .expect("new temporary file")
            .write_all(&bytes)
            .map_err(|error| io_error("写入唯一临时文件", error))?;
        temp.file
            .as_ref()
            .expect("new temporary file")
            .sync_all()
            .map_err(|error| io_error("同步临时元数据", error))?;
        self.verify(expected)?;
        if self.fail(2) {
            return Err(MetadataError::new(
                503,
                "metadata_test_failure",
                "模拟原子替换失败",
            ));
        }
        // Close the temporary handle first for Windows. Never delete the old
        // target to make rename succeed; a failed replacement keeps it intact.
        temp.file.take();
        fs::rename(&temp.path, self.directory.join(METADATA_FILENAME))
            .map_err(|error| io_error("原子替换元数据", error))?;
        if self.fail(3) {
            return Err(MetadataError::uncertain());
        }
        #[cfg(unix)]
        self.directory_handle
            .sync_all()
            .map_err(|_| MetadataError::uncertain())?;
        // std has no portable Windows directory-fsync contract. File content
        // was flushed before rename; Windows power-loss durability is not an
        // asserted guarantee and is documented separately.
        self.check_owner().map_err(|_| MetadataError::uncertain())?;
        Ok(hash(&bytes))
    }

    fn create_temp(&self) -> Result<Temporary, MetadataError> {
        for _ in 0..8 {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).map_err(|_| {
                MetadataError::new(503, "metadata_entropy", "无法生成唯一临时文件名")
            })?;
            let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = self.directory.join(format!("{TEMP_PREFIX}{name}"));
            match private_options().create_new(true).open(&path) {
                Ok(file) => {
                    let id = identity(
                        &file
                            .metadata()
                            .map_err(|error| io_error("检查临时文件", error))?,
                    );
                    return Ok(Temporary {
                        path,
                        file: Some(file),
                        identity: id,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_error("创建唯一临时文件", error)),
            }
        }
        Err(MetadataError::new(
            503,
            "metadata_temp_collision",
            "无法安全创建唯一临时文件；未删除已有文件",
        ))
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

impl Drop for Disk {
    fn drop(&mut self) {
        // Closing alone waits for all duplicate open file descriptions (such
        // as a concurrent fork's pre-exec copy). Release this writer's lease
        // explicitly without unlinking the stable lock file.
        let _ = self.lock.unlock();
    }
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}

struct Temporary {
    path: PathBuf,
    file: Option<File>,
    identity: Identity,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        self.file.take();
        if let Ok(metadata) = fs::symlink_metadata(&self.path)
            && metadata.is_file()
            && !is_link(&metadata)
            && identity(&metadata) == self.identity
        {
            // Exactly our own successfully create_new'd temporary file; never
            // clean arbitrary old temp names, the lock, or another writer's file.
            let _ = fs::remove_file(&self.path);
        }
    }
}
