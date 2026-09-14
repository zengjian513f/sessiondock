//! Private creation-intent types: `LaunchSpec`, `Record`, and `BindingSpec`.
//! No process launch, host I/O, or native-history writes. Specs are data, not
//! authority; serde cannot bypass cwd/identity checks.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

use super::store::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Claude,
    Codex,
    Grok,
}

/// Private persisted intent data, not authority to bind a native session.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingSpec {
    source: Source,
    sid: String,
    uid: String,
}
impl BindingSpec {
    pub(super) fn new(source: Source, sid: String, uid: String) -> Result<Self, Error> {
        let spec = Self { source, sid, uid };
        spec.validate()?;
        Ok(spec)
    }
    pub fn source(&self) -> Source {
        self.source
    }
    pub fn sid(&self) -> &str {
        &self.sid
    }
    pub fn uid(&self) -> &str {
        &self.uid
    }
    fn validate(&self) -> Result<(), Error> {
        let prefix = match self.source {
            Source::Claude => "claude:",
            Source::Codex => "codex:",
            Source::Grok => "grok:",
        };
        if self.sid.is_empty()
            || !self
                .uid
                .strip_prefix(prefix)
                .is_some_and(|suffix| !suffix.is_empty())
        {
            return Err(Error::InvalidSpec);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingState {
    Intent,
    Confirmed,
    Uncertain,
}
/// Who asserted the association. `Operator` is the explicit confirmation
/// dialog; `Process` is the server's own process-tree/file-descriptor
/// evidence (the launched host's child holds the native record open), the
/// `new-status` resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingMethod {
    Operator,
    Process,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingRecord {
    pub(super) spec: BindingSpec,
    pub(super) state: BindingState,
    pub(super) method: BindingMethod,
    /// Short private note of the evidence behind a `Process` binding (pids
    /// and the held file); `None` for operator confirmations.
    pub(super) evidence: Option<String>,
    /// Unix seconds when the binding was first confirmed by guarded Info.
    pub(super) bound_at: Option<u64>,
}
impl BindingRecord {
    pub fn spec(&self) -> &BindingSpec {
        &self.spec
    }
    pub fn state(&self) -> BindingState {
        self.state
    }
    pub fn method(&self) -> BindingMethod {
        self.method
    }
    pub fn evidence(&self) -> Option<&str> {
        self.evidence.as_deref()
    }
    pub fn bound_at(&self) -> Option<u64> {
        self.bound_at
    }
    pub(super) fn validate(&self) -> Result<(), Error> {
        self.spec.validate()?;
        Ok(())
    }
}

/// Fixed launch-kind contract, decided by code rather than by configuration:
/// a legacy adapter runs only its fixed argv; a CLI profile starts a new
/// session either without any upfront SID (Codex/Grok: identity stays pending
/// until explicit native binding) or with a server-generated SID (Claude
/// `--session-id`); resume names one full native SID/UID resolved by the
/// server from its index's native catalog. The kind is private intent data, never a
/// proof that the CLI accepted the identity.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Launch {
    Fixed,
    NewPending,
    NewAssigned,
    Resume { sid: String, uid: String },
}
impl Launch {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::NewPending => "new_pending",
            Self::NewAssigned => "new_assigned",
            Self::Resume { .. } => "resume",
        }
    }
    fn validate(&self, source: Source) -> Result<(), Error> {
        let ok = match self {
            Self::Fixed => true,
            // A UUID is assigned to Claude and Grok; Codex discovers its
            // thread identity after launch.
            Self::NewPending => source == Source::Codex,
            Self::NewAssigned => source != Source::Codex,
            Self::Resume { sid, uid } => native_sid(sid) && native_uid(source, uid),
        };
        if ok { Ok(()) } else { Err(Error::InvalidSpec) }
    }
}

