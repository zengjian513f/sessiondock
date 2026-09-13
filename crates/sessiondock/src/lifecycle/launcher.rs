//! Explicit adapter allowlist and one-authority process spawn. No discovery,
//! native identity binding, readiness claim, automatic retry or Drop termination.

use cap_fs_ext::{DirExt, FollowSymlinks, MetadataExt, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, Metadata, OpenOptions},
};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Child,
    sync::Arc,
};

use super::{
    model::{self, Launch, LaunchSpec, Record, Source, State},
    store::StartAuthority,
};
use std::process::{Command, Stdio};

pub const MAX_CONFIG_BYTES: usize = 64 * 1024;
const MAX_ENTRIES: usize = 16;
const MAX_ARGS: usize = 64;
const MAX_ARG_BYTES: usize = 4096;
const MAX_ENV: usize = 64;
const MAX_ENV_VALUE_BYTES: usize = 8192;
const MAX_ENV_REMOVE: usize = 16;
pub const MAX_COMPLETIONS: usize = 50;
pub const DEFAULT_COMPLETIONS: usize = 24;
const MAX_SCANNED_ENTRIES: usize = 4096;

/// Fixed argv placeholders. They must be a whole argument; the server never
/// splices a SID into a longer string or a shell command line.
pub const SESSION_ID_PLACEHOLDER: &str = "{session_id}";
pub const SID_PLACEHOLDER: &str = "{sid}";
/// Session-identity variables a CLI must never inherit from the Web service.
/// The launcher already clears its environment and the imported host strips
/// these again; configuration cannot add them back. The spawner clues
/// (`CODEX_THREAD_ID`, `CODEX_SESSION_ID`, `CLAUDE_PID`, Python
/// `SPAWN_ENV_KEYS`) are refused too: a web-created session must not be
/// recorded as the child of whatever session started this service.
pub const DENIED_ENV: [&str; 8] = [
    "CLAUDE_CODE_SESSION_ID",
    "CODEX_COMPANION_SESSION_ID",
    "GROK_SESSION_ID",
    "CODEX_THREAD_ID",
    "CODEX_SESSION_ID",
    "CLAUDE_PID",
    "TMUX",
    "AGENTHUB_SESSION",
];
/// Explicit allowlist for CLI-profile environment additions: exact names or
/// reviewed prefixes. Legacy adapters keep the older syntax-only rule.
const ALLOWED_ENV_NAMES: [&str; 42] = [
    "HOME",
    "PATH",
    "TERM",
    "COLORTERM",
    "LANG",
    "LANGUAGE",
    "SHELL",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "EDITOR",
    "VISUAL",
    "PAGER",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    // Windows: the cleared environment must carry the system locations the
    // loader, Winsock and the Node-based CLIs read; spelled as Windows sets
    // them (its own variable names are case-insensitive, `Command` folds
    // duplicates, so a profile lists each once).
    "SystemRoot",
    "SYSTEMROOT",
    "windir",
    "SystemDrive",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "TEMP",
    "TMP",
    "COMSPEC",
    "PATHEXT",
    "HOMEDRIVE",
    "HOMEPATH",
    "USERNAME",
    "PROGRAMDATA",
    "ProgramData",
    "ProgramFiles",
    "ProgramW6432",
];
/// The one Windows variable whose name is not a plain identifier.
const PROGRAM_FILES_X86: &str = "ProgramFiles(x86)";
const ALLOWED_ENV_PREFIXES: [&str; 9] = [
    "LC_",
    "XDG_",
    "ANTHROPIC_",
    "CLAUDE_",
    "CODEX_",
    "OPENAI_",
    "GROK_",
    "XAI_",
    "AGENTHUB_TEST_",
];

fn default_schema() -> u32 {
    1
}

/// Private administrator configuration; intentionally not Debug or Serialize.
/// Schema 1 carries only fixed-argv `adapters`; schema 2 additionally allows
/// per-source CLI `profiles`. Unknown fields fail closed in both.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_schema")]
    pub schema: u32,
    pub host_binary: PathBuf,
    pub host_dir: PathBuf,
    pub cwd_roots: Vec<PathBuf>,
    #[serde(default)]
    pub adapters: Vec<Adapter>,
    #[serde(default)]
    pub profiles: Vec<CliProfile>,
    /// Batch 41: which ordinary CLI profile a bug-report worker uses per
    /// source. Each id must name a `profiles` entry of that source; its argv
    /// is additionally held to `bug_report_policy` when a worker launches.
    #[serde(default)]
    pub bug_report_profiles: BugReportProfiles,
}

/// Python `bug_report.WORKER_SOURCES` mapped to launch profiles. Absent
/// sources cannot run a worker (`503` at the route, like a missing CLI).
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BugReportProfiles {
    pub claude: Option<String>,
    pub codex: Option<String>,
    pub grok: Option<String>,
}

