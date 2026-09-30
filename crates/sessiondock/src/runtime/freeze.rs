//! Linux diagnostic process-tree pause. The independent PTY host stays live.
use super::ProcessIdentity;
use std::io;

pub fn frozen(identity: ProcessIdentity) -> bool {
    stamp(identity).is_ok_and(|state| matches!(state, 'T' | 't'))
}

fn stamp(identity: ProcessIdentity) -> io::Result<char> {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", identity.pid))?;
    let current = ProcessIdentity::parse_stat(identity.pid, &stat)
        .map_err(|_| io::Error::other("invalid process identity"))?;
    identity
        .verify(&current)
        .map_err(|_| io::Error::other("process replaced"))?;
    Ok(stat[stat.rfind(')').unwrap() + 1..]
        .trim_start()
        .chars()
        .next()
        .unwrap())
}

#[cfg(target_os = "linux")]
fn signal(identity: ProcessIdentity, value: i32) -> io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // Open first, then verify: pidfd_send_signal cannot target a recycled PID.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    stamp(identity)?;
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            value,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn children(parent: ProcessIdentity) -> io::Result<Vec<ProcessIdentity>> {
    stamp(parent)?;
    let mut result = Vec::new();
    for entry in std::fs::read_dir(format!("/proc/{}/task", parent.pid))? {
        let entry = entry?;
        let text = match std::fs::read_to_string(entry.path().join("children")) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for pid in text
            .split_whitespace()
            .filter_map(|pid| pid.parse::<u32>().ok())
        {
            let path = format!("/proc/{pid}/stat");
            if let Ok(stat) = std::fs::read_to_string(path) {
                let identity = ProcessIdentity::parse_stat(pid, &stat)
                    .map_err(|_| io::Error::other("invalid child identity"))?;
                // A child can exit/reparent between reading children and stat.
                let ppid = stat[stat.rfind(')').unwrap() + 1..]
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse::<u32>().ok());
                if ppid == Some(parent.pid) && !result.contains(&identity) {
                    result.push(identity);
                }
            }
        }
    }
    Ok(result)
}

/// Freeze parents before enumerating children so they cannot spawn new work.
/// Resume children before their parents. Roll back newly stopped processes if
/// any step fails; pre-existing stops are preserved.
pub async fn set(root: ProcessIdentity, pause: bool) -> io::Result<usize> {
    tokio::task::spawn_blocking(move || {
        static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _gate = GATE.lock().unwrap_or_else(|error| error.into_inner());
        set_tree(root, pause)
    })
    .await
    .map_err(io::Error::other)?
}

#[cfg(not(target_os = "linux"))]
fn set_tree(_root: ProcessIdentity, _pause: bool) -> io::Result<usize> {
    Err(io::Error::other("process freeze requires Linux"))
}

#[cfg(target_os = "linux")]
fn set_tree(root: ProcessIdentity, pause: bool) -> io::Result<usize> {
    if stamp(root)? == 'Z' {
        return Err(io::Error::other("CLI has exited"));
    }
    let mut tree = vec![root];
    let mut stopped = Vec::new();
    let outcome = (|| {
        let mut index = 0;
        while index < tree.len() {
            let identity = tree[index];
            if pause {
                let state = stamp(identity)?;
                if !matches!(state, 'T' | 't' | 'Z') {
                    signal(identity, libc::SIGSTOP)?;
                    stopped.push(identity);
                }
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while !matches!(stamp(identity)?, 'T' | 't' | 'Z') {
                    if std::time::Instant::now() >= deadline {
                        return Err(io::Error::other("process has not stopped"));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            for child in children(identity)? {
                if !tree.contains(&child) {
                    tree.push(child);
                }
            }
            index += 1;
        }
        if !pause {
            for identity in tree.iter().rev() {
                if matches!(stamp(*identity)?, 'T' | 't') {
                    signal(*identity, libc::SIGCONT)?;
                }
            }
        }
        Ok(tree.len())
    })();
    if outcome.is_err() && pause {
        for identity in stopped.iter().rev() {
            let _ = signal(*identity, libc::SIGCONT);
        }
    }
    outcome
}
