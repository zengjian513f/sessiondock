//! Checked, chunked native input and a disposable raw-prefix index.
//! This does not parse JSON, authorize media, or raise production input limits.
use super::{FileStamp, SessionError, budgets, metadata_stamp, stamp, trusted_path};
use cap_fs_ext::{DirExt, FollowSymlinks, MetadataExt, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, Metadata, OpenOptions},
};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

const CHUNK: usize = 64 * 1024;
const HEAD: usize = 4096;
const MAX_BYTES: u64 = budgets::FILE_BYTES;
const MAX_CHECKPOINTS: usize = budgets::FILE_CHECKPOINTS;
const MAX_COMPONENTS: usize = 256;
const INDEX_BYTES: usize = budgets::INDEX_BYTES;
static INDEX_BUDGET: OnceLock<Arc<IndexBudget>> = OnceLock::new();

struct IndexBudget {
    maximum: usize,
    used: AtomicUsize,
}
impl IndexBudget {
    fn new(maximum: usize) -> Self {
        Self {
            maximum,
            used: AtomicUsize::new(0),
        }
    }
}
struct IndexCharge {
    budget: Arc<IndexBudget>,
    bytes: usize,
}
impl IndexCharge {
    fn reserve(&mut self, bytes: usize) -> Result<(), SessionError> {
        self.budget
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|next| *next <= self.budget.maximum)
            })
            .map_err(|_| limit_error())?;
        self.bytes += bytes;
        Ok(())
    }
    fn release(&mut self, bytes: usize) {
        self.bytes -= bytes;
        self.budget.used.fetch_sub(bytes, Ordering::AcqRel);
    }
    /// Charge old AND replacement allocations during growth, before allocating.
    /// Transfer the new allocation only after the old allocation is dropped.
    fn capacity<T>(&mut self, vector: &mut Vec<T>, capacity: usize) -> Result<(), SessionError> {
        if vector.capacity() >= capacity {
            return Ok(());
        }
        let bytes = capacity
            .checked_mul(std::mem::size_of::<T>())
            .ok_or_else(limit_error)?;
        let mut pending = Self {
            budget: self.budget.clone(),
            bytes: 0,
        };
        pending.reserve(bytes)?;
        let mut replacement = Vec::new();
        replacement
            .try_reserve_exact(capacity)
            .map_err(|_| limit_error())?;
        // The accounting contract is actual Vec capacity, not allocator/RSS.
        // Do not keep an allocation with an unexpected larger reported capacity.
        if replacement.capacity() != capacity {
            return Err(limit_error());
        }
        replacement.append(vector);
        let old = std::mem::replace(vector, replacement);
        let old_bytes = old.capacity() * std::mem::size_of::<T>();
        drop(old);
        self.bytes += pending.bytes;
        pending.bytes = 0;
        self.release(old_bytes);
        Ok(())
    }
}
impl Drop for IndexCharge {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

fn changed() -> SessionError {
    SessionError::new(503, "原生输入在打开或读取期间变化，请重试")
}
fn read_error() -> SessionError {
    SessionError::new(503, "原生输入读取失败，不能发布不完整快照")
}
fn limit_error() -> SessionError {
    SessionError::new(413, "原生输入超过此操作的读取或索引预算")
}
fn linked(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.is_symlink()
}
fn ordinary(metadata: &Metadata, directory: bool) -> Result<(), SessionError> {
    if linked(metadata)
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file() || metadata.nlink() != 1
        }
    {
        return Err(SessionError::new(
            403,
            "原生输入必须经过普通目录且为无链接普通文件",
        ));
    }
    Ok(())
}
type Identity = (u64, u64);
fn identity(metadata: &Metadata) -> Identity {
    (metadata.dev(), metadata.ino())
}
struct Edge {
    parent: Arc<Dir>,
    name: OsString,
    identity: Identity,
}
impl Edge {
    fn verify(&self) -> Result<(), SessionError> {
        let metadata = self
            .parent
            .symlink_metadata(&self.name)
            .map_err(|_| changed())?;
        ordinary(&metadata, true)?;
        if identity(&metadata) != self.identity {
            return Err(changed());
        }
        Ok(())
    }
}

