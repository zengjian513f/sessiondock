//! Lossless active-stream segments. Logical byte offsets never change. A new
//! host seals immutable prefixes; old running hosts keep their ordinary files.
use crate::platform::{self, OpenOptionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
const SEGMENT: u64 = 8 * 1024 * 1024;
fn directory(path: &Path) -> PathBuf {
    path.with_extension("parts")
}
fn lock(path: &Path) -> io::Result<File> {
    platform::lock(&path.with_extension("storage-lock"))
}
fn create(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
}
fn pieces(path: &Path) -> io::Result<Vec<(u64, PathBuf)>> {
    let mut rows = std::collections::BTreeMap::new();
    for entry in fs::read_dir(directory(path))? {
        let path = entry?.path();
        if !matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("raw" | "gz")
        ) {
            continue;
        }
        let Some(start) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        // The gzip is published before removing its immutable raw source.
        if path.extension().is_some_and(|e| e == "gz") || !rows.contains_key(&start) {
            rows.insert(start, path);
        }
    }
    Ok(rows.into_iter().collect())
}
fn source(path: &Path) -> io::Result<Box<dyn Read>> {
    let file = File::open(path)?;
    Ok(if path.extension().is_some_and(|e| e == "gz") {
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    })
}
pub fn exists(path: &Path) -> bool {
    directory(path).is_dir() && !path.with_extension("gz").is_file()
}

pub struct Writer {
    path: PathBuf,
    tail: PathBuf,
    file: File,
    start: u64,
    end: u64,
    segmented: bool,
}
impl Writer {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new().append(true).mode(0o600).open(&path)?;
        Ok(Self {
            tail: path.clone(),
            path,
            file,
            start: 0,
            end: 0,
            segmented: false,
        })
    }
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let _lock = lock(&self.path)?;
        self.file.write_all(bytes)?;
        self.end += bytes.len() as u64;
        if self.end - self.start < SEGMENT {
            return Ok(());
        }
        let dir = directory(&self.path);
        if !self.segmented {
            platform::private_dir(&dir)?;
            self.tail = dir.join(format!("{:020}.raw", self.start));
            platform::replace(&self.path, &self.tail)?;
            self.segmented = true;
        }
        let encoded = self.tail.with_extension("gz");
        let temporary = self.tail.with_extension("tmp");
        let output = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        let mut encoder = flate2::write::GzEncoder::new(output, flate2::Compression::fast());
        io::copy(&mut File::open(&self.tail)?, &mut encoder)?;
        encoder.finish()?.sync_all()?;
        platform::replace(&temporary, &encoded)?;
        #[cfg(unix)]
        File::open(&dir)?.sync_all()?;
        let next = dir.join(format!("{:020}.raw", self.end));
        self.file = create(&next)?;
        fs::remove_file(&self.tail)?;
        self.tail = next;
        self.start = self.end;
        Ok(())
    }
    pub fn finish(self) -> io::Result<()> {
        let _lock = lock(&self.path)?;
        self.file.sync_all()?;
        if !self.segmented {
            return Ok(());
        }
        drop(self.file);
        // Exited hosts expose the already-supported single gzip format, so
        // older clients and rollback binaries retain their offline recovery.
        let temporary = self.path.with_extension("finish-tmp");
        let output = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        let mut encoder = flate2::write::GzEncoder::new(output, flate2::Compression::fast());
        for (_, path) in pieces(&self.path)? {
            io::copy(&mut source(&path)?, &mut encoder)?;
        }
        encoder.finish()?.sync_all()?;
        platform::replace(&temporary, &self.path.with_extension("gz"))?;
        #[cfg(unix)]
        File::open(self.path.parent().unwrap())?.sync_all()?;
        fs::remove_dir_all(directory(&self.path))?;
        Ok(())
    }
}

/// Bounded seeks into active segments. A single recently decoded segment is
/// cached for sequential recovery; it never limits accepted output or history.
type DecodedSegment = (PathBuf, std::time::SystemTime, u64, Vec<u8>);
static CACHE: std::sync::Mutex<Option<DecodedSegment>> = std::sync::Mutex::new(None);
pub fn read(path: &Path, offset: u64, count: usize) -> io::Result<Vec<u8>> {
    let guard = lock(path)?;
    if !exists(path) || (path.is_file() && pieces(path).is_ok_and(|p| p.is_empty())) {
        drop(guard);
        let mut file = crate::cold::open(path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; count];
        let n = file.read(&mut bytes)?;
        bytes.truncate(n);
        return Ok(bytes);
    }
    let rows = pieces(path)?;
    let Some((start, part)) = rows.iter().rev().find(|(start, _)| *start <= offset) else {
        return Ok(Vec::new());
    };
    let within = offset - start;
    if part.extension().is_some_and(|e| e == "raw") {
        let mut file = File::open(part)?;
        file.seek(SeekFrom::Start(within))?;
        let mut bytes = vec![0; count];
        let n = file.read(&mut bytes)?;
        bytes.truncate(n);
        return Ok(bytes);
    }
    let mut cache = CACHE.lock().unwrap();
    let metadata = fs::metadata(part)?;
    let modified = metadata.modified()?;
    let size = metadata.len();
    if cache
        .as_ref()
        .is_none_or(|(name, time, len, _)| name != part || *time != modified || *len != size)
    {
        let mut bytes = Vec::new();
        source(part)?.read_to_end(&mut bytes)?;
        *cache = Some((part.clone(), modified, size, bytes));
    }
    let bytes = &cache.as_ref().unwrap().3;
    let start = usize::try_from(within)
        .unwrap_or(usize::MAX)
        .min(bytes.len());
    Ok(bytes[start..start.saturating_add(count).min(bytes.len())].to_vec())
}

pub fn len(path: &Path) -> io::Result<u64> {
    let _lock = lock(path)?;
    if !exists(path) {
        return Ok(fs::metadata(path)?.len());
    }
    let Some((start, part)) = pieces(path)?.pop() else {
        return Ok(fs::metadata(path).map(|m| m.len()).unwrap_or(0));
    };
    let size = if part.extension().is_some_and(|e| e == "gz") {
        let mut file = File::open(part)?;
        file.seek(SeekFrom::End(-4))?;
        let mut bytes = [0; 4];
        file.read_exact(&mut bytes)?;
        u32::from_le_bytes(bytes) as u64
    } else {
        fs::metadata(part)?.len()
    };
    Ok(start + size)
}

/// A synchronous logical stream for read-only projection upgrades. It works
/// with ordinary files, active segments and the final cold gzip representation.
pub struct Reader {
    path: PathBuf,
    offset: u64,
}
impl Reader {
    pub fn new(path: PathBuf) -> Self {
        Self { path, offset: 0 }
    }
}
impl Read for Reader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let bytes = read(&self.path, self.offset, buffer.len())?;
        buffer[..bytes.len()].copy_from_slice(&bytes);
        self.offset += bytes.len() as u64;
        Ok(bytes.len())
    }
}
