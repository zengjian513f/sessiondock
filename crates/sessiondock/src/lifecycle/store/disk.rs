//! Adapted locally from delivery/store/disk.rs; keep its reviewed durability
//! contract and race limitations. No shared-store refactor is implied.
//! Fixed-name, single-writer filesystem boundary. Never discovers a home,
//! creates a directory, unlinks a ledger, or cleans another process's temp file.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

use sha1::{Digest, Sha1};

use super::{Error, LEDGER_FILENAME, LOCK_FILENAME, MAX_BYTES};

const TEMP_PREFIX: &str = ".lifecycle-tmp-";

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
        // Windows: std exposes no stable file index; the creation time is the
        // replacement check behind the exclusive lock (WP-W), as for metadata.
        Identity(format!("{:?}", metadata.created().ok()))
    }
}

fn io_error(operation: &'static str, error: io::Error) -> Error {
    Error::Io(operation, error.kind())
}

fn is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

fn private_file(metadata: &fs::Metadata) -> Result<(), Error> {
    if !metadata.is_file() || is_link(metadata) {
        return Err(Error::UnsafePath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o777 != 0o600 || metadata.nlink() != 1 {
            return Err(Error::UnsafePermissions);
        }
    }
    if metadata.permissions().readonly() {
        return Err(Error::UnsafePermissions);
    }
    Ok(())
}

fn trusted_directory(path: &Path) -> Result<fs::Metadata, Error> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let metadata = fs::symlink_metadata(ancestor)
            .map_err(|error| io_error("check directory ancestry", error))?;
        if !metadata.is_dir() || is_link(&metadata) {
            return Err(Error::UnsafePath);
        }
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|error| io_error("check directory", error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o777 != 0o700 {
            return Err(Error::UnsafePermissions);
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
    #[cfg(all(test, unix))]
    pub(super) fn duplicate_lock(&self) -> File {
        self.lock.try_clone().unwrap()
    }

    pub(super) fn open(path: &Path, initialize: bool) -> Result<Self, Error> {
        if !cfg!(any(unix, windows)) {
            // Do not claim a durable Persisted barrier where the platform is
            // unreviewed. Windows (WP-W) syncs the temp file and renames it
            // through MoveFileEx(REPLACE_EXISTING); there is no directory
            // handle to flush, NTFS journals the rename itself.
            return Err(Error::DurabilityUnavailable);
        }
        if path.as_os_str().is_empty() || path.components().any(|c| c == Component::ParentDir) {
            return Err(Error::UnsafePath);
        }
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| io_error("resolve explicit directory", error))?
                .join(path)
        };
        trusted_directory(&absolute)?;
        let directory = absolute
            .canonicalize()
            .map_err(|error| io_error("canonicalize directory", error))?;
        let directory_identity = identity(&trusted_directory(&directory)?);
        let mut has_lock = false;
        let mut has_ledger = false;
        for (count, entry) in fs::read_dir(&directory)
            .map_err(|error| io_error("enumerate dedicated directory", error))?
            .enumerate()
        {
            if initialize {
                return Err(Error::AlreadyInitialized);
            }
            if count >= 128 {
                return Err(Error::Limit);
            }
            let entry = entry.map_err(|error| io_error("read directory entry", error))?;
            let name = entry.file_name();
            let name = name.to_str().ok_or(Error::ForeignDirectory)?;
            let own_temp = name.strip_prefix(TEMP_PREFIX).is_some_and(|suffix| {
                suffix.len() == 32 && suffix.bytes().all(|c| c.is_ascii_hexdigit())
            });
            if name != LEDGER_FILENAME && name != LOCK_FILENAME && !own_temp {
                return Err(Error::ForeignDirectory);
            }
            private_file(
                &fs::symlink_metadata(entry.path())
                    .map_err(|error| io_error("check directory entry", error))?,
            )?;
            has_lock |= name == LOCK_FILENAME;
            has_ledger |= name == LEDGER_FILENAME;
        }
        if !initialize && (!has_lock || !has_ledger) {
            return Err(Error::MissingLedger);
        }
        let lock_path = directory.join(LOCK_FILENAME);
        let mut options = private_options();
        if initialize {
            options.create_new(true);
        }
        let lock = options.open(&lock_path).map_err(|error| {
            if initialize && error.kind() == io::ErrorKind::AlreadyExists {
                Error::AlreadyInitialized
            } else {
                io_error("open stable lifecycle lock", error)
            }
        })?;
        let metadata = lock
            .metadata()
            .map_err(|error| io_error("inspect lock handle", error))?;
        private_file(&metadata)?;
        let lock_identity = identity(&metadata);
        let path_metadata = fs::symlink_metadata(&lock_path)
            .map_err(|error| io_error("inspect lock path", error))?;
        private_file(&path_metadata)?;
        if identity(&path_metadata) != lock_identity {
            return Err(Error::Changed);
        }
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(Error::WriterLocked),
            Err(_) => return Err(Error::Io("acquire OS lifecycle lock", io::ErrorKind::Other)),
        }
        let disk = Self {
            #[cfg(unix)]
            directory_handle: File::open(&directory)
                .map_err(|error| io_error("open directory handle", error))?,
            directory,
            directory_identity,
            lock,
            lock_identity,
            #[cfg(test)]
            failpoint: std::sync::atomic::AtomicU8::new(0),
        };
        disk.check_owner()?;
        if initialize {
            disk.lock
                .sync_all()
                .map_err(|error| io_error("sync initial lock", error))?;
        }
        Ok(disk)
    }

    fn check_owner(&self) -> Result<(), Error> {
        if identity(&trusted_directory(&self.directory)?) != self.directory_identity {
            return Err(Error::Changed);
        }
        #[cfg(unix)]
        if identity(
            &self
                .directory_handle
                .metadata()
                .map_err(|error| io_error("inspect directory handle", error))?,
        ) != self.directory_identity
        {
            return Err(Error::Changed);
        }
        let metadata = fs::symlink_metadata(self.directory.join(LOCK_FILENAME))
            .map_err(|error| io_error("check lifecycle lock", error))?;
        private_file(&metadata)?;
        let handle = self
            .lock
            .metadata()
            .map_err(|error| io_error("check lock handle", error))?;
        private_file(&handle)?;
        if identity(&metadata) != self.lock_identity || identity(&handle) != self.lock_identity {
            return Err(Error::Changed);
        }
        Ok(())
    }

    pub(super) fn read(&self) -> Result<Option<Vec<u8>>, Error> {
        self.check_owner()?;
        let path = self.directory.join(LEDGER_FILENAME);
        let before = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error("inspect lifecycle ledger", error)),
        };
        private_file(&before)?;
        if before.len() > MAX_BYTES as u64 {
            return Err(Error::Limit);
        }
        let mut file =
            File::open(&path).map_err(|error| io_error("open lifecycle ledger", error))?;
        let opened = file
            .metadata()
            .map_err(|error| io_error("inspect ledger handle", error))?;
        private_file(&opened)?;
        if identity(&opened) != identity(&before) {
            return Err(Error::Changed);
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| io_error("read lifecycle ledger", error))?;
        if bytes.len() > MAX_BYTES {
            return Err(Error::Limit);
        }
        let after = fs::symlink_metadata(&path)
            .map_err(|error| io_error("restamp lifecycle ledger", error))?;
        private_file(&after)?;
        let handle = file
            .metadata()
            .map_err(|error| io_error("restamp ledger handle", error))?;
        if identity(&after) != identity(&before)
            || identity(&handle) != identity(&before)
            || after.len() != before.len()
            || handle.len() != before.len()
            || after.modified().ok() != before.modified().ok()
            || handle.modified().ok() != before.modified().ok()
        {
            return Err(Error::Changed);
        }
        self.check_owner()?;
        Ok(Some(bytes))
    }

    pub(super) fn verify(&self, expected: Option<&str>) -> Result<(), Error> {
        let actual = self.read()?.map(|bytes| hash(&bytes));
        if actual.as_deref() != expected {
            return Err(Error::Changed);
        }
        Ok(())
    }

    pub(super) fn persist(&self, bytes: &[u8], expected: Option<&str>) -> Result<String, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::Limit);
        }
        self.verify(expected)?;
        let mut temporary = self.create_temp()?;
        if self.fail(1) {
            return Err(Error::Io("injected before write", io::ErrorKind::Other));
        }
        let file = temporary.file.as_mut().expect("owned temp handle");
        file.write_all(bytes)
            .map_err(|error| io_error("write lifecycle temp", error))?;
        file.sync_all()
            .map_err(|error| io_error("sync lifecycle temp", error))?;
        self.verify(expected)?;
        if self.fail(2) {
            return Err(Error::Io("injected before rename", io::ErrorKind::Other));
        }
        // Verify that the temp pathname still names the exact private file we
        // wrote. The private directory is required; this is not a hostile-user
        // capability boundary against replacement races between syscalls.
        let meta = fs::symlink_metadata(&temporary.path)
            .map_err(|error| io_error("check temp path", error))?;
        private_file(&meta)?;
        if identity(&meta) != temporary.identity {
            return Err(Error::Changed);
        }
        temporary.file.take();
        fs::rename(&temporary.path, self.directory.join(LEDGER_FILENAME))
            .map_err(|error| io_error("replace lifecycle ledger", error))?;
        if self.fail(3) {
            return Err(Error::Uncertain);
        }
        #[cfg(unix)]
        self.directory_handle
            .sync_all()
            .map_err(|_| Error::Uncertain)?;
        if self.fail(4) {
            return Err(Error::Uncertain);
        }
        self.check_owner().map_err(|_| Error::Uncertain)?;
        // Verify the actual installed bytes before acknowledging memory. Any
        // post-rename failure is uncertain, never a safe-before-write error.
        let fingerprint = hash(bytes);
        self.verify(Some(&fingerprint))
            .map_err(|_| Error::Uncertain)?;
        Ok(fingerprint)
    }

    fn create_temp(&self) -> Result<Temporary, Error> {
        for _ in 0..8 {
            self.check_owner()?;
            let mut random = [0u8; 16];
            getrandom::fill(&mut random)
                .map_err(|_| Error::Io("generate lifecycle temp name", io::ErrorKind::Other))?;
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = self.directory.join(format!("{TEMP_PREFIX}{suffix}"));
            match private_options().create_new(true).open(&path) {
                Ok(file) => {
                    let metadata = file
                        .metadata()
                        .map_err(|error| io_error("inspect new temp", error))?;
                    private_file(&metadata)?;
                    return Ok(Temporary {
                        path,
                        file: Some(file),
                        identity: identity(&metadata),
                        directory: self.directory.clone(),
                        directory_identity: self.directory_identity.clone(),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_error("create unique lifecycle temp", error)),
            }
        }
        Err(Error::Io(
            "lifecycle temp collision",
            io::ErrorKind::AlreadyExists,
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
        // A forked/duplicated file description must not retain this owner's
        // lease. Keep the stable lock pathname; never unlink/recreate it.
        let _ = self.lock.unlock();
    }
}

pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}

struct Temporary {
    path: PathBuf,
    file: Option<File>,
    identity: Identity,
    directory: PathBuf,
    directory_identity: Identity,
}

impl Drop for Temporary {
    fn drop(&mut self) {
        self.file.take();
        if trusted_directory(&self.directory)
            .is_ok_and(|meta| identity(&meta) == self.directory_identity)
            && let Ok(metadata) = fs::symlink_metadata(&self.path)
            && private_file(&metadata).is_ok()
            && identity(&metadata) == self.identity
        {
            // Only this invocation's exact create_new temporary file. No other
            // stale files, ledger, lock or unrecognized paths are ever removed.
            let _ = fs::remove_file(&self.path);
        }
    }
}
