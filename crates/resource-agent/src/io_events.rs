//! Bounded, read-only application I/O accounting. Values are deltas per completed
//! two-second generation, never physical disk/NFS wire counters. Short-lived
//! processes remain in the completed map even after exit. Kernel/function/BTF
//! incompatibility fails this collector without affecting the workload.
use process_links::Process;
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Write},
    os::fd::AsRawFd,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{SyncSender, TrySendError},
    },
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IoKind {
    TcpSend,
    TcpReceive,
    LocalRead,
    LocalWrite,
    NfsRead,
    NfsWrite,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IoSample {
    pub process: Process,
    /// Linux superblock dev_t (major = device >> 20, minor = device & 0xfffff).
    /// Zero for TCP. NFS logical I/O is keyed by its filesystem, not an endpoint.
    pub device: u64,
    pub kind: IoKind,
    pub bytes: u64,
    pub operations: u64,
    pub generation: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Ready,
    /// Completed aggregation interval, including an empty interval. generation
    /// starts at zero; missed generations mean a gap, not zero activity.
    Batch {
        generation: u64,
        samples: Vec<IoSample>,
    },
    Failed,
}

#[path = "io_bpf.rs"]
mod bpf;

#[derive(Serialize, Deserialize)]
struct Message {
    event: Event,
    lost: u64,
}

/// Each demand creates one isolated helper. Its alarm starts before libbpf
/// loading, independently of the API/sampler. Killing/reaping this child closes
/// every unpinned BPF FD. No compiler or bpftrace is invoked at runtime.
pub fn start(uid: u32, sender: SyncSender<Event>, lost: Arc<AtomicU64>) -> std::io::Result<Child> {
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--io-probe-helper", &uid.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let input = child.stdin.as_ref().unwrap().as_raw_fd();
    unsafe {
        libc::fcntl(
            input,
            libc::F_SETFL,
            libc::fcntl(input, libc::F_GETFL) | libc::O_NONBLOCK,
        );
    }
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let errors = lost.clone();
    std::thread::spawn(move || {
        let mut logged = false;
        for line in BufReader::new(stderr).lines() {
            if !matches!(line, Ok(ref line) if line.trim().is_empty()) {
                // libbpf diagnostics imply incomplete coverage; don't relay raw
                // kernel payloads or unbounded repeated diagnostics.
                errors.fetch_add(1, Ordering::Relaxed);
                if !logged {
                    if let Ok(ref line) = line {
                        eprintln!("I/O helper: {}", line.chars().take(400).collect::<String>());
                    }
                    logged = true;
                }
            }
        }
    });
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let message = line
                .ok()
                .and_then(|line| serde_json::from_str::<Message>(&line).ok());
            let Some(message) = message else {
                lost.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            lost.fetch_add(message.lost, Ordering::Relaxed);
            match sender.try_send(message.event) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    lost.fetch_add(1, Ordering::Relaxed);
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
        let _ = sender.try_send(Event::Failed);
    });
    Ok(child)
}

pub fn renew(child: &mut Child, remaining: std::time::Duration) -> std::io::Result<()> {
    let deadline =
        bpf::monotonic_ns().saturating_add(remaining.as_nanos().min(60_000_000_000) as u64);
    writeln!(
        child
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::other("helper input closed"))?,
        "{deadline}"
    )
}

pub fn helper_main() -> std::io::Result<()> {
    // exec child owns these FDs. SIGALRM's default fatal disposition gives a
    // kernel-enforced watchdog even if stdout blocks or verifier loading stalls.
    let parent = unsafe { libc::getppid() };
    unsafe {
        libc::signal(libc::SIGALRM, libc::SIG_DFL);
        libc::alarm(60);
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if parent <= 1 || libc::getppid() != parent {
            return Err(std::io::Error::other("I/O helper parent exited"));
        }
    }
    let launched = bpf::monotonic_ns();
    let deadline = launched + 60_000_000_000;
    let uid = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| std::io::Error::other("missing I/O helper UID"))?;
    let mut probe = bpf::Probe::open(uid, deadline)?;
    let (sender, renewals) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(std::io::stdin()).lines() {
            let Ok(line) = line else {
                break;
            };
            if let Ok(deadline) = line.parse::<u64>() {
                if sender.send(deadline).is_err() {
                    break;
                }
            }
        }
    });
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let mut emit = |event, lost| -> std::io::Result<()> {
        serde_json::to_writer(&mut output, &Message { event, lost })?;
        output.write_all(b"\n")?;
        output.flush()
    };
    emit(Event::Ready, 0)?;
    let mut previous_losses = 0;
    let mut generation = 0;
    loop {
        // A small grace period allows calls completing just before the boundary
        // to finish their bounded kernel-map update. Late generations are loss.
        let boundary = probe.start_ns + (generation + 1) * 2_000_000_000 + 100_000_000;
        let now = bpf::monotonic_ns();
        let wait_until = boundary.min(probe.stop_ns);
        match renewals.recv_timeout(std::time::Duration::from_nanos(
            wait_until.saturating_sub(now),
        )) {
            Ok(deadline) => {
                probe.renew(deadline)?;
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = bpf::monotonic_ns();
        if now >= probe.stop_ns {
            break;
        }
        let (samples, mut loss) = probe.drain(generation)?;
        let kernel_losses = probe.losses()?;
        loss += kernel_losses.saturating_sub(previous_losses);
        previous_losses = kernel_losses;
        if now > boundary + 1_000_000_000 {
            loss += 1;
        }
        emit(
            Event::Batch {
                generation,
                samples,
            },
            loss,
        )?;
        generation += 1;
    }
    // Drop unloads links before the helper exits. The final partial interval is
    // never mislabeled as a completed two-second interval.
    drop(probe);
    Ok(())
}
