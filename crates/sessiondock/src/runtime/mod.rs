//! Read-only controlled-host observations against a frozen native inventory.
//! No native file reads, launch, cleanup, or control authority. External CLI
//! discovery lives next to it in `procscan` (explicit `/proc` scan)
//! and `spawn` (spawner recording); this module never reads it.
//!
//! Run state is evidence-based and instance-scoped: `running` needs a reachable
//! host reporting its child alive plus a verified process identity; `exited`
//! needs an explicit host exit, a durable exit receipt, or the disappearance of
//! an identity this service verified earlier; everything else stays `unknown`
//! with a typed reason. Sessions without any managed instance are never listed
//! and therefore never look stopped.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    sync::Arc,
    time::Duration,
};

use futures_util::{StreamExt, stream};
use ptyhost_client::{
    Association, AssociationState, HostClient, HostObservation, NativeBindingState, SessionSummary,
    Source,
};
use serde::Serialize;
use serde_json::Value;
use tokio::time::{Instant, timeout_at};

pub mod process;
pub mod procscan;
pub mod spawn;
use process::ProcClock;
pub use process::{IdentityFailure, ProcessIdentity, StartTime};

#[derive(Clone)]
struct NativeSession {
    uid: String,
    source: Source,
    sid: String,
    supported: bool,
    subagent: bool,
    scope: Option<crate::sessions::NativeScope>,
    identity_error: Option<AssociationReason>,
}

/// Immutable identity inventory. Only SessionStore/SessionSnapshot construct
/// verified catalogs from parsed native evidence. Public rows remain a legacy
/// compatibility input, never proof for the explicit native-binding protocol.
#[derive(Default)]
pub struct NativeCatalog {
    rows: Vec<NativeSession>,
    by_uid: BTreeMap<String, Vec<usize>>,
    by_sid: BTreeMap<(Source, String), Vec<usize>>,
    verified: bool,
    // Same-snapshot public identity compatibility, never binding evidence.
    legacy: Option<Box<NativeCatalog>>,
}

impl NativeCatalog {
    pub fn from_rows(rows: &[Value]) -> Self {
        let mut catalog = Self::default();
        for row in rows {
            let Some(source) = row["source"].as_str().and_then(Source::parse) else {
                continue;
            };
            let (Some(uid), Some(sid)) = (row["uid"].as_str(), row["sid"].as_str()) else {
                continue;
            };
            if !identifier(uid)
                || !identifier(sid)
                || !uid.starts_with(&format!("{}:", source.as_str()))
            {
                continue;
            }
            let index = catalog.rows.len();
            catalog.by_uid.entry(uid.into()).or_default().push(index);
            catalog
                .by_sid
                .entry((source, sid.into()))
                .or_default()
                .push(index);
            catalog.rows.push(NativeSession {
                uid: uid.into(),
                source,
                sid: sid.into(),
                // Existing public catalog always declares this flag. Unknown is
                // not a supported session when a different caller supplies rows.
                supported: row["supported"] == true,
                subagent: row["_is_subagent"] == true
                    || row["is_subagent"] == true
                    || row["agent_id"]
                        .as_str()
                        .is_some_and(|id| !id.is_empty() && id != "main"),
                scope: None,
                identity_error: None,
            });
        }
        catalog
    }

    pub(crate) fn from_native_entries(
        entries: Vec<crate::sessions::NativeCatalogEntry>,
        legacy_rows: &[Value],
    ) -> Self {
        let mut catalog = Self {
            verified: true,
            legacy: Some(Box::new(Self::from_rows(legacy_rows))),
            ..Self::default()
        };
        for entry in entries {
            let Some(source) = Source::parse(&entry.source) else {
                continue;
            };
            let scope = entry.scope.and_then(|scope| {
                if scope.source != entry.source
                    || scope.uid != entry.uid
                    || scope.agent_id.is_some()
                    || !identifier(&scope.session_id)
                    || !identifier(&scope.uid)
                {
                    return Err(crate::sessions::SessionError {
                        status: 501,
                        message: "原生范围不支持主会话绑定".into(),
                    });
                }
                Ok(scope)
            });
            let identity_error = if entry.declared_ids.len() > 1 {
                Some(AssociationReason::NativeConflict)
            } else {
                scope.as_ref().err().map(|error| {
                    if error.status == 409 {
                        AssociationReason::NativeConflict
                    } else {
                        AssociationReason::UnsupportedNative
                    }
                })
            };
            let scope = scope.ok();
            let index = catalog.rows.len();
            catalog
                .by_uid
                .entry(entry.uid.clone())
                .or_default()
                .push(index);
            // Claude child files legitimately repeat their owner's sessionId;
            // they remain UID-rejectable agents, not duplicate main sessions.
            if !(source == Source::Claude && entry.subagent) {
                for sid in entry.declared_ids {
                    catalog.by_sid.entry((source, sid)).or_default().push(index);
                }
            }
            catalog.rows.push(NativeSession {
                uid: entry.uid,
                source,
                sid: scope
                    .as_ref()
                    .map(|scope| scope.session_id.clone())
                    .unwrap_or_default(),
                supported: scope.is_some(),
                subagent: entry.subagent,
                scope,
                identity_error,
            });
        }
        catalog
    }