impl BugReportProfiles {
    pub fn get(&self, source: Source) -> Option<&str> {
        match source {
            Source::Claude => self.claude.as_deref(),
            Source::Codex => self.codex.as_deref(),
            Source::Grok => self.grok.as_deref(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.claude.is_none() && self.codex.is_none() && self.grok.is_none()
    }
}

/// The cheapest configuration per CLI (AGENTS.md real-CLI rule): the exact
/// dated Claude ID at low effort, Codex `gpt-5.6-luna` at low reasoning
/// effort, Grok 4.6 at low effort. Fixed in code, not configurable.
pub const BUG_REPORT_CLAUDE_MODEL: &str = "claude-haiku-4-5-20251001";
pub const BUG_REPORT_CODEX_MODEL: &str = "gpt-5.6-luna";
pub const BUG_REPORT_GROK_MODEL: &str = "grok-4.6";

/// Whether `argv` (the profile's fixed `args` followed by `new_args`) pins the
/// cheapest model and low effort for `source`. Flags are whole arguments
/// followed by their value, so a model named inside another argument or a
/// missing effort flag is a mismatch.
pub fn bug_report_policy(source: Source, argv: &[String]) -> bool {
    let pair = |flags: &[&str], values: &[&str]| {
        argv.windows(2).any(|window| {
            flags.contains(&window[0].as_str()) && values.contains(&window[1].as_str())
        })
    };
    match source {
        Source::Claude => {
            pair(&["--model"], &[BUG_REPORT_CLAUDE_MODEL]) && pair(&["--effort"], &["low"])
        }
        Source::Codex => {
            pair(&["--model", "-m"], &[BUG_REPORT_CODEX_MODEL])
                && pair(
                    &["-c", "--config"],
                    &[
                        "model_reasoning_effort=\"low\"",
                        "model_reasoning_effort=low",
                        "model_reasoning_effort='low'",
                    ],
                )
        }
        Source::Grok => {
            pair(&["--model", "-m"], &[BUG_REPORT_GROK_MODEL])
                && pair(&["--reasoning-effort"], &["low"])
        }
    }
}

/// One real CLI installation. `args` is the fixed prefix; `new_args` /
/// `resume_args` are argv templates whose only substitutions are the whole
/// arguments `{session_id}` (Claude new, server UUID) and `{sid}` (resume).
/// Per-source rules are fixed in code, not configurable: Claude `new_args`
/// must contain `{session_id}` exactly once, Codex/Grok must not; an empty
/// `resume_args` means resume is unsupported for this profile.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CliProfile {
    pub id: String,
    pub source: Source,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub new_args: Vec<String>,
    #[serde(default)]
    pub resume_args: Vec<String>,
    #[serde(default, deserialize_with = "unique_environment")]
    pub env: BTreeMap<String, String>,
    /// Names guaranteed absent from the launcher-built environment (beyond
    /// the always-denied session identity variables), e.g. the default TERM.
    #[serde(default)]
    pub env_remove: Vec<String>,
    /// Explicit roots for this profile; each must lie inside a global cwd root.
    pub cwd_roots: Vec<PathBuf>,
}

/// Public catalog row for HTTP selection: which allowlisted IDs exist, their
/// source, whether they are real CLI profiles, whether they can resume, and
/// whether they are reserved for the bug-report worker (`worker`): a worker
/// profile is launched by id from `bug_report::worker` only and never counts
/// as the interactive CLI of its source, so a source with one CLI profile
/// plus one worker profile still has exactly one interactive choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub source: Source,
    pub profile: bool,
    pub resume: bool,
    pub worker: bool,
}
impl Entry {
    /// Selectable without an explicit `adapter_id`: everything but worker profiles.
    pub fn interactive(&self) -> bool {
        !self.worker
    }
}
pub fn entries(config: &Config) -> Vec<Entry> {
    config
        .adapters
        .iter()
        .map(|adapter| Entry {
            id: adapter.id.clone(),
            source: adapter.source,
            profile: false,
            resume: false,
            worker: false,
        })
        .chain(config.profiles.iter().map(|profile| Entry {
            id: profile.id.clone(),
            source: profile.source,
            profile: true,
            resume: !profile.resume_args.is_empty(),
            worker: config.bug_report_profiles.get(profile.source) == Some(profile.id.as_str()),
        }))
        .collect()
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adapter {
    pub id: String,
    pub source: Source,
    pub executable: PathBuf,
    pub args: Vec<String>,
    #[serde(deserialize_with = "unique_environment")]
    pub env: BTreeMap<String, String>,
}

fn unique_environment<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    struct Unique;
    impl<'de> Visitor<'de> for Unique {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an explicit environment object with unique keys")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                if result.len() == MAX_ENV || result.insert(key, value).is_some() {
                    return Err(de::Error::custom("duplicate or excessive environment keys"));
                }
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(Unique)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidConfig,
    UnsafePath,
    UnsafePermissions,
    Changed,
    ConfigUnavailable,
    AdapterUnavailable,
    InvalidSpec,
    EndpointOccupied,
    UnsupportedPlatform,
    SpawnFailed(std::io::ErrorKind),
}
impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "lifecycle launcher: {self:?}")
    }
}
impl std::error::Error for Error {}

