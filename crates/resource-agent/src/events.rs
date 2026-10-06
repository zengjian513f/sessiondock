use process_links::{Connection, Process};
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{SyncSender, TrySendError},
    },
};

pub enum Event {
    Ready,
    Fork(Process, Process),
    Exec(Process),
    Exit(Process, f64),
    Opened(Process, Connection, f64, bool),
    Closed(Process, Connection, f64),
    Received(Process, Connection, f64),
    Shared(Process),
    Failed,
}
pub fn start(uid: u32, sender: SyncSender<Event>, lost: Arc<AtomicU64>) -> std::io::Result<Child> {
    let script = format!(
        "{}\n{}",
        include_str!("lifecycle.bt"),
        include_str!("connections.bt")
    )
    .replace("TARGET_UID", &uid.to_string());
    let mut child = Command::new("/usr/bin/bpftrace")
        .args(["-q", "-B", "line", "-e", &script])
        .env("BPFTRACE_MAX_STRLEN", "96")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    let mut boot: libc::timespec = unsafe { std::mem::zeroed() };
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut boot) } != 0 {
        let _ = child.kill();
        let _ = child.wait();
        return Err(std::io::Error::last_os_error());
    }
    let boot_epoch = crate::server::now() - boot.tv_sec as f64 - boot.tv_nsec as f64 / 1e9;
    let errors = lost.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            // Expose loss/degradation without exporting commands or environment.
            if !line.trim().is_empty() {
                errors.fetch_add(1, Ordering::Relaxed);
                eprintln!("kernel collector: {line}");
            }
        }
    });
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.to_ascii_lowercase().contains("lost") {
                lost.fetch_add(1, Ordering::Relaxed);
            }
            let words: Vec<_> = line.split_whitespace().collect();
            let identity = |i: usize| -> Option<Process> {
                Some(Process {
                    pid: words.get(i)?.parse().ok()?,
                    start: (words.get(i + 1)?.parse::<u64>().ok()? as u128 * ticks as u128
                        / 1_000_000_000) as u64,
                })
            };
            let event = match words.first().copied() {
                Some("READY") => Some(Event::Ready),
                Some("F") => identity(1).zip(identity(3)).map(|(p, c)| Event::Fork(p, c)),
                Some("E") => identity(1).map(Event::Exec),
                Some("X") => identity(1)
                    .zip(words.get(3).and_then(|s| s.parse::<u64>().ok()))
                    .map(|(p, at)| Event::Exit(p, boot_epoch + at as f64 / 1e9)),
                Some("M") => identity(1).map(Event::Shared),
                Some(kind @ ("C" | "D" | "R")) => (|| {
                    let process = identity(1)?;
                    let at = boot_epoch + words.get(3)?.parse::<u64>().ok()? as f64 / 1e9;
                    let start = if kind == "C" { 5 } else { 4 };
                    let connection = Connection::parse(&words.get(start..)?.join(" "))?;
                    Some(match kind {
                        "C" => Event::Opened(process, connection, at, *words.get(4)? == "1"),
                        "D" => Event::Closed(process, connection, at),
                        _ => Event::Received(process, connection, at),
                    })
                })(),
                _ => None,
            };
            if let Some(event) = event {
                match sender.try_send(event) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        lost.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
        }
        let _ = sender.send(Event::Failed);
    });
    Ok(child)
}
