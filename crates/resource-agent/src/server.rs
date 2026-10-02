use crate::{
    bandwidth,
    events::{self, Event},
    gpu, io_events, memory,
};
use process_links::{
    Process, Report,
    agent::{CollectorStatus, Request, Resources, Sample},
    engine::{Engine, Saved},
    linux::{self, Snapshot},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufRead, BufReader, BufWriter, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
struct Config {
    node_id: String,
    uid: u32,
    proc_root: PathBuf,
    socket: PathBuf,
    state: PathBuf,
    events: bool,
    io_events: bool,
    memory_bandwidth: bool,
}
impl Config {
    fn load() -> io::Result<Self> {
        let mut args = std::env::args().skip(1);
        let mut options = BTreeMap::new();
        while let Some(key) = args.next() {
            if key == "--help" {
                println!(
                    "resource-agent --node-id-file PATH --uid UID [--socket PATH] [--state PATH] [--proc-root PATH] [--events auto|off] [--io-events off|on (default off)] [--memory-bandwidth off|on (default off)]\nLocal JSON-line API: health, report, resources, catalog, publish, probe (60-second lease). Never signals workloads or changes allocation; memory bandwidth uses monitor-only thread groups."
                );
                std::process::exit(0);
            }
            if ![
                "--node-id-file",
                "--uid",
                "--socket",
                "--state",
                "--proc-root",
                "--events",
                "--io-events",
                "--memory-bandwidth",
            ]
            .contains(&key.as_str())
            {
                return Err(io::Error::other(format!("unknown option {key}")));
            }
            options.insert(
                key,
                args.next()
                    .ok_or_else(|| io::Error::other("missing option value"))?,
            );
        }
        let node_file = options
            .get("--node-id-file")
            .ok_or_else(|| io::Error::other("--node-id-file required"))?;
        let node_id = fs::read_to_string(node_file)?.trim().to_owned();
        if node_id.len() != 32
            || !node_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(io::Error::other("invalid node identity"));
        }
        let uid = options
            .get("--uid")
            .ok_or_else(|| io::Error::other("--uid required"))?
            .parse()
            .map_err(io::Error::other)?;
        let event_mode = options
            .get("--events")
            .map(String::as_str)
            .unwrap_or("auto");
        if !["auto", "off"].contains(&event_mode) {
            return Err(io::Error::other("events must be auto or off"));
        }
        let io_mode = options
            .get("--io-events")
            .map(String::as_str)
            .unwrap_or("off");
        if !["on", "off"].contains(&io_mode) {
            return Err(io::Error::other("io-events must be on or off"));
        }
        let bandwidth_mode = options
            .get("--memory-bandwidth")
            .map(String::as_str)
            .unwrap_or("off");
        if !["on", "off"].contains(&bandwidth_mode) {
            return Err(io::Error::other("memory-bandwidth must be on or off"));
        }
        Ok(Self {
            memory_bandwidth: bandwidth_mode == "on",
            io_events: io_mode == "on",
            node_id,
            uid,
            proc_root: options
                .get("--proc-root")
                .map_or_else(|| PathBuf::from("/proc"), PathBuf::from),
            socket: options
                .get("--socket")
                .map_or_else(process_links::agent::socket_path, PathBuf::from),
            state: options.get("--state").map_or_else(
                || PathBuf::from("/var/lib/resource-agent/state.json"),
                PathBuf::from,
            ),
            events: event_mode == "auto",
        })
    }
}
struct Diagnostic {
    supported: bool,
    deadline: Option<Instant>,
    leases: BTreeMap<Option<String>, Instant>,
    state: &'static str,
    window: IoWindow,
    lost: Arc<AtomicU64>,
}
impl Diagnostic {
    fn value(&self) -> Value {
        let remaining = self
            .deadline
            .map(|d| {
                d.saturating_duration_since(Instant::now())
                    .as_secs_f64()
                    .ceil() as u64
            })
            .unwrap_or(0);
        json!({"state": if !self.supported { "unsupported" } else { self.state }, "remaining_seconds":remaining,
            "error": if self.state == "failed" { Some("探测启动失败或已异常停止，请检查采集服务日志") } else { None }})
    }
}
fn diagnostic_worker(uid: u32, control: Arc<Mutex<Diagnostic>>) {
    let mut child: Option<std::process::Child> = None;
    let mut receiver = None;
    let mut sent_deadline = None;
    loop {
        let mut d = control.lock().unwrap_or_else(|e| e.into_inner());
        d.leases.retain(|_, deadline| *deadline > Instant::now());
        d.deadline = d.leases.values().copied().max();
        let expired = d
            .deadline
            .is_some_and(|deadline| deadline <= Instant::now());
        if STOP.load(Ordering::Relaxed) || expired || d.deadline.is_none() {
            d.deadline = None;
            d.leases.clear();
            d.window = IoWindow::new(false);
            if let Some(mut running) = child.take() {
                let failed = d.state == "failed";
                d.state = "stopping";
                drop(d);
                // No pinned objects: reaping the helper closes and detaches every link.
                let _ = running.kill();
                let _ = running.wait();
                receiver = None;
                d = control.lock().unwrap_or_else(|e| e.into_inner());
                d.state = if failed { "failed" } else { "off" };
            } else if d.state == "stopping" {
                d.state = "off";
            }
            if STOP.load(Ordering::Relaxed) {
                break;
            }
        } else if child.is_none() {
            let (sender, rx) = mpsc::sync_channel(128);
            match io_events::start(uid, sender, d.lost.clone()) {
                Ok(running) => {
                    child = Some(running);
                    sent_deadline = None;
                    receiver = Some(rx);
                }
                Err(error) => {
                    eprintln!("I/O probe start failed: {error}");
                    d.deadline = None;
                    d.leases.clear();
                    d.state = "failed";
                    d.window = IoWindow::new(false);
                }
            }
        }
        if let (Some(running), Some(deadline)) = (child.as_mut(), d.deadline) {
            if sent_deadline != Some(deadline) {
                if io_events::renew(running, deadline.saturating_duration_since(Instant::now()))
                    .is_ok()
                {
                    sent_deadline = Some(deadline);
                } else {
                    d.state = "failed";
                    d.deadline = None;
                    d.leases.clear();
                }
            }
        }
        if let Some(rx) = &receiver {
            for event in rx.try_iter() {
                match &event {
                    io_events::Event::Ready => d.state = "active",
                    io_events::Event::Failed => {
                        d.state = "failed";
                        d.deadline = None;
                        d.leases.clear();
                    }
                    _ => {}
                }
                let lost = d.lost.load(Ordering::Relaxed);
                d.window.observe(event, lost);
            }
        }
        if let Some(running) = child.as_mut() {
            if let Ok(Some(_)) = running.try_wait() {
                child = None;
                receiver = None;
                if d.deadline.is_some() {
                    d.state = "failed";
                }
                d.deadline = None;
                d.leases.clear();
                d.window = IoWindow::new(false);
            }
        }
        drop(d);
        std::thread::sleep(Duration::from_millis(50));
    }
}
struct State {
    engine: Engine,
    snapshot: Snapshot,
    report: Report,
    resources: Resources,
    status: CollectorStatus,
    forks: u64,
    execs: u64,
    exits: u64,
    diagnostic: Arc<Mutex<Diagnostic>>,
}
impl State {
    fn update(&mut self, snapshot: Snapshot, resources: Resources) {
        self.report = self
            .engine
            .update(&snapshot, resources.sampled_at, self.status.clone());
        self.snapshot = snapshot;
        self.resources = resources;
    }
    fn refresh(&mut self) {
        self.report = self.engine.update(
            &self.snapshot,
            self.resources.sampled_at,
            self.status.clone(),
        );
    }
    fn handle(&mut self, request: Request) -> Result<Value, &'static str> {
        match request {
            Request::Health => Ok(
                json!({"service":"resource-agent", "version":1, "node_id":self.engine.node_id, "boot_id":self.engine.boot_id, "sampled_at":self.report.sampled_at,"collector":self.status,"processes":self.snapshot.entries.len(),"fork_events":self.forks,"exec_events":self.execs,"exit_events":self.exits}),
            ),
            Request::Probe {
                enabled,
                lease_id,
                lease_seconds,
            } => {
                let mut diagnostic = self.diagnostic.lock().unwrap_or_else(|e| e.into_inner());
                if !diagnostic.supported {
                    return Err("此机器不支持临时 I/O 探测");
                }
                diagnostic
                    .leases
                    .retain(|_, deadline| *deadline > Instant::now());
                let was_active = diagnostic.deadline.is_some_and(|d| d > Instant::now());
                if enabled {
                    if lease_id.is_some() || !diagnostic.leases.contains_key(&None) {
                        let seconds = if lease_id.is_some() {
                            lease_seconds.unwrap_or(60).clamp(1, 60)
                        } else {
                            60
                        };
                        diagnostic
                            .leases
                            .insert(lease_id, Instant::now() + Duration::from_secs(seconds));
                    }
                } else if lease_id.is_some() {
                    diagnostic.leases.remove(&lease_id);
                } else {
                    diagnostic.leases.clear();
                }
                diagnostic.deadline = diagnostic.leases.values().copied().max();
                if diagnostic.deadline.is_some() && !was_active {
                    diagnostic.state = "starting";
                    diagnostic.window = IoWindow::new(true);
                    diagnostic.lost.store(0, Ordering::Relaxed);
                } else if diagnostic.deadline.is_none() {
                    if matches!(diagnostic.state, "starting" | "active") {
                        diagnostic.state = "stopping";
                    }
                    diagnostic.window = IoWindow::new(false);
                }
                Ok(diagnostic.value())
            }
            Request::Report => Ok(json!(self.report)),
            Request::Resources => {
                let mut resources = self.resources.clone();
                let processes: BTreeSet<_> = resources
                    .samples
                    .iter()
                    .map(|sample| &sample.process)
                    .collect();
                resources.bindings = self
                    .report
                    .bindings
                    .iter()
                    .filter(|binding| processes.contains(&binding.process))
                    .cloned()
                    .collect();
                resources.sessions = process_links::resource_summary::sessions(&resources);
                let diagnostic = self.diagnostic.lock().unwrap_or_else(|e| e.into_inner());
                if diagnostic.state != "active"
                    || diagnostic.deadline.is_none_or(|d| d <= Instant::now())
                {
                    for sample in &mut resources.samples {
                        for (name, _, _, _) in IO_METRICS {
                            sample.metrics.insert(
                                name.into(),
                                metric(Value::Null, "unavailable", "临时探测未开启或正在启动"),
                            );
                        }
                    }
                    for (name, _, _, _) in IO_METRICS {
                        resources.metric_availability.insert(
                            name.into(),
                            metric(Value::Null, "unavailable", "临时探测未开启或正在启动"),
                        );
                    }
                    resources.sessions = process_links::resource_summary::sessions(&resources);
                }
                let mut value = json!(resources);
                value["diagnostic"] = diagnostic.value();
                Ok(value)
            }
            Request::Catalog(catalog) => {
                if !self.engine.catalog(catalog) {
                    return Err("catalog belongs to another node or boot");
                }
                self.refresh();
                Ok(json!(self.report))
            }
            Request::Publish(published) => {
                self.engine.publish(published, &self.snapshot);
                self.refresh();
                Ok(json!(self.report))
            }
        }
    }
}
fn metric(value: Value, status: &str, reason: &str) -> Value {
    json!({"value":value, "status":status, "reason":reason})
}
const IO_METRICS: [(&str, io_events::IoKind, &str, bool); 10] = [
    (
        "disk_read_bytes_per_second",
        io_events::IoKind::LocalRead,
        "Synchronous local regular-file VFS reads; excludes NFS, mmap, io_uring and splice; logical bytes, not physical disk traffic",
        false,
    ),
    (
        "disk_write_bytes_per_second",
        io_events::IoKind::LocalWrite,
        "Synchronous local regular-file VFS writes; excludes NFS, mmap, io_uring and splice; logical bytes, not physical disk traffic",
        false,
    ),
    (
        "network_receive_bytes_per_second",
        io_events::IoKind::TcpReceive,
        "Application TCP recvmsg payload; excludes UDP, splice and kernel/NFS wire traffic",
        false,
    ),
    (
        "network_send_bytes_per_second",
        io_events::IoKind::TcpSend,
        "Application TCP sendmsg payload; excludes UDP, splice and kernel/NFS wire traffic",
        false,
    ),
    (
        "nfs_read_bytes_per_second",
        io_events::IoKind::NfsRead,
        "Synchronous NFS VFS reads, including page-cache hits; excludes mmap/io_uring/splice; not RPC or network bytes",
        false,
    ),
    (
        "nfs_write_bytes_per_second",
        io_events::IoKind::NfsWrite,
        "Synchronous NFS VFS writes; excludes mmap/io_uring/splice; not RPC, retransmissions or network bytes",
        false,
    ),
    (
        "disk_read_operations_per_second",
        io_events::IoKind::LocalRead,
        "Successful synchronous disk file read operations including cache hits; excludes mmap/io_uring/splice; not physical device IOPS or NFS RPCs",
        true,
    ),
    (
        "disk_write_operations_per_second",
        io_events::IoKind::LocalWrite,
        "Successful synchronous disk file write operations including cache hits; excludes mmap/io_uring/splice; not physical device IOPS or NFS RPCs",
        true,
    ),
    (
        "nfs_read_operations_per_second",
        io_events::IoKind::NfsRead,
        "Successful synchronous nfs file read operations including cache hits; excludes mmap/io_uring/splice; not physical device IOPS or NFS RPCs",
        true,
    ),
    (
        "nfs_write_operations_per_second",
        io_events::IoKind::NfsWrite,
        "Successful synchronous nfs file write operations including cache hits; excludes mmap/io_uring/splice; not physical device IOPS or NFS RPCs",
        true,
    ),
];

