//! Observed task-local writes, excluding I/O inherited when children are reaped.
use process_links::Process;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    os::fd::AsRawFd,
    path::Path,
    time::{Duration, Instant},
};

struct Thread {
    start: u64,
    writes: u64,
    // A proc inode pins the task identity. Reads fail after exit, even when its
    // numeric TID is reused. Ordinary fixture files do not have this property.
    io: Option<File>,
}
#[derive(Default)]
struct Counter {
    total: u64,
    threads: BTreeMap<u32, Thread>,
}
#[derive(Default)]
pub struct ThreadWrites {
    counters: BTreeMap<Process, Counter>,
    sampled: Option<Instant>,
    boot: String,
}
impl ThreadWrites {
    pub fn begin(&mut self, boot: &str, at: Instant, live: impl Iterator<Item = Process>) {
        if self.boot != boot {
            self.counters.clear();
            self.boot = boot.into();
        } else if self
            .sampled
            .is_some_and(|old| at.duration_since(old) > Duration::from_secs(10))
        {
            for counter in self.counters.values_mut() {
                counter.threads.clear();
            }
        }
        self.sampled = Some(at);
        let live: std::collections::BTreeSet<_> = live.collect();
        self.counters.retain(|p, _| live.contains(p));
    }
    pub fn missing(&mut self, process: &Process) {
        if let Some(counter) = self.counters.get_mut(process) {
            counter.threads.clear();
        }
    }
    pub fn sample(&mut self, root: &Path, process: &Process) -> Option<u64> {
        let counter = self.counters.entry(process.clone()).or_default();
        let mut prior = std::mem::take(&mut counter.threads);
        let dir = root.join(process.pid.to_string());
        let entries = fs::read_dir(dir.join("task")).ok()?;
        let mut current = BTreeMap::new();
        let mut delta = 0u64;
        for entry in entries.flatten() {
            let Some(tid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let old = prior.remove(&tid);
            let mut observed = None;
            if let Some(mut old) = old {
                if let Some(file) = old.io.as_mut()
                    && let Some(writes) = read_writes(file)
                {
                    delta = delta.saturating_add(writes.saturating_sub(old.writes));
                    old.writes = writes;
                    observed = Some(old);
                } else if let Some(new) = open_thread(&entry.path()) {
                    if new.start == old.start {
                        delta = delta.saturating_add(new.writes.saturating_sub(old.writes));
                    }
                    observed = Some(new);
                }
            } else {
                observed = open_thread(&entry.path());
            }
            if let Some(thread) = observed {
                current.insert(tid, thread);
            }
        }
        if start_time(&dir.join("stat")) != Some(process.start) || current.is_empty() {
            return None;
        }
        counter.total = counter.total.saturating_add(delta);
        // Missing/unreadable tasks lose both baseline and descriptor.
        counter.threads = current;
        Some(counter.total)
    }
}
fn read_writes(file: &mut File) -> Option<u64> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut raw = String::with_capacity(512);
    file.read_to_string(&mut raw).ok()?;
    raw.lines()
        .find_map(|line| line.strip_prefix("write_bytes:")?.trim().parse().ok())
}
fn open_thread(path: &Path) -> Option<Thread> {
    let start = start_time(&path.join("stat"))?;
    let mut file = File::open(path.join("io")).ok()?;
    let writes = read_writes(&mut file)?;
    // Verify the descriptor was opened during the observed incarnation.
    if start_time(&path.join("stat")) != Some(start) {
        return None;
    }
    let mut kind: libc::statfs = unsafe { std::mem::zeroed() };
    let proc_inode = unsafe { libc::fstatfs(file.as_raw_fd(), &mut kind) } == 0
        && kind.f_type == libc::PROC_SUPER_MAGIC;
    Some(Thread {
        start,
        writes,
        io: proc_inode.then_some(file),
    })
}
fn start_time(path: &Path) -> Option<u64> {
    let mut raw = String::with_capacity(1024);
    File::open(path).ok()?.read_to_string(&mut raw).ok()?;
    raw[raw.rfind(')')? + 1..]
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}
