//! Adapted locally from delivery/store/disk.rs; keep its reviewed durability
//! contract and race limitations. No shared-store refactor is implied.
//! Ordinary directory paths with atomic ledger replacement and owned temp cleanup.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use sha1::{Digest, Sha1};

use super::{Error, LEDGER_FILENAME};

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
        // Best available identity for temporary-file cleanup on Windows.
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
    if !metadata.is_file() && !is_link(metadata) {
        return Err(Error::UnsafePath);
    }
    Ok(())
}

fn trusted_directory(path: &Path) -> Result<fs::Metadata, Error> {
    let metadata = fs::metadata(path).map_err(|error| io_error("check directory", error))?;
    if !metadata.is_dir() {
        return Err(Error::UnsafePath);
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
    #[cfg(unix)]
    directory_handle: File,
    #[cfg(test)]
    pub(super) failpoint: std::sync::atomic::AtomicU8,
    /// Ledger reads so far; tests pin how many a batch operation needs.
    #[cfg(test)]
    pub(super) reads: std::sync::atomic::AtomicUsize,
}

impl Disk {
    pub(super) fn open(path: &Path, initialize: bool) -> Result<Self, Error> {
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
        let has_ledger = directory.join(LEDGER_FILENAME).exists();
        if initialize && has_ledger {
            return Err(Error::AlreadyInitialized);
        }
        if !initialize && !has_ledger {
            return Err(Error::MissingLedger);
        }
        let disk = Self {
            #[cfg(unix)]
            directory_handle: File::open(&directory)
                .map_err(|error| io_error("open directory handle", error))?,
            directory,
            directory_identity,
            #[cfg(test)]
            failpoint: std::sync::atomic::AtomicU8::new(0),
            #[cfg(test)]
            reads: std::sync::atomic::AtomicUsize::new(0),
        };
        Ok(disk)
    }

    pub(super) fn read(&self) -> Result<Option<Vec<u8>>, Error> {
        #[cfg(test)]
        self.reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = self.directory.join(LEDGER_FILENAME);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error("inspect lifecycle ledger", error)),
        }
    }

    pub(super) fn persist(&self, bytes: &[u8], expected: Option<&str>) -> Result<String, Error> {
        let _ = expected;
        let mut temporary = self.create_temp()?;
        if self.fail(1) {
            return Err(Error::Io("injected before write", io::ErrorKind::Other));
        }
        let file = temporary.file.as_mut().expect("owned temp handle");
        file.write_all(bytes)
            .map_err(|error| io_error("write lifecycle temp", error))?;
        file.sync_all()
            .map_err(|error| io_error("sync lifecycle temp", error))?;
        if self.fail(2) {
            return Err(Error::Io("injected before rename", io::ErrorKind::Other));
        }
        // Verify that the temporary pathname still names the file we wrote.
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
        // Verify the actual installed bytes before acknowledging memory. Any
        // post-rename failure is uncertain, never a safe-before-write error.
        let fingerprint = hash(bytes);
        Ok(fingerprint)
    }

    fn create_temp(&self) -> Result<Temporary, Error> {
        for _ in 0..8 {
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
            // stale files, ledger or unrecognized paths are ever removed.
            let _ = fs::remove_file(&self.path);
        }
    }
}