/// Read only the originally checked range. No public Seek implementation:
/// callers cannot reset or expand this operation's physical read budget.
pub(super) struct CheckedNative {
    file: File,
    root: PathBuf,
    path: PathBuf,
    expected: FileStamp,
    parent: Arc<Dir>,
    leaf: OsString,
    leaf_identity: Identity,
    edges: Vec<Edge>,
    read_end: u64,
    consumed: u64,
    failed: bool,
}
impl CheckedNative {
    pub(super) fn open(
        root: &Path,
        path: &Path,
        expected: &FileStamp,
        limit: u64,
    ) -> Result<Self, SessionError> {
        if expected.size > limit {
            return Err(limit_error());
        }
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
    /// All subsequent reads are confined to [start, end), capped by FILE_BYTES.
    pub(super) fn open_range(
        root: &Path,
        path: &Path,
        expected: &FileStamp,
        start: u64,
        end: u64,
    ) -> Result<Self, SessionError> {
        if start > end || end > expected.size || end - start > MAX_BYTES {
            return Err(limit_error());
        }
        trusted_path(root, path)?;
        if stamp(path)? != *expected {
            return Err(changed());
        }
        let mut base = PathBuf::new();
        let mut names = Vec::new();
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => base.push(component.as_os_str()),
                Component::Normal(name) => names.push(name.to_os_string()),
                _ => return Err(SessionError::new(403, "原生输入需要已验证的绝对规范路径")),
            }
        }
        if !path.is_absolute() || names.is_empty() {
            return Err(changed());
        }
        if names.len() > MAX_COMPONENTS {
            return Err(limit_error());
        }
        // Only the filesystem root is opened ambiently. Every named ancestor,
        // including the configured native root, is traversed without following.
        let mut parent =
            Arc::new(Dir::open_ambient_dir(base, ambient_authority()).map_err(|_| changed())?);
        let mut edges = Vec::new();
        for name in &names[..names.len() - 1] {
            let before = parent.symlink_metadata(name).map_err(|_| changed())?;
            ordinary(&before, true)?;
            let child = Arc::new(parent.open_dir_nofollow(name).map_err(|_| changed())?);
            let opened = child.dir_metadata().map_err(|_| changed())?;
            ordinary(&opened, true)?;
            if identity(&before) != identity(&opened) {
                return Err(changed());
            }
            edges.push(Edge {
                parent,
                name: name.clone(),
                identity: identity(&opened),
            });
            parent = child;
        }
        let leaf = names.pop().expect("checked nonempty components");
        let before = parent.symlink_metadata(&leaf).map_err(|_| changed())?;
        ordinary(&before, false)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let opened = parent.open_with(&leaf, &options).map_err(|_| changed())?;
        let metadata = opened.metadata().map_err(|_| changed())?;
        ordinary(&metadata, false)?;
        let leaf_identity = identity(&metadata);
        if leaf_identity != identity(&before) {
            return Err(changed());
        }
        let mut result = Self {
            file: opened.into_std(),
            root: root.into(),
            path: path.into(),
            expected: expected.clone(),
            parent,
            leaf,
            leaf_identity,
            edges,
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
        for edge in &self.edges {
            edge.verify()?;
        }
        let location = self
            .parent
            .symlink_metadata(&self.leaf)
            .map_err(|_| changed())?;
        ordinary(&location, false)?;
        if identity(&location) != self.leaf_identity {
            return Err(changed());
        }
        let opened = self.file.metadata().map_err(|_| changed())?;
        if !opened.is_file() || metadata_stamp(&opened) != self.expected {
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

#[derive(Clone, Copy)]
pub(super) struct IndexLimits {
    pub max_bytes: u64,
    /// Counts every LF, including whitespace-only lines; zero is implicit.
    pub max_checkpoints: usize,
}
impl Default for IndexLimits {
    fn default() -> Self {
        Self {
            max_bytes: MAX_BYTES,
            max_checkpoints: MAX_CHECKPOINTS,
        }
    }
}
struct Checkpoint {
    end: u64,
    digest: [u8; 20],
}
/// No raw body is retained: <=4096 head bytes plus one 32-byte logical entry per
/// LF (at most FILE_CHECKPOINTS). Limits cap physical scan work, not JSON record budgets.
pub(crate) struct RawIndex {
    length: u64,
    committed: u64,
    head: Vec<u8>,
    digest: String,
    checkpoints: Vec<Checkpoint>,
    committed_digest: [u8; 32],
    probe_digest: Option<[u8; 32]>,
    // Last field: backing allocations drop before their global reservation.
    _charge: IndexCharge,
}
impl RawIndex {
    pub(super) fn scan(reader: impl Read, limits: IndexLimits) -> Result<Self, SessionError> {
        Self::scan_with_probe(reader, limits, None)
    }
    pub(super) fn scan_with_probe(
        reader: impl Read,
        limits: IndexLimits,
        probe: Option<u64>,
    ) -> Result<Self, SessionError> {
        Self::scan_inner(
            reader,
            limits,
            INDEX_BUDGET
                .get_or_init(|| Arc::new(IndexBudget::new(INDEX_BYTES)))
                .clone(),
            probe,
        )
    }
    #[cfg(test)]
    fn scan_with_budget(
        reader: impl Read,
        limits: IndexLimits,
        budget: Arc<IndexBudget>,
    ) -> Result<Self, SessionError> {
        Self::scan_inner(reader, limits, budget, None)
    }
    fn scan_inner(
        mut reader: impl Read,
        limits: IndexLimits,
        budget: Arc<IndexBudget>,
        probe: Option<u64>,
    ) -> Result<Self, SessionError> {
        let mut builder = RawIndexBuilder::with_budget(limits, probe, budget)?;
        let mut buffer = [0u8; CHUNK];
        loop {
            // At an exact limit, only probe one byte to distinguish EOF from
            // over-budget input. Never return a successful truncated index.
            let amount =
                ((limits.max_bytes - builder.index.length).min(CHUNK as u64) as usize).max(1);
            let count = match reader.read(&mut buffer[..amount]) {
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
    /// Capacity-based retained allocation weight, excluding allocator overhead.
    /// A process-global RAII budget charges this across all stores/old snapshots.
    pub(super) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.head.capacity())
            .saturating_add(self.digest.capacity())
            .saturating_add(
                self.checkpoints
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Checkpoint>()),
            )
    }
    /// Physical start of the record that ends at `end`: the previous LF
    /// checkpoint, or zero for the first line (delivery evidence, batch 30).
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
/// truncated index. Index allocations retain the same global RAII reservation.
pub(super) struct RawIndexBuilder {
    index: RawIndex,
    limits: IndexLimits,
    probe: Option<u64>,
    hash: Sha1,
    strong: Sha256,
    committed_strong: Sha256,
    failed: bool,
}
impl RawIndexBuilder {
    pub(super) fn new(limits: IndexLimits, probe: Option<u64>) -> Result<Self, SessionError> {
        Self::with_budget(
            limits,
            probe,
            INDEX_BUDGET
                .get_or_init(|| Arc::new(IndexBudget::new(INDEX_BYTES)))
                .clone(),
        )
    }
    fn with_budget(
        limits: IndexLimits,
        probe: Option<u64>,
        budget: Arc<IndexBudget>,
    ) -> Result<Self, SessionError> {
        if limits.max_bytes > MAX_BYTES || limits.max_checkpoints > MAX_CHECKPOINTS {
            return Err(limit_error());
        }
        let mut charge = IndexCharge { budget, bytes: 0 };
        charge.reserve(std::mem::size_of::<RawIndex>())?;
        Ok(Self {
            index: RawIndex {
                length: 0,
                committed: 0,
                head: Vec::new(),
                digest: String::new(),
                checkpoints: Vec::new(),
                committed_digest: Sha256::digest([]).into(),
                probe_digest: (probe == Some(0)).then(|| Sha256::digest([]).into()),
                _charge: charge,
            },
            limits,
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
        if bytes.len() as u64 > self.limits.max_bytes - index.length {
            return Err(limit_error());
        }
        let head_count = bytes.len().min(HEAD - index.head.len());
        if head_count > 0 {
            let capacity = index
                .head
                .capacity()
                .saturating_mul(2)
                .max(index.head.len() + head_count)
                .min(HEAD);
            index._charge.capacity(&mut index.head, capacity)?;
            index.head.extend_from_slice(&bytes[..head_count]);
        }
        let mut start = 0;
        for (offset, byte) in bytes.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            if index.checkpoints.len() >= self.limits.max_checkpoints {
                return Err(limit_error());
            }
            self.hash.update(&bytes[start..=offset]);
            index.committed = index.length + offset as u64 + 1;
            if index.checkpoints.len() == index.checkpoints.capacity() {
                let grow = index
                    .checkpoints
                    .capacity()
                    .max(16)
                    .min(self.limits.max_checkpoints - index.checkpoints.len());
                let capacity = index.checkpoints.len() + grow;
                index._charge.capacity(&mut index.checkpoints, capacity)?;
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
        self.index._charge.capacity(&mut encoded, 40)?;
        for byte in self.hash.finalize() {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            encoded.push(HEX[(byte >> 4) as usize]);
            encoded.push(HEX[(byte & 15) as usize]);
        }
        self.index.digest = String::from_utf8(encoded).expect("ASCII digest");
        self.index.committed_digest = self.committed_strong.finalize().into();
        debug_assert_eq!(self.index._charge.bytes, self.index.retained_bytes());
        Ok(self.index)
    }
}

#[cfg(test)]
mod tests;
