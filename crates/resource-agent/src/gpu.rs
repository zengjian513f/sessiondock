//! NVIDIA compute-process residency, not whole-device utilization attribution.
use process_links::Process;
use std::{
    collections::BTreeMap,
    io::{self, Read},
    os::fd::AsRawFd,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct GpuSample {
    pub process: Process,
    /// Aggregate device counts as a set of (execution node, UUID).
    pub device_uuid: String,
    /// GPU context framebuffer residency. None means unsupported, not zero.
    pub memory_bytes: Option<u64>,
    /// This query cannot measure per-process compute utilization.
    pub utilization_percent: Option<f64>,
}

pub fn collect(
    root: &Path,
    processes: &[Process],
    timeout: Duration,
) -> io::Result<Vec<GpuSample>> {
    let current: BTreeMap<_, _> = processes
        .iter()
        .filter(|p| crate::memory::matches(root, p))
        .map(|p| (p.pid, p.clone()))
        .collect();
    let output = query(timeout)?;
    Ok(parse(&output)?
        .into_iter()
        .filter_map(|((pid, device_uuid), memory_bytes)| {
            let process = current.get(&pid)?;
            crate::memory::matches(root, process).then(|| GpuSample {
                process: process.clone(),
                device_uuid,
                memory_bytes,
                utilization_percent: None,
            })
        })
        .collect())
}

fn query(timeout: Duration) -> io::Result<String> {
    let mut command = Command::new("nvidia-smi");
    command.args([
        "--query-compute-apps=pid,gpu_uuid,used_gpu_memory",
        "--format=csv,noheader,nounits",
    ]);
    query_command(&mut command, timeout)
}

fn query_command(command: &mut Command, timeout: Duration) -> io::Result<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = (|| {
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing GPU query stdout"))?;
        let fd = stdout.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let started = Instant::now();
        let mut bytes = Vec::new();
        loop {
            match stdout.read_to_end(&mut bytes) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(io::Error::other(format!(
                        "NVIDIA compute query failed ({status})"
                    )));
                }
                stdout.read_to_end(&mut bytes)?;
                return String::from_utf8(bytes)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
            }
            if started.elapsed() >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "NVIDIA compute query timed out",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        // Only the collector's own query process is ever signalled.
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

type Rows = BTreeMap<(u32, String), Option<u64>>;

fn parse(output: &str) -> io::Result<Rows> {
    let mut rows = BTreeMap::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.split(',').map(str::trim).collect();
        if fields.len() != 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid GPU query row",
            ));
        }
        let pid = fields[0]
            .parse::<u32>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid GPU process id"))?;
        if fields[1].is_empty() || fields[1].contains("N/A") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "missing GPU UUID",
            ));
        }
        let memory = fields[2]
            .parse::<u64>()
            .ok()
            .and_then(|mb| mb.checked_mul(1024 * 1024));
        // Duplicate rows must not inflate card count or framebuffer usage. A
        // conflicting duplicate cannot safely be interpreted as a sum (MIG).
        rows.entry((pid, fields[1].to_owned()))
            .and_modify(|old| {
                if *old != memory {
                    *old = None;
                }
            })
            .or_insert(memory);
    }
    Ok(rows)
}
