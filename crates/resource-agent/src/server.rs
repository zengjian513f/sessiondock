use crate::events::{self, Event};
use process_links::{
    Report,
    agent::{CollectorStatus, Request, Resources, Sample},
    engine::{Engine, Saved},
    linux::{self, Snapshot},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, BufRead, BufReader, Write},
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
}
impl Config {
    fn load() -> io::Result<Self> {
        let mut args = std::env::args().skip(1);
        let mut options = BTreeMap::new();
        while let Some(key) = args.next() {
            if key == "--help" {
                println!(
                    "resource-agent --node-id-file PATH --uid UID [--socket PATH] [--state PATH] [--proc-root PATH] [--events auto|off]\nLocal JSON-line API: health, report, resources, catalog, publish. Observes only; never signals or moves workloads."
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
        Ok(Self {
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
struct State {
    engine: Engine,
    snapshot: Snapshot,
    report: Report,
    resources: Resources,
    status: CollectorStatus,
    forks: u64,
    execs: u64,
    exits: u64,
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
            Request::Report => Ok(json!(self.report)),
            Request::Resources => Ok(json!(self.resources)),
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
fn resources(root: &Path, snapshot: &Snapshot, node: &str) -> Resources {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as u64;
    let samples = snapshot
        .entries
        .values()
        .filter_map(|entry| {
            let dir = root.join(entry.process.pid.to_string());
            let raw = fs::read_to_string(dir.join("stat")).ok()?;
            let fields: Vec<_> = raw[raw.rfind(')')? + 1..].split_whitespace().collect();
            let value = |i: usize| fields.get(i)?.parse::<u64>().ok();
            if value(19)? != entry.process.start {
                return None;
            }
            let cpu_seconds = (value(11)? as f64 + value(12)? as f64) / ticks;
            let io = fs::read_to_string(dir.join("io")).ok();
            let counter = |name: &str| {
                io.as_ref()?
                    .lines()
                    .find_map(|line| line.strip_prefix(name)?.trim().parse::<u64>().ok())
            };
            Some(Sample {
                process: entry.process.clone(),
                cpu_seconds,
                rss_bytes: value(21)?.saturating_mul(page_size),
                threads: value(17)?,
                read_bytes: counter("read_bytes:"),
                write_bytes: counter("write_bytes:"),
            })
        })
        .collect();
    Resources {
        version: 1,
        node_id: node.into(),
        boot_id: snapshot.boot_id.clone(),
        sampled_at: now(),
        availability: if snapshot.boot_id.is_empty() {
            "unavailable"
        } else {
            "observed"
        }
        .into(),
        method: "linux_proc_cumulative_cpu_rss_io".into(),
        samples,
        unavailable: [
            "gpu",
            "network_per_session",
            "nfs_per_session",
            "pss",
            "complete_exited_process_accounting",
        ]
        .iter()
        .map(|s| (*s).into())
        .collect(),
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
    serde_json::to_writer(&mut stream, &response)?;
    stream.write_all(b"\n")
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
    let data = resources(&config.proc_root, &snapshot, &config.node_id);
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
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as usize);
        libc::signal(libc::SIGINT, stop as *const () as usize);
    }
    let worker_state = state.clone();
    let worker_config = config.clone();
    let worker = std::thread::spawn(move || {
        let mut sampled = Instant::now();
        let mut saved_at = Instant::now();
        while !STOP.load(Ordering::Relaxed) {
            if let Ok(event) = receiver.recv_timeout(Duration::from_millis(100)) {
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
            } else {
                std::thread::sleep(Duration::from_millis(100));
            }
            if sampled.elapsed() >= Duration::from_secs(2) {
                let snapshot = linux::collect_uid(&worker_config.proc_root, worker_config.uid);
                let data = resources(&worker_config.proc_root, &snapshot, &worker_config.node_id);
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
    if let Some(child) = tracer.as_mut() {
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
    let _ = worker.join();
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