#[derive(Clone)]
struct IoWindow {
    status: &'static str,
    samples: Vec<io_events::IoSample>,
    received: Option<Instant>,
    sampled_at: f64,
    generation: Option<u64>,
    lost_before: u64,
    interval_lost: u64,
}
impl IoWindow {
    fn new(enabled: bool) -> Self {
        Self {
            status: if enabled { "warming_up" } else { "unavailable" },
            samples: Vec::new(),
            received: None,
            sampled_at: 0.0,
            generation: None,
            lost_before: 0,
            interval_lost: 0,
        }
    }
    fn update(&mut self, event: io_events::Event) {
        match event {
            io_events::Event::Ready => self.status = "warming_up",
            io_events::Event::Failed => {
                self.status = "unavailable";
                self.samples.clear();
            }
            io_events::Event::Batch {
                generation,
                samples,
            } => {
                self.samples = samples;
                self.status = "partial";
                self.received = Some(Instant::now());
                self.sampled_at = now();
                self.generation = Some(generation);
            }
        }
    }
    fn observe(&mut self, event: io_events::Event, total_lost: u64) {
        if matches!(&event, io_events::Event::Batch { .. }) {
            self.interval_lost = total_lost.saturating_sub(self.lost_before);
            self.lost_before = total_lost;
        }
        self.update(event);
    }
    fn metric(
        &self,
        kind: io_events::IoKind,
        reason: &str,
        process: Option<&Process>,
        started_at: f64,
        total_lost: u64,
        operations: bool,
    ) -> Value {
        let mut result =
            self.measurement(kind, reason, process, started_at, total_lost, operations);
        result["io_lost_events_total"] = json!(total_lost);
        result
    }
    fn measurement(
        &self,
        kind: io_events::IoKind,
        reason: &str,
        process: Option<&Process>,
        started_at: f64,
        total_lost: u64,
        operations: bool,
    ) -> Value {
        let lost = self.interval_lost + total_lost.saturating_sub(self.lost_before);
        if self.status != "partial" {
            return metric(
                Value::Null,
                self.status,
                "I/O kernel collector disabled, unsupported, failed or awaiting first complete interval",
            );
        }
        if self
            .received
            .is_none_or(|at| at.elapsed() > Duration::from_secs(6))
        {
            return metric(
                Value::Null,
                "unavailable",
                "I/O kernel collector stopped producing completed intervals",
            );
        }
        if started_at > self.sampled_at {
            return metric(
                Value::Null,
                "warming_up",
                "Process started after the latest I/O interval",
            );
        }
        if process.is_none() && lost > 0 {
            return metric(
                Value::Null,
                "unavailable",
                "I/O collector reported errors/loss; absence of a process measurement cannot imply zero",
            );
        }
        let value = process
            .map(|process| {
                let rows: Vec<_> = self
                    .samples
                    .iter()
                    .filter(|s| s.process == *process && s.kind == kind)
                    .collect();
                if lost > 0 && rows.is_empty() {
                    Value::Null
                } else {
                    json!(
                        rows.iter()
                            .map(|s| if operations {
                                s.operations as f64
                            } else {
                                s.bytes as f64
                            })
                            .sum::<f64>()
                            / 2.0
                    )
                }
            })
            .unwrap_or(Value::Null);
        let reason = if lost > 0 {
            format!("{reason}; collector reported {lost} errors/lost events; values are incomplete")
        } else {
            reason.to_owned()
        };
        let mut result = metric(value, "partial", &reason);
        result["interval_seconds"] = json!(2.0);
        result["sampled_at"] = json!(self.sampled_at);
        result["generation"] = json!(self.generation);
        result["io_lost_events_total"] = json!(total_lost);
        result
    }
}

