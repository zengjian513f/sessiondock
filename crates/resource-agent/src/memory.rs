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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expensive_pss_reads_pay_for_their_cpu_before_the_next_read() {
        assert_eq!(
            sampling_pause(Duration::from_millis(100), Duration::from_millis(20)),
            Duration::from_millis(400)
        );
        assert_eq!(
            sampling_pause(Duration::from_millis(1), Duration::from_millis(20)),
            Duration::from_millis(20)
        );
    }

    #[test]
    fn proportional_memory_is_not_rss_or_dirty_pss() {
        assert_eq!(
            parse_pss("Rss: 999 kB\nPss_Dirty: 2 kB\nPss: 47 kB\n"),
            Some(47 * 1024)
        );
        assert_eq!(parse_pss("Rss: 999 kB\nPss_Dirty: 2 kB\n"), None);
        assert_eq!(parse_pss("Pss: 0 kB"), Some(0));
        assert_eq!(parse_pss("Pss: 100 MB"), None);
        assert_eq!(parse_pss("Pss: 18446744073709551615 kB"), None);
    }

    #[test]
    fn reject_reused_process_and_missing_rollup() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "resource-agent-memory-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("123")).unwrap();
        let mut fields = vec!["0"; 22];
        fields[19] = "100";
        fs::write(
            root.join("123/stat"),
            format!("123 (name with ) space) {}", fields.join(" ")),
        )
        .unwrap();
        fs::write(root.join("123/smaps_rollup"), "Pss: 47 kB\n").unwrap();
        assert_eq!(
            pss_bytes(
                &root,
                &Process {
                    pid: 123,
                    start: 100
                }
            ),
            Some(47 * 1024)
        );
        assert_eq!(
            pss_bytes(
                &root,
                &Process {
                    pid: 123,
                    start: 101
                }
            ),
            None
        );
        fs::remove_file(root.join("123/smaps_rollup")).unwrap();
        assert_eq!(
            pss_bytes(
                &root,
                &Process {
                    pid: 123,
                    start: 100
                }
            ),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }
}