/// Neither result type has Debug or a Drop implementation that kills a process.
/// The coordinator must retain/reap Child and settle the authority separately.
pub struct Started {
    pub authority: StartAuthority,
    pub child: Child,
}
pub struct LaunchFailure {
    pub authority: StartAuthority,
    pub error: Error,
}

/// Read one explicit private file through no-follow directory/file handles.
/// Serde rejects duplicate struct fields; the environment visitor rejects map
/// duplicates. Parse errors are deliberately replaced by a static error code.
pub fn read_config(path: &Path) -> Result<Config, Error> {
    let mut opened = CheckedFile::open(path, FileKind::Config)?;
    if opened.stamp.len > MAX_CONFIG_BYTES as u64 {
        return Err(Error::InvalidConfig);
    }
    let mut bytes = Vec::new();
    opened
        .file
        .by_ref()
        .take(MAX_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::ConfigUnavailable)?;
    opened.verify()?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(Error::InvalidConfig);
    }
    let config: Config = serde_json::from_slice(&bytes).map_err(|_| Error::InvalidConfig)?;
    config_bounds(&config)?;
    Ok(config)
}

struct CheckedAdapter {
    config: Adapter,
    executable: CheckedFile,
}
struct CheckedProfile {
    config: CliProfile,
    executable: CheckedFile,
    cwd_roots: Vec<CheckedDirectory>,
}

pub struct Launcher {
    host_binary: CheckedFile,
    host_directory: CheckedDirectory,
    cwd_roots: Vec<CheckedDirectory>,
    adapters: BTreeMap<String, CheckedAdapter>,
    profiles: BTreeMap<String, CheckedProfile>,
    entries: Vec<Entry>,
}

/// The bug-report worker profiles a launcher configuration names, resolved
/// to the argv each would run (executable, fixed args, new-session template
/// with `{session_id}` unsubstituted). `config_bounds` already proved every
/// named id exists with the right source.
pub fn bug_report_profiles(config: &Config) -> Vec<BugReportProfile> {
    [Source::Claude, Source::Codex, Source::Grok]
        .into_iter()
        .filter_map(|source| {
            let id = config.bug_report_profiles.get(source)?;
            let profile = config
                .profiles
                .iter()
                .find(|profile| profile.id == id && profile.source == source)?;
            let mut argv = vec![profile.executable.to_string_lossy().into_owned()];
            argv.extend(profile.args.iter().cloned());
            argv.extend(profile.new_args.iter().cloned());
            Some(BugReportProfile {
                id: id.to_owned(),
                source,
                argv,
                cwd_roots: profile.cwd_roots.clone(),
            })
        })
        .collect()
}

/// Public view of a bug-report worker profile (batch 41): enough for the
/// policy assertion and the launch spec, no environment or host credential.
#[derive(Clone)]
pub struct BugReportProfile {
    pub id: String,
    pub source: Source,
    /// Executable first, then the fixed args and the new-session template
    /// with its `{session_id}` placeholder unsubstituted.
    pub argv: Vec<String>,
    pub cwd_roots: Vec<PathBuf>,
}

impl BugReportProfile {
    /// The cheapest-model rule, asserted right before a worker launches.
    pub fn policy_ok(&self) -> bool {
        bug_report_policy(self.source, &self.argv[1..])
    }
}

impl Launcher {
    pub fn new(config: Config) -> Result<Self, Error> {
        config_bounds(&config)?;
        if config.host_dir.parent().is_none()
            || config.cwd_roots.iter().any(|path| path.parent().is_none())
        {
            return Err(Error::UnsafePath);
        }
        let entries = entries(&config);
        let host_binary = CheckedFile::open(&config.host_binary, FileKind::Executable)?;
        let host_directory = CheckedDirectory::open(&config.host_dir, true)?;
        let mut cwd_roots = Vec::new();
        for path in config.cwd_roots {
            if overlap(&path, &config.host_dir)
                || cwd_roots
                    .iter()
                    .any(|root: &CheckedDirectory| overlap(&path, &root.path))
            {
                return Err(Error::UnsafePath);
            }
            cwd_roots.push(CheckedDirectory::open(&path, false)?);
        }
        let mut adapters = BTreeMap::new();
        for config in config.adapters {
            let executable = CheckedFile::open(&config.executable, FileKind::Executable)?;
            adapters.insert(config.id.clone(), CheckedAdapter { config, executable });
        }
        let mut profiles = BTreeMap::new();
        for config in config.profiles {
            let executable = CheckedFile::open(&config.executable, FileKind::Executable)?;
            let mut roots = Vec::new();
            for path in &config.cwd_roots {
                if !cwd_roots.iter().any(|root| path.starts_with(&root.path)) {
                    return Err(Error::UnsafePath);
                }
                roots.push(CheckedDirectory::open(path, false)?);
            }
            profiles.insert(
                config.id.clone(),
                CheckedProfile {
                    config,
                    executable,
                    cwd_roots: roots,
                },
            );
        }
        Ok(Self {
            host_binary,
            host_directory,
            cwd_roots,
            adapters,
            profiles,
            entries,
        })
    }

