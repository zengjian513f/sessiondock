//! Exclusive browser terminal leases, independent of host and WebSocket I/O.
//!
//! A random server token authorizes one page's reservation. `bind` consumes that
//! reservation once and assigns a server-side connection identity. Every input
//! write and cleanup must use the resulting `BoundLease`, never just a name/IP.
//! Replacements are published before the old connection is signalled; the old
//! connection's cleanup therefore cannot remove the replacement.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ptyhost_client::{BoundTarget, LaunchTarget};
use serde::Serialize;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::sync::watch;

pub const RESERVATION_TTL: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnershipError {
    InvalidName,
    InvalidPage,
    InvalidToken,
    InvalidClock,
    EntropyUnavailable,
    NotOwner,
    /// HTTP input without any current lease for that terminal name.
    NoLease,
    /// HTTP input with a credential that a newer claim has replaced.
    Revoked,
    AlreadyBound,
    BindingMismatch,
    LaunchRetired,
    ConnectionIdsExhausted,
    Unavailable,
}

impl OwnershipError {
    pub fn status(self) -> u16 {
        match self {
            Self::InvalidName | Self::InvalidPage => 400,
            Self::NoLease => 403,
            Self::Revoked => 409,
            Self::InvalidToken
            | Self::NotOwner
            | Self::AlreadyBound
            | Self::BindingMismatch
            | Self::LaunchRetired => 409,
            Self::InvalidClock
            | Self::EntropyUnavailable
            | Self::ConnectionIdsExhausted
            | Self::Unavailable => 503,
        }
    }
}

impl fmt::Display for OwnershipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidName => "终端名称无效",
            Self::InvalidPage => "页面标识无效",
            Self::InvalidToken => "终端控制凭证格式无效",
            Self::InvalidClock => "终端控制权时钟不可用",
            Self::EntropyUnavailable => "安全随机数暂不可用",
            Self::NotOwner => "终端控制权已失效，请重新预约",
            Self::NoLease => "终端控制权已失效，请重新预约",
            Self::Revoked => "终端控制权已被其他页面接管或重新预约，本页输入已拒绝",
            Self::AlreadyBound => "终端控制权已绑定其他连接",
            Self::BindingMismatch => "终端租约的会话或实例绑定不匹配，不能降级为普通连接",
            Self::LaunchRetired => "该启动实例已撤销终端授权，不能重新预约",
            Self::ConnectionIdsExhausted => "终端连接标识已耗尽",
            Self::Unavailable => "终端控制权状态不可用",
        })
    }
}

impl std::error::Error for OwnershipError {}

/// Portable ASCII subset of the ptyhost single-component name contract.
/// Do not truncate or normalize: distinct request strings must not alias.
pub fn validate_name(name: &str) -> Result<(), OwnershipError> {
    if name.is_empty()
        || name.len() > 128
        || name.starts_with('.')
        || name.ends_with('.')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
    {
        return Err(OwnershipError::InvalidName);
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(OwnershipError::InvalidName);
    }
    Ok(())
}

pub fn validate_page(page: &str) -> Result<(), OwnershipError> {
    if page.is_empty() || page.chars().count() > 128 {
        return Err(OwnershipError::InvalidPage);
    }
    Ok(())
}

/// `monotonic` controls deadlines; wall time is display-only, just like IP.
#[derive(Clone, Copy, Debug)]
pub struct ClockSample {
    pub monotonic: Duration,
    pub unix_seconds: f64,
}

pub trait Clock: Send + Sync {
    fn now(&self) -> ClockSample;
}

struct SystemClock {
    origin: Instant,
}

impl Clock for SystemClock {
    fn now(&self) -> ClockSample {
        ClockSample {
            monotonic: self.origin.elapsed(),
            unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
        }
    }
}

