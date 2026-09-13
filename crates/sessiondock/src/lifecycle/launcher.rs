//! Configured adapters and one-authority process spawn. No discovery,
//! native identity binding, readiness claim, automatic retry or Drop termination.

use cap_fs_ext::{DirExt, FollowSymlinks, MetadataExt, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, Metadata, OpenOptions},
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::{Component, Path, PathBuf},
    process::Child,
    sync::Arc,
};

use super::{
    model::{self, Launch, LaunchSpec, Record, Source, State},
    store::StartAuthority,
};
use std::process::{Command, Stdio};

pub const MAX_COMPLETIONS: usize = 50;
pub const DEFAULT_COMPLETIONS: usize = 24;

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
pub const DENIED_ENV: [&str; 6] = [
    "CLAUDE_CODE_SESSION_ID",
    "CODEX_COMPANION_SESSION_ID",
    "GROK_SESSION_ID",
    "CODEX_THREAD_ID",
    "CODEX_SESSION_ID",
    "CLAUDE_PID",
];
fn default_schema() -> u32 {
    1
}

/// Private administrator configuration; intentionally not Debug or Serialize.
/// Schema 1 carries only fixed-argv `adapters`; schema 2 additionally allows
/// per-source CLI `profiles`. A bug-report worker launches the source's one
/// configured CLI exactly like `term/create` (Python `WORKER_SOURCES`); a
/// leftover `bug_report_profiles` table from batch 41 is ignored like any
/// other unknown key.
#[derive(Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_schema")]
    pub schema: u32,
    pub host_binary: PathBuf,
    pub host_dir: PathBuf,
    #[serde(default)]
    pub adapters: Vec<Adapter>,
    #[serde(default)]
    pub profiles: Vec<CliProfile>,
}

/// One real CLI installation. `args` is the fixed prefix. Legacy `new_args`
/// and `resume_args` may contain whole-argument `{session_id}` / `{sid}`
/// substitutions; when absent, Python's source-specific identity arguments
/// are appended automatically.
#[derive(Clone, Deserialize)]
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
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Names removed from the inherited environment, e.g. `TERM`. Python's
    /// lineage variables are removed separately for every CLI launch.
    #[serde(default)]
    pub env_remove: Vec<String>,
}

/// Public catalog row for HTTP selection: which configured IDs exist, their
/// source, whether they are real CLI profiles and whether they can resume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub source: Source,
    pub profile: bool,
    pub resume: bool,
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
        })
        .chain(config.profiles.iter().map(|profile| Entry {
            id: profile.id.clone(),
            source: profile.source,
            profile: true,
            resume: true,
        }))
        .collect()
}

