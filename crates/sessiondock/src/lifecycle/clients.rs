//! Installed agent CLI versions and manual updates for the machine settings.
//! Every configured CLI profile answers `--version`; an update runs the CLI's
//! own `update` subcommand in the background, outside every managed session,
//! so a slow download never holds a request or the lifecycle admission.
//! Claude, Codex, Grok and OpenCode all name it `update` and install without
//! asking; stdin is closed so a CLI that did ask would read end-of-file.
//! The newest published version comes from where each CLI's own updater looks
//! (Grok answers `update --check --json` itself), cached for a while.

use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::Read,
    path::PathBuf,
    process::{Command, ExitStatus, Stdio},
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::{
    launcher::{CliProfile, DENIED_ENV, current_executable},
    model::Source,
};

/// One `--version` answer (a wrapper may first load a shell rc).
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
/// One whole update: download and install.
const UPDATE_TIMEOUT: Duration = Duration::from_secs(600);
/// Tail of the update's output kept for the settings page.
const OUTPUT_LIMIT: usize = 4000;
/// One lookup of the newest published version.
const LATEST_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a looked-up newest version is reused; a failed lookup is retried sooner.
const LATEST_TTL: Duration = Duration::from_secs(600);
const LATEST_RETRY: Duration = Duration::from_secs(60);

#[derive(Serialize, Clone, Debug, Default)]
pub struct Update {
    pub running: bool,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    /// Exit status; absent while running and after a timeout or spawn failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    pub output: String,
}

#[derive(Serialize, Debug)]
pub struct Client {
    pub id: String,
    pub source: Source,
    /// The version number from `--version`, when one could be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// First line of the `--version` answer, as printed.
    pub detail: String,
    /// False when the command cannot be started or the shell reports it
    /// missing (126/127), the same rule as the new-session picker.
    pub installed: bool,
    /// The newest published version on the CLI's update channel, when the
    /// lookup answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<Update>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateError {
    /// No configured agent CLI profile has this ID.
    Unknown,
    /// This profile's update is still running.
    Running,
}

/// Latest update and looked-up newest version per profile ID, kept for the
/// life of the service.
#[derive(Default)]
pub struct Updates {
    updates: Mutex<BTreeMap<String, Update>>,
    latest: Mutex<BTreeMap<String, (Instant, Option<String>)>>,
}

impl Updates {
    fn get(&self, id: &str) -> Option<Update> {
        self.updates.lock().ok()?.get(id).cloned()
    }
    /// The profile's newest published version, looked up again once the
    /// cached answer is older than [`LATEST_TTL`] ([`LATEST_RETRY`] after a failure).
    fn latest(&self, profile: &CliProfile) -> Option<String> {
        if let Some((at, latest)) = self.latest.lock().ok()?.get(&profile.id).cloned() {
            let ttl = if latest.is_some() {
                LATEST_TTL
            } else {
                LATEST_RETRY
            };
            if at.elapsed() < ttl {
                return latest;
            }
        }
        let latest = latest(profile);
        if let Ok(mut cache) = self.latest.lock() {
            cache.insert(profile.id.clone(), (Instant::now(), latest.clone()));
        }
        latest
    }
    /// Claim the profile's update slot; refused while one is running.
    pub fn begin(&self, id: &str) -> Result<(), UpdateError> {
        let mut updates = self.updates.lock().map_err(|_| UpdateError::Running)?;
        if updates.get(id).is_some_and(|update| update.running) {
            return Err(UpdateError::Running);
        }
        updates.insert(
            id.to_owned(),
            Update {
                running: true,
                started_at: now(),
                ..Update::default()
            },
        );
        Ok(())
    }
    fn finish(&self, id: &str, update: Update) {
        if let Ok(mut updates) = self.updates.lock() {
            updates.insert(id.to_owned(), update);
        }
    }
}

/// Every agent CLI profile with its current version, newest published
/// version and latest update, probed in parallel. Blocking.
pub fn list<'a>(profiles: impl Iterator<Item = &'a CliProfile>, updates: &Updates) -> Vec<Client> {
    let profiles: Vec<&CliProfile> = profiles
        .filter(|profile| profile.source != Source::Shell)
        .collect();
    std::thread::scope(|scope| {
        let probes: Vec<_> = profiles
            .iter()
            .map(|profile| {
                let probe = scope.spawn(move || {
                    let (installed, detail) = version(profile);
                    let latest = installed.then(|| updates.latest(profile)).flatten();
                    (installed, detail, latest)
                });
                (profile, probe)
            })
            .collect();
        probes
            .into_iter()
            .map(|(profile, probe)| {
                let (installed, detail, latest) =
                    probe.join().unwrap_or((false, String::new(), None));
                Client {
                    id: profile.id.clone(),
                    source: profile.source,
                    version: installed.then(|| version_number(&detail)).flatten(),
                    detail,
                    installed,
                    latest,
                    update: updates.get(&profile.id),
                }
            })
            .collect()
    })
}