#[derive(Default)]
struct ResourceSampler {
    previous: Option<(Resources, Instant)>,
}
type PssCache = BTreeMap<Process, (Option<u64>, f64)>;
#[derive(Clone, Default)]
struct GpuObservation {
    sampled_at: f64,
    processes: BTreeSet<Process>,
    result: Option<Result<Vec<gpu::GpuSample>, String>>,
}
#[derive(Clone, Default)]
struct SlowObservations {
    bandwidth: bandwidth::Observation,
    pss: PssCache,
    gpu: GpuObservation,
}
impl ResourceSampler {
    fn collect(
        &mut self,
        root: &Path,
        snapshot: &Snapshot,
        node: &str,
        io: &IoWindow,
        io_lost: u64,
        slow_cache: &SlowObservations,
    ) -> Resources {
        let sampled = Instant::now();
        let sampled_at = now();
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as u64;
        let previous = self
            .previous
            .as_ref()
            .filter(|(r, _)| r.boot_id == snapshot.boot_id);
        let elapsed = previous.map(|(_, at)| sampled.duration_since(*at).as_secs_f64());
        let previous: BTreeMap<_, _> = previous
            .into_iter()
            .flat_map(|(r, _)| &r.samples)
            .map(|s| (&s.process, s))
            .collect();
        let observation = &slow_cache.gpu;
        let (gpu_status, gpu_reason) = if root != Path::new("/proc") {
            (
                "unavailable",
                Some("GPU collection requires the live process filesystem".to_owned()),
            )
        } else if observation.result.is_none() {
            (
                "warming_up",
                Some("Waiting for first background GPU observation".to_owned()),
            )
        } else if sampled_at - observation.sampled_at > 30.0 {
            (
                "unavailable",
                Some("GPU observation is older than 30 seconds".to_owned()),
            )
        } else if let Some(Err(reason)) = &observation.result {
            ("unavailable", Some(reason.clone()))
        } else {
            ("partial", None)
        };
        let mut gpu_by_process: BTreeMap<Process, Vec<&gpu::GpuSample>> = BTreeMap::new();
        if let Some(Ok(samples)) = &observation.result {
            for sample in samples {
                gpu_by_process
                    .entry(sample.process.clone())
                    .or_default()
                    .push(sample);
            }
        }
        let mut samples = Vec::new();
        for entry in snapshot.entries.values() {
            let Some(mut sample) = basic_sample(root, &entry.process, ticks, page_size) else {
                continue;
            };
            let prior = previous.get(&sample.process).copied();
            sample.metrics.insert(
                "cpu_cores".into(),
                rate(
                    Some(sample.cpu_seconds),
                    prior.map(|s| s.cpu_seconds),
                    elapsed,
                    "Logical CPU cores from process CPU-time delta / monotonic wall time",
                ),
            );
            for (name, value, old) in [
                (
                    "proc_storage_read_bytes_per_second",
                    sample.read_bytes,
                    prior.and_then(|s| s.read_bytes),
                ),
                (
                    "proc_storage_write_bytes_per_second",
                    sample.write_bytes,
                    prior.and_then(|s| s.write_bytes),
                ),
            ] {
                let mut value = rate(
                    value.map(|v| v as f64),
                    old.map(|v| v as f64),
                    elapsed,
                    "Linux /proc/PID/io storage-layer bytes; distinct from local VFS/NFS logical I/O; delayed writeback may differ",
                );
                if value["status"] == "ok" {
                    value["status"] = json!("partial");
                }
                sample.metrics.insert(name.into(), value);
            }
            let pss = if root == Path::new("/proc") {
                slow_cache.pss.get(&sample.process).copied()
            } else {
                Some((memory::pss_bytes(root, &sample.process), sampled_at))
            };
            let mut pss_metric = match pss {
                Some((value, at)) if sampled_at - at <= 60.0 => {
                    let mut result = metric(
                        json!(value),
                        if value.is_some() { "ok" } else { "unavailable" },
                        "Proportional resident memory from smaps_rollup; background refresh approximately every 30 seconds; never substitutes RSS",
                    );
                    result["sampled_at"] = json!(at);
                    result
                }
                Some(_) => metric(
                    Value::Null,
                    "unavailable",
                    "PSS observation is older than 60 seconds",
                ),
                None => metric(
                    Value::Null,
                    "warming_up",
                    "Waiting for first background PSS observation",
                ),
            };
            pss_metric["refresh_interval_seconds"] = json!(30);
            sample.metrics.insert("memory_pss_bytes".into(), pss_metric);
            if let Some(reason) = gpu_reason.as_ref() {
                for name in ["gpu_devices", "gpu_memory_bytes"] {
                    sample
                        .metrics
                        .insert(name.into(), metric(Value::Null, gpu_status, reason));
                }
            } else if !observation.processes.contains(&sample.process) {
                for name in ["gpu_devices", "gpu_memory_bytes"] {
                    sample.metrics.insert(
                        name.into(),
                        metric(
                            Value::Null,
                            "warming_up",
                            "Process has not yet been included in a GPU observation",
                        ),
                    );
                }
            } else {
                let gpu_samples = gpu_by_process.get(&sample.process);
                let devices: Vec<_> = gpu_samples
                    .into_iter()
                    .flatten()
                    .map(|s| &s.device_uuid)
                    .collect();
                let memory = gpu_samples
                    .into_iter()
                    .flatten()
                    .try_fold(0u64, |sum, s| sum.checked_add(s.memory_bytes?));
                sample.metrics.insert("gpu_devices".into(), metric(json!(devices), "partial", "NVIDIA compute-app residency by GPU UUID; excludes pure graphics; MPS may expose only its server; residency is not compute utilization"));
                sample.metrics.insert("gpu_memory_bytes".into(), metric(json!(memory), if memory.is_some() {"partial"} else {"unavailable"}, "NVIDIA compute-app framebuffer residency; unsupported/conflicting memory reports remain unknown"));
                // The sampler has no per-process utilization source; never use whole-card utilization.
                let _ = gpu_samples
                    .into_iter()
                    .flatten()
                    .any(|s| s.utilization_percent.is_some());
            }
            for name in ["gpu_devices", "gpu_memory_bytes"] {
                if observation.result.is_some() {
                    sample.metrics.get_mut(name).unwrap()["sampled_at"] =
                        json!(observation.sampled_at);
                }
                sample.metrics.get_mut(name).unwrap()["refresh_interval_seconds"] = json!(10);
            }
            for (name, kind, reason, operations) in IO_METRICS {
                sample.metrics.insert(
                    name.into(),
                    io.metric(
                        kind,
                        reason,
                        Some(&sample.process),
                        entry.started_at,
                        io_lost,
                        operations,
                    ),
                );
            }
            // Reject exit/reuse during CPU, memory and GPU reads as one incarnation.
            if memory::matches(root, &sample.process) {
                samples.push(sample);
            }
        }
        let mut metric_availability = BTreeMap::new();
        metric_availability.insert(
            "cpu_cores".into(),
            metric(
                Value::Null,
                if elapsed.is_some() {
                    "ok"
                } else {
                    "warming_up"
                },
                "Logical CPU cores from process CPU-time deltas",
            ),
        );
        let pss_ok = samples
            .iter()
            .any(|s| s.metrics["memory_pss_bytes"]["status"] == "ok");
        metric_availability.insert(
            "memory_pss_bytes".into(),
            metric(
                Value::Null,
                if pss_ok { "partial" } else { "unavailable" },
                "Per-process smaps_rollup access determines availability",
            ),
        );
        for name in ["gpu_devices", "gpu_memory_bytes"] {
            metric_availability.insert(name.into(), metric(Value::Null, gpu_status, gpu_reason.as_deref().unwrap_or("NVIDIA compute-app residency only; excludes graphics; utilization unavailable")));
        }
        for name in [
            "proc_storage_read_bytes_per_second",
            "proc_storage_write_bytes_per_second",
        ] {
            let readable = samples
                .iter()
                .any(|s| s.metrics[name]["status"] != "unavailable");
            metric_availability.insert(name.into(), metric(Value::Null,
                if !readable {"unavailable"} else if elapsed.is_none() {"warming_up"} else {"partial"},
                "Linux /proc/PID/io storage-layer counters; not VFS/NFS logical bytes; delayed writeback attribution may differ"));
        }
        for (name, kind, reason, operations) in IO_METRICS {
            metric_availability.insert(
                name.into(),
                io.metric(kind, reason, None, 0.0, io_lost, operations),
            );
        }
        let mut result = Resources {
            version: 1,
            node_id: node.into(),
            boot_id: snapshot.boot_id.clone(),
            sampled_at,
            availability: if snapshot.boot_id.is_empty() {
                "unavailable"
            } else {
                "observed"
            }
            .into(),
            method: "linux_proc_pss_nvidia_compute_apps_bpf_vfs_tcp".into(),
            samples,
            bindings: Vec::new(),
            metric_availability,
            sessions: Vec::new(),
            session_measurements: Vec::new(),
            unavailable: vec![
                "gpu_per_process_compute_utilization".into(),
                "nfs_wire_rpc_accounting".into(),
                "complete_exited_process_accounting".into(),
            ],
        };
        for (metric_name, legacy_name) in [
            ("gpu_devices", "gpu"),
            ("memory_pss_bytes", "pss"),
            ("network_receive_bytes_per_second", "network_per_session"),
            ("nfs_read_bytes_per_second", "nfs_per_session"),
        ] {
            if result.metric_availability[metric_name]["status"] == "unavailable" {
                result.unavailable.push(legacy_name.into());
            }
        }
        let bandwidth = &slow_cache.bandwidth;
        let fresh = bandwidth.sampled_at > 0.0 && sampled_at - bandwidth.sampled_at <= 15.0;
        result.metric_availability.insert(
            bandwidth::METRIC.into(),
            metric(
                Value::Null,
                if fresh
                    && bandwidth.rows.iter().any(|row| {
                        row.metrics
                            .get(bandwidth::METRIC)
                            .is_some_and(|m| !m["value"].is_null())
                    })
                {
                    "partial"
                } else {
                    "unavailable"
                },
                if fresh {
                    &bandwidth.reason
                } else {
                    "内存带宽尚未启用、硬件不支持或采样已过期"
                },
            ),
        );
        if fresh {
            result.session_measurements = bandwidth.rows.clone();
        }
        self.previous = Some((result.clone(), sampled));
        result
    }
}
fn basic_sample(root: &Path, process: &Process, ticks: f64, page_size: u64) -> Option<Sample> {
    let dir = root.join(process.pid.to_string());
    let raw = fs::read_to_string(dir.join("stat")).ok()?;
    let fields: Vec<_> = raw[raw.rfind(')')? + 1..].split_whitespace().collect();
    let value = |i: usize| fields.get(i)?.parse::<u64>().ok();
    if value(19)? != process.start {
        return None;
    }
    let io = fs::read_to_string(dir.join("io")).ok();
    let counter = |name: &str| {
        io.as_ref()?
            .lines()
            .find_map(|line| line.strip_prefix(name)?.trim().parse::<u64>().ok())
    };
    Some(Sample {
        process: process.clone(),
        cpu_seconds: (value(11)? as f64 + value(12)? as f64) / ticks,
        rss_bytes: value(21)?.saturating_mul(page_size),
        threads: value(17)?,
        read_bytes: counter("read_bytes:"),
        write_bytes: counter("write_bytes:"),
        metrics: BTreeMap::new(),
    })
}
fn rate(current: Option<f64>, previous: Option<f64>, elapsed: Option<f64>, reason: &str) -> Value {
    let Some(current) = current else {
        return metric(Value::Null, "unavailable", reason);
    };
    let (Some(previous), Some(elapsed)) = (previous, elapsed) else {
        return metric(Value::Null, "warming_up", reason);
    };
    if elapsed <= 0.0 || current < previous {
        return metric(
            Value::Null,
            "warming_up",
            "Counter reset or invalid sampling interval",
        );
    }
    metric(json!((current - previous) / elapsed), "ok", reason)
}

