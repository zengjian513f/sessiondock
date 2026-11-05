//! Observed task-local writes, excluding I/O inherited when children are reaped.
use process_links::Process;
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Counter {
    total: u64,
    threads: BTreeMap<(u32, u64), u64>,
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
            // Keep monotonic totals, but never bridge an unobserved interval.
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
        let prior = std::mem::take(&mut counter.threads);
        let dir = root.join(process.pid.to_string());
        let entries = fs::read_dir(dir.join("task")).ok()?;
        let mut current = BTreeMap::new();
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let Some(tid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let Some(start) = start_time(&entry.path().join("stat")) else {
                continue;
            };
            let Some(writes) = fs::read_to_string(entry.path().join("io"))
                .ok()
                .and_then(|text| {
                    text.lines().find_map(|line| {
                        line.strip_prefix("write_bytes:")?
                            .trim()
                            .parse::<u64>()
                            .ok()
                    })
                })
            else {
                continue;
            };
            // A recycled TID between stat and io must not join two lifetimes.
            if start_time(&entry.path().join("stat")) != Some(start) {
                continue;
            }
            current.insert((tid, start), writes);
        }
        if start_time(&dir.join("stat")) != Some(process.start) || current.is_empty() {
            return None;
        }
        for (thread, writes) in &current {
            if let Some(old) = prior.get(thread) {
                if let Some(delta) = writes.checked_sub(*old) {
                    counter.total = counter.total.saturating_add(delta);
                }
            }
        }
        // Missing/unreadable tasks lose their baseline, even if other tasks were read.
        counter.threads = current;
        Some(counter.total)
    }
}
fn start_time(path: &Path) -> Option<u64> {
    let raw = fs::read_to_string(path).ok()?;
    raw[raw.rfind(')')? + 1..]
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}
