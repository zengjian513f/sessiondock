//! Checked, chunked native input and a disposable raw-prefix index.
//! This does not parse JSON or authorize media.
use super::{FileStamp, SessionError, file_stamp, stamp, trusted_path};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

const CHUNK: usize = 64 * 1024;
const HEAD: usize = 4096;
fn changed() -> SessionError {
    SessionError::new(503, "原生输入在打开或读取期间变化，请重试")
}
fn read_error() -> SessionError {
    SessionError::new(503, "原生输入读取失败，不能发布不完整快照")
}
fn invalid_range() -> SessionError {
    SessionError::new(409, "原生输入范围无效或已变化，请重试")
}
fn allocation_error() -> SessionError {
    SessionError::new(503, "原生输入索引内存分配失败")
}
fn ensure_capacity<T>(vector: &mut Vec<T>, capacity: usize) -> Result<(), SessionError> {
    if vector.capacity() < capacity {
        vector
            .try_reserve_exact(capacity - vector.len())
            .map_err(|_| allocation_error())?;
    }
    Ok(())
}
/// Read only the originally checked range. No public Seek implementation:
/// callers cannot reset or expand the authorized range.
pub(super) struct CheckedNative {
    file: File,
    root: PathBuf,
    path: PathBuf,
    expected: FileStamp,
    read_end: u64,
    consumed: u64,
    failed: bool,
}
impl CheckedNative {
    pub(super) fn open(
        root: &Path,
        path: &Path,
        expected: &FileStamp,
    ) -> Result<Self, SessionError> {
        Self::open_prefix(root, path, expected, expected.size)
    }
    /// The authorized range is only [0, end). Expected identity still describes
    /// the entire current file, including its unread suffix. This is not Seek.
    pub(super) fn open_prefix(
        root: &Path,
        path: &Path,
        expected: &FileStamp,
        end: u64,
    ) -> Result<Self, SessionError> {
        Self::open_range(root, path, expected, 0, end)
    }
    /// Seek once, after opening and verifying the trusted whole-file identity.
    /// All subsequent reads are confined to the stamped [start, end) range.
    pub(super) fn open_range(
        root: &Path,
        path: &Path,
        expected: &FileStamp,
        start: u64,
        end: u64,
    ) -> Result<Self, SessionError> {
        if start > end || end > expected.size {
            return Err(invalid_range());
        }
        trusted_path(root, path)?;
        if stamp(path)? != *expected {
            return Err(changed());
        }
        if !path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(changed());
        }
        let opened = File::open(path).map_err(|_| changed())?;
        if file_stamp(&opened).map_err(|_| changed())? != *expected {
            return Err(changed());
        }
        let mut result = Self {
            file: opened,
            root: root.into(),
            path: path.into(),
            expected: expected.clone(),
            read_end: end,
            consumed: start,
            failed: false,
        };
        result.verify()?;
        if result
            .file
            .seek(SeekFrom::Start(start))
            .map_err(|_| read_error())?
            != start
        {
            return Err(read_error());
        }
        result.verify()?;
        Ok(result)
    }
    pub(super) fn verify(&self) -> Result<(), SessionError> {
        if self.failed {
            return Err(read_error());
        }
        if file_stamp(&self.file).map_err(|_| changed())? != self.expected {
            return Err(changed());
        }
        trusted_path(&self.root, &self.path)?;
        if stamp(&self.path)? != self.expected {
            return Err(changed());
        }
        Ok(())
    }
    /// Completion is explicit. Merely dropping a partially read handle never
    /// claims that a snapshot was complete or passed its final identity check.
    pub(super) fn finish(self) -> Result<(), SessionError> {
        if self.consumed != self.read_end {
            return Err(read_error());
        }
        self.verify()
    }
}
impl Read for CheckedNative {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.failed {
            return Err(io::Error::other(read_error()));
        }
        if output.is_empty() {
            return Ok(0);
        }
        let remaining = self.read_end - self.consumed;
        if remaining == 0 {
            return match self.verify() {
                Ok(()) => Ok(0),
                Err(error) => {
                    self.failed = true;
                    Err(io::Error::other(error))
                }
            };
        }
        let length = output
            .len()
            .min(CHUNK)
            .min(remaining.min(CHUNK as u64) as usize);
        match self.file.read(&mut output[..length]) {
            Ok(0) => {
                self.failed = true;
                Err(io::Error::new(io::ErrorKind::UnexpectedEof, read_error()))
            }
            Ok(read) => {
                self.consumed += read as u64;
                Ok(read)
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Err(error),
            Err(_) => {
                self.failed = true;
                Err(io::Error::other(read_error()))
            }
        }
    }
}