    /// A uniquely resolved, supported main-session scope proven by this exact
    /// native snapshot. Display-only catalogs can never return verified scopes.
    pub fn verified_scope(
        &self,
        uid: &str,
    ) -> Result<crate::sessions::NativeScope, AssociationReason> {
        if !self.verified {
            return Err(AssociationReason::UnverifiedCatalog);
        }
        let index = match self.by_uid.get(uid).map(Vec::as_slice) {
            Some([index]) => *index,
            Some(_) => return Err(AssociationReason::NativeAmbiguous),
            None => return Err(AssociationReason::NativeMissing),
        };
        let row = &self.rows[index];
        if row.subagent {
            return Err(AssociationReason::Subagent);
        }
        if let Some(reason) = row.identity_error {
            return Err(reason);
        }
        let row = self.match_identity(&Association {
            source: row.source,
            uid: Some(row.uid.clone()),
            sid: Some(row.sid.clone()),
        })?;
        row.scope
            .clone()
            .ok_or(AssociationReason::UnsupportedNative)
    }

    fn match_observation(
        &self,
        observation: &HostObservation,
    ) -> Result<&NativeSession, AssociationReason> {
        match &observation.native_binding {
            NativeBindingState::Bound(binding) => {
                let scope = self.verified_scope(binding.uid())?;
                if scope.source != binding.source().as_str() || scope.session_id != binding.sid() {
                    return Err(AssociationReason::NativeConflict);
                }
                self.match_identity(&Association {
                    source: binding.source(),
                    uid: Some(binding.uid().into()),
                    sid: Some(binding.sid().into()),
                })
            }
            NativeBindingState::Invalid => Err(AssociationReason::InvalidMetadata),
            // Legacy immutable metadata remains supported on a newer host that
            // advertises the binding extension but has no explicit binding.
            NativeBindingState::Unsupported | NativeBindingState::Unbound => {
                self.match_declared(&observation.association)
            }
        }
    }

    /// Declared legacy metadata resolved against the same-snapshot public
    /// rows. Used for verified observations and, separately, to name what an
    /// unreachable record claims; the latter is never association evidence.
    fn match_declared(
        &self,
        association: &AssociationState,
    ) -> Result<&NativeSession, AssociationReason> {
        match association {
            AssociationState::Missing => Err(AssociationReason::MissingMetadata),
            AssociationState::Invalid => Err(AssociationReason::InvalidMetadata),
            AssociationState::Declared(identity) => self
                .legacy
                .as_deref()
                .unwrap_or(self)
                .match_identity(identity),
        }
    }