/// Run the profile's `update` and record the outcome under its ID. The slot
/// must already be claimed with [`Updates::begin`]. Blocking for minutes;
/// run it on its own thread.
pub fn update(profile: &CliProfile, updates: &Updates) {
    let started_at = updates
        .get(&profile.id)
        .map_or_else(now, |update| update.started_at);
    let before = version(profile).1;
    let outcome = run(profile, &["update"], UPDATE_TIMEOUT);
    let after = version(profile).1;
    let (ok, code, output) = match outcome {
        Ok(Outcome {
            status: Some(status),
            output,
        }) => (status.success(), status.code(), output),
        Ok(Outcome {
            status: None,
            output,
        }) => (
            false,
            None,
            format!(
                "{output}\n更新超过 {} 秒未结束，已终止。",
                UPDATE_TIMEOUT.as_secs()
            ),
        ),
        Err(error) => (false, None, format!("无法启动更新命令：{error}")),
    };
    updates.finish(
        &profile.id,
        Update {
            running: false,
            started_at,
            finished_at: Some(now()),
            ok: Some(ok),
            code,
            before: version_number(&before),
            after: version_number(&after),
            output: tail(output.trim()),
        },
    );
}

/// Whether the CLI is installed, and the first line of its `--version`.
fn version(profile: &CliProfile) -> (bool, String) {
    match run(profile, &["--version"], VERSION_TIMEOUT) {
        Ok(Outcome {
            status: Some(status),
            ..
        }) if matches!(status.code(), Some(126 | 127)) => (false, String::new()),
        Ok(Outcome { output, .. }) => (
            true,
            output
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or_default()
                .to_owned(),
        ),
        Err(_) => (false, String::new()),
    }
}

/// The first `1.2.3`-shaped word of a `--version` line: `2.1.285 (Claude
/// Code)`, `codex-cli 0.159.2`, `grok 1.0.34 (…) [stable]`, `opencode v2.0.18`.
pub fn version_number(line: &str) -> Option<String> {
    line.split_whitespace().find_map(|word| {
        let word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let word = word.strip_prefix(['v', 'V']).unwrap_or(word);
        (word.starts_with(|c: char| c.is_ascii_digit()) && word.contains('.'))
            .then(|| word.to_owned())
    })
}

/// The newest version on the channel each CLI's own updater follows:
/// Claude's npm dist-tag named by `autoUpdatesChannel` (default `latest`),
/// Codex's npm `latest`, OpenCode's own release API, Grok's `update --check`.
fn latest(profile: &CliProfile) -> Option<String> {
    match profile.source {
        Source::Claude => {
            let settings = super::models::cli_home(profile, "CLAUDE_CONFIG_DIR", ".claude")
                .join("settings.json");
            let channel = std::fs::read(settings)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value.get("autoUpdatesChannel")?.as_str().map(str::to_owned))
                .filter(|channel| channel.chars().all(|c| c.is_ascii_alphanumeric()))
                .unwrap_or_else(|| "latest".to_owned());
            let tags = fetch(
                profile,
                "https://registry.npmjs.org/-/package/@anthropic-ai/claude-code/dist-tags",
            )?;
            text(&tags, &channel)
        }
        Source::Codex => text(
            &fetch(profile, "https://registry.npmjs.org/@openai/codex/latest")?,
            "version",
        ),
        Source::Opencode => text(
            &fetch(profile, "https://opencode.ai/update/api/latest/cli/npm")?,
            "version",
        ),
        Source::Grok => {
            let outcome = run(profile, &["update", "--check", "--json"], LATEST_TIMEOUT).ok()?;
            let answer = outcome
                .output
                .lines()
                .find(|line| line.trim_start().starts_with('{'))?;
            text(&serde_json::from_str(answer).ok()?, "latestVersion")
        }
        Source::Shell => None,
    }
}

fn text(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().and_then(version_number)
}

/// A JSON document fetched with `curl` in the profile's environment, so the
/// CLI's own proxy settings apply.
fn fetch(profile: &CliProfile, url: &str) -> Option<serde_json::Value> {
    let mut command = Command::new("curl");
    command.args(["-fsSL", "--max-time", "12", url]);
    let outcome = bounded(command, profile, LATEST_TIMEOUT).ok()?;
    if !outcome.status?.success() {
        return None;
    }
    serde_json::from_str(outcome.output.trim()).ok()
}

