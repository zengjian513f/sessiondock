//! Durable, sampled accounting. No browser request advances counters.
use process_links::{Process, Report, Session, agent::Resources};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
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
    #[serde(default)]
    storage_write_bytes: f64,
    #[serde(default)]
    storage_write_observed_seconds: f64,
    #[serde(default)]
    storage_write_v2_bytes: f64,
    #[serde(default)]
    storage_write_v2_observed_seconds: f64,
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
    #[serde(default)]
    rows: BTreeMap<String, Row>,
}
struct Prior {
    owner: String,
    cpu: f64,
    write_bytes: Option<u64>,
}
pub struct Ledger {
    db: Connection,
    dirty: BTreeSet<String>,
    persisted_gaps: usize,
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
        let database = path.with_extension("sqlite3");
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&database)?;
        fs::set_permissions(&database, fs::Permissions::from_mode(0o600))?;
        let db = Connection::open(database).map_err(io::Error::other)?;
        db.busy_timeout(std::time::Duration::from_secs(10))
            .map_err(io::Error::other)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS usage_meta(id INTEGER PRIMARY KEY CHECK(id=1),data TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS usage_rows(key TEXT PRIMARY KEY,data TEXT NOT NULL) WITHOUT ROWID;
          CREATE TABLE IF NOT EXISTS usage_gaps(id INTEGER PRIMARY KEY,start REAL NOT NULL,end REAL NOT NULL);").map_err(io::Error::other)?;
        let meta: Option<String> = db
            .query_row("SELECT data FROM usage_meta WHERE id=1", [], |r| r.get(0))
            .optional()
            .map_err(io::Error::other)?;
        let imported = meta.is_none();
        let saved = if let Some(meta) = meta {
            let mut saved: Saved = serde_json::from_str(&meta).map_err(io::Error::other)?;
            let mut rows = db
                .prepare("SELECT key,data FROM usage_rows")
                .map_err(io::Error::other)?;
            for row in rows
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(io::Error::other)?
            {
                let (key, data) = row.map_err(io::Error::other)?;
                saved
                    .rows
                    .insert(key, serde_json::from_str(&data).map_err(io::Error::other)?);
            }
            let mut gaps = db
                .prepare("SELECT start,end FROM usage_gaps ORDER BY id")
                .map_err(io::Error::other)?;
            saved.gaps = gaps
                .query_map([], |r| Ok([r.get(0)?, r.get(1)?]))
                .map_err(io::Error::other)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(io::Error::other)?;
            saved
        } else {
            match fs::File::open(path) {
                Ok(file) => {
                    file.set_permissions(fs::Permissions::from_mode(0o600))?;
                    serde_json::from_reader::<_, Saved>(io::BufReader::new(file))
                        .map_err(io::Error::other)?
                }
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
            }
        };
        if saved.node_id != node || saved.version != 1 {
            return Err(io::Error::other("usage ledger identity/version mismatch"));
        }
        let dirty = if imported {
            saved.rows.keys().cloned().collect()
        } else {
            BTreeSet::new()
        };
        let persisted_gaps = if imported { 0 } else { saved.gaps.len() };
        let mut ledger = Self {
            db,
            dirty,
            persisted_gaps,
            saved,
            boot: String::new(),
            at: 0.,
            gpu_known: BTreeSet::new(),
            utilization: BTreeMap::new(),
            previous: BTreeMap::new(),
            devices: BTreeMap::new(),
        };
        if imported {
            ledger.save()?;
        }
        Ok(ledger)
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
        let mut groups: BTreeMap<String, (Session, String, f64, BTreeSet<String>, f64, bool)> =
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
            let write_delta = if continuous {
                self.previous
                    .get(&sample.process)
                    .filter(|p| p.owner == owner)
                    .and_then(|p| sample.write_bytes.zip(p.write_bytes))
                    .and_then(|(now, old)| now.checked_sub(old))
            } else {
                None
            };
            previous.insert(
                sample.process.clone(),
                Prior {
                    owner: owner.clone(),
                    cpu: sample.cpu_seconds,
                    write_bytes: sample.write_bytes,
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
                let group = groups.entry(key).or_insert((
                    session,
                    scope.into(),
                    0.,
                    BTreeSet::new(),
                    0.,
                    false,
                ));
                group.2 += delta;
                if let Some(bytes) = write_delta {
                    group.4 += bytes as f64;
                    group.5 = true;
                }
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
        for (key, (session, scope, cpu, uuids, writes, writes_known)) in groups {
            if continuous {
                // Split at Beijing day boundaries; callers select local date ranges explicitly.
                let mut start = self.at;
                while start < at {
                    let day = ((start + 28800.) / 86400.).floor() as i64;
                    let end = at.min((day + 1) as f64 * 86400. - 28800.);
                    let fraction = (end - start) / dt;
                    let row_key = format!("{day}:{key}");
                    self.dirty.insert(row_key.clone());
                    let row = self.saved.rows.entry(row_key).or_insert(Row {
                        session: session.clone(),
                        scope: scope.clone(),
                        day,
                        cpu_seconds: 0.,
                        storage_write_bytes: 0.,
                        storage_write_observed_seconds: 0.,
                        storage_write_v2_bytes: 0.,
                        storage_write_v2_observed_seconds: 0.,
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
                    row.storage_write_v2_bytes += writes * fraction;
                    if writes_known {
                        row.storage_write_v2_observed_seconds += end - start;
                    }
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
        json!({"version":1,"node_id":self.saved.node_id,"started_at":self.saved.started_at,"updated_at":self.saved.updated_at,"gap_seconds":self.saved.gap_seconds,"gaps":self.saved.gaps,"rows":self.saved.rows.values().collect::<Vec<_>>(),"cpu_status":"partial","storage_write_status":"partial","storage_write_version":2,"gpu_status":"estimated","compute_status":"estimated","device_models":self.saved.device_models,"timezone":"Asia/Shanghai","reason":"CPU 为已观察进程累计执行时间差，短命进程可能漏计；GPU 为计算上下文驻留采样，非独占卡时；有效计算卡时按 pmon 进程 SM 利用率估算，缺失不补零；同卡并发利用率相加后封顶，可能重叠。存储写入 v2 按 /proc/PID/task/TID/io 的线程身份连续差分累计，排除回收子进程历史；首次、漏采与重启仅建立基线。旧 storage_write_bytes 保留为历史，当前使用 storage_write_v2_bytes，不扣取消写入，不代表实际落盘量或完整 NFS 写入量；短命进程及未观测区间可能漏计。每日桶按北京时间划分。"})
    }
    pub fn save(&mut self) -> io::Result<()> {
        let tx = self.db.transaction().map_err(io::Error::other)?;
        let meta = json!({"version":self.saved.version,"node_id":self.saved.node_id,
            "started_at":self.saved.started_at,"updated_at":self.saved.updated_at,
            "gap_seconds":self.saved.gap_seconds,"device_models":self.saved.device_models});
        tx.execute(
            "INSERT INTO usage_meta VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
            [meta.to_string()],
        )
        .map_err(io::Error::other)?;
        {
            let mut insert=tx.prepare_cached("INSERT INTO usage_rows VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET data=excluded.data").map_err(io::Error::other)?;
            for key in &self.dirty {
                if let Some(row) = self.saved.rows.get(key) {
                    insert
                        .execute(params![
                            key,
                            serde_json::to_string(row).map_err(io::Error::other)?
                        ])
                        .map_err(io::Error::other)?;
                }
            }
        }
        for (id, gap) in self.saved.gaps.iter().enumerate().skip(self.persisted_gaps) {
            tx.execute(
                "INSERT INTO usage_gaps VALUES(?1,?2,?3)",
                params![id, gap[0], gap[1]],
            )
            .map_err(io::Error::other)?;
        }
        tx.commit().map_err(io::Error::other)?;
        self.dirty.clear();
        self.persisted_gaps = self.saved.gaps.len();
        Ok(())
    }
}