    fn match_identity(
        &self,
        association: &Association,
    ) -> Result<&NativeSession, AssociationReason> {
        let exact = |indices: Option<&Vec<usize>>| match indices.map(Vec::as_slice) {
            None | Some([]) => Err(AssociationReason::NativeMissing),
            Some([index]) => Ok(*index),
            Some(_) => Err(AssociationReason::NativeAmbiguous),
        };
        let uid = association
            .uid
            .as_ref()
            .map(|uid| exact(self.by_uid.get(uid)))
            .transpose()?;
        let sid = association
            .sid
            .as_ref()
            .map(|sid| exact(self.by_sid.get(&(association.source, sid.clone()))))
            .transpose()?;
        if matches!((uid, sid), (Some(left), Some(right)) if left != right) {
            return Err(AssociationReason::NativeConflict);
        }
        let index = uid.or(sid).ok_or(AssociationReason::MissingMetadata)?;
        let row = &self.rows[index];
        if row.source != association.source {
            return Err(AssociationReason::NativeConflict);
        }
        if row.subagent {
            return Err(AssociationReason::Subagent);
        }
        if let Some(reason) = row.identity_error {
            return Err(reason);
        }
        if !row.supported {
            return Err(AssociationReason::UnsupportedNative);
        }
        Ok(row)
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Liveness {
    Running,
    Exited,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeFailure {
    Unreachable,
    Timeout,
    Authentication,
    InvalidProtocol,
    IdentityChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssociationReason {
    UnverifiedCatalog,
    MissingMetadata,
    InvalidMetadata,
    NativeMissing,
    NativeAmbiguous,
    NativeConflict,
    UnsupportedNative,
    Subagent,
    DuplicateHost,
    HostUnverified,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NativeAssociation {
    Matched {
        uid: String,
        source: Source,
        sid: String,
    },
    Unknown {
        reason: AssociationReason,
    },
}

/// Process identity evidence gathered for one host observation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProcessEvidence {
    /// Both the child and the host process match the identity captured when
    /// this instance was first observed (or were captured just now).
    Verified {
        child: ProcessIdentity,
        host: ProcessIdentity,
    },
    Unverifiable {
        reason: IdentityFailure,
    },
    /// The host reported child exit; a reaped child has no identity to read.
    Reaped,
    /// The host itself was not verified, so no process was read.
    Unchecked,
}

/// What an unreachable or unverified record declares. It is displayed so an
/// `unknown` row can name the session it claims; it proves nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeclaredIdentity {
    pub source: Source,
    pub sid: Option<String>,
    pub uid: Option<String>,
    pub instance_id: Option<String>,
}

/// A current read-only observation. Even a matched instance requires a fresh
/// exact identity check in the future control layer; name alone is not authority.
#[derive(Clone, Debug, Serialize)]
pub struct ManagedHost {
    pub summary: SessionSummary,
    pub known: bool,
    pub liveness: Liveness,
    pub association: NativeAssociation,
    pub instance_id: Option<String>,
    /// Missing launcher nonce is explicitly distinguished from a matched SID.
    pub identity_unverified: bool,
    pub instance_guard_v1: bool,
    #[serde(skip)]
    control: Option<ControlTarget>,
    pub probe_error: Option<ProbeFailure>,
    pub process: ProcessEvidence,
    /// Child process start as legacy Unix seconds, only with verified identity.
    pub started_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared: Option<DeclaredIdentity>,
}

#[derive(Clone)]
struct ControlTarget(ptyhost_client::BoundTarget);
impl fmt::Debug for ControlTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControlTarget")
            .finish_non_exhaustive()
    }
}

impl ManagedHost {
    /// Candidate only: the terminal service must freshly revalidate under its
    /// per-host gate and use guarded dispatch. Never serialize this as a lease.
    pub fn bound_target(&self) -> Option<&ptyhost_client::BoundTarget> {
        self.control.as_ref().map(|control| &control.0)
    }
    pub fn native_uid(&self) -> Option<&str> {
        match &self.association {
            NativeAssociation::Matched { uid, .. } => Some(uid),
            NativeAssociation::Unknown { .. } => None,
        }
    }

    fn unknown(
        summary: SessionSummary,
        error: ProbeFailure,
        declared: Option<DeclaredIdentity>,
    ) -> Self {
        Self {
            summary,
            known: false,
            liveness: Liveness::Unknown,
            association: NativeAssociation::Unknown {
                reason: AssociationReason::HostUnverified,
            },
            instance_id: None,
            identity_unverified: true,
            instance_guard_v1: false,
            control: None,
            probe_error: Some(error),
            process: ProcessEvidence::Unchecked,
            started_at: None,
            declared,
        }
    }