#[cfg(test)]
mod metric_tests {
    use super::*;

    #[test]
    fn rates_require_a_previous_incarnation_and_nonreset_counter() {
        assert_eq!(rate(Some(5.0), Some(1.0), Some(2.0), "test")["value"], 2.0);
        assert_eq!(
            rate(Some(5.0), None, Some(2.0), "test")["status"],
            "warming_up"
        );
        assert_eq!(
            rate(Some(1.0), Some(5.0), Some(2.0), "test")["value"],
            Value::Null
        );
        assert_eq!(
            rate(None, Some(1.0), Some(2.0), "test")["status"],
            "unavailable"
        );
    }

    #[test]
    fn io_failure_loss_and_pid_reuse_do_not_invent_measurements() {
        let mut window = IoWindow::new(true);
        let process = Process {
            pid: 12,
            start: 100,
        };
        let measure = |window: &IoWindow, process: &Process, lost| {
            window.metric(
                io_events::IoKind::NfsRead,
                "logical NFS",
                Some(process),
                0.0,
                lost,
                false,
            )
        };
        assert_eq!(measure(&window, &process, 0)["status"], "warming_up");
        window.update(io_events::Event::Batch {
            generation: 0,
            samples: vec![io_events::IoSample {
                process: process.clone(),
                device: 7,
                kind: io_events::IoKind::NfsRead,
                bytes: 4096,
                operations: 8,
                generation: 0,
            }],
        });
        assert_eq!(measure(&window, &process, 0)["value"], 2048.0);
        assert_eq!(
            window.metric(
                io_events::IoKind::NfsRead,
                "logical operations",
                Some(&process),
                0.0,
                0,
                true
            )["value"],
            4.0
        );
        assert_eq!(measure(&window, &process, 0)["status"], "partial");
        assert_eq!(
            measure(
                &window,
                &Process {
                    pid: 12,
                    start: 101
                },
                0
            )["value"],
            0.0
        );
        assert_eq!(
            measure(
                &window,
                &Process {
                    pid: 12,
                    start: 101
                },
                1
            )["value"],
            Value::Null
        );
        window.update(io_events::Event::Failed);
        assert_eq!(measure(&window, &process, 0)["status"], "unavailable");
        assert_eq!(measure(&window, &process, 0)["value"], Value::Null);
    }