struct Outcome {
    /// `None` when the deadline passed and the command was killed.
    status: Option<ExitStatus>,
    /// Standard output followed by standard error, terminal escapes removed.
    output: String,
}

/// The profile's executable with its fixed arguments plus `arguments`.
fn run(profile: &CliProfile, arguments: &[&str], timeout: Duration) -> std::io::Result<Outcome> {
    let executable = current_executable(&profile.executable).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "configured executable is unusable",
        )
    })?;
    let mut command = Command::new(executable);
    command.args(&profile.args).args(arguments);
    bounded(command, profile, timeout)
}

/// `command` in the profile's environment and home, killed with its whole
/// process group at the deadline (an installer runs `curl | sh` below the CLI).
fn bounded(
    mut command: Command,
    profile: &CliProfile,
    timeout: Duration,
) -> std::io::Result<Outcome> {
    for name in &profile.env_remove {
        command.env_remove(name);
    }
    command.envs(&profile.env);
    for name in DENIED_ENV {
        command.env_remove(name);
    }
    if let Some(home) = profile
        .env
        .get("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_dir())
    {
        command.current_dir(home);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn()?;
    let stdout = Collector::start(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let stderr = Collector::start(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    // A process the CLI left running (a restarted background service) may
    // keep the pipes open: take what arrived instead of waiting for it.
    let drained = Instant::now() + PIPE_GRACE;
    let mut output = String::from_utf8_lossy(&stdout.finish(drained)).into_owned();
    let errors = String::from_utf8_lossy(&stderr.finish(drained)).into_owned();
    if !errors.trim().is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&errors);
    }
    Ok(Outcome {
        status,
        output: plain(&output),
    })
}

/// How long the output pipes may stay open after the command exits.
const PIPE_GRACE: Duration = Duration::from_secs(5);

/// A pipe read on its own thread into a shared buffer, so the bytes read so
/// far are available even when the pipe never closes.
struct Collector {
    bytes: std::sync::Arc<Mutex<Vec<u8>>>,
    done: std::sync::mpsc::Receiver<()>,
}

impl Collector {
    fn start(stream: Option<Box<dyn Read + Send>>) -> Self {
        let bytes = std::sync::Arc::new(Mutex::new(Vec::new()));
        let (sender, done) = std::sync::mpsc::channel();
        let buffer = bytes.clone();
        std::thread::spawn(move || {
            if let Some(mut stream) = stream {
                let mut chunk = [0u8; 8192];
                while let Ok(read) = stream.read(&mut chunk) {
                    if read == 0 {
                        break;
                    }
                    if let Ok(mut buffer) = buffer.lock() {
                        buffer.extend_from_slice(&chunk[..read]);
                    }
                }
            }
            let _ = sender.send(());
        });
        Self { bytes, done }
    }
    fn finish(self, deadline: Instant) -> Vec<u8> {
        let _ = self
            .done
            .recv_timeout(deadline.saturating_duration_since(Instant::now()));
        self.bytes
            .lock()
            .map(|bytes| bytes.clone())
            .unwrap_or_default()
    }
}

/// Terminal colour/cursor sequences and carriage-return progress removed.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                } else {
                    chars.next();
                }
            }
            '\r' => {
                // `\r\n` is a line end; a bare `\r` redraws the line.
                if chars.peek() != Some(&'\n') {
                    let start = out.rfind('\n').map_or(0, |i| i + 1);
                    out.truncate(start);
                }
            }
            c if c.is_control() && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

fn tail(text: &str) -> String {
    if text.len() <= OUTPUT_LIMIT {
        return text.to_owned();
    }
    let mut start = text.len() - OUTPUT_LIMIT;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_numbers_of_each_cli() {
        assert_eq!(
            version_number("2.1.285 (Claude Code)").as_deref(),
            Some("2.1.285")
        );
        assert_eq!(
            version_number("codex-cli 0.159.2").as_deref(),
            Some("0.159.2")
        );
        assert_eq!(
            version_number("grok 1.0.34 (3736acbc8658) [stable]").as_deref(),
            Some("1.0.34")
        );
        assert_eq!(
            version_number("opencode v2.0.18").as_deref(),
            Some("2.0.18")
        );
        assert_eq!(version_number("no version here"), None);
    }

    #[test]
    fn plain_drops_escapes_and_redrawn_progress() {
        assert_eq!(
            plain("\u{1b}[32m==>\u{1b}[0m ok\r\n10%\r50%\r100%\ndone"),
            "==> ok\n100%\ndone"
        );
    }
}