/// Nonempty native SID supplied by the verified catalog.
pub fn native_sid(text: &str) -> bool {
    !text.is_empty()
}
/// Full local UID `source:` + nonempty identifier suffix.
pub fn native_uid(source: Source, text: &str) -> bool {
    let prefix = match source {
        Source::Claude => "claude:",
        Source::Codex => "codex:",
        Source::Grok => "grok:",
    };
    text.strip_prefix(prefix)
        .is_some_and(|suffix| !suffix.is_empty())
}

/// Only an adapter identity, an explicit working directory and a typed launch
/// kind; no arbitrary command, shell, arguments, environment, credential or
/// guessed native-session claim. Deliberately no Debug: cwd is private data.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchSpec {
    source: Source,
    adapter_id: String,
    cwd: String,
    launch: Launch,
}
impl LaunchSpec {
    /// Fixed-argv adapter launch. Synchronous filesystem validation: a future
    /// async caller must run this off its reactor. Existing receipts do not
    /// require cwd to remain present.
    pub fn new(source: Source, adapter_id: String, cwd: &Path) -> Result<Self, Error> {
        Self::with_launch(source, adapter_id, cwd, Launch::Fixed)
    }
    /// CLI-profile new session; the kind follows the fixed per-source rule.
    pub fn profile_new(source: Source, adapter_id: String, cwd: &Path) -> Result<Self, Error> {
        let launch = if source != Source::Codex {
            Launch::NewAssigned
        } else {
            Launch::NewPending
        };
        Self::with_launch(source, adapter_id, cwd, launch)
    }
    /// Resume of a full native SID/UID that the caller resolved from the
    /// frozen session inventory, never from a client-provided path.
    pub fn resume(
        source: Source,
        adapter_id: String,
        cwd: &Path,
        sid: String,
        uid: String,
    ) -> Result<Self, Error> {
        Self::with_launch(source, adapter_id, cwd, Launch::Resume { sid, uid })
    }
    pub fn with_launch(
        source: Source,
        adapter_id: String,
        cwd: &Path,
        launch: Launch,
    ) -> Result<Self, Error> {
        if adapter_id.is_empty() {
            return Err(Error::InvalidSpec);
        }
        let text = cwd.to_str().ok_or(Error::InvalidSpec)?.trim();
        let expanded = expand_user(Path::new(text)).ok_or(Error::InvalidSpec)?;
        let cwd = expanded.as_path();
        if text.is_empty() || text.contains('\0') || !cwd.is_absolute() {
            return Err(Error::InvalidSpec);
        }
        // Resolve symlinks. The cwd is a process
        // launch location; it does not authorize any file API or native root.
        let canonical = plain_canonical(cwd).ok_or(Error::InvalidSpec)?;
        let cwd: &Path = &canonical;
        if !cwd.is_dir() {
            return Err(Error::InvalidSpec);
        }
        let cwd = cwd.to_str().ok_or(Error::InvalidSpec)?.to_owned();
        let spec = Self {
            source,
            adapter_id,
            cwd,
            launch,
        };
        spec.validate()?;
        Ok(spec)
    }
    pub fn source(&self) -> Source {
        self.source
    }
    pub fn adapter_id(&self) -> &str {
        &self.adapter_id
    }
    pub fn cwd(&self) -> &Path {
        Path::new(&self.cwd)
    }
    pub fn launch(&self) -> &Launch {
        &self.launch
    }
    pub(super) fn validate(&self) -> Result<(), Error> {
        if self.adapter_id.is_empty() || !valid_cwd(&self.cwd) {
            return Err(Error::InvalidSpec);
        }
        self.launch.validate(self.source)
    }
    pub(super) fn verify_directory(&self) -> Result<(), Error> {
        let checked = Self::with_launch(
            self.source,
            self.adapter_id.clone(),
            self.cwd(),
            self.launch.clone(),
        )?;
        if checked == *self {
            Ok(())
        } else {
            Err(Error::InvalidSpec)
        }
    }
}
/// The canonical cwd in the spelling the CLI uses.
/// Windows `canonicalize` answers `\\?\C:\…`; the launcher roots are written
/// `C:\…`, Node keeps whatever cwd it is started in and Claude derives its
/// project directory name from that, so a verbatim disk prefix is folded back
/// Other prefixes (UNC, devices) stay as they are and match no root.
pub(super) fn plain_canonical(path: &Path) -> Option<std::path::PathBuf> {
    let canonical = path.canonicalize().ok()?;
    #[cfg(windows)]
    {
        let mut components = canonical.components();
        if let Some(Component::Prefix(prefix)) = components.next()
            && let std::path::Prefix::VerbatimDisk(letter) = prefix.kind()
        {
            let mut plain = std::path::PathBuf::from(format!("{}:\\", char::from(letter)));
            for component in components {
                if let Component::Normal(name) = component {
                    plain.push(name);
                }
            }
            return Some(plain);
        }
    }
    Some(canonical)
}

