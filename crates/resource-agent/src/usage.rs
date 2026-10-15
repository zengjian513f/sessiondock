//! Durable, sampled accounting. No browser request advances counters.
use process_links::{Process, Report, Session, agent::Resources};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

fn identity(s: &Session) -> String {
    serde_json::to_string(&(&s.node_id, &s.source, &s.sid)).unwrap()
}
#[derive(Clone, Serialize, Deserialize)]
struct Row {
    session: Session,
    scope: String,
    day: i64,
    cpu_seconds: f64,
    gpu_seconds: BTreeMap<String, f64>,
    compute_seconds: BTreeMap<String, f64>,
    compute_observed_seconds: f64,
    gpu_observed_seconds: f64,
    observed_seconds: f64,
    first_at: f64,
    last_at: f64,
}
#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    node_id: String,
    started_at: f64,
    updated_at: f64,
    gap_seconds: f64,
    #[serde(default)]
    gaps: Vec<[f64; 2]>,
    device_models: BTreeMap<String, String>,
    rows: BTreeMap<String, Row>,
}
struct Prior {
    owner: String,
    cpu: f64,
}
pub struct Ledger {
    path: PathBuf,
    saved: Saved,
    boot: String,
    at: f64,
    gpu_known: BTreeSet<String>,
    utilization: BTreeMap<String, BTreeMap<String, Option<f64>>>,
    previous: BTreeMap<Process, Prior>,
    devices: BTreeMap<String, BTreeSet<String>>,
}
impl Ledger {
    pub fn open(path: &Path, node: &str, at: f64) -> io::Result<Self> {
        let saved = match fs::read(path) {
            Ok(bytes) => serde_json::from_slice::<Saved>(&bytes).map_err(io::Error::other)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Saved {
                version: 1,
                node_id: node.into(),
                started_at: at,
                updated_at: at,
                gap_seconds: 0.,
                gaps: Vec::new(),
                device_models: BTreeMap::new(),
                rows: BTreeMap::new(),
            },
            Err(e) => return Err(e),
        };
        if saved.node_id != node || saved.version != 1 {
            return Err(io::Error::other("usage ledger identity/version mismatch"));
        }
        Ok(Self {
            path: path.into(),
            saved,
            boot: String::new(),
            at: 0.,
            gpu_known: BTreeSet::new(),
            utilization: BTreeMap::new(),
            previous: BTreeMap::new(),
            devices: BTreeMap::new(),
        })
    }
    pub fn observe(&mut self, resources: &Resources, report: &Report) {
        let at = resources.sampled_at;
        if at <= self.at {
            return;
        }
        let dt = at - self.at;
        let continuous = self.at > 0. && self.boot == resources.boot_id && dt <= 10.;
        if !continuous {
            self.saved.gap_seconds += (at - self.saved.updated_at).max(0.);
            self.saved.gaps.push([self.saved.updated_at, at]);
        }
        let bindings: BTreeMap<_, _> = report.bindings.iter().map(|b| (&b.process, b)).collect();
        let mut previous = BTreeMap::new();
        let mut groups: BTreeMap<String, (Session, String, f64, BTreeSet<String>)> =
            BTreeMap::new();
        let mut gpu_known = BTreeSet::new();
        let mut utilization: BTreeMap<String, BTreeMap<String, Option<f64>>> = BTreeMap::new();
        for sample in &resources.samples {
            if let Some(models) = sample.metrics.get("gpu_models").and_then(Value::as_object) {
                for (uuid, name) in models {
                    if let Some(name) = name.as_str() {
                        self.saved.device_models.insert(uuid.clone(), name.into());
                    }
                }
            }
            let Some(binding) = bindings.get(&sample.process) else {
                continue;
            };
            if previous.contains_key(&sample.process) {
                continue;
            }
            let owner = identity(&binding.session);
            let delta = if continuous {
                self.previous
                    .get(&sample.process)
                    .filter(|p| p.owner == owner && sample.cpu_seconds >= p.cpu)
                    .map_or(0., |p| sample.cpu_seconds - p.cpu)
            } else {
                0.
            };
            previous.insert(
                sample.process.clone(),
                Prior {
                    owner: owner.clone(),
                    cpu: sample.cpu_seconds,
                },
            );
            let mut targets = BTreeMap::from([
                (
                    format!("direct:{owner}"),
                    (binding.session.clone(), "direct"),
                ),
                (
                    format!("inclusive:{owner}"),
                    (binding.session.clone(), "inclusive"),
                ),
            ]);
            for ancestor in binding
                .launch_chain
                .iter()
                .map(|l| &l.session)
                .chain(binding.initiator.iter())
                .chain(binding.spawner.iter())
            {
                targets.insert(
                    format!("inclusive:{}", identity(ancestor)),
                    (ancestor.clone(), "inclusive"),
                );
            }
            let gpu = sample.metrics.get("gpu_devices");
            let fresh = gpu.is_some_and(|g| {
                matches!(g["status"].as_str(), Some("ok" | "partial"))
                    && g["sampled_at"]
                        .as_f64()
                        .is_some_and(|t| at - t >= 0. && at - t <= 30.)
            });
            for (key, (session, scope)) in targets {
                if fresh {
                    gpu_known.insert(key.clone());
                }
                let compute = utilization.entry(key.clone()).or_default();
                let group =
                    groups
                        .entry(key)
                        .or_insert((session, scope.into(), 0., BTreeSet::new()));
                group.2 += delta;
                if fresh {
                    for uuid in gpu.unwrap()["value"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        group.3.insert(uuid.into());
                        let value = sample
                            .metrics
                            .get("gpu_utilization_by_device")
                            .and_then(|v| v["value"][uuid].as_f64())
                            .filter(|v| (0. ..=100.).contains(v))
                            .map(|v| v / 100.);
                        let entry = compute.entry(uuid.into()).or_insert(Some(0.));
                        *entry = entry.zip(value).map(|(a, b)| (a + b).min(1.));
                    }
                }
            }
        }
        let mut devices = BTreeMap::new();
        for (key, (session, scope, cpu, uuids)) in groups {
            if continuous {
                // Split at Beijing day boundaries; callers select local date ranges explicitly.
                let mut start = self.at;
                while start < at {
                    let day = ((start + 28800.) / 86400.).floor() as i64;
                    let end = at.min((day + 1) as f64 * 86400. - 28800.);
                    let fraction = (end - start) / dt;
                    let row = self
                        .saved
                        .rows
                        .entry(format!("{day}:{key}"))
                        .or_insert(Row {
                            session: session.clone(),
                            scope: scope.clone(),
                            day,
                            cpu_seconds: 0.,
                            gpu_seconds: BTreeMap::new(),
                            compute_seconds: BTreeMap::new(),
                            compute_observed_seconds: 0.,
                            gpu_observed_seconds: 0.,
                            observed_seconds: 0.,
                            first_at: start,
                            last_at: end,
                        });
                    if session.title.is_some() {
                        row.session.title = session.title.clone();
                    }
                    row.cpu_seconds += cpu * fraction;
                    row.observed_seconds += end - start;
                    row.last_at = end;
                    if gpu_known.contains(&key) && self.gpu_known.contains(&key) {
                        row.gpu_observed_seconds += end - start;
                    }
                    if let Some(old) = self.devices.get(&key) {
                        for uuid in uuids.intersection(old) {
                            *row.gpu_seconds.entry(uuid.clone()).or_default() += end - start;
                            let before = self
                                .utilization
                                .get(&key)
                                .and_then(|m| m.get(uuid))
                                .copied()
                                .flatten();
                            let after = utilization
                                .get(&key)
                                .and_then(|m| m.get(uuid))
                                .copied()
                                .flatten();
                            if let Some((a, b)) = before.zip(after) {
                                *row.compute_seconds.entry(uuid.clone()).or_default() +=
                                    (a + b) / 2. * (end - start);
                                row.compute_observed_seconds += end - start;
                            }
                        }
                    }
                    start = end;
                }
            }
            devices.insert(key, uuids);
        }
        self.gpu_known = gpu_known;
        self.utilization = utilization;
        self.devices = devices;
        self.previous = previous;
        self.at = at;
        self.boot = resources.boot_id.clone();
        self.saved.updated_at = at;
    }
    pub fn view(&self) -> Value {
        json!({"version":1,"node_id":self.saved.node_id,"started_at":self.saved.started_at,"updated_at":self.saved.updated_at,"gap_seconds":self.saved.gap_seconds,"gaps":self.saved.gaps,"rows":self.saved.rows.values().collect::<Vec<_>>(),"cpu_status":"partial","gpu_status":"estimated","compute_status":"estimated","device_models":self.saved.device_models,"timezone":"Asia/Shanghai","reason":"CPU 为已观察进程累计执行时间差，短命进程可能漏计；GPU 为计算上下文驻留采样，非独占卡时；有效计算卡时按 pmon 进程 SM 利用率估算，缺失不补零；同卡并发利用率相加后封顶，可能重叠。每日桶按北京时间划分。"})
    }
    pub fn save(&self) -> io::Result<()> {
        let tmp = self.path.with_extension("tmp");
        let mut file = fs::File::create(&tmp)?;
        serde_json::to_writer(&mut file, &self.saved)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(tmp, &self.path)?;
        if let Some(parent) = self.path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}
