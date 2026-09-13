//! Python-compatible atomic file persistence for the delivery ledger.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use sha1::{Digest, Sha1};

use super::{Error, LEDGER_FILENAME};

const TEMP_PREFIX: &str = ".delivery-tmp-";

fn io_error(operation: &'static str, error: io::Error) -> Error {
    Error::Io(operation, error.kind())
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

pub(super) struct Disk {
    directory: PathBuf,
    #[cfg(test)]
    pub(super) failpoint: std::sync::atomic::AtomicU8,
}

impl Disk {
    pub(super) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(super) fn open(path: &Path, initialize: bool) -> Result<Self, Error> {
        let directory = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| io_error("resolve delivery directory", error))?
                .join(path)
        };
        if initialize {
            fs::create_dir_all(&directory)
                .map_err(|error| io_error("create delivery directory", error))?;
        } else if !directory.is_dir() {
            return Err(Error::MissingLedger);
        }
        if !directory.is_dir() {
            return Err(Error::Invalid);
        }
        let ledger = directory.join(LEDGER_FILENAME);
        if initialize && ledger.exists() {
            return Err(Error::AlreadyInitialized);
        }
        if !initialize && !ledger.exists() {
            return Err(Error::MissingLedger);
        }
        Ok(Self {
            directory,
            #[cfg(test)]
            failpoint: std::sync::atomic::AtomicU8::new(0),
        })
    }

    pub(super) fn replace(path: &Path) -> Result<Self, Error> {
        let directory = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| io_error("resolve delivery directory", error))?
                .join(path)
        };
        fs::create_dir_all(&directory)
            .map_err(|error| io_error("create delivery directory", error))?;
        if !directory.is_dir() {
            return Err(Error::Invalid);
        }
        Ok(Self {
            directory,
            #[cfg(test)]
            failpoint: std::sync::atomic::AtomicU8::new(0),
        })
    }

    pub(super) fn read(&self) -> Result<Option<Vec<u8>>, Error> {
        match fs::read(self.directory.join(LEDGER_FILENAME)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error("read delivery ledger", error)),
        }
    }

    pub(super) fn verify(&self, expected: Option<&str>) -> Result<(), Error> {
        let actual = self.read()?.map(|bytes| hash(&bytes));
        if actual.as_deref() == expected {
            Ok(())
        } else {
            Err(Error::Changed)
        }
    }

    pub(super) fn persist(&self, bytes: &[u8], expected: Option<&str>) -> Result<String, Error> {
        self.verify(expected)?;
        let mut temporary = self.create_temp()?;
        if self.fail(1) {
            return Err(Error::Io("injected before write", io::ErrorKind::Other));
        }
        let file = temporary.file.as_mut().expect("owned temp handle");
        file.write_all(bytes)
            .map_err(|error| io_error("write delivery temp", error))?;
        file.sync_all()
            .map_err(|error| io_error("sync delivery temp", error))?;
        self.verify(expected)?;
        if self.fail(2) {
            return Err(Error::Io("injected before rename", io::ErrorKind::Other));
        }
        temporary.file.take();
        fs::rename(&temporary.path, self.directory.join(LEDGER_FILENAME))
            .map_err(|error| io_error("replace delivery ledger", error))?;

        // Python treats directory fsync as best effort after the atomic replace.
        #[cfg(unix)]
        if let Ok(directory) = fs::File::open(&self.directory) {
            let _ = directory.sync_all();
        }
        Ok(hash(bytes))
    }

    fn create_temp(&self) -> Result<Temporary, Error> {
        loop {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random)
                .map_err(|_| Error::Io("generate delivery temp name", io::ErrorKind::Other))?;
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = self.directory.join(format!("{TEMP_PREFIX}{suffix}"));
            match private_options().create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(Temporary {
                        path,
                        file: Some(file),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_error("create delivery temp", error)),
            }
        }
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
    file: Option<fs::File>,
}

impl Drop for Temporary {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}