    pub fn host_dir(&self) -> &Path {
        &self.host_directory.path
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Synchronous filesystem checks; async callers run this in bounded blocking
    /// work. Constructor snapshots alone cannot authorize a later changed path.
    pub fn validate_spec(&self, spec: &LaunchSpec) -> Result<(), Error> {
        spec.validate().map_err(|_| Error::InvalidSpec)?;
        let (source, executable, roots, kind_allowed) =
            if let Some(adapter) = self.adapters.get(spec.adapter_id()) {
                (
                    adapter.config.source,
                    &adapter.executable,
                    &self.cwd_roots,
                    *spec.launch() == Launch::Fixed,
                )
            } else if let Some(profile) = self.profiles.get(spec.adapter_id()) {
                (
                    profile.config.source,
                    &profile.executable,
                    &profile.cwd_roots,
                    match spec.launch() {
                        Launch::Fixed => false,
                        Launch::NewPending | Launch::NewAssigned => true,
                        Launch::Resume { .. } => !profile.config.resume_args.is_empty(),
                    },
                )
            } else {
                return Err(Error::AdapterUnavailable);
            };
        if source != spec.source() {
            return Err(Error::AdapterUnavailable);
        }
        if !kind_allowed {
            return Err(Error::InvalidSpec);
        }
        self.host_binary.verify()?;
        self.host_directory.verify()?;
        executable.verify()?;
        let root = roots
            .iter()
            .find(|root| spec.cwd().starts_with(&root.path))
            .ok_or(Error::InvalidSpec)?;
        root.verify()?;
        CheckedDirectory::open(spec.cwd(), false)?.verify()?;
        root.verify()?;
        Ok(())
    }

    /// The exact child argv (executable first) for a receipt, with the only
    /// substitutions being whole-argument `{session_id}` / `{sid}` from the
    /// receipt's validated identity. No shell, quoting or joining step.
    pub fn argv(&self, record: &Record) -> Result<Vec<OsString>, Error> {
        let spec = record.spec();
        if let Some(adapter) = self.adapters.get(spec.adapter_id()) {
            if *spec.launch() != Launch::Fixed {
                return Err(Error::InvalidSpec);
            }
            let mut argv = vec![adapter.config.executable.as_os_str().to_owned()];
            argv.extend(adapter.config.args.iter().map(OsString::from));
            return Ok(argv);
        }
        let profile = self
            .profiles
            .get(spec.adapter_id())
            .ok_or(Error::AdapterUnavailable)?;
        let (template, placeholder, value): (&[String], &str, Option<&str>) = match spec.launch() {
            Launch::Fixed => return Err(Error::InvalidSpec),
            Launch::NewPending => (&profile.config.new_args, SESSION_ID_PLACEHOLDER, None),
            Launch::NewAssigned => (
                &profile.config.new_args,
                SESSION_ID_PLACEHOLDER,
                Some(record.session_id().ok_or(Error::InvalidSpec)?),
            ),
            Launch::Resume { sid, .. } => {
                if profile.config.resume_args.is_empty() {
                    return Err(Error::InvalidSpec);
                }
                (&profile.config.resume_args, SID_PLACEHOLDER, Some(sid))
            }
        };
        if value.is_some_and(|value| !model::native_sid(value)) {
            return Err(Error::InvalidSpec);
        }
        let mut argv = vec![profile.config.executable.as_os_str().to_owned()];
        argv.extend(profile.config.args.iter().map(OsString::from));
        for arg in template {
            if arg == placeholder {
                argv.push(OsString::from(value.ok_or(Error::InvalidSpec)?));
            } else {
                argv.push(OsString::from(arg));
            }
        }
        Ok(argv)
    }

    /// Immutable host metadata: the reviewed launch identity, plus the full
    /// declared native SID/UID when the command line itself names one. It is
    /// an association hint for the runtime catalog, never proof of acceptance.
    pub fn metadata(record: &Record) -> serde_json::Value {
        let mut metadata = serde_json::json!({"source":record.spec().source(),
            "launch_id":record.launch_id(),"instance_id":record.instance_id()});
        if let Some(sid) = record.declared_sid() {
            metadata["sid"] = serde_json::Value::from(sid);
        }
        if let Some(uid) = record.declared_uid() {
            metadata["uid"] = serde_json::Value::from(uid);
        }
        metadata
    }

    /// Bounded shell-style completion strictly inside the global cwd roots.
    /// Symlinked entries are never followed (nor listed), parents outside every
    /// root yield nothing, and each result is `<typed parent><name>/`.
    pub fn complete_directories(&self, text: &str, limit: usize) -> Result<Vec<String>, Error> {
        if text.len() > model::MAX_CWD_BYTES || text.chars().any(char::is_control) {
            return Err(Error::InvalidSpec);
        }
        let limit = limit.clamp(1, MAX_COMPLETIONS);
        let text = text.trim();
        let mut rows = BTreeSet::new();
        for root in &self.cwd_roots {
            let root_text = root.path.to_str().ok_or(Error::UnsafePath)?;
            let root_slash = format!("{root_text}/");
            if text.is_empty() || (text.len() < root_slash.len() && root_slash.starts_with(text)) {
                rows.insert(root_slash);
                continue;
            }
            if !text.starts_with(&root_slash) {
                continue;
            }
            let (parent, prefix) = match text.rfind('/') {
                Some(index) => (&text[..=index], &text[index + 1..]),
                None => continue,
            };
            let relative = &parent[root_slash.len()..];
            let parts: Vec<&str> = relative
                .split('/')
                .filter(|part| !part.is_empty())
                .collect();
            // The prefix only filters entry names of the listed parent; the
            // parent itself must be a normalized chain of plain components.
            if relative.len() != parts.iter().map(|part| part.len() + 1).sum::<usize>()
                || parts.iter().any(|part| *part == "." || *part == "..")
            {
                continue;
            }
            root.verify()?;
            let mut directory = root.directory.clone();
            let mut reachable = true;
            for part in parts {
                let metadata = match directory.symlink_metadata(part) {
                    Ok(metadata) => metadata,
                    Err(_) => {
                        reachable = false;
                        break;
                    }
                };
                if ordinary(&metadata, true).is_err() {
                    reachable = false;
                    break;
                }
                match directory.open_dir_nofollow(part) {
                    Ok(child) => directory = Arc::new(child),
                    Err(_) => {
                        reachable = false;
                        break;
                    }
                }
            }
            if !reachable {
                continue;
            }
            let Ok(entries) = directory.entries() else {
                continue;
            };
            for entry in entries.take(MAX_SCANNED_ENTRIES) {
                let Ok(entry) = entry else { continue };
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if !name.starts_with(prefix) || (name.starts_with('.') && !prefix.starts_with('.'))
                {
                    continue;
                }
                // file_type never follows symlinks: a link is skipped even when
                // it points inside the root, so no completion escapes a root.
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if !kind.is_dir() || kind.is_symlink() {
                    continue;
                }
                rows.insert(format!("{parent}{name}/"));
            }
        }
        let mut rows: Vec<String> = rows.into_iter().collect();
        rows.sort_by(|left, right| {
            left.to_lowercase()
                .cmp(&right.to_lowercase())
                .then_with(|| left.cmp(right))
        });
        rows.truncate(limit);
        Ok(rows)
    }

    /// Consumes a one-use durable Starting authority and returns it in exactly
    /// one result. Spawn success is not readiness. There is no post-spawn fallible
    /// path that could lose the owned Child or report a false no-spawn failure.
    // The error intentionally returns the original non-Clone durable authority
    // intact; preserve the coordinator's explicit ownership-handoff API.
    #[allow(clippy::result_large_err)]
    pub fn launch(&self, authority: StartAuthority) -> Result<Started, LaunchFailure> {
        let record = authority.record();
        let checked = (|| {
            if record.state() != State::Starting {
                return Err(Error::InvalidSpec);
            }
            self.validate_spec(record.spec())?;
            // The imported host can replace a socket at bind time; never
            // intentionally hand it an already occupied generated endpoint.
            for extension in ["json", "sock", "log"] {
                let name = format!("{}.{}", record.host_name(), extension);
                match self.host_directory.directory.symlink_metadata(name) {
                    Ok(_) => return Err(Error::EndpointOccupied),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(_) => return Err(Error::Changed),
                }
            }
            Ok(())
        })();
        if let Err(error) = checked {
            return Err(LaunchFailure { authority, error });
        }
        let argv = match self.argv(record) {
            Ok(argv) => argv,
            Err(error) => return Err(LaunchFailure { authority, error }),
        };
        match spawn_detached(self.command(record, &argv)) {
            Ok(child) => Ok(Started { authority, child }),
            Err(error) => Err(LaunchFailure {
                authority,
                error: Error::SpawnFailed(error.kind()),
            }),
        }
    }

    /// The platform-independent host command line: explicit environment,
    /// authorized cwd, null stdio, the host arguments, then the CLI argv.
    /// Only how the child is detached from this process differs per platform.
    fn command(&self, record: &Record, argv: &[OsString]) -> Command {
        let metadata = Self::metadata(record);
        let mut command = Command::new(&self.host_binary.path);
        command.env_clear().env("TERM", "xterm-256color");
        if let Some(adapter) = self.adapters.get(record.spec().adapter_id()) {
            command.envs(&adapter.config.env);
        } else if let Some(profile) = self.profiles.get(record.spec().adapter_id()) {
            for name in &profile.config.env_remove {
                command.env_remove(name);
            }
            command.envs(&profile.config.env);
        }
        for name in DENIED_ENV {
            command.env_remove(name);
        }
        command
            .current_dir(record.spec().cwd())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .arg("--dir")
            .arg(self.host_dir())
            .args(["run", "--name", record.host_name(), "--cwd"])
            .arg(record.spec().cwd())
            .arg("--meta")
            .arg(metadata.to_string())
            .arg("--")
            .args(argv);
        command
    }
}

/// Unix: a new process group, so a signal aimed at the service's group never
/// reaches the host.
#[cfg(unix)]
fn spawn_detached(mut command: Command) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt;
    command.process_group(0).spawn()
}

