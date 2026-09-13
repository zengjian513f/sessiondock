//! Private creation-intent types: `LaunchSpec`, `Record`, and `BindingSpec`.
//! No process launch, host I/O, or native-history writes. Specs are data, not
//! authority; serde cannot bypass cwd/identity checks. Limits are
//! `MAX_RECORDS`, `MAX_CWD_BYTES`, `MAX_ADAPTER_BYTES`, and `MAX_NATIVE_ID_BYTES`.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path},
};

use super::store::Error;

pub const MAX_RECORDS: usize = 128;
pub const MAX_CWD_BYTES: usize = 4096;
pub const MAX_ADAPTER_BYTES: usize = 64;
pub const MAX_NATIVE_ID_BYTES: usize = 256;
/// Bound of the private evidence note a process-evidence binding records.
pub const MAX_EVIDENCE_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Claude,
    Codex,
    Grok,
}

/// Private persisted intent data, not authority to bind a native session.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
        let valid = |value: &str| {
            !value.is_empty()
                && value.len() <= MAX_NATIVE_ID_BYTES
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
        };
        if !valid(&self.sid)
            || !valid(&self.uid)
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
/// Rust counterpart of Python's `new-status` resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingMethod {
    Operator,
    Process,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
        if self.evidence.as_deref().is_some_and(|note| {
            note.is_empty() || note.len() > MAX_EVIDENCE_BYTES || note.chars().any(char::is_control)
        }) {
            return Err(Error::Invalid);
        }
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
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
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
            // Claude new sessions always receive a server UUID; other sources
            // never receive one upfront (plan rule: no inferred Codex/Grok SID).
            Self::NewPending => source != Source::Claude,
            Self::NewAssigned => source == Source::Claude,
            Self::Resume { sid, uid } => native_sid(sid) && native_uid(source, uid),
        };
        if ok { Ok(()) } else { Err(Error::InvalidSpec) }
    }
}

/// Full native SID shape shared by all three CLIs: a lowercase RFC 4122 text
/// UUID. Anything else (spaces, slashes, options, display prefixes) is rejected
/// before it can reach a `{sid}` argument substitution.
pub fn native_sid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
            }
        })
}
/// Full local UID `source:` + nonempty identifier suffix, at most 256 bytes.
pub fn native_uid(source: Source, text: &str) -> bool {
    let prefix = match source {
        Source::Claude => "claude:",
        Source::Codex => "codex:",
        Source::Grok => "grok:",
    };
    text.len() <= MAX_NATIVE_ID_BYTES
        && text.strip_prefix(prefix).is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
        })
}

/// Only an adapter identity, an explicit working directory and a typed launch
/// kind; no arbitrary command, shell, arguments, environment, credential or
/// guessed native-session claim. Deliberately no Debug: cwd is private data.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
        let launch = if source == Source::Claude {
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
        if !identifier(&adapter_id, 1, MAX_ADAPTER_BYTES) {
            return Err(Error::InvalidSpec);
        }
        let text = cwd.to_str().ok_or(Error::InvalidSpec)?;
        if !valid_cwd(text) {
            return Err(Error::InvalidSpec);
        }
        for ancestor in cwd.ancestors() {
            let metadata = fs::symlink_metadata(ancestor).map_err(|_| Error::InvalidSpec)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(Error::InvalidSpec);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(Error::InvalidSpec);
                }
            }
        }
        let cwd = plain_canonical(cwd)
            .ok_or(Error::InvalidSpec)?
            .to_str()
            .ok_or(Error::InvalidSpec)?
            .to_owned();
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
        if !identifier(&self.adapter_id, 1, MAX_ADAPTER_BYTES) || !valid_cwd(&self.cwd) {
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
/// The canonical cwd in the spelling the CLI and the configured roots use.
/// Windows `canonicalize` answers `\\?\C:\…`; the launcher roots are written
/// `C:\…`, Node keeps whatever cwd it is started in and Claude derives its
/// project directory name from that, so a verbatim disk prefix is folded back
/// (WP-W). Other prefixes (UNC, devices) stay as they are and match no root.
fn plain_canonical(path: &Path) -> Option<std::path::PathBuf> {
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
fn valid_cwd(text: &str) -> bool {
    let path = Path::new(text);
    !text.is_empty()
        && text.len() <= MAX_CWD_BYTES
        && !text.chars().any(char::is_control)
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
#[serde(deny_unknown_fields)]
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
            || !self.host_name.strip_prefix("agenthub-").is_some_and(nonce)
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
