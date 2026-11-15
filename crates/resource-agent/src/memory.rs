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
    fs::write(slow.join("cpu.max"), "20000 100000")?;
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
