//! Lossless cold storage for immutable, exited-host streams. Materialize once
//! on demand so subsequent offset reads remain ordinary bounded seeks.
use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

fn lock(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(path.with_extension("storage-lock"))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}
fn gzip(path: &Path) -> PathBuf {
    path.with_extension("gz")
}
fn convert(input: &Path, output: &Path, decode: bool) -> io::Result<()> {
    let temp = output.with_extension("cold-tmp");
    let result = (|| {
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        if decode {
            let mut source = flate2::read::GzDecoder::new(File::open(input)?);
            io::copy(&mut source, &mut file.try_clone()?)?;
        } else {
            let mut encoder =
                flate2::write::GzEncoder::new(file.try_clone()?, flate2::Compression::fast());
            io::copy(&mut File::open(input)?, &mut encoder)?;
            encoder.finish()?;
        }
        file.sync_all()?;
        fs::rename(&temp, output)?;
        File::open(
            output
                .parent()
                .ok_or_else(|| io::Error::other("stream parent missing"))?,
        )?
        .sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
pub fn open(path: &Path) -> io::Result<File> {
    let _lock = lock(path)?;
    if !path.is_file() && gzip(path).is_file() {
        convert(&gzip(path), path, true)?;
    }
    File::open(path)
}
pub fn archive(path: &Path) -> io::Result<bool> {
    let _lock = lock(path)?;
    if !path.is_file() {
        return Ok(false);
    }
    // Regenerate from the immutable source; no copy/hash comparison is needed.
    convert(path, &gzip(path), false)?;
    fs::remove_file(path)?;
    Ok(true)
}
