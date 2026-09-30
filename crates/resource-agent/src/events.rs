use process_links::Process;
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
    Exit(Process),
    Failed,
}
pub fn start(uid: u32, sender: SyncSender<Event>, lost: Arc<AtomicU64>) -> std::io::Result<Child> {
    let script = include_str!("lifecycle.bt").replace("TARGET_UID", &uid.to_string());
    let mut child = Command::new("/usr/bin/bpftrace")
        .args(["-q", "-B", "line", "-e", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
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
                Some("X") => identity(1).map(Event::Exit),
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