/// Windows: no console, own process group (console Ctrl events stay with the
/// service) and out of the service's job object, so that a job-terminated
/// service leaves the host alive — Python `term_host._spawn`. A job that
/// forbids breakaway makes `CreateProcess` refuse with access denied; the host
/// is then started inside the job rather than not at all, again like Python.
#[cfg(windows)]
fn spawn_detached(mut command: Command) -> std::io::Result<Child> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let detached = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    match command
        .creation_flags(detached | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
    {
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            command.creation_flags(detached).spawn()
        }
        result => result,
    }
}

#[cfg(not(any(unix, windows)))]
fn spawn_detached(_command: Command) -> std::io::Result<Child> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        Error::UnsupportedPlatform,
    ))
}

fn versioned_id(id: &str) -> bool {
    let version = id.rsplit_once("-v").or_else(|| id.rsplit_once("_v"));
    model::identifier(id, 1, model::MAX_ADAPTER_BYTES)
        && version.is_some_and(|(prefix, version)| {
            !prefix.is_empty()
                && !version.is_empty()
                && version.len() <= 6
                && !version.starts_with('0')
                && version.bytes().all(|b| b.is_ascii_digit())
        })
}
fn env_name(key: &str) -> bool {
    key == PROGRAM_FILES_X86
        || !key.is_empty()
            && key.len() <= 128
            && key.bytes().enumerate().all(|(i, byte)| {
                byte == b'_' || byte.is_ascii_alphabetic() || (i > 0 && byte.is_ascii_digit())
            })
}
fn allowed_profile_env(key: &str) -> bool {
    !DENIED_ENV.contains(&key)
        && (key == PROGRAM_FILES_X86
            || ALLOWED_ENV_NAMES.contains(&key)
            || ALLOWED_ENV_PREFIXES
                .iter()
                .any(|prefix| key.starts_with(prefix) && key.len() > prefix.len()))
}
/// Every argument is either an exact whole placeholder or contains no
/// placeholder text at all; any other `{name}`-shaped argument is rejected.
fn placeholder_count(args: &[String], placeholder: Option<&str>) -> Result<usize, Error> {
    let mut count = 0;
    for arg in args {
        if placeholder == Some(arg.as_str()) {
            count += 1;
        } else if arg.contains(SESSION_ID_PLACEHOLDER)
            || arg.contains(SID_PLACEHOLDER)
            || (arg.len() > 2
                && arg.starts_with('{')
                && arg.ends_with('}')
                && arg[1..arg.len() - 1]
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'_'))
        {
            return Err(Error::InvalidConfig);
        }
    }
    Ok(count)
}
fn check_args(args: &[String], size: &mut usize) -> Result<(), Error> {
    for arg in args {
        if arg.len() > MAX_ARG_BYTES || arg.contains('\0') {
            return Err(Error::InvalidConfig);
        }
        *size += arg.len();
    }
    Ok(())
}
fn check_env(env: &BTreeMap<String, String>, size: &mut usize) -> Result<(), Error> {
    if env.len() > MAX_ENV {
        return Err(Error::InvalidConfig);
    }
    for (key, value) in env {
        if !env_name(key) || value.len() > MAX_ENV_VALUE_BYTES || value.contains('\0') {
            return Err(Error::InvalidConfig);
        }
        *size += key.len() + value.len();
    }
    Ok(())
}