struct Checkpoint {
    end: u64,
    digest: [u8; 20],
}
/// No raw body is retained: <=4096 head bytes plus one logical entry per LF.
pub(crate) struct RawIndex {
    length: u64,
    committed: u64,
    head: Vec<u8>,
    digest: String,
    checkpoints: Vec<Checkpoint>,
    committed_digest: [u8; 32],
    probe_digest: Option<[u8; 32]>,
}
impl RawIndex {
    pub(super) fn scan(reader: impl Read) -> Result<Self, SessionError> {
        Self::scan_with_probe(reader, None)
    }
    pub(super) fn scan_with_probe(
        mut reader: impl Read,
        probe: Option<u64>,
    ) -> Result<Self, SessionError> {
        let mut builder = RawIndexBuilder::new(probe)?;
        let mut buffer = [0u8; CHUNK];
        loop {
            let count = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(read_error()),
            };
            builder.push(&buffer[..count])?;
        }
        builder.finish()
    }
    pub(super) fn length(&self) -> u64 {
        self.length
    }
    pub(super) fn committed(&self) -> u64 {
        self.committed
    }
    pub(super) fn head_bytes(&self) -> &[u8] {
        &self.head
    }
    pub(super) fn digest(&self) -> &str {
        &self.digest
    }
    pub(super) fn committed_digest(&self) -> [u8; 32] {
        self.committed_digest
    }
    pub(super) fn probe_digest(&self) -> Option<[u8; 32]> {
        self.probe_digest
    }
    /// Physical start of the record that ends at `end`: the previous LF
    /// checkpoint, or zero for the first line (delivery evidence).
    pub(super) fn record_start(&self, end: u64) -> u64 {
        match self
            .checkpoints
            .binary_search_by_key(&end, |entry| entry.end)
        {
            Ok(0) | Err(0) => 0,
            Ok(index) | Err(index) => self.checkpoints[index - 1].end,
        }
    }
    pub(super) fn is_checkpoint(&self, end: u64) -> bool {
        end == 0
            || self
                .checkpoints
                .binary_search_by_key(&end, |entry| entry.end)
                .is_ok()
    }
    pub(super) fn prefix_hash(&self, end: u64) -> Option<String> {
        if end == 0 {
            return Some(format!("{:x}", Sha1::digest([])));
        }
        let index = self
            .checkpoints
            .binary_search_by_key(&end, |entry| entry.end)
            .ok()?;
        if end == self.length {
            return Some(self.digest().to_owned());
        }
        Some(
            self.checkpoints[index]
                .digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        )
    }
}

/// Push-based counterpart to `scan`: the trusted input owner must push every
/// physical byte exactly once and call finish only after its checked EOF.
/// Any push failure poisons the builder, so ignoring an error cannot publish a
/// truncated index.
pub(super) struct RawIndexBuilder {
    index: RawIndex,
    probe: Option<u64>,
    hash: Sha1,
    strong: Sha256,
    committed_strong: Sha256,
    failed: bool,
}
impl RawIndexBuilder {
    pub(super) fn new(probe: Option<u64>) -> Result<Self, SessionError> {
        Ok(Self {
            index: RawIndex {
                length: 0,
                committed: 0,
                head: Vec::new(),
                digest: String::new(),
                checkpoints: Vec::new(),
                committed_digest: Sha256::digest([]).into(),
                probe_digest: (probe == Some(0)).then(|| Sha256::digest([]).into()),
            },
            probe,
            hash: Sha1::new(),
            strong: Sha256::new(),
            committed_strong: Sha256::new(),
            failed: false,
        })
    }
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<(), SessionError> {
        if self.failed {
            return Err(read_error());
        }
        let result = self.push_inner(bytes);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn push_inner(&mut self, bytes: &[u8]) -> Result<(), SessionError> {
        let index = &mut self.index;
        let head_count = bytes.len().min(HEAD - index.head.len());
        if head_count > 0 {
            let capacity = index
                .head
                .capacity()
                .saturating_mul(2)
                .max(index.head.len() + head_count)
                .min(HEAD);
            ensure_capacity(&mut index.head, capacity)?;
            index.head.extend_from_slice(&bytes[..head_count]);
        }
        let mut start = 0;
        for (offset, byte) in bytes.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            self.hash.update(&bytes[start..=offset]);
            index.committed = index.length + offset as u64 + 1;
            if index.checkpoints.len() == index.checkpoints.capacity() {
                let grow = index.checkpoints.capacity().max(16);
                let capacity = index.checkpoints.len() + grow;
                ensure_capacity(&mut index.checkpoints, capacity)?;
            }
            index.checkpoints.push(Checkpoint {
                end: index.committed,
                digest: self.hash.clone().finalize().into(),
            });
            start = offset + 1;
        }
        self.hash.update(&bytes[start..]);
        // SHA256 visits each byte once; only the requested probe is finalized
        // midstream, and only the last LF state in each push is retained.
        let probe_at = self
            .probe
            .filter(|end| *end > index.length && *end <= index.length + bytes.len() as u64)
            .map(|end| (end - index.length) as usize)
            .filter(|at| bytes[*at - 1] == b'\n');
        if let Some(at) = probe_at {
            self.strong.update(&bytes[..at]);
            index.probe_digest = Some(self.strong.clone().finalize().into());
            self.strong.update(&bytes[at..start]);
        } else {
            self.strong.update(&bytes[..start]);
        }
        if start > 0 {
            self.committed_strong = self.strong.clone();
        }
        self.strong.update(&bytes[start..]);
        index.length += bytes.len() as u64;
        Ok(())
    }
    pub(super) fn finish(mut self) -> Result<RawIndex, SessionError> {
        if self.failed {
            return Err(read_error());
        }
        let mut encoded = Vec::new();
        ensure_capacity(&mut encoded, 40)?;
        for byte in self.hash.finalize() {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            encoded.push(HEX[(byte >> 4) as usize]);
            encoded.push(HEX[(byte & 15) as usize]);
        }
        self.index.digest = String::from_utf8(encoded).expect("ASCII digest");
        self.index.committed_digest = self.committed_strong.finalize().into();
        Ok(self.index)
    }
}

#[cfg(test)]
mod tests;
