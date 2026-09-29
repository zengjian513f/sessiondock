//! Login-shell environment drift.
//!
//! The service is started through the operator's shell wrapper (for example
//! `with-zshrc`), so every CLI it launches inherits one prepared environment.
//! A later edit of the shell startup files only reaches new CLIs after the
//! service restarts. This module notices that: it captures what the wrapper
//! produces from a fixed minimal environment (`<command> /usr/bin/env -0`) at
//! startup, and again whenever a watched file changes or `RECHECK` has
//! passed, and reports the variable *names* whose values differ. Values never
//! leave the process. `POST /api/shell-env/restart` exits with
//! `RESTART_EXIT_CODE` after the normal graceful shutdown so the supervisor
//! (systemd `Restart=on-failure`) starts a fresh service; managed CLI hosts
//! outlive the service as on any restart.

use std::{
    collections::BTreeMap,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime},
};

use serde_json::{Value, json};

/// A full recapture at least this often, for files the watch list misses.
pub const RECHECK: Duration = Duration::from_secs(600);
/// One capture may take this long (a shell startup is ~1.5 s).
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(30);
/// The exit status that asks the supervisor for a restart.
pub const RESTART_EXIT_CODE: i32 = 75;
/// Set by the shell itself on every start; never a configuration change.
const VOLATILE: [&str; 4] = ["_", "SHLVL", "PWD", "OLDPWD"];

static RESTART: AtomicBool = AtomicBool::new(false);

/// Whether `/api/shell-env/restart` asked this process to exit for a restart.
pub fn restart_requested() -> bool {
    RESTART.load(Ordering::SeqCst)
}

pub fn request_restart() {
    RESTART.store(true, Ordering::SeqCst);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn stamp(path: &PathBuf) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

#[derive(Default)]
struct Inner {
    baseline: Option<BTreeMap<String, String>>,
    stamps: Vec<Option<Stamp>>,
    captured: Option<Instant>,
    checked_at: Option<String>,
    changed: Vec<String>,
    error: Option<String>,
}

pub struct ShellEnv {
    command: PathBuf,
    watch: Vec<PathBuf>,
    started_at: String,
    inner: tokio::sync::Mutex<Inner>,
}

fn now() -> String {
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64);
    chrono::DateTime::from_timestamp_millis(millis)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl ShellEnv {
    pub fn new(command: PathBuf, watch: Vec<PathBuf>) -> Self {
        Self {
            command,
            watch,
            started_at: now(),
            inner: tokio::sync::Mutex::new(Inner::default()),
        }
    }

    /// `<command> /usr/bin/env -0` from HOME/USER/LOGNAME/LANG and a system
    /// PATH only, so two captures differ only when the startup files do.
    fn capture(&self) -> Result<BTreeMap<String, String>, String> {
        let mut command = Command::new(&self.command);
        command
            .arg("/usr/bin/env")
            .arg("-0")
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for key in ["HOME", "USER", "LOGNAME", "LANG"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("无法运行 {}: {error}", self.command.display()))?;
        let mut stdout = child.stdout.take().expect("piped stdout");
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        });
        let deadline = Instant::now() + CAPTURE_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{} 超时", self.command.display()));
                }
            }
        };
        let bytes = reader.join().unwrap_or_default();
        if !status.success() {
            return Err(format!("{} 退出码 {status}", self.command.display()));
        }
        Ok(String::from_utf8_lossy(&bytes)
            .split('\0')
            .filter_map(|entry| entry.split_once('='))
            .filter(|(key, _)| !key.is_empty() && !VOLATILE.contains(key))
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect())
    }

    /// Recapture when a watched file changed, `RECHECK` passed, or no
    /// baseline exists yet; otherwise report the last result.
    pub async fn check(self: &std::sync::Arc<Self>) -> Value {
        let mut inner = self.inner.lock().await;
        let stamps: Vec<_> = self.watch.iter().map(stamp).collect();
        let due = inner.baseline.is_none()
            || stamps != inner.stamps
            || inner.captured.is_none_or(|at| at.elapsed() >= RECHECK);
        if due {
            let this = self.clone();
            let captured = tokio::task::spawn_blocking(move || this.capture())
                .await
                .unwrap_or_else(|_| Err("环境检查任务失败".into()));
            inner.stamps = stamps;
            inner.captured = Some(Instant::now());
            inner.checked_at = Some(now());
            match captured {
                Ok(current) => {
                    inner.error = None;
                    match &inner.baseline {
                        None => inner.baseline = Some(current),
                        Some(baseline) => inner.changed = changed_names(baseline, &current),
                    }
                }
                Err(error) => inner.error = Some(error),
            }
        }
        json!({
            "configured": true,
            "stale": !inner.changed.is_empty(),
            "changed": inner.changed,
            "started_at": self.started_at,
            "checked_at": inner.checked_at,
            "error": inner.error,
        })
    }
}

/// Names added, removed or given another value, sorted.
fn changed_names(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Vec<String> {
    let mut names: Vec<String> = before
        .iter()
        .filter(|(key, value)| after.get(*key) != Some(value))
        .map(|(key, _)| key.clone())
        .chain(after.keys().filter(|key| !before.contains_key(*key)).cloned())
        .collect();
    names.sort();
    names.dedup();
    names
}