fn config_bounds(config: &Config) -> Result<(), Error> {
    if !(1..=MAX_ENTRIES).contains(&config.cwd_roots.len())
        || config.adapters.len() > MAX_ENTRIES
        || config.profiles.len() > MAX_ENTRIES
        || config.adapters.is_empty() && config.profiles.is_empty()
        || !matches!(config.schema, 1 | 2)
        || (config.schema == 1 && !config.profiles.is_empty())
    {
        return Err(Error::InvalidConfig);
    }
    let mut size = path_text(&config.host_binary)?.len() + path_text(&config.host_dir)?.len();
    for root in &config.cwd_roots {
        size += path_text(root)?.len();
    }
    let mut identifiers = BTreeSet::new();
    for adapter in &config.adapters {
        if !versioned_id(&adapter.id)
            || !identifiers.insert(&adapter.id)
            || adapter.args.len() > MAX_ARGS
        {
            return Err(Error::InvalidConfig);
        }
        size += adapter.id.len() + path_text(&adapter.executable)?.len();
        check_args(&adapter.args, &mut size)?;
        check_env(&adapter.env, &mut size)?;
    }
    for profile in &config.profiles {
        if !versioned_id(&profile.id)
            || !identifiers.insert(&profile.id)
            || profile.args.len() + profile.new_args.len() + profile.resume_args.len() > MAX_ARGS
            || profile.env_remove.len() > MAX_ENV_REMOVE
            || !(1..=MAX_ENTRIES).contains(&profile.cwd_roots.len())
        {
            return Err(Error::InvalidConfig);
        }
        size += profile.id.len() + path_text(&profile.executable)?.len();
        for args in [&profile.args, &profile.new_args, &profile.resume_args] {
            check_args(args, &mut size)?;
        }
        check_env(&profile.env, &mut size)?;
        if profile.env.keys().any(|key| !allowed_profile_env(key)) {
            return Err(Error::InvalidConfig);
        }
        let mut removed = BTreeSet::new();
        for name in &profile.env_remove {
            if !env_name(name) || profile.env.contains_key(name) || !removed.insert(name) {
                return Err(Error::InvalidConfig);
            }
            size += name.len();
        }
        // Fixed per-source contract: only Claude receives an upfront SID; a
        // resume template names `{sid}` exactly once or is absent entirely.
        placeholder_count(&profile.args, None)?;
        let resume_placeholders = placeholder_count(&profile.resume_args, Some(SID_PLACEHOLDER))?;
        if placeholder_count(&profile.new_args, Some(SESSION_ID_PLACEHOLDER))?
            != usize::from(profile.source == Source::Claude)
            || (!profile.resume_args.is_empty() && resume_placeholders != 1)
        {
            return Err(Error::InvalidConfig);
        }
        let mut roots = BTreeSet::new();
        for root in &profile.cwd_roots {
            size += path_text(root)?.len();
            if root.parent().is_none()
                || !config
                    .cwd_roots
                    .iter()
                    .any(|global| root.starts_with(global))
                || !roots.insert(root)
            {
                return Err(Error::UnsafePath);
            }
        }
    }
    for source in [Source::Claude, Source::Codex, Source::Grok] {
        if let Some(id) = config.bug_report_profiles.get(source) {
            size += id.len();
            if !config
                .profiles
                .iter()
                .any(|profile| profile.id == id && profile.source == source)
            {
                return Err(Error::InvalidConfig);
            }
        }
    }
    if size > MAX_CONFIG_BYTES {
        return Err(Error::InvalidConfig);
    }
    Ok(())
}