/// Expand `~`, including `~name` through the system account
/// database on Unix. This only resolves a home name; it grants no authority.
pub(crate) fn expand_user(path: &Path) -> Option<std::path::PathBuf> {
    let text = path.to_str()?;
    let Some(tail) = text.strip_prefix('~') else {
        return Some(path.to_owned());
    };
    #[cfg(windows)]
    let split = tail.find(['/', '\\']).unwrap_or(tail.len());
    #[cfg(not(windows))]
    let split = tail.find('/').unwrap_or(tail.len());
    let (name, rest) = tail.split_at(split);
    #[cfg(windows)]
    let rest = rest.trim_start_matches(['/', '\\']);
    #[cfg(not(windows))]
    let rest = rest.trim_start_matches('/');
    let home = user_home(name)?;
    Some(if rest.is_empty() {
        home
    } else {
        home.join(rest)
    })
}

#[cfg(unix)]
fn user_home(name: &str) -> Option<std::path::PathBuf> {
    use std::ffi::{CStr, CString, OsString};
    use std::os::unix::ffi::OsStringExt;
    if name.is_empty()
        && let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
    {
        return Some(home.into());
    }
    let configured = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let mut buffer = vec![0_u8; usize::try_from(configured).unwrap_or(16_384).max(1024)];
    let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    let status = if name.is_empty() {
        unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        }
    } else {
        let name = CString::new(name).ok()?;
        unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        }
    };
    if status != 0 || result.is_null() {
        return None;
    }
    let record = unsafe { record.assume_init() };
    let bytes = unsafe { CStr::from_ptr(record.pw_dir) }.to_bytes().to_vec();
    Some(OsString::from_vec(bytes).into())
}

#[cfg(windows)]
fn user_home(name: &str) -> Option<std::path::PathBuf> {
    let current = std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            Some(
                std::path::PathBuf::from(std::env::var_os("HOMEDRIVE")?)
                    .join(std::env::var_os("HOMEPATH")?),
            )
        })?;
    if name.is_empty() || std::env::var("USERNAME").is_ok_and(|user| user == name) {
        return Some(current);
    }
    Some(current.parent()?.join(name))
}