    #[test]
    fn complete_io_window_recovers_after_historical_loss() {
        let mut window = IoWindow::new(true);
        window.observe(
            io_events::Event::Batch {
                generation: 0,
                samples: vec![],
            },
            1,
        );
        assert_eq!(
            window.metric(io_events::IoKind::TcpReceive, "tcp", None, 0.0, 1, false)["status"],
            "unavailable"
        );
        window.observe(
            io_events::Event::Batch {
                generation: 1,
                samples: vec![],
            },
            1,
        );
        let value = window.metric(io_events::IoKind::TcpReceive, "tcp", None, 0.0, 1, false);
        assert_eq!(value["status"], "partial");
        assert_eq!(value["io_lost_events_total"], 1);
    }
}

fn save(path: &Path, saved: &Saved) -> io::Result<()> {
    let bytes = serde_json::to_vec(saved)?;
    if fs::read(path).ok().as_ref() == Some(&bytes) {
        return Ok(());
    }
    let temp = path.with_extension("new");
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(temp, path)
}
fn serve(mut stream: UnixStream, state: Arc<Mutex<State>>, uid: u32) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut size,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if credentials.uid != uid && credentials.uid != 0 {
        return Err(io::Error::other("peer uid denied"));
    }
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let response = match serde_json::from_str::<Request>(&line) {
        Ok(request) => match state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .handle(request)
        {
            Ok(result) => json!({"ok":true,"result":result}),
            Err(error) => json!({"ok":false,"error":error}),
        },
        Err(_) => json!({"ok":false,"error":"invalid request"}),
    };
    let mut writer = BufWriter::new(&mut stream);
    serde_json::to_writer(&mut writer, &response)?;
    writer.write_all(b"\n")?;
    writer.flush()
}
pub fn run() -> io::Result<()> {
    let config = Arc::new(Config::load()?);
    fs::create_dir_all(
        config
            .state
            .parent()
            .ok_or_else(|| io::Error::other("state parent required"))?,
    )?;
    fs::create_dir_all(
        config
            .socket
            .parent()
            .ok_or_else(|| io::Error::other("socket parent required"))?,
    )?;
    // One writer per state directory. A second instance must never unlink a live socket.
    use std::os::fd::AsRawFd;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(config.state.with_extension("lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::other("resource-agent state is already in use"));
    }
    if UnixStream::connect(&config.socket).is_ok() {
        return Err(io::Error::other("socket is already in use"));
    }
    if let Err(e) = fs::remove_file(&config.socket)
        && e.kind() != io::ErrorKind::NotFound
    {
        return Err(e);
    }
    let listener = UnixListener::bind(&config.socket)?;
    fs::set_permissions(&config.socket, fs::Permissions::from_mode(0o600))?;
    if unsafe { libc::geteuid() } == 0 {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(config.socket.as_os_str().as_bytes())?;
        if unsafe { libc::chown(path.as_ptr(), config.uid, u32::MAX) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    listener.set_nonblocking(true)?;
    let snapshot = linux::collect_uid(&config.proc_root, config.uid);
    if snapshot.boot_id.is_empty() {
        return Err(io::Error::other("cannot read Linux boot identity"));
    }
    let saved = fs::read(&config.state)
        .ok()
        .and_then(|s| serde_json::from_slice::<Saved>(&s).ok());
    let mut engine = Engine::new(config.node_id.clone(), snapshot.boot_id.clone(), saved);
    let status = CollectorStatus {
        service: "resource-agent".into(),
        events: if config.events {
            "starting"
        } else {
            "disabled"
        }
        .into(),
        lost_events: 0,
    };
    let io_enabled = config.events && config.io_events && config.proc_root == Path::new("/proc");
    let diagnostic = Arc::new(Mutex::new(Diagnostic {
        supported: config.events && config.proc_root == Path::new("/proc"),
        deadline: io_enabled.then(|| Instant::now() + Duration::from_secs(60)),
        leases: io_enabled
            .then(|| (None, Instant::now() + Duration::from_secs(60)))
            .into_iter()
            .collect(),
        state: if io_enabled { "starting" } else { "off" },
        window: IoWindow::new(io_enabled),
        lost: Arc::new(AtomicU64::new(0)),
    }));
    let io_window = IoWindow::new(false);
    let mut sampler = ResourceSampler::default();
    let slow_cache = Arc::new(Mutex::new(SlowObservations::default()));
    let data = sampler.collect(
        &config.proc_root,
        &snapshot,
        &config.node_id,
        &io_window,
        0,
        &SlowObservations::default(),
    );
    let report = engine.update(&snapshot, data.sampled_at, status.clone());
    let state = Arc::new(Mutex::new(State {
        engine,
        snapshot,
        report,
        resources: data,
        status,
        forks: 0,
        execs: 0,
        exits: 0,
        diagnostic: diagnostic.clone(),
    }));
    let (sender, receiver) = mpsc::sync_channel(8192);
    let lost = Arc::new(AtomicU64::new(0));
    let mut tracer = if config.events {
        match events::start(config.uid, sender, lost.clone()) {
            Ok(child) => Some(child),
            Err(error) => {
                eprintln!("kernel events unavailable: {error}");
                state.lock().unwrap().status.events = "unavailable".into();
                None
            }
        }
    } else {
        None
    };
    let io_control = diagnostic.clone();
    let io_uid = config.uid;
    let io_worker = std::thread::spawn(move || diagnostic_worker(io_uid, io_control));
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as usize);
        libc::signal(libc::SIGINT, stop as *const () as usize);
    }
    // PSS walks page tables and is substantially more expensive than stat/io.
    // Spread a 30-second refresh over processes instead of blocking the fast sampler.
    let pss_worker = if config.proc_root == Path::new("/proc") {
        let pss_state = state.clone();
        let cache = slow_cache.clone();
        Some(std::thread::spawn(move || {
            while !STOP.load(Ordering::Relaxed) {
                let started = Instant::now();
                let processes: Vec<_> = pss_state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .snapshot
                    .entries
                    .values()
                    .map(|entry| entry.process.clone())
                    .collect();
                let current: BTreeSet<_> = processes.iter().cloned().collect();
                cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pss
                    .retain(|process, _| current.contains(process));
                // Budget ~15 seconds per scan for pacing, leaving time for other collectors.
                let spacing = Duration::from_secs_f64(15.0 / processes.len().max(1) as f64);
                for process in processes {
                    if STOP.load(Ordering::Relaxed) {
                        return;
                    }
                    let cpu_started = memory::thread_cpu_time();
                    let value = memory::pss_bytes(Path::new("/proc"), &process);
                    let pause_for = memory::sampling_pause(
                        memory::thread_cpu_time().saturating_sub(cpu_started),
                        spacing,
                    );
                    cache
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .pss
                        .insert(process, (value, now()));
                    let pause = Instant::now();
                    while pause.elapsed() < pause_for && !STOP.load(Ordering::Relaxed) {
                        std::thread::sleep(
                            Duration::from_millis(20)
                                .min(pause_for.saturating_sub(pause.elapsed())),
                        );
                    }
                }
                while started.elapsed() < Duration::from_secs(30) && !STOP.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }))
    } else {
        None
    };
    let bandwidth_worker = if config.memory_bandwidth && config.proc_root == Path::new("/proc") {
        let bandwidth_state = state.clone();
        let cache = slow_cache.clone();
        let node = config.node_id.clone();
        Some(std::thread::spawn(move || {
            let mut collector = None;
            while !STOP.load(Ordering::Relaxed) {
                let started = Instant::now();
                if collector.is_none() {
                    match bandwidth::Collector::open(
                        Path::new("/sys/fs/resctrl"),
                        &node,
                        Path::new("/run/resource-agent/mbm.lock"),
                    ) {
                        Ok(value) => collector = Some(value),
                        Err(error) => {
                            cache.lock().unwrap_or_else(|e| e.into_inner()).bandwidth =
                                bandwidth::Observation {
                                    rows: Vec::new(),
                                    sampled_at: now(),
                                    reason: format!("内存带宽不可用：{error}"),
                                };
                        }
                    }
                }
                if let Some(collector) = collector.as_mut() {
                    let bindings = bandwidth_state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .report
                        .bindings
                        .clone();
                    let observation = collector.collect(&bindings, Path::new("/proc"), now());
                    cache.lock().unwrap_or_else(|e| e.into_inner()).bandwidth = observation;
                }
                while started.elapsed() < Duration::from_secs(5) && !STOP.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            // Drop removes only this agent's monitor groups; processes keep running.
        }))
    } else {
        None
    };
    let gpu_worker = if config.proc_root == Path::new("/proc") {
        let gpu_state = state.clone();
        let cache = slow_cache.clone();
        Some(std::thread::spawn(move || {
            while !STOP.load(Ordering::Relaxed) {
                let started = Instant::now();
                let processes: Vec<_> = gpu_state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .snapshot
                    .entries
                    .values()
                    .map(|entry| entry.process.clone())
                    .collect();
                let result = gpu::collect(Path::new("/proc"), &processes, Duration::from_secs(3))
                    .map_err(|e| e.to_string());
                cache.lock().unwrap_or_else(|e| e.into_inner()).gpu = GpuObservation {
                    sampled_at: now(),
                    processes: processes.into_iter().collect(),
                    result: Some(result),
                };
                while started.elapsed() < Duration::from_secs(10) && !STOP.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }))
    } else {
        None
    };
    let worker_state = state.clone();
    let worker_config = config.clone();
    let worker = std::thread::spawn(move || {
        let mut sampled = Instant::now();
        let mut saved_at = Instant::now();
        while !STOP.load(Ordering::Relaxed) {
            let first = match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => Some(event),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(Duration::from_millis(100));
                    None
                }
            };
            for event in first.into_iter().chain(receiver.try_iter()) {
                let mut s = worker_state.lock().unwrap_or_else(|e| e.into_inner());
                match event {
                    Event::Ready => s.status.events = "bpf".into(),
                    Event::Failed => s.status.events = "unavailable".into(),
                    Event::Fork(parent, child) => {
                        s.engine.fork(&parent, child, now());
                        s.forks += 1;
                    }
                    Event::Exec(process) => {
                        let _ = process;
                        s.execs += 1;
                    }
                    Event::Exit(process) => {
                        s.engine.exit(&process);
                        s.exits += 1;
                    }
                }
            }
            if sampled.elapsed() >= Duration::from_secs(2) {
                let snapshot = linux::collect_uid(&worker_config.proc_root, worker_config.uid);
                let slow = slow_cache.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let (io_window, io_lost) = {
                    let diagnostic = diagnostic.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        diagnostic.window.clone(),
                        diagnostic.lost.load(Ordering::Relaxed),
                    )
                };
                let data = sampler.collect(
                    &worker_config.proc_root,
                    &snapshot,
                    &worker_config.node_id,
                    &io_window,
                    io_lost,
                    &slow,
                );
                let mut s = worker_state.lock().unwrap_or_else(|e| e.into_inner());
                s.status.lost_events = lost.load(Ordering::Relaxed);
                s.update(snapshot, data);
                sampled = Instant::now();
            }
            if saved_at.elapsed() >= Duration::from_secs(2) {
                let saved = worker_state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .engine
                    .saved();
                if let Err(error) = save(&worker_config.state, &saved) {
                    eprintln!("cannot save attribution: {error}");
                }
                saved_at = Instant::now();
            }
        }
    });
    while !STOP.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let state = state.clone();
                let uid = config.uid;
                std::thread::spawn(move || {
                    let _ = serve(stream, state, uid);
                });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(error) => {
                eprintln!("socket accept: {error}");
                STOP.store(true, Ordering::Relaxed);
            }
        }
    }
    for child in tracer.as_mut().into_iter() {
        unsafe {
            libc::kill(child.id() as i32, libc::SIGINT);
        }
        let until = Instant::now() + Duration::from_secs(2);
        while child.try_wait()?.is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        if child.try_wait()?.is_none() {
            child.kill()?;
        }
        child.wait()?;
    }
    let _ = io_worker.join();
    let _ = worker.join();
    if let Some(worker) = pss_worker {
        let _ = worker.join();
    }
    if let Some(worker) = bandwidth_worker {
        let _ = worker.join();
    }
    if let Some(worker) = gpu_worker {
        let _ = worker.join();
    }
    save(
        &config.state,
        &state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .engine
            .saved(),
    )?;
    fs::remove_file(&config.socket)?;
    Ok(())
}