/// Who is claiming, as the takeover prompts describe it: the display
/// address (`api::terminal::claimant_ip`) and the coarse device label from
/// the browser's `User-Agent` (`device::device_label`), empty when unknown.
/// Neither is identity; a bare address is an unlabeled claimant such as the
/// server's own delivery lease.
#[derive(Clone, Debug, PartialEq)]
pub struct Claimant {
    pub ip: IpAddr,
    pub label: String,
}

impl From<IpAddr> for Claimant {
    fn from(ip: IpAddr) -> Self {
        Self {
            ip,
            label: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PublicOwner {
    pub ip: IpAddr,
    /// Device label of the holder, `""` when its client gave none.
    pub label: String,
    pub since: f64,
}

impl PublicOwner {
    /// `"<label>，<ip>"` / `"<ip>"` for messages that name the holder.
    pub fn describe(&self) -> String {
        if self.label.is_empty() {
            self.ip.to_string()
        } else {
            format!("{}，{}", self.label, self.ip)
        }
    }
}

// Intentionally not Debug or Serialize. The only string exposure is in an
// explicit successful ClaimResponse::into_api_json call for the claimant.
#[derive(Clone)]
struct LeaseToken([u8; 32]);

impl LeaseToken {
    fn generate() -> Result<Self, OwnershipError> {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).map_err(|_| OwnershipError::EntropyUnavailable)?;
        Ok(Self(bytes))
    }

    fn parse(token: &str) -> Result<Self, OwnershipError> {
        if token.len() != 64 {
            return Err(OwnershipError::InvalidToken);
        }
        let mut bytes = [0; 32];
        for (index, pair) in token.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            let nibble = |byte| match byte {
                b'0'..=b'9' => Ok(byte - b'0'),
                b'a'..=b'f' => Ok(byte - b'a' + 10),
                _ => Err(OwnershipError::InvalidToken),
            };
            bytes[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Ok(Self(bytes))
    }

    fn matches(&self, other: &Self) -> bool {
        bool::from(self.0.ct_eq(&other.0))
    }

    fn expose_for_claim(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut token = String::with_capacity(64);
        for byte in self.0 {
            token.push(HEX[(byte >> 4) as usize] as char);
            token.push(HEX[(byte & 15) as usize] as char);
        }
        token
    }
}

/// A granted claim holds a secret. Do not add Debug/Serialize to this type.
pub enum ClaimResponse {
    Granted(GrantedClaim),
    Conflict(PublicOwner),
}

pub struct GrantedClaim {
    token: LeaseToken,
    owner: PublicOwner,
}

impl ClaimResponse {
    pub fn status(&self) -> u16 {
        match self {
            Self::Granted(_) => 200,
            Self::Conflict(_) => 409,
        }
    }

    pub fn owner(&self) -> &PublicOwner {
        match self {
            Self::Granted(claim) => &claim.owner,
            Self::Conflict(owner) => owner,
        }
    }

    /// The server is itself a claimant (a server-held lease around one
    /// send). Like `into_api_json`, this is the only other
    /// way the secret leaves the registry; it is consumed, never logged.
    pub fn into_server_token(self) -> Result<String, PublicOwner> {
        match self {
            Self::Granted(claim) => Ok(claim.token.expose_for_claim()),
            Self::Conflict(owner) => Err(owner),
        }
    }

    /// Explicitly expose only this claimant's browser lease token, never a
    /// ptyhost credential. HTTP handlers must mark this response `no-store`.
    pub fn into_api_json(self) -> Value {
        match self {
            Self::Granted(claim) => {
                json!({"ok":true,"token":claim.token.expose_for_claim(),"owner":claim.owner})
            }
            Self::Conflict(owner) => json!({"conflict":true,"owner":owner}),
        }
    }
}

/// Allocated by the registry, never accepted from a browser. Not an auth token.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ConnectionId(u64);

impl ConnectionId {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevocationReason {
    Replaced,
    Released,
    LaunchRetired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Revocation {
    pub reason: RevocationReason,
    /// Display-only address of the new claimant; absent on explicit release,
    /// and on replacement from the old page's own address (through the hub
    /// every page of one user shares it, so the label would say nothing).
    pub new_ip: Option<IpAddr>,
    /// Device label of the new claimant, `""` when unknown or on release.
    pub new_label: String,
    /// Same-page reconnects still stop the old transport but are silent in UI.
    pub notify: bool,
}

/// Internal authority kind; native and launch targets cannot substitute for
/// each other or be consumed by the legacy raw-name path.
#[derive(Clone)]
pub(super) enum LeaseTarget {
    Raw,
    Native(Arc<BoundTarget>),
    Launch(Arc<LaunchTarget>),
}

impl LeaseTarget {
    fn launch_key(&self) -> Option<LaunchKey> {
        match self {
            Self::Launch(target) => Some(LaunchKey::from(target.as_ref())),
            Self::Native(target) => target.origin_launch_id().map(|launch| LaunchKey {
                name: target.name().into(),
                source: target.source().as_str().into(),
                launch: launch.into(),
                instance: target.instance_id().into(),
            }),
            Self::Raw => None,
        }
    }
    fn same_kind(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Raw, Self::Raw)
                | (Self::Native(_), Self::Native(_))
                | (Self::Launch(_), Self::Launch(_))
        )
    }
}

/// The caller's declared lease kind and identity. It must equal the pinned
/// lease target exactly; no kind can substitute for another.
#[derive(Clone, Copy)]
pub enum ExpectedTarget<'a> {
    Raw,
    Native { uid: &'a str, instance: &'a str },
    Launch { launch: &'a str, instance: &'a str },
}

impl LeaseTarget {
    fn accepts(&self, expected: ExpectedTarget<'_>) -> bool {
        match (self, expected) {
            (Self::Raw, ExpectedTarget::Raw) => true,
            (Self::Native(target), ExpectedTarget::Native { uid, instance }) => {
                target.uid() == uid && target.instance_id() == instance
            }
            (Self::Launch(target), ExpectedTarget::Launch { launch, instance }) => {
                target.launch_id() == launch && target.instance_id() == instance
            }
            _ => false,
        }
    }
}

#[derive(Eq, PartialEq, Ord, PartialOrd)]
struct LaunchKey {
    name: String,
    source: String,
    launch: String,
    instance: String,
}

impl From<&LaunchTarget> for LaunchKey {
    fn from(target: &LaunchTarget) -> Self {
        Self {
            name: target.name().into(),
            source: target.source().as_str().into(),
            launch: target.launch_id().into(),
            instance: target.instance_id().into(),
        }
    }
}

/// Opaque bound credentials. Intentionally not Clone, Debug, or Serialize.
/// Retain this for current-owner checks and cleanup even if host attach fails.
pub struct BoundLease {
    name: String,
    page: String,
    token: LeaseToken,
    connection_id: ConnectionId,
    revoked: watch::Receiver<Option<Revocation>>,
    target: LeaseTarget,
}

impl BoundLease {
    pub(super) fn lease_target(&self) -> &LeaseTarget {
        &self.target
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn connection_id(&self) -> ConnectionId {
        self.connection_id
    }

    /// Clone the signal receiver for a WS select loop. Check its current value
    /// before waiting; a replacement may already have occurred after bind.
    /// Drop watch borrow guards before doing any I/O or awaiting anything.
    pub fn revocations(&self) -> watch::Receiver<Option<Revocation>> {
        self.revoked.clone()
    }
}

struct Binding {
    connection_id: ConnectionId,
    revoked: watch::Sender<Option<Revocation>>,
}

struct Lease {
    page: String,
    token: LeaseToken,
    owner: PublicOwner,
    reservation_deadline: Duration,
    binding: Option<Binding>,
    target: LeaseTarget,
}

#[derive(Default)]
struct State {
    leases: BTreeMap<String, Lease>,
    retired: BTreeSet<LaunchKey>,
    last_tick: Duration,
    next_connection: u64,
}

impl State {
    fn advance(&mut self, sample: ClockSample) -> (Duration, usize) {
        // Concurrent callers can sample before another caller takes the lock.
        // Never move the logical clock backwards or resurrect a reservation.
        let now = self.last_tick.max(sample.monotonic);
        self.last_tick = now;
        let before = self.leases.len();
        self.leases
            .retain(|_, lease| lease.binding.is_some() || now < lease.reservation_deadline);
        (now, before - self.leases.len())
    }
}

pub struct Registry {
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
}

impl Registry {
    pub fn new() -> Result<Self, OwnershipError> {
        Self::with_clock(Arc::new(SystemClock {
            origin: Instant::now(),
        }))
    }

    /// A supplied clock must be cheap and non-blocking. It is sampled outside
    /// the mutex; monotonic regressions are clamped under the mutex.
    pub fn with_clock(clock: Arc<dyn Clock>) -> Result<Self, OwnershipError> {
        Ok(Self {
            clock,
            state: Mutex::new(State::default()),
        })
    }

    fn sample(&self) -> Result<ClockSample, OwnershipError> {
        let sample = self.clock.now();
        if !sample.unix_seconds.is_finite() || sample.unix_seconds < 0.0 {
            return Err(OwnershipError::InvalidClock);
        }
        Ok(sample)
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>, OwnershipError> {
        self.state.lock().map_err(|_| OwnershipError::Unavailable)
    }

    /// Reserves a known host name. The caller must separately validate host
    /// inventory membership and HTTP authorization before calling this method.
    pub fn claim(
        &self,
        name: &str,
        page: &str,
        claimant: impl Into<Claimant>,
        force: bool,
    ) -> Result<ClaimResponse, OwnershipError> {
        self.claim_target(name, page, claimant.into(), force, LeaseTarget::Raw)
    }

    pub(super) fn claim_bound(
        &self,
        target: Arc<BoundTarget>,
        page: &str,
        claimant: impl Into<Claimant>,
        force: bool,
    ) -> Result<ClaimResponse, OwnershipError> {
        let name = target.name().to_owned();
        self.claim_target(
            &name,
            page,
            claimant.into(),
            force,
            LeaseTarget::Native(target),
        )
    }

    pub(super) fn claim_launch(
        &self,
        target: Arc<LaunchTarget>,
        page: &str,
        claimant: impl Into<Claimant>,
        force: bool,
    ) -> Result<ClaimResponse, OwnershipError> {
        let name = target.name().to_owned();
        self.claim_target(
            &name,
            page,
            claimant.into(),
            force,
            LeaseTarget::Launch(target),
        )
    }

    pub(super) fn check_launch(&self, target: &LaunchTarget) -> Result<(), OwnershipError> {
        if self.lock()?.retired.contains(&LaunchKey::from(target)) {
            return Err(OwnershipError::LaunchRetired);
        }
        Ok(())
    }

    /// Caller serializes this with input/claim using the same per-name I/O gate.
    /// No eviction: forgetting a retired nonce would silently restore authority.
    pub(super) fn retire_launch(&self, target: &LaunchTarget) -> Result<(), OwnershipError> {
        let key = LaunchKey::from(target);
        let removed = {
            let mut state = self.lock()?;
            state.retired.insert(key);
            let matches = state.leases.get(target.name()).is_some_and(|lease| {
                lease.target.launch_key().as_ref() == Some(&LaunchKey::from(target))
            });
            if matches {
                state.leases.remove(target.name())
            } else {
                None
            }
        };
        if let Some(lease) = removed
            && let Some(binding) = lease.binding
        {
            binding.revoked.send_replace(Some(Revocation {
                reason: RevocationReason::LaunchRetired,
                new_ip: None,
                new_label: String::new(),
                notify: false,
            }));
        }
        Ok(())
    }

    fn claim_target(
        &self,
        name: &str,
        page: &str,
        claimant: Claimant,
        force: bool,
        target: LeaseTarget,
    ) -> Result<ClaimResponse, OwnershipError> {
        validate_name(name)?;
        validate_page(page)?;
        // OS entropy and injected clock callbacks must never run under lock.
        // Failure cannot revoke an existing live lease.
        let token = LeaseToken::generate()?;
        let sample = self.sample()?;
        let owner = PublicOwner {
            ip: claimant.ip,
            label: claimant.label,
            since: sample.unix_seconds,
        };
        let old = {
            let mut state = self.lock()?;
            let (now, _) = state.advance(sample);
            if let Some(key) = target.launch_key()
                && state.retired.contains(&key)
            {
                return Err(OwnershipError::LaunchRetired);
            }
            if let Some(old) = state.leases.get(name) {
                if !old.target.same_kind(&target) {
                    return Err(OwnershipError::BindingMismatch);
                }
                if old.page != page && !force {
                    return Ok(ClaimResponse::Conflict(old.owner.clone()));
                }
            }
            let deadline = now
                .checked_add(RESERVATION_TTL)
                .ok_or(OwnershipError::InvalidClock)?;
            state.leases.insert(
                name.to_owned(),
                Lease {
                    page: page.to_owned(),
                    token: token.clone(),
                    owner: owner.clone(),
                    reservation_deadline: deadline,
                    binding: None,
                    target,
                },
            )
        };
        // Publish first, signal second. No lock is held while waking consumers;
        // the registry does not wait for WebSocket close or host cleanup.
        if let Some(old) = old
            && let Some(binding) = old.binding
        {
            binding.revoked.send_replace(Some(Revocation {
                reason: RevocationReason::Replaced,
                new_ip: Some(owner.ip).filter(|new| *new != old.owner.ip),
                new_label: owner.label.clone(),
                notify: old.page != page,
            }));
        }
        Ok(ClaimResponse::Granted(GrantedClaim { token, owner }))
    }

    /// Atomically consumes an unexpired reservation exactly once. The returned
    /// connection identity is allocated here, not supplied by the HTTP caller.
    pub fn bind(&self, name: &str, page: &str, token: &str) -> Result<BoundLease, OwnershipError> {
        self.bind_target(name, page, token, ExpectedTarget::Raw)
    }

    pub(super) fn bind_bound(
        &self,
        name: &str,
        page: &str,
        token: &str,
        uid: &str,
        instance: &str,
    ) -> Result<BoundLease, OwnershipError> {
        self.bind_target(name, page, token, ExpectedTarget::Native { uid, instance })
    }

    pub(super) fn bind_launch(
        &self,
        name: &str,
        page: &str,
        token: &str,
        launch: &str,
        instance: &str,
    ) -> Result<BoundLease, OwnershipError> {
        self.bind_target(
            name,
            page,
            token,
            ExpectedTarget::Launch { launch, instance },
        )
    }

    fn bind_target(
        &self,
        name: &str,
        page: &str,
        token: &str,
        expected: ExpectedTarget<'_>,
    ) -> Result<BoundLease, OwnershipError> {
        validate_name(name)?;
        validate_page(page)?;
        let token = LeaseToken::parse(token)?;
        let sample = self.sample()?;
        let mut state = self.lock()?;
        state.advance(sample);
        let lease = state.leases.get(name).ok_or(OwnershipError::NotOwner)?;
        let token_matches = lease.token.matches(&token);
        if !token_matches || lease.page != page {
            return Err(OwnershipError::NotOwner);
        }
        if !lease.target.accepts(expected) {
            return Err(OwnershipError::BindingMismatch);
        }
        if lease.binding.is_some() {
            return Err(OwnershipError::AlreadyBound);
        }
        let target = lease.target.clone();
        let next = state
            .next_connection
            .checked_add(1)
            .ok_or(OwnershipError::ConnectionIdsExhausted)?;
        state.next_connection = next;
        let connection_id = ConnectionId(next);
        let (sender, receiver) = watch::channel(None);
        state
            .leases
            .get_mut(name)
            .expect("checked under same lock")
            .binding = Some(Binding {
            connection_id,
            revoked: sender,
        });
        Ok(BoundLease {
            name: name.to_owned(),
            page: page.to_owned(),
            token,
            connection_id,
            revoked: receiver,
            target,
        })
    }

    pub fn owner(&self, name: &str) -> Result<Option<PublicOwner>, OwnershipError> {
        validate_name(name)?;
        let sample = self.sample()?;
        let mut state = self.lock()?;
        state.advance(sample);
        Ok(state.leases.get(name).map(|lease| lease.owner.clone()))
    }

    /// Authorize one HTTP input request against the current lease without
    /// consuming or binding it. An unexpired reservation and a bound WebSocket
    /// connection are the same claim; a stale credential is `Revoked`, an
    /// absent lease `NoLease`, and a different kind/identity `BindingMismatch`.
    /// The returned target is the pinned immutable identity to send through.
    pub(super) fn authorize_input(
        &self,
        name: &str,
        page: &str,
        token: &str,
        expected: ExpectedTarget<'_>,
    ) -> Result<LeaseTarget, OwnershipError> {
        validate_name(name)?;
        validate_page(page)?;
        let token = LeaseToken::parse(token)?;
        let sample = self.sample()?;
        let mut state = self.lock()?;
        state.advance(sample);
        let lease = state.leases.get(name).ok_or(OwnershipError::NoLease)?;
        let token_matches = lease.token.matches(&token);
        if !token_matches || lease.page != page {
            return Err(OwnershipError::Revoked);
        }
        if !lease.target.accepts(expected) {
            return Err(OwnershipError::BindingMismatch);
        }
        Ok(lease.target.clone())
    }

    pub fn is_current(&self, bound: &BoundLease) -> Result<bool, OwnershipError> {
        let state = self.lock()?;
        Ok(state
            .leases
            .get(&bound.name)
            .is_some_and(|lease| matches_bound(lease, bound)))
    }

    /// Cleanup only the precise page/token/connection tuple. Calling this from
    /// an old transport's finally block cannot release a newly claimed lease.
    pub fn release(&self, bound: &BoundLease) -> Result<bool, OwnershipError> {
        let removed = {
            let mut state = self.lock()?;
            if !state
                .leases
                .get(&bound.name)
                .is_some_and(|lease| matches_bound(lease, bound))
            {
                return Ok(false);
            }
            state.leases.remove(&bound.name)
        };
        if let Some(lease) = removed
            && let Some(binding) = lease.binding
        {
            binding.revoked.send_replace(Some(Revocation {
                reason: RevocationReason::Released,
                new_ip: None,
                new_label: String::new(),
                notify: false,
            }));
        }
        Ok(true)
    }

    /// Optional cancellation for an unbound claim. A browser token alone is
    /// never sufficient to release an already bound WebSocket connection.
    pub fn release_reservation(
        &self,
        name: &str,
        page: &str,
        token: &str,
    ) -> Result<bool, OwnershipError> {
        validate_name(name)?;
        validate_page(page)?;
        let token = LeaseToken::parse(token)?;
        let sample = self.sample()?;
        let mut state = self.lock()?;
        state.advance(sample);
        let matches = state.leases.get(name).is_some_and(|lease| {
            let token_matches = lease.token.matches(&token);
            token_matches && lease.page == page && lease.binding.is_none()
        });
        if matches {
            state.leases.remove(name);
        }
        Ok(matches)
    }

    /// Expire abandoned reservations across all names; bound connections do
    /// not expire on the reservation TTL. An optional service tick can call it.
    pub fn expire(&self) -> Result<usize, OwnershipError> {
        let sample = self.sample()?;
        Ok(self.lock()?.advance(sample).1)
    }
}

fn matches_bound(lease: &Lease, bound: &BoundLease) -> bool {
    let token_matches = lease.token.matches(&bound.token);
    token_matches
        && lease.page == bound.page
        && lease
            .binding
            .as_ref()
            .is_some_and(|binding| binding.connection_id == bound.connection_id)
}