    fn instance_key(&self) -> InstanceKey {
        InstanceKey {
            name: self.summary.name.clone(),
            host_pid: self.summary.host_pid,
            pid: self.summary.pid,
            created: self.summary.created,
            instance_id: self.instance_id.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Exited,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEvidence {
    /// Host Info reports the child alive and its process identity verified.
    HostInfo,
    /// Host Info reports an explicit child exit.
    HostExit,
    /// A durable lifecycle exit receipt for this exact instance.
    ExitReceipt,
    /// The child identity verified earlier is gone from the process table
    /// while its host no longer answers: that exact incarnation ended.
    IdentityGone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownReason {
    /// No managed instance names this session. Never a stopped session.
    NoInstance,
    /// A record names this session but its host did not answer.
    HostUnreachable,
    /// A verified instance's record vanished while its process still exists.
    RecordMissing,
    DuplicateHost,
    /// The host answers but the process identity could not be verified.
    IdentityUnverifiable,
    /// This target has no process table support; nothing is inferred.
    PlatformUnsupported,
}

/// Instance-scoped run state for one native UID.
#[derive(Clone, Debug, Serialize)]
pub struct SessionRunState {
    pub state: RunState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<RunEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<UnknownReason>,
    pub host: Option<String>,
    pub instance_id: Option<String>,
    pub pid: Option<u32>,
    pub started_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_error: Option<ProbeFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<IdentityFailure>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct UnlistedState {
    pub state: RunState,
    pub reason: UnknownReason,
}

/// A durable lifecycle exit receipt bound to one native UID and instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitReceipt {
    pub uid: String,
    pub host: String,
    pub instance_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeSnapshot {
    /// The configured managed inventory was observed. External/unmanaged CLI
    /// discovery is not implemented, so absence from `sessions` is never a
    /// negative liveness assertion.
    pub known: bool,
    pub partial: bool,
    pub external_detection: &'static str,
    pub process_identity: &'static str,
    pub observed_at: f64,
    pub hosts: Vec<ManagedHost>,
    pub sessions: BTreeMap<String, SessionRunState>,
    pub unlisted: UnlistedState,
}

impl RuntimeSnapshot {
    pub fn running_uids(&self) -> Vec<&str> {
        self.sessions
            .iter()
            .filter(|(_, session)| session.state == RunState::Running)
            .map(|(uid, _)| uid.as_str())
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeLimits {
    pub parallel_probes: usize,
    pub probe_timeout: Duration,
    pub snapshot_timeout: Duration,
    /// Shared observations younger than this answer `/api/live` without probing.
    pub cache_ttl: Duration,
    /// Seconds a child may appear to start after its record's `created` stamp
    /// before the PID is treated as reused.
    pub record_slack: u64,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            parallel_probes: 8,
            probe_timeout: Duration::from_secs(2),
            snapshot_timeout: Duration::from_secs(5),
            cache_ttl: Duration::from_secs(2),
            record_slack: 5,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RuntimeError {
    InvalidLimits,
    DiscoveryFailed,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidLimits => "invalid controlled runtime limits",
            Self::DiscoveryFailed => "controlled host inventory unavailable",
        })
    }
}

impl std::error::Error for RuntimeError {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct InstanceKey {
    name: String,
    host_pid: u32,
    pid: u32,
    created: u64,
    instance_id: Option<String>,
}

#[derive(Clone, Debug)]
struct Captured {
    child: ProcessIdentity,
    host: ProcessIdentity,
    /// The exact UID matched when captured. Only such entries can later show
    /// an exit; unassociated instances are forgotten as soon as they vanish.
    uid: Option<String>,
    last_seen: Instant,
    gone: bool,
}

#[derive(Default)]
struct IdentityMemory {
    entries: HashMap<InstanceKey, Captured>,
}

impl IdentityMemory {
    fn remember(&mut self, key: InstanceKey, captured: Captured) {
        if let Some(uid) = &captured.uid {
            // A newly verified instance supersedes an ended one for its UID.
            self.entries.retain(|other, entry| {
                entry.uid.as_ref() != Some(uid) || !entry.gone || other == &key
            });
        }
        self.entries.insert(key, captured);
    }
}

/// A fresh or reused shared observation for display endpoints. Control paths
/// must keep using `observe`, which never reads this cache.
pub struct SharedObservation {
    pub snapshot: Arc<RuntimeSnapshot>,
    pub age: Duration,
    pub cached: bool,
}

#[derive(Debug)]
pub enum SharedError<E> {
    Prepare(E),
    Runtime(RuntimeError),
}

struct CachedSnapshot {
    observed: Instant,
    snapshot: Arc<RuntimeSnapshot>,
}

pub struct ManagedRuntime {
    client: HostClient,
    limits: RuntimeLimits,
    clock: tokio::sync::OnceCell<Option<ProcClock>>,
    memory: std::sync::Mutex<IdentityMemory>,
    refresh: tokio::sync::Mutex<()>,
    cached: std::sync::Mutex<Option<CachedSnapshot>>,
}

struct Candidate {
    rank: u8,
    key: Option<InstanceKey>,
    state: SessionRunState,
}

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| (elapsed.as_secs_f64() * 1000.0).round() / 1000.0)
        .unwrap_or(0.0)
}

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl ManagedRuntime {
    pub fn new(client: HostClient, limits: RuntimeLimits) -> Result<Self, RuntimeError> {
        if limits.parallel_probes == 0
            || limits.probe_timeout.is_zero()
            || limits.snapshot_timeout.is_zero()
        {
            return Err(RuntimeError::InvalidLimits);
        }
        Ok(Self {
            client,
            limits,
            clock: tokio::sync::OnceCell::new(),
            memory: std::sync::Mutex::default(),
            refresh: tokio::sync::Mutex::new(()),
            cached: std::sync::Mutex::new(None),
        })
    }

    pub fn limits(&self) -> &RuntimeLimits {
        &self.limits
    }

    async fn clock(&self) -> Option<ProcClock> {
        *self.clock.get_or_init(process::load_clock).await
    }

    /// The last shared observation if it is younger than the cache TTL.
    pub fn cached(&self) -> Option<SharedObservation> {
        let cached = lock(&self.cached);
        let entry = cached.as_ref()?;
        let age = entry.observed.elapsed();
        (age < self.limits.cache_ttl).then(|| SharedObservation {
            snapshot: entry.snapshot.clone(),
            age,
            cached: true,
        })
    }

    /// Single-flight shared observation: concurrent callers wait for one
    /// refresh and reuse it; `force` bypasses the TTL but still serializes.
    /// `prepare` runs only when a refresh is really needed, so it may hold
    /// admission and freeze the native catalog; its guard lives through the
    /// probe and is dropped before the result is published.
    pub async fn observe_shared<T, E>(
        &self,
        force: bool,
        prepare: impl AsyncFnOnce() -> Result<(T, NativeCatalog, Vec<ExitReceipt>), E>,
    ) -> Result<SharedObservation, SharedError<E>> {
        if !force && let Some(hit) = self.cached() {
            return Ok(hit);
        }
        let _refresh = self.refresh.lock().await;
        if !force && let Some(hit) = self.cached() {
            return Ok(hit);
        }
        let (guard, catalog, receipts) = prepare().await.map_err(SharedError::Prepare)?;
        let snapshot = Arc::new(
            self.observe_with(&catalog, &receipts)
                .await
                .map_err(SharedError::Runtime)?,
        );
        drop(guard);
        *lock(&self.cached) = Some(CachedSnapshot {
            observed: Instant::now(),
            snapshot: snapshot.clone(),
        });
        Ok(SharedObservation {
            snapshot,
            age: Duration::ZERO,
            cached: false,
        })
    }

    /// A fresh observation. Never served from the shared cache: ownership
    /// claims must not be authorized by a stale cross-request snapshot.
    pub async fn observe(&self, catalog: &NativeCatalog) -> Result<RuntimeSnapshot, RuntimeError> {
        self.observe_with(catalog, &[]).await
    }

    pub async fn observe_with(
        &self,
        catalog: &NativeCatalog,
        receipts: &[ExitReceipt],
    ) -> Result<RuntimeSnapshot, RuntimeError> {
        let deadline = Instant::now() + self.limits.snapshot_timeout;
        let records = timeout_at(deadline, self.client.discover())
            .await
            .map_err(|_| RuntimeError::DiscoveryFailed)?
            .map_err(|_| RuntimeError::DiscoveryFailed)?;
        let clock = self.clock().await;
        let record_names: HashSet<String> = records.iter().map(|row| row.name.clone()).collect();
        let mut hosts: Vec<ManagedHost> = stream::iter(records)
            .map(|summary| async move {
                if Instant::now() >= deadline {
                    return ManagedHost::unknown(summary, ProbeFailure::Timeout, None);
                }
                let end = deadline.min(Instant::now() + self.limits.probe_timeout);
                match timeout_at(end, self.client.probe(&summary.name)).await {
                    Ok(Ok(observed)) => self.observed_host(catalog, observed, clock.as_ref()).await,
                    Ok(Err(error)) => {
                        let failure = match error {
                            ptyhost_client::Error::Timeout => ProbeFailure::Timeout,
                            ptyhost_client::Error::Rejected => ProbeFailure::Authentication,
                            ptyhost_client::Error::IdentityChanged => ProbeFailure::IdentityChanged,
                            ptyhost_client::Error::Io(_) | ptyhost_client::Error::NotFound => {
                                ProbeFailure::Unreachable
                            }
                            _ => ProbeFailure::InvalidProtocol,
                        };
                        let declared = self.declared(&summary.name, end).await;
                        ManagedHost::unknown(summary, failure, declared)
                    }
                    Err(_) => {
                        let declared = self.declared(&summary.name, deadline).await;
                        ManagedHost::unknown(summary, ProbeFailure::Timeout, declared)
                    }
                }
            })
            .buffer_unordered(self.limits.parallel_probes)
            .collect()
            .await;
        let mut counts = BTreeMap::<String, usize>::new();
        for host in &hosts {
            if let Some(uid) = host.native_uid() {
                *counts.entry(uid.into()).or_default() += 1
            }
        }
        let mut candidates: Vec<(String, Candidate)> = Vec::new();
        for host in &mut hosts {
            if host
                .native_uid()
                .is_some_and(|uid| counts.get(uid).copied().unwrap_or(0) > 1)
            {
                let uid = host.native_uid().expect("checked").to_owned();
                candidates.push((
                    uid,
                    Candidate {
                        rank: 2,
                        key: Some(host.instance_key()),
                        state: SessionRunState {
                            state: RunState::Unknown,
                            evidence: None,
                            reason: Some(UnknownReason::DuplicateHost),
                            host: Some(host.summary.name.clone()),
                            instance_id: host.instance_id.clone(),
                            pid: Some(host.summary.pid),
                            started_at: None,
                            probe_error: None,
                            identity: None,
                        },
                    },
                ));
                host.association = NativeAssociation::Unknown {
                    reason: AssociationReason::DuplicateHost,
                };
                host.control = None;
            }
        }
        hosts.sort_by(|a, b| a.summary.name.cmp(&b.summary.name));
        let mut seen = HashSet::new();
        for host in &hosts {
            if host.known {
                seen.insert(host.instance_key());
            }
            if let Some(candidate) = host_candidate(host, catalog) {
                candidates.push(candidate);
            }
        }
        candidates.extend(
            self.memory_candidates(&seen, &record_names, deadline, clock.as_ref())
                .await,
        );
        for receipt in receipts {
            candidates.push((
                receipt.uid.clone(),
                Candidate {
                    rank: 6,
                    key: None,
                    state: SessionRunState {
                        state: RunState::Exited,
                        evidence: Some(RunEvidence::ExitReceipt),
                        reason: None,
                        host: Some(receipt.host.clone()),
                        instance_id: Some(receipt.instance_id.clone()),
                        pid: None,
                        started_at: None,
                        probe_error: None,
                        identity: None,
                    },
                },
            ));
        }
        Ok(RuntimeSnapshot {
            known: true,
            partial: true,
            external_detection: "not_implemented",
            process_identity: process::PLATFORM,
            observed_at: unix_now(),
            hosts,
            sessions: fold_sessions(candidates),
            unlisted: UnlistedState {
                state: RunState::Unknown,
                reason: UnknownReason::NoInstance,
            },
        })
    }

    async fn declared(&self, name: &str, end: Instant) -> Option<DeclaredIdentity> {
        if Instant::now() >= end {
            return None;
        }
        let record = timeout_at(end, self.client.declared(name))
            .await
            .ok()?
            .ok()??;
        match record.association {
            AssociationState::Declared(identity) => Some(DeclaredIdentity {
                source: identity.source,
                sid: identity.sid,
                uid: identity.uid,
                instance_id: record.instance_id,
            }),
            _ => None,
        }
    }

    async fn observed_host(
        &self,
        catalog: &NativeCatalog,
        observed: HostObservation,
        clock: Option<&ProcClock>,
    ) -> ManagedHost {
        let association = catalog.match_observation(&observed);
        let control = match &association {
            Ok(row) => ptyhost_client::BoundTarget::from_observation(
                &observed, row.source, &row.sid, &row.uid,
            )
            .ok()
            .map(ControlTarget),
            Err(_) => None,
        };
        let uid = association.as_ref().ok().map(|row| row.uid.clone());
        let key = InstanceKey {
            name: observed.summary.name.clone(),
            host_pid: observed.summary.host_pid,
            pid: observed.summary.pid,
            created: observed.summary.created,
            instance_id: observed.instance_id.clone(),
        };
        let process = if observed.exited {
            ProcessEvidence::Reaped
        } else {
            self.identify(key, uid, clock).await
        };
        let started_at = match (&process, clock) {
            (ProcessEvidence::Verified { child, .. }, Some(clock)) => {
                Some(clock.unix_seconds(child.start_time))
            }
            _ => None,
        };
        ManagedHost {
            summary: observed.summary,
            known: true,
            liveness: if observed.exited {
                Liveness::Exited
            } else {
                Liveness::Running
            },
            association: match association {
                Ok(row) => NativeAssociation::Matched {
                    uid: row.uid.clone(),
                    source: row.source,
                    sid: row.sid.clone(),
                },
                Err(reason) => NativeAssociation::Unknown { reason },
            },
            identity_unverified: observed.instance_id.is_none(),
            instance_guard_v1: observed.instance_guard_v1,
            control,
            instance_id: observed.instance_id,
            probe_error: None,
            process,
            started_at,
            declared: None,
        }
    }

    /// Capture the identity of a newly seen instance or verify a remembered
    /// one. The memory lock is never held across a `/proc` read.
    async fn identify(
        &self,
        key: InstanceKey,
        uid: Option<String>,
        clock: Option<&ProcClock>,
    ) -> ProcessEvidence {
        let expected = lock(&self.memory)
            .entries
            .get(&key)
            .map(|entry| (entry.child, entry.host));
        match expected {
            Some((child, host)) => {
                let observed_child = process::observe(key.pid, false).await;
                let observed_host = process::observe(key.host_pid, false).await;
                let result = match (observed_child, observed_host) {
                    (Ok(now_child), Ok(now_host)) => child
                        .verify(&now_child)
                        .and_then(|()| host.verify(&now_host)),
                    (Err(reason), _) | (_, Err(reason)) => Err(reason),
                };
                match result {
                    Ok(()) => {
                        let mut memory = lock(&self.memory);
                        if let Some(entry) = memory.entries.get_mut(&key) {
                            entry.last_seen = Instant::now();
                            entry.gone = false;
                            if uid.is_some() {
                                entry.uid = uid;
                            }
                        }
                        ProcessEvidence::Verified { child, host }
                    }
                    Err(reason) => ProcessEvidence::Unverifiable { reason },
                }
            }
            None => {
                let child = match process::observe(key.pid, true).await {
                    Ok(child) => child,
                    Err(reason) => return ProcessEvidence::Unverifiable { reason },
                };
                let host = match process::observe(key.host_pid, true).await {
                    Ok(host) => host,
                    Err(reason) => return ProcessEvidence::Unverifiable { reason },
                };
                if let Some(clock) = clock
                    && key.created > 0
                    && child.start_time
                        > clock.latest_start_for(key.created, self.limits.record_slack)
                {
                    return ProcessEvidence::Unverifiable {
                        reason: IdentityFailure::StartedAfterRecord,
                    };
                }
                lock(&self.memory).remember(
                    key,
                    Captured {
                        child,
                        host,
                        uid,
                        last_seen: Instant::now(),
                        gone: false,
                    },
                );
                ProcessEvidence::Verified { child, host }
            }
        }
    }

    /// Remembered instances that did not answer this round. An associated
    /// instance whose verified child is gone proves that incarnation ended;
    /// one whose process still exists stays unknown. Unassociated entries are
    /// dropped: nothing can be said about a session from them.
    async fn memory_candidates(
        &self,
        seen: &HashSet<InstanceKey>,
        record_names: &HashSet<String>,
        deadline: Instant,
        clock: Option<&ProcClock>,
    ) -> Vec<(String, Candidate)> {
        let stale: Vec<(InstanceKey, Captured)> = {
            let mut memory = lock(&self.memory);
            memory
                .entries
                .retain(|key, entry| seen.contains(key) || entry.uid.is_some());
            memory
                .entries
                .iter()
                .filter(|(key, _)| !seen.contains(*key))
                .map(|(key, entry)| (key.clone(), entry.clone()))
                .collect()
        };
        let checked: Vec<(InstanceKey, Captured, Result<bool, IdentityFailure>)> =
            stream::iter(stale)
                .map(|(key, entry)| async move {
                    let result = if entry.gone {
                        Ok(true)
                    } else if Instant::now() >= deadline {
                        Err(IdentityFailure::Unreadable)
                    } else {
                        match timeout_at(deadline, process::observe(key.pid, false)).await {
                            Ok(Ok(now)) => Ok(entry.child.verify(&now).is_err()),
                            Ok(Err(IdentityFailure::NotVisible)) => Ok(true),
                            Ok(Err(reason)) => Err(reason),
                            Err(_) => Err(IdentityFailure::Unreadable),
                        }
                    };
                    (key, entry, result)
                })
                .buffer_unordered(self.limits.parallel_probes)
                .collect()
                .await;
        let mut memory = lock(&self.memory);
        let mut candidates = Vec::new();
        for (key, entry, result) in checked {
            let Some(uid) = entry.uid.clone() else {
                continue;
            };
            let gone = matches!(result, Ok(true));
            if let Some(stored) = memory.entries.get_mut(&key) {
                stored.gone = stored.gone || gone;
            }
            let (rank, state, evidence, reason, identity) = match result {
                Ok(true) => (
                    3,
                    RunState::Exited,
                    Some(RunEvidence::IdentityGone),
                    None,
                    None,
                ),
                Ok(false) if record_names.contains(&key.name) => (
                    4,
                    RunState::Unknown,
                    None,
                    Some(UnknownReason::HostUnreachable),
                    None,
                ),
                Ok(false) => (
                    5,
                    RunState::Unknown,
                    None,
                    Some(UnknownReason::RecordMissing),
                    None,
                ),
                Err(failure) => (
                    5,
                    RunState::Unknown,
                    None,
                    Some(UnknownReason::IdentityUnverifiable),
                    Some(failure),
                ),
            };
            candidates.push((
                uid,
                Candidate {
                    rank,
                    key: Some(key.clone()),
                    state: SessionRunState {
                        state,
                        evidence,
                        reason,
                        host: Some(key.name.clone()),
                        instance_id: key.instance_id.clone(),
                        pid: Some(key.pid),
                        started_at: clock.map(|clock| clock.unix_seconds(entry.child.start_time)),
                        probe_error: None,
                        identity,
                    },
                },
            ));
        }
        candidates
    }
}

fn host_candidate(host: &ManagedHost, catalog: &NativeCatalog) -> Option<(String, Candidate)> {
    let key = Some(host.instance_key());
    let base = |state, evidence, reason, identity| SessionRunState {
        state,
        evidence,
        reason,
        host: Some(host.summary.name.clone()),
        instance_id: host.instance_id.clone(),
        pid: Some(host.summary.pid),
        started_at: host.started_at,
        probe_error: host.probe_error,
        identity,
    };
    if let Some(uid) = host.native_uid() {
        let candidate = match (host.liveness, &host.process) {
            (Liveness::Exited, _) => Candidate {
                rank: 1,
                key,
                state: base(RunState::Exited, Some(RunEvidence::HostExit), None, None),
            },
            (Liveness::Running, ProcessEvidence::Verified { .. }) => Candidate {
                rank: 0,
                key,
                state: base(RunState::Running, Some(RunEvidence::HostInfo), None, None),
            },
            (
                _,
                ProcessEvidence::Unverifiable {
                    reason: IdentityFailure::UnsupportedPlatform,
                },
            ) => Candidate {
                rank: 2,
                key,
                state: base(
                    RunState::Unknown,
                    None,
                    Some(UnknownReason::PlatformUnsupported),
                    Some(IdentityFailure::UnsupportedPlatform),
                ),
            },
            (_, ProcessEvidence::Unverifiable { reason }) => Candidate {
                rank: 2,
                key,
                state: base(
                    RunState::Unknown,
                    None,
                    Some(UnknownReason::IdentityUnverifiable),
                    Some(*reason),
                ),
            },
            // A matched host is always verified by Info, so these remain
            // defensive: no run state is claimed without process evidence.
            (Liveness::Unknown | Liveness::Running, _) => Candidate {
                rank: 2,
                key,
                state: base(
                    RunState::Unknown,
                    None,
                    Some(UnknownReason::IdentityUnverifiable),
                    None,
                ),
            },
        };
        return Some((uid.to_owned(), candidate));
    }
    // An unreachable record only names the session it claims.
    let declared = host.declared.as_ref()?;
    let row = catalog
        .match_declared(&AssociationState::Declared(Association {
            source: declared.source,
            sid: declared.sid.clone(),
            uid: declared.uid.clone(),
        }))
        .ok()?;
    Some((
        row.uid.clone(),
        Candidate {
            rank: 4,
            key: Some(InstanceKey {
                name: host.summary.name.clone(),
                host_pid: host.summary.host_pid,
                pid: host.summary.pid,
                created: host.summary.created,
                instance_id: declared.instance_id.clone(),
            }),
            state: SessionRunState {
                state: RunState::Unknown,
                evidence: None,
                reason: Some(UnknownReason::HostUnreachable),
                host: Some(host.summary.name.clone()),
                instance_id: declared.instance_id.clone(),
                pid: Some(host.summary.pid),
                started_at: None,
                probe_error: host.probe_error,
                identity: None,
            },
        },
    ))
}

/// Precedence per UID. Lower rank is stronger: a verified running instance
/// (0), an explicit host exit (1), a reachable-but-unverifiable current
/// instance (2), an ended remembered identity (3), an unreachable record (4),
/// a vanished record with a live process (5), a durable exit receipt (6).
/// Ranks 3 and 6 only apply against records/memories of the same instance:
/// a different current instance keeps the session unknown.
fn fold_sessions(candidates: Vec<(String, Candidate)>) -> BTreeMap<String, SessionRunState> {
    let mut grouped: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
    for (uid, candidate) in candidates {
        grouped.entry(uid).or_default().push(candidate);
    }
    grouped
        .into_iter()
        .map(|(uid, mut candidates)| {
            candidates.sort_by_key(|candidate| candidate.rank);
            let chosen = select(candidates);
            (uid, chosen)
        })
        .collect()
}

fn select(candidates: Vec<Candidate>) -> SessionRunState {
    let same_instance = |left: &Candidate, right: &Candidate| match (&left.key, &right.key) {
        (Some(a), Some(b)) => a == b,
        _ => left.state.instance_id.is_some() && left.state.instance_id == right.state.instance_id,
    };
    let strongest = candidates.iter().map(|candidate| candidate.rank).min();
    match strongest {
        Some(0..=2) | None => {}
        Some(3) => {
            let gone = candidates
                .iter()
                .find(|candidate| candidate.rank == 3)
                .expect("rank");
            if let Some(newer) = candidates.iter().find(|candidate| {
                candidate.rank >= 4 && candidate.rank <= 5 && !same_instance(gone, candidate)
            }) {
                return newer.state.clone();
            }
        }
        Some(4..=5) => {
            let current = candidates
                .iter()
                .find(|candidate| candidate.rank <= 5)
                .expect("rank");
            if let Some(receipt) = candidates.iter().find(|candidate| {
                candidate.rank == 6 && current.rank == 4 && same_instance(current, candidate)
            }) {
                return receipt.state.clone();
            }
        }
        Some(_) => {}
    }
    candidates
        .into_iter()
        .next()
        .map(|candidate| candidate.state)
        .expect("nonempty candidate group")
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod native_binding_tests;
