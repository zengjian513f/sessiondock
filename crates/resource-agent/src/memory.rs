//! Proportional resident memory; unavailable rollups never fall back to RSS.
use process_links::Process;
use std::{fs, path::Path, time::Duration};

/// CPU time, rather than wall time, includes expensive kernel page-table walks.
pub fn thread_cpu_time() -> Duration {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } == 0 {
        Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
    } else {
        Duration::ZERO
    }
}

/// At most one fifth of a core averaged over a read plus its following pause.
pub fn sampling_pause(cpu_used: Duration, spacing: Duration) -> Duration {
    spacing.max(cpu_used.saturating_mul(4))
}

pub(crate) fn matches(root: &Path, process: &Process) -> bool {
    fs::read_to_string(root.join(process.pid.to_string()).join("stat"))
        .ok()
        .and_then(|raw| {
            raw.get(raw.rfind(')')? + 1..)?
                .split_whitespace()
                .nth(19)?
                .parse::<u64>()
                .ok()
        })
        == Some(process.start)
}

pub fn pss_bytes(root: &Path, process: &Process) -> Option<u64> {
    if !matches(root, process) {
        return None;
    }
    let raw = fs::read_to_string(root.join(process.pid.to_string()).join("smaps_rollup")).ok()?;
    let pss = parse_pss(&raw)?;
    matches(root, process).then_some(pss)
}

fn parse_pss(raw: &str) -> Option<u64> {
    let mut fields = raw
        .lines()
        .find_map(|line| line.strip_prefix("Pss:"))?
        .split_whitespace();
    let kb = fields.next()?.parse::<u64>().ok()?;
    (fields.next()? == "kB").then_some(())?;
    kb.checked_mul(1024)
}

/// The service explicitly delegates only its own cgroup subtree. Enforce the
/// slow collectors' budget during kernel reads, rather than only afterwards.
pub fn slow_cpu_budget() -> std::io::Result<Option<std::path::PathBuf>> {
    if std::env::var_os("RESOURCE_AGENT_SLOW_CPU_BUDGET").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return Ok(None);
    }
    let membership = fs::read_to_string("/proc/self/cgroup")?;
    let relative = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::/"))
        .filter(|path| !path.is_empty())
        .ok_or_else(|| std::io::Error::other("no private unified cgroup"))?;
    let root = Path::new("/sys/fs/cgroup").join(relative);
    let slow = root.join("slow-collectors");
    fs::create_dir_all(&slow)?;
    fs::write(slow.join("cgroup.type"), "threaded")?;
    fs::write(root.join("cgroup.subtree_control"), "+cpu")?;
    fs::write(slow.join("cpu.max"), "10000 100000")?;
    let sampler = root.join("fast-sampler");
    fs::create_dir_all(&sampler)?;
    fs::write(sampler.join("cgroup.type"), "threaded")?;
    fs::write(sampler.join("cpu.max"), "40000 100000")?;
    Ok(Some(slow))
}

pub fn enter_cpu_budget(path: Option<&Path>) -> bool {
    let Some(path) = path else { return false };
    let tid = unsafe { libc::syscall(libc::SYS_gettid) };
    if let Err(error) = fs::write(path.join("cgroup.threads"), tid.to_string()) {
        crate::runtime_log::emit(
            "warn",
            "resource.slow_cpu_budget_failed",
            serde_json::json!({"error":error.to_string()}),
        );
        return false;
    }
    true
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Observation {
    pub value: Option<u64>,
    pub sampled_at: f64,
}

pub fn observation(root: &Path, process: &Process) -> Observation {
    use std::io::{Read, Write};
    let started = crate::server::now();
    if let Some(socket) = std::env::var_os("RESOURCE_AGENT_PSS_SOCKET") {
        let read = || -> std::io::Result<Observation> {
            let mut stream = std::os::unix::net::UnixStream::connect(socket)?;
            stream.set_read_timeout(Some(Duration::from_millis(200)))?;
            stream.set_write_timeout(Some(Duration::from_secs(2)))?;
            serde_json::to_writer(&mut stream, process)?;
            stream.write_all(b"\n")?;
            let until = std::time::Instant::now() + Duration::from_secs(90);
            let mut line = Vec::new();
            let mut buffer = [0u8; 1024];
            loop {
                if crate::server::STOP.load(std::sync::atomic::Ordering::Relaxed)
                    || std::time::Instant::now() >= until
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "PSS read interrupted or expired",
                    ));
                }
                match stream.read(&mut buffer) {
                    Ok(0) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "PSS worker disconnected",
                        ));
                    }
                    Ok(n) => {
                        line.extend_from_slice(&buffer[..n]);
                        if line.ends_with(b"\n") {
                            break;
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(error) => return Err(error),
                }
            }
            serde_json::from_slice(&line).map_err(std::io::Error::other)
        };
        return read().unwrap_or(Observation {
            value: None,
            sampled_at: started,
        });
    }
    Observation {
        value: pss_bytes(root, process),
        sampled_at: started,
    }
}

/// Runs in its own service CPU domain. A large uninterruptible kernel page-table
/// walk must not charge the request/sampling service's quota.
pub fn worker_main() -> std::io::Result<()> {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::fs::{MetadataExt, PermissionsExt},
        os::unix::net::UnixListener,
        path::PathBuf,
    };
    let mut options = std::collections::BTreeMap::new();
    let mut args = std::env::args().skip(2);
    while let Some(key) = args.next() {
        if !["--uid", "--socket", "--proc-root"].contains(&key.as_str()) {
            return Err(std::io::Error::other("unknown PSS worker option"));
        }
        options.insert(
            key,
            args.next()
                .ok_or_else(|| std::io::Error::other("missing PSS worker value"))?,
        );
    }
    let uid: u32 = options
        .get("--uid")
        .ok_or_else(|| std::io::Error::other("--uid required"))?
        .parse()
        .map_err(std::io::Error::other)?;
    let socket = PathBuf::from(
        options
            .get("--socket")
            .ok_or_else(|| std::io::Error::other("--socket required"))?,
    );
    let root = PathBuf::from(
        options
            .get("--proc-root")
            .map(String::as_str)
            .unwrap_or("/proc"),
    );
    if socket.exists() {
        fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    for stream in listener.incoming() {
        let mut stream = stream?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut line = String::new();
        if BufReader::new(&stream).read_line(&mut line).is_err() {
            continue;
        }
        let Ok(process) = serde_json::from_str::<Process>(&line) else {
            continue;
        };
        let sampled_at = crate::server::now();
        let value = fs::metadata(root.join(process.pid.to_string()))
            .ok()
            .filter(|meta| meta.uid() == uid)
            .and_then(|_| pss_bytes(&root, &process));
        let _ = serde_json::to_writer(&mut stream, &Observation { value, sampled_at });
        let _ = stream.write_all(b"\n");
    }
    Ok(())
}