#[cfg(not(any(unix, windows)))]
fn user_home(name: &str) -> Option<std::path::PathBuf> {
    name.is_empty()
        .then(|| std::env::var_os("HOME").map(Into::into))
        .flatten()
}
fn valid_cwd(text: &str) -> bool {
    let path = Path::new(text);
    !text.is_empty()
        && !text.contains('\0')
        && path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
        && path
            .components()
            .collect::<std::path::PathBuf>()
            .as_os_str()
            == path.as_os_str()
}
pub(super) fn identifier(text: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
pub(super) fn nonce(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Prepared,
    Starting,
    Running,
    Failed,
    Uncertain,
    Exited,
    CancelRequested,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    AdapterUnavailable,
    LaunchRejected,
    OwnershipLost,
    PreparationCancelled,
}

/// A private full receipt, not a public diagnostic projection.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub(super) record_id: String,
    pub(super) request_id: String,
    pub(super) spec: LaunchSpec,
    pub(super) launch_id: String,
    pub(super) instance_id: String,
    pub(super) host_name: String,
    pub(super) revision: u64,
    pub(super) state: State,
    pub(super) failure: Option<Failure>,
    pub(super) cancel_requested: bool,
    pub(super) binding: Option<BindingRecord>,
    /// Server-generated once per `NewAssigned` intent, before durable Prepared,
    /// so an idempotent replay repeats the same `--session-id`.
    pub(super) session_id: Option<String>,
    /// Unix seconds when the intent was persisted (schema 5; `None` for
    /// receipts migrated from older ledgers). Display only.
    pub(super) created_at: Option<u64>,
    /// Unix seconds when the receipt reached a terminal state (Exited or
    /// Failed); `term/list` archives pending rows from it.
    pub(super) finished_at: Option<u64>,
    /// The operator dropped this finished receipt from the pending view. A
    /// display tombstone only: nothing is deleted and no process is touched.
    pub(super) discarded: bool,
}
impl Record {
    pub fn created_at(&self) -> Option<u64> {
        self.created_at
    }
    pub fn finished_at(&self) -> Option<u64> {
        self.finished_at
    }
    pub fn discarded(&self) -> bool {
        self.discarded
    }
    /// A receipt that can be discarded from the pending view: it ended, or it
    /// was durably cancelled and can never become Running again.
    pub fn discardable(&self) -> bool {
        matches!(self.state, State::Exited | State::Failed) || self.cancel_requested
    }
    /// The exact SID handed to the CLI on its command line, if any. A declared
    /// SID is a launch argument, not evidence that the CLI created/resumed it.
    pub fn declared_sid(&self) -> Option<&str> {
        match self.spec.launch() {
            Launch::NewAssigned => self.session_id.as_deref(),
            Launch::Resume { sid, .. } => Some(sid),
            Launch::Fixed | Launch::NewPending => None,
        }
    }
    pub fn declared_uid(&self) -> Option<&str> {
        match self.spec.launch() {
            Launch::Resume { uid, .. } => Some(uid),
            _ => None,
        }
    }
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
    pub fn record_id(&self) -> &str {
        &self.record_id
    }
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    pub fn spec(&self) -> &LaunchSpec {
        &self.spec
    }
    pub fn launch_id(&self) -> &str {
        &self.launch_id
    }
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
    /// Routing name only; the immutable instance_id must guard future host work.
    pub fn host_name(&self) -> &str {
        &self.host_name
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn failure(&self) -> Option<Failure> {
        self.failure
    }
    pub fn cancel_requested(&self) -> bool {
        self.cancel_requested
    }
    pub fn binding(&self) -> Option<&BindingRecord> {
        self.binding.as_ref()
    }
    pub(super) fn validate(&self) -> Result<(), Error> {
        self.spec.validate()?;
        if self.session_id.is_some() != matches!(self.spec.launch, Launch::NewAssigned)
            || self
                .session_id
                .as_deref()
                .is_some_and(|sid| !native_sid(sid))
        {
            return Err(Error::Invalid);
        }
        if self.finished_at.is_some() && !matches!(self.state, State::Exited | State::Failed) {
            return Err(Error::Invalid);
        }
        if self.discarded && !self.discardable() {
            return Err(Error::Invalid);
        }
        if let Some(binding) = &self.binding {
            binding.validate()?;
            // A launch that already declares its full native identity on the
            // command line never carries a separate operator binding.
            if self.declared_sid().is_some()
                || binding.spec.source != self.spec.source
                || !matches!(
                    self.state,
                    State::Running | State::Uncertain | State::CancelRequested | State::Exited
                )
            {
                return Err(Error::Invalid);
            }
        }
        if !nonce(&self.record_id)
            || !identifier(&self.request_id, 8, 128)
            || !nonce(&self.launch_id)
            || !nonce(&self.instance_id)
            || !self
                .host_name
                .strip_prefix("sessiondock-")
                .is_some_and(nonce)
            || self.revision == 0
            || (self.state == State::Failed) != self.failure.is_some()
            || (self.state == State::CancelRequested && !self.cancel_requested)
            || (self.cancel_requested
                && !matches!(
                    self.state,
                    State::CancelRequested | State::Uncertain | State::Exited
                ))
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
