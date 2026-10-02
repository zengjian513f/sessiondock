//! Low-frequency resctrl MBM monitoring. Only MON groups are created: no
//! schemata, CPU affinity, allocation, bandwidth limits or workload signals.
use process_links::{Binding, Process, Session, agent::SessionMeasurement};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    time::Instant,
};

pub const METRIC: &str = "memory_bandwidth_bytes_per_second";
type Identity = (String, String, String);
fn identity(s: &Session) -> Identity {
    (s.node_id.clone(), s.source.clone(), s.sid.clone())
}
fn unavailable(reason: &str) -> Value {
    json!({"value":null,"status":"unavailable","reason":reason,"refresh_interval_seconds":5})
}
#[derive(Clone, Default)]
pub struct Observation {
    pub rows: Vec<SessionMeasurement>,
    pub sampled_at: f64,
    pub reason: String,
}
struct Group {
    path: PathBuf,
    previous: Option<(Instant, BTreeMap<String, u64>)>,
}
pub struct Collector {
    root: PathBuf,
    prefix: String,
    groups: BTreeMap<Identity, Group>,
    next: u64,
    _lock: File,
}
fn directories(path: &Path) -> Vec<PathBuf> {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect()
}
fn tids(path: &Path) -> BTreeSet<u32> {
    fs::read_to_string(path.join("tasks"))
        .unwrap_or_default()
        .lines()
        .filter_map(|s| s.parse().ok())
        .collect()
}
fn counters(path: &Path) -> BTreeMap<String, u64> {
    directories(&path.join("mon_data"))
        .into_iter()
        .filter_map(|p| {
            let v = fs::read_to_string(p.join("mbm_total_bytes"))
                .ok()?
                .trim()
                .parse()
                .ok()?;
            Some((p.file_name()?.to_str()?.to_owned(), v))
        })
        .collect()
}
/// Counter reset/unavailable domains never turn into a huge rate or fake zero.
fn delta(old: &BTreeMap<String, u64>, new: &BTreeMap<String, u64>, elapsed: f64) -> Option<f64> {
    let values: Vec<_> = new
        .iter()
        .filter_map(|(k, v)| v.checked_sub(*old.get(k)?))
        .collect();
    (!values.is_empty() && elapsed > 0.0)
        .then(|| values.iter().map(|v| *v as f64).sum::<f64>() / elapsed)
}
impl Collector {
    pub fn open(root: &Path, node: &str, lock_path: &Path) -> io::Result<Self> {
        let features = fs::read_to_string(root.join("info/L3_MON/mon_features"))?;
        if !features.lines().any(|f| f == "mbm_total_bytes") {
            return Err(io::Error::other("硬件不支持内存带宽监控"));
        }
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path)?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::other("内存带宽采集已由其他实例占用"));
        }
        let prefix = format!("sessiondock-{node}-");
        // Recover only our own orphan MON groups after a killed/restarted agent.
        for p in directories(&root.join("mon_groups")) {
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(&prefix))
            {
                fs::remove_dir(p)?;
            }
        }
        Ok(Self {
            root: root.into(),
            prefix,
            groups: BTreeMap::new(),
            next: 0,
            _lock: lock,
        })
    }
    pub fn collect(
        &mut self,
        bindings: &[Binding],
        proc_root: &Path,
        sampled_at: f64,
    ) -> Observation {
        let mut desired: BTreeMap<Identity, (Session, BTreeSet<Process>)> = BTreeMap::new();
        for b in bindings {
            desired
                .entry(identity(&b.session))
                .or_insert_with(|| (b.session.clone(), BTreeSet::new()))
                .1
                .insert(b.process.clone());
        }
        self.groups.retain(|id, g| {
            if desired.contains_key(id) {
                true
            } else {
                let _ = fs::remove_dir(&g.path);
                false
            }
        });
        // Respect another application's monitor/control assignments. Never steal
        // its threads, delete its groups or alter its resource allocation.
        let mut foreign = BTreeSet::new();
        for p in directories(&self.root) {
            let name = p.file_name().unwrap().to_string_lossy();
            if !["info", "mon_data", "mon_groups"].contains(&name.as_ref()) {
                foreign.extend(tids(&p));
                for g in directories(&p.join("mon_groups")) {
                    foreign.extend(tids(&g));
                }
            }
        }
        for p in directories(&self.root.join("mon_groups")) {
            if !p
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&self.prefix)
            {
                foreign.extend(tids(&p));
            }
        }
        let mut rows = Vec::new();
        for (id, (session, processes)) in desired {
            if !self.groups.contains_key(&id) {
                self.next += 1;
                let path = self
                    .root
                    .join("mon_groups")
                    .join(format!("{}{}", self.prefix, self.next));
                if let Err(e) = fs::create_dir(&path) {
                    rows.push(SessionMeasurement {
                        session,
                        metrics: BTreeMap::from([(
                            METRIC.into(),
                            unavailable(&format!("监控组不可用或硬件计数器已用尽：{e}")),
                        )]),
                    });
                    continue;
                }
                self.groups.insert(
                    id.clone(),
                    Group {
                        path,
                        previous: None,
                    },
                );
            }
            let g = self.groups.get_mut(&id).unwrap();
            let current = tids(&g.path);
            let mut assigned = BTreeSet::new();
            let mut skipped = 0;
            for process in &processes {
                if !crate::memory::matches(proc_root, process) {
                    skipped += 1;
                    continue;
                }
                for task in directories(&proc_root.join(process.pid.to_string()).join("task")) {
                    let Some(tid) = task
                        .file_name()
                        .and_then(|s| s.to_str())
                        .and_then(|s| s.parse::<u32>().ok())
                    else {
                        continue;
                    };
                    // The open proc task directory belongs to this process. Check
                    // incarnation again immediately before a kernel assignment.
                    if foreign.contains(&tid) || !crate::memory::matches(proc_root, process) {
                        skipped += 1;
                        continue;
                    }
                    if current.contains(&tid)
                        || fs::write(g.path.join("tasks"), format!("{tid}\n")).is_ok()
                    {
                        assigned.insert(tid);
                    } else {
                        skipped += 1;
                    }
                }
            }
            // A live unbound thread can remain after an attribution change.
            // Returning it to the parent monitor does not change its cgroup.
            for tid in current.difference(&assigned) {
                if !foreign.contains(tid) {
                    let _ = fs::write(self.root.join("tasks"), format!("{tid}\n"));
                }
            }
            let values = counters(&g.path);
            let at = Instant::now();
            let rate = g
                .previous
                .as_ref()
                .and_then(|(t, v)| delta(v, &values, at.duration_since(*t).as_secs_f64()));
            let status = if assigned.is_empty() {
                "unavailable"
            } else if rate.is_some() {
                "partial"
            } else {
                "warming_up"
            };
            let value = if assigned.is_empty() { None } else { rate };
            let item = json!({"value":value,"status":status,"reason":format!("硬件 MBM 会话总内存流量，未拆分读写；约 5 秒更新，仅覆盖已归属线程及有效计数域，短命进程与分组变化可能漏计；跳过 {skipped} 个线程或进程"),"sampled_at":sampled_at,"refresh_interval_seconds":5,"observed_domains":values.len(),"observed_threads":assigned.len()});
            g.previous = Some((at, values));
            rows.push(SessionMeasurement {
                session,
                metrics: BTreeMap::from([(METRIC.into(), item)]),
            });
        }
        Observation {
            rows,
            sampled_at,
            reason: "按活跃会话采集硬件内存带宽；不提供逐进程带宽".into(),
        }
    }
}
impl Drop for Collector {
    fn drop(&mut self) {
        for g in self.groups.values() {
            let _ = fs::remove_dir(&g.path);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn domains_reset_and_missing_are_not_zero() {
        let old = BTreeMap::from([("a".into(), 100), ("b".into(), 900)]);
        assert_eq!(
            delta(&old, &BTreeMap::from([("a".into(), 200)]), 5.0),
            Some(20.0)
        );
        assert_eq!(delta(&old, &BTreeMap::from([("a".into(), 1)]), 5.0), None);
        assert_eq!(delta(&old, &BTreeMap::new(), 5.0), None);
        assert_eq!(delta(&old, &old, 5.0), Some(0.0));
    }
}