#[derive(Clone, Deserialize)]
pub struct Adapter {
    pub id: String,
    pub source: Source,
    pub executable: PathBuf,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
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

/// Read the configured launcher file with the same ordinary path semantics as
/// Python's JSON configuration load. Parse errors are replaced by a static
/// error code so configuration contents never leak through the API.
pub fn read_config(path: &Path) -> Result<Config, Error> {
    let bytes = std::fs::read(path).map_err(|_| Error::ConfigUnavailable)?;
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
}

pub struct Launcher {
    host_binary: CheckedFile,
    host_directory: CheckedDirectory,
    adapters: BTreeMap<String, CheckedAdapter>,
    profiles: BTreeMap<String, CheckedProfile>,
    entries: Vec<Entry>,
}

impl Launcher {
    pub fn new(config: Config) -> Result<Self, Error> {
        config_bounds(&config)?;
        if config.host_dir.parent().is_none() {
            return Err(Error::UnsafePath);
        }
        let entries = entries(&config);
        // Executables may be symlinks (npm/volta shims, WinGet links, `which`
        // results): resolve them like Python's `shutil.which`, then apply the
        // no-follow identity checks to the real file.
        let host_binary = CheckedFile::open(&resolved_executable(&config.host_binary)?)?;
        let host_directory = CheckedDirectory::open(&config.host_dir)?;
        let mut adapters = BTreeMap::new();
        for config in config.adapters {
            let executable = CheckedFile::open(&resolved_executable(&config.executable)?)?;
            adapters.insert(config.id.clone(), CheckedAdapter { config, executable });
        }
        let mut profiles = BTreeMap::new();
        for config in config.profiles {
            let executable = CheckedFile::open(&resolved_executable(&config.executable)?)?;
            profiles.insert(config.id.clone(), CheckedProfile { config, executable });
        }
        Ok(Self {
            host_binary,
            host_directory,
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
        let (source, executable, kind_allowed) =
            if let Some(adapter) = self.adapters.get(spec.adapter_id()) {
                (
                    adapter.config.source,
                    &adapter.executable,
                    *spec.launch() == Launch::Fixed,
                )
            } else if let Some(profile) = self.profiles.get(spec.adapter_id()) {
                (
                    profile.config.source,
                    &profile.executable,
                    match spec.launch() {
                        Launch::Fixed => false,
                        Launch::NewPending | Launch::NewAssigned => true,
                        Launch::Resume { .. } => true,
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
        CheckedDirectory::open(spec.cwd())?.verify()?;
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
            let mut argv = vec![adapter.executable.path.as_os_str().to_owned()];
            argv.extend(adapter.config.args.iter().map(OsString::from));
            return Ok(argv);
        }
        let profile = self
            .profiles
            .get(spec.adapter_id())
            .ok_or(Error::AdapterUnavailable)?;
        let (configured, defaults, placeholder, value): (&[String], &[&str], &str, Option<&str>) =
            match spec.launch() {
                Launch::Fixed => return Err(Error::InvalidSpec),
                Launch::NewPending => (&profile.config.new_args, &[], SESSION_ID_PLACEHOLDER, None),
                Launch::NewAssigned => (
                    &profile.config.new_args,
                    &["--session-id", SESSION_ID_PLACEHOLDER],
                    SESSION_ID_PLACEHOLDER,
                    Some(record.session_id().ok_or(Error::InvalidSpec)?),
                ),
                Launch::Resume { sid, .. } => {
                    let defaults: &[&str] = match spec.source() {
                        Source::Codex => &["resume", SID_PLACEHOLDER],
                        Source::Claude | Source::Grok => &["--resume", SID_PLACEHOLDER],
                    };
                    (
                        &profile.config.resume_args,
                        defaults,
                        SID_PLACEHOLDER,
                        Some(sid),
                    )
                }
            };
        if value.is_some_and(|value| !model::native_sid(value)) {
            return Err(Error::InvalidSpec);
        }
        let mut argv = vec![profile.executable.path.as_os_str().to_owned()];
        argv.extend(profile.config.args.iter().map(OsString::from));
        let has_placeholder = configured.iter().any(|arg| arg == placeholder);
        for arg in configured.iter().map(String::as_str).chain(
            (!has_placeholder)
                .then_some(defaults)
                .into_iter()
                .flatten()
                .copied(),
        ) {
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

    /// Python-compatible shell-style completion for any absolute directory.
    pub fn complete_directories(&self, text: &str, limit: usize) -> Result<Vec<String>, Error> {
        if text.chars().count() > 4096 {
            return Err(Error::InvalidSpec);
        }
        let limit = limit.clamp(1, MAX_COMPLETIONS);
        let display = text.trim();
        if display.is_empty()
            || (display.starts_with('~') && display != "~" && !display.starts_with("~/"))
        {
            return Ok(Vec::new());
        }
        let Some(expanded) = model::expand_user(Path::new(display)) else {
            return Ok(Vec::new());
        };
        if display == "~" {
            return Ok(if expanded.is_dir() {
                vec!["~/".into()]
            } else {
                Vec::new()
            });
        }
        if !expanded.is_absolute() {
            return Ok(Vec::new());
        }
        let (parent, prefix, display_parent) = if display.ends_with('/') {
            (expanded.as_path(), "", display.to_owned())
        } else {
            let Some(parent) = expanded.parent() else {
                return Ok(Vec::new());
            };
            let Some(prefix) = expanded.file_name().and_then(|name| name.to_str()) else {
                return Ok(Vec::new());
            };
            let split = display.rfind('/').map_or(0, |index| index + 1);
            (parent, prefix, display[..split].to_owned())
        };
        let mut rows = BTreeSet::new();
        let Ok(entries) = std::fs::read_dir(parent) else {
            return Ok(Vec::new());
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with(prefix) || (name.starts_with('.') && !prefix.starts_with('.')) {
                continue;
            }
            if entry.path().is_dir() {
                rows.insert(format!("{display_parent}{name}/"));
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

    /// The platform-independent host command line: inherited service environment
    /// with explicit profile overrides and stale session identities removed,
    /// authorized cwd, null stdio, the host arguments, then the CLI argv.
    /// Only how the child is detached from this process differs per platform.
    fn command(&self, record: &Record, argv: &[OsString]) -> Command {
        let metadata = Self::metadata(record);
        let mut command = Command::new(&self.host_binary.path);
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

fn env_name(key: &str) -> bool {
    // These are the constraints of a process environment, not shell variable
    // syntax: Python also accepts names such as PSModulePath and ProgramFiles(x86).
    !key.is_empty() && !key.contains(['=', '\0'])
}
fn allowed_profile_env(key: &str) -> bool {
    env_name(key) && !DENIED_ENV.contains(&key)
}
fn check_args(args: &[String]) -> Result<(), Error> {
    for arg in args {
        if arg.contains('\0') {
            return Err(Error::InvalidConfig);
        }
    }
    Ok(())
}
fn check_env(env: &BTreeMap<String, String>) -> Result<(), Error> {
    for (key, value) in env {
        if !env_name(key) || value.contains('\0') {
            return Err(Error::InvalidConfig);
        }
    }
    Ok(())
}

fn config_bounds(config: &Config) -> Result<(), Error> {
    if config.adapters.is_empty() && config.profiles.is_empty() {
        return Err(Error::InvalidConfig);
    }
    path_text(&config.host_binary)?;
    path_text(&config.host_dir)?;
    let mut identifiers = BTreeSet::new();
    for adapter in &config.adapters {
        if adapter.id.is_empty() || !identifiers.insert(&adapter.id) {
            return Err(Error::InvalidConfig);
        }
        path_text(&adapter.executable)?;
        check_args(&adapter.args)?;
        check_env(&adapter.env)?;
    }
    for profile in &config.profiles {
        if profile.id.is_empty() || !identifiers.insert(&profile.id) {
            return Err(Error::InvalidConfig);
        }
        path_text(&profile.executable)?;
        for args in [&profile.args, &profile.new_args, &profile.resume_args] {
            check_args(args)?;
        }
        check_env(&profile.env)?;
        if profile.env.keys().any(|key| !allowed_profile_env(key)) {
            return Err(Error::InvalidConfig);
        }
        for name in &profile.env_remove {
            if !env_name(name) {
                return Err(Error::InvalidConfig);
            }
        }
    }
    Ok(())
}

fn path_text(path: &Path) -> Result<&str, Error> {
    let text = path.to_str().ok_or(Error::UnsafePath)?;
    if !path.is_absolute()
        || text.contains('\0')
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        || path.components().collect::<PathBuf>().as_os_str() != path.as_os_str()
    {
        return Err(Error::UnsafePath);
    }
    Ok(text)
}
fn identity(metadata: &Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}
/// The real file behind a configured executable path (symlinks followed).
fn resolved_executable(path: &Path) -> Result<PathBuf, Error> {
    model::plain_canonical(path).ok_or(Error::UnsafePath)
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
}
impl CheckedDirectory {
    fn open(path: &Path) -> Result<Self, Error> {
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
        Ok(())
    }
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
}
impl CheckedFile {
    fn open(path: &Path) -> Result<Self, Error> {
        path_text(path)?;
        let name = path.file_name().ok_or(Error::UnsafePath)?.to_owned();
        let parent = CheckedDirectory::open(path.parent().ok_or(Error::UnsafePath)?)?;
        let before = parent
            .directory
            .symlink_metadata(&name)
            .map_err(|_| Error::UnsafePath)?;
        Self::check(&before)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let file = parent
            .directory
            .open_with(&name, &options)
            .map_err(|_| Error::UnsafePath)?;
        let after = file.metadata().map_err(|_| Error::UnsafePath)?;
        Self::check(&after)?;
        if stamp(&before) != stamp(&after) {
            return Err(Error::Changed);
        }
        let result = Self {
            path: path.to_owned(),
            parent,
            name,
            file,
            stamp: stamp(&after),
        };
        result.verify()?;
        Ok(result)
    }
    fn check(metadata: &Metadata) -> Result<(), Error> {
        ordinary(metadata, false)?;
        #[cfg(unix)]
        {
            let mode = cap_std::fs::MetadataExt::mode(metadata);
            if mode & 0o111 == 0 {
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
            Self::check(&metadata)?;
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
mod environment_tests {
    use super::{allowed_profile_env, env_name};

    #[test]
    fn configured_environment_accepts_operator_names_but_strips_session_identity() {
        for key in [
            "NODE_OPTIONS",
            "PSModulePath",
            "SystemRoot",
            "ProgramFiles(x86)",
            "1VAR",
            "custom.name",
        ] {
            assert!(env_name(key));
            assert!(allowed_profile_env(key));
        }
        for key in ["", "BAD=KEY", "BAD\0KEY"] {
            assert!(!env_name(key));
            assert!(!allowed_profile_env(key));
        }
        for key in super::DENIED_ENV {
            assert!(!allowed_profile_env(key));
        }
    }
}