fn path_text(path: &Path) -> Result<&str, Error> {
    let text = path.to_str().ok_or(Error::UnsafePath)?;
    if !path.is_absolute()
        || text.len() > model::MAX_CWD_BYTES
        || text.chars().any(char::is_control)
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        || path.components().collect::<PathBuf>().as_os_str() != path.as_os_str()
    {
        return Err(Error::UnsafePath);
    }
    Ok(text)
}
fn overlap(first: &Path, second: &Path) -> bool {
    first.starts_with(second) || second.starts_with(first)
}
fn identity(metadata: &Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}
fn ordinary(metadata: &Metadata, directory: bool) -> Result<(), Error> {
    #[cfg(windows)]
    let reparse = cap_std::fs::MetadataExt::file_attributes(metadata) & 0x400 != 0;
    #[cfg(not(windows))]
    let reparse = false;
    if metadata.is_symlink()
        || reparse
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(Error::UnsafePath);
    }
    Ok(())
}

struct Edge {
    parent: Arc<Dir>,
    name: OsString,
    identity: (u64, u64),
}
struct CheckedDirectory {
    path: PathBuf,
    directory: Arc<Dir>,
    edges: Vec<Edge>,
    #[cfg_attr(not(unix), allow(dead_code))] // Only Unix has a mode to check.
    private: bool,
}
impl CheckedDirectory {
    fn open(path: &Path, private: bool) -> Result<Self, Error> {
        path_text(path)?;
        let mut base = PathBuf::new();
        let mut names = Vec::new();
        for part in path.components() {
            match part {
                Component::Prefix(_) | Component::RootDir => base.push(part.as_os_str()),
                Component::Normal(name) => names.push(name.to_owned()),
                _ => return Err(Error::UnsafePath),
            }
        }
        let mut directory = Arc::new(
            Dir::open_ambient_dir(base, ambient_authority()).map_err(|_| Error::UnsafePath)?,
        );
        let mut edges = Vec::new();
        for name in names {
            let before = directory
                .symlink_metadata(&name)
                .map_err(|_| Error::UnsafePath)?;
            ordinary(&before, true)?;
            let child = Arc::new(
                directory
                    .open_dir_nofollow(&name)
                    .map_err(|_| Error::UnsafePath)?,
            );
            let after = child.dir_metadata().map_err(|_| Error::UnsafePath)?;
            ordinary(&after, true)?;
            if identity(&before) != identity(&after) {
                return Err(Error::Changed);
            }
            edges.push(Edge {
                parent: directory,
                name,
                identity: identity(&after),
            });
            directory = child;
        }
        let result = Self {
            path: path.to_owned(),
            directory,
            edges,
            private,
        };
        result.verify()?;
        Ok(result)
    }
    fn verify(&self) -> Result<(), Error> {
        for edge in &self.edges {
            let metadata = edge
                .parent
                .symlink_metadata(&edge.name)
                .map_err(|_| Error::Changed)?;
            ordinary(&metadata, true)?;
            if identity(&metadata) != edge.identity {
                return Err(Error::Changed);
            }
        }
        let metadata = self.directory.dir_metadata().map_err(|_| Error::Changed)?;
        ordinary(&metadata, true)?;
        #[cfg(unix)]
        if self.private && cap_std::fs::MetadataExt::mode(&metadata) & 0o777 != 0o700 {
            return Err(Error::UnsafePermissions);
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum FileKind {
    Executable,
    Config,
}
#[derive(PartialEq, Eq)]
struct Stamp {
    identity: (u64, u64),
    len: u64,
    modified: Option<cap_std::time::SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
}
fn stamp(metadata: &Metadata) -> Stamp {
    Stamp {
        identity: identity(metadata),
        len: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        changed: (
            cap_std::fs::MetadataExt::ctime(metadata),
            cap_std::fs::MetadataExt::ctime_nsec(metadata),
        ),
    }
}
struct CheckedFile {
    path: PathBuf,
    parent: CheckedDirectory,
    name: OsString,
    file: cap_std::fs::File,
    stamp: Stamp,
    kind: FileKind,
}
impl CheckedFile {
    fn open(path: &Path, kind: FileKind) -> Result<Self, Error> {
        path_text(path)?;
        let name = path.file_name().ok_or(Error::UnsafePath)?.to_owned();
        let parent = CheckedDirectory::open(path.parent().ok_or(Error::UnsafePath)?, false)?;
        let before = parent
            .directory
            .symlink_metadata(&name)
            .map_err(|_| Error::UnsafePath)?;
        Self::check(&before, kind)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let file = parent
            .directory
            .open_with(&name, &options)
            .map_err(|_| Error::UnsafePath)?;
        let after = file.metadata().map_err(|_| Error::UnsafePath)?;
        Self::check(&after, kind)?;
        if stamp(&before) != stamp(&after) {
            return Err(Error::Changed);
        }
        let result = Self {
            path: path.to_owned(),
            parent,
            name,
            file,
            stamp: stamp(&after),
            kind,
        };
        result.verify()?;
        Ok(result)
    }
    fn check(metadata: &Metadata, kind: FileKind) -> Result<(), Error> {
        ordinary(metadata, false)?;
        if matches!(kind, FileKind::Config) && metadata.nlink() != 1 {
            return Err(Error::UnsafePermissions);
        }
        #[cfg(unix)]
        {
            let mode = cap_std::fs::MetadataExt::mode(metadata);
            if (matches!(kind, FileKind::Config) && mode & 0o777 != 0o600)
                || (matches!(kind, FileKind::Executable) && mode & 0o111 == 0)
            {
                return Err(Error::UnsafePermissions);
            }
        }
        Ok(())
    }
    fn verify(&self) -> Result<(), Error> {
        self.parent.verify()?;
        for metadata in [
            self.file.metadata().map_err(|_| Error::Changed)?,
            self.parent
                .directory
                .symlink_metadata(&self.name)
                .map_err(|_| Error::Changed)?,
        ] {
            Self::check(&metadata, self.kind)?;
            if stamp(&metadata) != self.stamp {
                return Err(Error::Changed);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "launcher_tests.rs"]
mod tests;

#[cfg(test)]
mod env_allowlist_tests {
    use super::{allowed_profile_env, env_name};

    #[test]
    fn windows_system_variables_are_allowed_by_exact_name_only() {
        for name in [
            "SystemRoot",
            "SYSTEMROOT",
            "windir",
            "SystemDrive",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "TEMP",
            "TMP",
            "COMSPEC",
            "PATHEXT",
            "HOMEDRIVE",
            "HOMEPATH",
            "USERNAME",
            "PROGRAMDATA",
            "ProgramData",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
        ] {
            assert!(env_name(name), "{name}");
            assert!(allowed_profile_env(name), "{name}");
        }
        // The parenthesised spelling is one exact name, not a syntax relaxation.
        for name in [
            "ProgramFiles(x64)",
            "programfiles(x86)",
            "ProgramFiles(x86)=",
            "SYSTEMROOT ",
            "systemroot",
            "NODE_OPTIONS",
            "PSModulePath",
            "TMUX",
            "AGENTHUB_SESSION",
        ] {
            assert!(!allowed_profile_env(name), "{name}");
        }
        assert!(!env_name("ProgramFiles(x64)"));
        assert!(!env_name("Program Files"));
    }
}
