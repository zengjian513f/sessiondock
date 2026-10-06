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
