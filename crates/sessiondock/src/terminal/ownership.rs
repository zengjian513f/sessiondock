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
pub const MAX_CAPACITY: usize = 4_096;
pub const MAX_RETIRED_LAUNCHES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnershipError {
    InvalidName,
    InvalidPage,
    InvalidToken,
    InvalidCapacity,
    InvalidClock,
    EntropyUnavailable,
    Capacity,
    NotOwner,
    /// HTTP input without any current lease for that terminal name.
    NoLease,
    /// HTTP input with a credential that a newer claim has replaced.
    Revoked,
    AlreadyBound,
    BindingMismatch,
    LaunchRetired,
    RetirementCapacity,
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
            Self::Capacity | Self::RetirementCapacity => 429,
            Self::InvalidCapacity
            | Self::InvalidClock
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
            Self::InvalidCapacity => "终端控制权容量配置无效",
            Self::InvalidClock => "终端控制权时钟不可用",
            Self::EntropyUnavailable => "安全随机数暂不可用",
            Self::Capacity => "终端控制权预约达到容量限制",
            Self::NotOwner => "终端控制权已失效，请重新预约",
            Self::NoLease => "终端控制权已失效，请重新预约",
            Self::Revoked => "终端控制权已被其他页面接管或重新预约，本页输入已拒绝",
            Self::AlreadyBound => "终端控制权已绑定其他连接",
            Self::BindingMismatch => "终端租约的会话或实例绑定不匹配，不能降级为普通连接",
            Self::LaunchRetired => "该启动实例已撤销终端授权，不能重新预约",
            Self::RetirementCapacity => "终端启动撤销记录达到容量限制，未撤销该实例",
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
    if page.is_empty()
        || page.len() > 128
        || !page
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
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

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PublicOwner {
    pub ip: IpAddr,
    pub since: f64,
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

    /// Batch 31: the delivery executor is itself a claimant (a server-held
    /// lease around one send). Like `into_api_json`, this is the only other
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
    /// Display-only address of the new claimant; absent on explicit release.
    pub new_ip: Option<IpAddr>,
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

    /// Rate-limit key: one window per exact host instance, never shared.
    pub(super) fn instance_key(&self, name: &str) -> String {
        match self {
            Self::Raw => format!("{name}/raw"),
            Self::Native(target) => format!("{name}/{}", target.instance_id()),
            Self::Launch(target) => format!("{name}/{}", target.instance_id()),
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
    #[cfg(test)]
    pub(super) fn target(&self) -> Option<&BoundTarget> {
        match &self.target {
            LeaseTarget::Native(target) => Some(target),
            _ => None,
        }
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
    capacity: usize,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
}

impl Registry {
    pub fn new(capacity: usize) -> Result<Self, OwnershipError> {
        Self::with_clock(
            capacity,
            Arc::new(SystemClock {
                origin: Instant::now(),
            }),
        )
    }

    /// A supplied clock must be cheap and non-blocking. It is sampled outside
    /// the mutex; monotonic regressions are clamped under the mutex.
    pub fn with_clock(capacity: usize, clock: Arc<dyn Clock>) -> Result<Self, OwnershipError> {
        if capacity == 0 || capacity > MAX_CAPACITY {
            return Err(OwnershipError::InvalidCapacity);
        }
        Ok(Self {
            capacity,
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
        ip: IpAddr,
        force: bool,
    ) -> Result<ClaimResponse, OwnershipError> {
        self.claim_target(name, page, ip, force, LeaseTarget::Raw)
    }

    pub(super) fn claim_bound(
        &self,
        target: Arc<BoundTarget>,
        page: &str,
        ip: IpAddr,
        force: bool,
    ) -> Result<ClaimResponse, OwnershipError> {
        let name = target.name().to_owned();
        self.claim_target(&name, page, ip, force, LeaseTarget::Native(target))
    }

    pub(super) fn claim_launch(
        &self,
        target: Arc<LaunchTarget>,
        page: &str,
        ip: IpAddr,
        force: bool,
    ) -> Result<ClaimResponse, OwnershipError> {
        let name = target.name().to_owned();
        self.claim_target(&name, page, ip, force, LeaseTarget::Launch(target))
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
            if !state.retired.contains(&key) {
                if state.retired.len() >= MAX_RETIRED_LAUNCHES {
                    return Err(OwnershipError::RetirementCapacity);
                }
                state.retired.insert(key);
            }
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
                notify: false,
            }));
        }
        Ok(())
    }

    fn claim_target(
        &self,
        name: &str,
        page: &str,
        ip: IpAddr,
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
            ip,
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
            } else if state.leases.len() >= self.capacity {
                return Err(OwnershipError::Capacity);
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
                new_ip: Some(ip),
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

#[cfg(test)]
#[path = "ownership_launch_tests.rs"]
mod launch_tests;

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    use super::*;

    #[derive(Default)]
    struct FakeClock {
        millis: AtomicU64,
        wall: AtomicU64,
    }

    impl FakeClock {
        fn set(&self, millis: u64) {
            self.millis.store(millis, Ordering::SeqCst);
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> ClockSample {
            ClockSample {
                monotonic: Duration::from_millis(self.millis.load(Ordering::SeqCst)),
                unix_seconds: self.wall.load(Ordering::SeqCst) as f64,
            }
        }
    }

    fn setup(capacity: usize) -> (Arc<Registry>, Arc<FakeClock>) {
        let clock = Arc::new(FakeClock {
            wall: AtomicU64::new(1_700_000_000),
            ..Default::default()
        });
        (
            Arc::new(Registry::with_clock(capacity, clock.clone()).unwrap()),
            clock,
        )
    }

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([192, 0, 2, last])
    }

    fn claim(registry: &Registry, name: &str, page: &str, address: IpAddr, force: bool) -> String {
        let response = registry.claim(name, page, address, force).unwrap();
        assert_eq!(response.status(), 200);
        response.into_api_json()["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn target(instance: &str) -> Arc<BoundTarget> {
        use ptyhost_client::{
            Association, AssociationState, HostObservation, SessionSummary, Source,
        };
        let observation = HostObservation {
            summary: SessionSummary {
                name: "term".into(),
                created: 1,
                attached: false,
                pid: 1,
                host_pid: 2,
                cwd: String::new(),
                cmd: String::new(),
                cols: 80,
                rows: 24,
                owned: true,
                server: "ptyhost",
                backend: "ptyhost",
            },
            association: AssociationState::Declared(Association {
                source: Source::Codex,
                sid: Some("synthetic-session".into()),
                uid: Some("codex:0123456789abcdef".into()),
            }),
            instance_id: Some(instance.into()),
            exited: false,
            instance_guard_v1: true,
            launch: Default::default(),
            launch_guard_v1: false,
            native_binding: Default::default(),
        };
        Arc::new(
            BoundTarget::from_observation(
                &observation,
                Source::Codex,
                "synthetic-session",
                "codex:0123456789abcdef",
            )
            .unwrap(),
        )
    }

    fn claim_bound(
        registry: &Registry,
        target: Arc<BoundTarget>,
        page: &str,
        force: bool,
    ) -> String {
        registry
            .claim_bound(target, page, ip(1), force)
            .unwrap()
            .into_api_json()["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn input_authorization_follows_the_exact_lease_without_consuming_it() {
        let (registry, clock) = setup(4);
        let target = target("synthetic-instance-one");
        let native = ExpectedTarget::Native {
            uid: target.uid(),
            instance: target.instance_id(),
        };
        // No lease at all: 403.
        assert_eq!(
            registry
                .authorize_input("term", "page", &"0".repeat(64), native)
                .err(),
            Some(OwnershipError::NoLease)
        );
        assert_eq!(OwnershipError::NoLease.status(), 403);
        let token = claim_bound(&registry, target.clone(), "page", false);
        // An unbound reservation authorizes input and is not consumed by it.
        assert!(matches!(
            registry.authorize_input("term", "page", &token, native),
            Ok(LeaseTarget::Native(pinned)) if pinned.instance_id() == target.instance_id()
        ));
        assert_eq!(
            registry
                .authorize_input("term", "page", &token, ExpectedTarget::Raw)
                .err(),
            Some(OwnershipError::BindingMismatch)
        );
        assert_eq!(
            registry
                .authorize_input(
                    "term",
                    "page",
                    &token,
                    ExpectedTarget::Native {
                        uid: target.uid(),
                        instance: "synthetic-instance-two"
                    }
                )
                .err(),
            Some(OwnershipError::BindingMismatch)
        );
        // Wrong page or token with a live lease: revoked, 409.
        assert_eq!(
            registry
                .authorize_input("term", "other-page", &token, native)
                .err(),
            Some(OwnershipError::Revoked)
        );
        assert_eq!(OwnershipError::Revoked.status(), 409);
        let bound = registry
            .bind_bound("term", "page", &token, target.uid(), target.instance_id())
            .unwrap();
        assert!(
            registry
                .authorize_input("term", "page", &token, native)
                .is_ok()
        );
        // Replacement by another page revokes the old credential for input too.
        let replacement = claim_bound(&registry, target.clone(), "taker", true);
        assert_eq!(
            registry
                .authorize_input("term", "page", &token, native)
                .err(),
            Some(OwnershipError::Revoked)
        );
        assert!(
            registry
                .authorize_input("term", "taker", &replacement, native)
                .is_ok()
        );
        assert!(!registry.release(&bound).unwrap());
        // The replacement reservation expires like any other unbound claim.
        clock.set(RESERVATION_TTL.as_millis() as u64 + 1);
        assert_eq!(
            registry
                .authorize_input("term", "taker", &replacement, native)
                .err(),
            Some(OwnershipError::NoLease)
        );
    }

    #[test]
    fn bound_target_cannot_be_consumed_or_force_downgraded_through_raw_apis() {
        let (registry, _) = setup(1);
        let target = target("synthetic-instance-one");
        let token = claim_bound(&registry, target.clone(), "page", false);
        assert_eq!(
            registry.bind("term", "page", &token).err(),
            Some(OwnershipError::BindingMismatch)
        );
        assert_eq!(
            registry
                .bind_bound("term", "page", &token, "codex:wrong", target.instance_id())
                .err(),
            Some(OwnershipError::BindingMismatch)
        );
        assert_eq!(
            registry
                .bind_bound(
                    "term",
                    "page",
                    &token,
                    target.uid(),
                    "synthetic-instance-other"
                )
                .err(),
            Some(OwnershipError::BindingMismatch)
        );
        for (page, force) in [("page", false), ("page", true), ("other", true)] {
            assert_eq!(
                registry.claim("term", page, ip(2), force).err(),
                Some(OwnershipError::BindingMismatch)
            );
        }
        let bound = registry
            .bind_bound("term", "page", &token, target.uid(), target.instance_id())
            .unwrap();
        assert_eq!(bound.target().unwrap().sid(), "synthetic-session");
        assert!(registry.is_current(&bound).unwrap());
        assert_eq!(
            registry
                .bind_bound("term", "page", &token, target.uid(), target.instance_id())
                .err(),
            Some(OwnershipError::AlreadyBound)
        );
    }

    #[test]
    fn target_storage_is_reclaimed_with_exact_ttl_release_and_reservation_cancel() {
        let (registry, clock) = setup(1);
        let target = target("synthetic-instance-one");
        let _token = claim_bound(&registry, target.clone(), "page", false);
        assert_eq!(Arc::strong_count(&target), 2);
        clock.set(15_000);
        assert_eq!(registry.expire().unwrap(), 1);
        assert_eq!(Arc::strong_count(&target), 1);
        let token = claim_bound(&registry, target.clone(), "page", false);
        assert!(
            registry
                .release_reservation("term", "page", &token)
                .unwrap()
        );
        assert_eq!(Arc::strong_count(&target), 1);
        let token = claim_bound(&registry, target.clone(), "page", false);
        let bound = registry
            .bind_bound("term", "page", &token, target.uid(), target.instance_id())
            .unwrap();
        assert_eq!(Arc::strong_count(&target), 3);
        clock.set(100_000);
        assert_eq!(registry.expire().unwrap(), 0);
        assert!(registry.release(&bound).unwrap());
        assert_eq!(Arc::strong_count(&target), 2);
        drop(bound);
        assert_eq!(Arc::strong_count(&target), 1);
    }

    #[test]
    fn replacement_keeps_immutable_target_and_old_cleanup_cannot_remove_new_target() {
        let (registry, _) = setup(1);
        let original = target("synthetic-instance-one");
        let replacement = target("synthetic-instance-two");
        let token = claim_bound(&registry, original.clone(), "first", false);
        let old = registry
            .bind_bound(
                "term",
                "first",
                &token,
                original.uid(),
                original.instance_id(),
            )
            .unwrap();
        let token = claim_bound(&registry, replacement.clone(), "second", true);
        assert!(!registry.is_current(&old).unwrap());
        assert!(!registry.release(&old).unwrap());
        assert_eq!(old.target().unwrap().instance_id(), original.instance_id());
        let new = registry
            .bind_bound(
                "term",
                "second",
                &token,
                replacement.uid(),
                replacement.instance_id(),
            )
            .unwrap();
        assert_eq!(
            new.target().unwrap().instance_id(),
            replacement.instance_id()
        );
        assert!(registry.is_current(&new).unwrap());
        drop(old);
        assert_eq!(Arc::strong_count(&original), 1);
        assert_eq!(Arc::strong_count(&replacement), 3);
    }

    #[test]
    fn claim_exposes_only_its_own_token_and_public_owner() {
        let (registry, _) = setup(2);
        let first = registry.claim("term", "page-a", ip(1), false).unwrap();
        assert_eq!(first.owner().ip, ip(1));
        let json = first.into_api_json();
        assert_eq!(json["ok"], true);
        assert_eq!(
            json["owner"],
            json!({"ip":"192.0.2.1","since":1_700_000_000.0})
        );
        let token = json["token"].as_str().unwrap();
        assert_eq!(token.len(), 64);
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        let owner_json = serde_json::to_value(registry.owner("term").unwrap()).unwrap();
        assert_eq!(owner_json, json["owner"]);
        assert!(!owner_json.to_string().contains(token));
        let conflict = registry.claim("term", "page-b", ip(1), false).unwrap();
        assert_eq!(conflict.status(), 409);
        let conflict = conflict.into_api_json();
        assert_eq!(conflict, json!({"conflict":true,"owner":owner_json}));
        assert!(conflict.get("token").is_none());
    }

    #[test]
    fn replacement_generates_distinct_opaque_tokens() {
        let (registry, _) = setup(1);
        let mut tokens = std::collections::BTreeSet::new();
        for _ in 0..64 {
            assert!(tokens.insert(claim(&registry, "term", "page", ip(1), false)));
        }
    }

    #[test]
    fn ip_is_display_only_page_and_token_are_both_required() {
        let (registry, _) = setup(1);
        let token = claim(&registry, "term", "page-a", ip(1), false);
        assert_eq!(
            registry
                .claim("term", "page-b", ip(1), false)
                .unwrap()
                .status(),
            409
        );
        assert_eq!(
            registry.bind("term", "page-b", &token).err(),
            Some(OwnershipError::NotOwner)
        );
        assert_eq!(
            registry.bind("term", "page-a", &"0".repeat(64)).err(),
            Some(OwnershipError::NotOwner)
        );
        let replacement = claim(&registry, "term", "page-a", ip(2), false);
        assert_eq!(
            registry.bind("term", "page-a", &token).err(),
            Some(OwnershipError::NotOwner)
        );
        assert!(registry.bind("term", "page-a", &replacement).is_ok());
        assert_eq!(registry.owner("term").unwrap().unwrap().ip, ip(2));
    }

    #[test]
    fn reservation_expires_at_exact_deadline_not_one_tick_later() {
        let (registry, clock) = setup(1);
        let token = claim(&registry, "term", "page", ip(1), false);
        clock.set(14_999);
        assert!(registry.owner("term").unwrap().is_some());
        clock.set(15_000);
        assert_eq!(
            registry.bind("term", "page", &token).err(),
            Some(OwnershipError::NotOwner)
        );
        assert!(registry.owner("term").unwrap().is_none());
        assert_eq!(registry.expire().unwrap(), 0);
    }

    #[test]
    fn bound_connection_does_not_expire_on_reservation_ttl() {
        let (registry, clock) = setup(1);
        let token = claim(&registry, "term", "page", ip(1), false);
        clock.set(14_999);
        let bound = registry.bind("term", "page", &token).unwrap();
        clock.set(86_400_000);
        assert_eq!(registry.expire().unwrap(), 0);
        assert!(registry.owner("term").unwrap().is_some());
        assert!(registry.is_current(&bound).unwrap());
        assert_eq!(
            registry.claim("other", "other-page", ip(2), false).err(),
            Some(OwnershipError::Capacity)
        );
    }

    #[test]
    fn bind_is_once_per_claim_and_connection_id_is_server_allocated() {
        let (registry, _) = setup(1);
        let token = claim(&registry, "term", "page", ip(1), false);
        let first = registry.bind("term", "page", &token).unwrap();
        assert_eq!(first.connection_id().as_u64(), 1);
        assert_eq!(
            registry.bind("term", "page", &token).err(),
            Some(OwnershipError::AlreadyBound)
        );
        let token = claim(&registry, "term", "page", ip(1), false);
        let second = registry.bind("term", "page", &token).unwrap();
        assert_eq!(second.connection_id().as_u64(), 2);
        assert_ne!(first.connection_id(), second.connection_id());
        assert!(!registry.is_current(&first).unwrap());
        assert!(registry.is_current(&second).unwrap());
    }

    #[test]
    fn force_publishes_replacement_and_signals_old_connection_without_cleanup_wait() {
        let (registry, _) = setup(1);
        let first_token = claim(&registry, "term", "page-a", ip(1), false);
        let first = registry.bind("term", "page-a", &first_token).unwrap();
        let revoked = first.revocations();
        assert!(revoked.borrow().is_none());
        let second_token = claim(&registry, "term", "page-b", ip(2), true);
        assert_eq!(registry.owner("term").unwrap().unwrap().ip, ip(2));
        assert_eq!(
            *revoked.borrow(),
            Some(Revocation {
                reason: RevocationReason::Replaced,
                new_ip: Some(ip(2)),
                notify: true
            })
        );
        assert!(!registry.release(&first).unwrap());
        assert_eq!(registry.owner("term").unwrap().unwrap().ip, ip(2));
        let second = registry.bind("term", "page-b", &second_token).unwrap();
        assert!(registry.is_current(&second).unwrap());
    }

    #[test]
    fn same_page_replacement_still_revokes_but_without_ui_notice() {
        let (registry, _) = setup(1);
        let token = claim(&registry, "term", "page", ip(1), false);
        let first = registry.bind("term", "page", &token).unwrap();
        claim(&registry, "term", "page", ip(2), false);
        // Subscribing after replacement still exposes the latest signal.
        let revoked = first.revocations();
        assert_eq!(
            *revoked.borrow(),
            Some(Revocation {
                reason: RevocationReason::Replaced,
                new_ip: Some(ip(2)),
                notify: false
            })
        );
        assert!(!registry.is_current(&first).unwrap());
    }

    #[test]
    fn denied_claim_does_not_revoke_or_change_current_lease() {
        let (registry, _) = setup(1);
        let token = claim(&registry, "term", "page-a", ip(1), false);
        let first = registry.bind("term", "page-a", &token).unwrap();
        let revoked = first.revocations();
        assert_eq!(
            registry
                .claim("term", "page-b", ip(2), false)
                .unwrap()
                .status(),
            409
        );
        assert!(revoked.borrow().is_none());
        assert!(registry.is_current(&first).unwrap());
    }

    #[test]
    fn release_requires_page_token_and_exact_connection_identity() {
        let (registry, _) = setup(1);
        let token = claim(&registry, "term", "page", ip(1), false);
        let bound = registry.bind("term", "page", &token).unwrap();
        // These forgeries are constructible only inside this private module.
        // Public callers cannot manufacture or deserialize a BoundLease.
        for (page, token, connection_id) in [
            ("other-page", bound.token.clone(), bound.connection_id),
            ("page", LeaseToken([0; 32]), bound.connection_id),
            ("page", bound.token.clone(), ConnectionId(999)),
        ] {
            let forged = BoundLease {
                target: LeaseTarget::Raw,
                name: "term".to_owned(),
                page: page.to_owned(),
                token,
                connection_id,
                revoked: bound.revocations(),
            };
            assert!(!registry.is_current(&forged).unwrap());
            assert!(!registry.release(&forged).unwrap());
            assert!(registry.is_current(&bound).unwrap());
        }
        assert!(
            !registry
                .release_reservation("term", "page", &token)
                .unwrap()
        );
        let revoked = bound.revocations();
        assert!(registry.release(&bound).unwrap());
        assert!(!registry.release(&bound).unwrap());
        assert!(!registry.is_current(&bound).unwrap());
        assert_eq!(
            *revoked.borrow(),
            Some(Revocation {
                reason: RevocationReason::Released,
                new_ip: None,
                notify: false
            })
        );
    }

    #[test]
    fn cancellation_only_releases_matching_unbound_reservation() {
        let (registry, _) = setup(1);
        let old = claim(&registry, "term", "page", ip(1), false);
        assert!(
            !registry
                .release_reservation("term", "other-page", &old)
                .unwrap()
        );
        assert!(
            !registry
                .release_reservation("term", "page", &"0".repeat(64))
                .unwrap()
        );
        let new = claim(&registry, "term", "page", ip(1), false);
        assert!(!registry.release_reservation("term", "page", &old).unwrap());
        assert!(registry.release_reservation("term", "page", &new).unwrap());
        assert!(!registry.release_reservation("term", "page", &new).unwrap());
        assert!(registry.owner("term").unwrap().is_none());
    }

    #[test]
    fn capacity_sweeps_expired_names_but_does_not_evict_live_reservations() {
        let (registry, clock) = setup(2);
        claim(&registry, "a", "page", ip(1), false);
        claim(&registry, "b", "page", ip(1), false);
        assert_eq!(
            registry.claim("c", "page", ip(1), false).err(),
            Some(OwnershipError::Capacity)
        );
        assert!(registry.owner("a").unwrap().is_some());
        claim(&registry, "a", "other-page", ip(2), true);
        clock.set(15_000);
        claim(&registry, "c", "page", ip(1), false);
        assert!(registry.owner("a").unwrap().is_none());
        assert!(registry.owner("b").unwrap().is_none());
        assert!(registry.owner("c").unwrap().is_some());
        clock.set(30_000);
        assert_eq!(registry.expire().unwrap(), 1);
    }

    #[test]
    fn clock_rollback_and_wall_clock_changes_do_not_extend_reservations() {
        let (registry, clock) = setup(1);
        claim(&registry, "term", "page", ip(1), false);
        clock.set(14_999);
        assert!(registry.owner("term").unwrap().is_some());
        clock.wall.store(0, Ordering::SeqCst);
        clock.set(1);
        assert!(registry.owner("term").unwrap().is_some());
        clock.set(15_000);
        assert!(registry.owner("term").unwrap().is_none());
        clock.set(1);
        assert!(registry.owner("term").unwrap().is_none());
        claim(&registry, "term", "page", ip(1), false);
        clock.set(29_999);
        assert!(registry.owner("term").unwrap().is_some());
        clock.set(30_000);
        assert!(registry.owner("term").unwrap().is_none());
    }

    #[test]
    fn names_pages_and_tokens_are_bounded_and_never_normalized() {
        for name in [
            "",
            ".",
            "..",
            "../outside",
            "/absolute",
            "a/b",
            "a\\b",
            "a:b",
            "a\n",
            "a ",
            " spaced",
            "a?b",
            "a*b",
            "a|b",
            "a\"b",
            "a<b",
            "a>b",
            "term.",
            ".term",
            "控制台",
            "CON",
            "con.txt",
            "PRN",
            "aux",
            "NUL",
            "COM1",
            "Lpt9.log",
        ] {
            assert_eq!(
                validate_name(name),
                Err(OwnershipError::InvalidName),
                "{name:?}"
            );
        }
        assert_eq!(
            validate_name(&"a".repeat(129)),
            Err(OwnershipError::InvalidName)
        );
        for name in [
            "shell-test",
            "codex_123",
            "a.b",
            "COM10",
            "LPT0",
            "console",
            "-a",
        ] {
            assert!(validate_name(name).is_ok(), "{name:?}");
        }
        assert!(validate_name(&"a".repeat(128)).is_ok());
        for page in ["", " page", "a.b", "a/b", "a\\b", "a\n", "中文"] {
            assert_eq!(validate_page(page), Err(OwnershipError::InvalidPage));
        }
        assert!(validate_page("01234567-89ab-4def-aaaa-000000000000").is_ok());
        assert!(validate_page(&"a".repeat(128)).is_ok());
        assert_eq!(
            validate_page(&"a".repeat(129)),
            Err(OwnershipError::InvalidPage)
        );
        let (registry, _) = setup(1);
        for token in [
            String::new(),
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            "g".repeat(64),
            "秘密".repeat(11),
        ] {
            let error = registry.bind("term", "page", &token).err().unwrap();
            assert_eq!(error, OwnershipError::InvalidToken);
            if !token.is_empty() {
                assert!(!error.to_string().contains(&token));
            }
        }
    }

    #[test]
    fn capacity_configuration_is_bounded() {
        assert_eq!(
            Registry::new(0).err(),
            Some(OwnershipError::InvalidCapacity)
        );
        assert_eq!(
            Registry::new(MAX_CAPACITY + 1).err(),
            Some(OwnershipError::InvalidCapacity)
        );
        assert!(Registry::new(MAX_CAPACITY).is_ok());
    }

    #[test]
    fn invalid_clock_and_connection_id_overflow_fail_closed() {
        struct BrokenClock;
        impl Clock for BrokenClock {
            fn now(&self) -> ClockSample {
                ClockSample {
                    monotonic: Duration::ZERO,
                    unix_seconds: f64::NAN,
                }
            }
        }
        let broken = Registry::with_clock(1, Arc::new(BrokenClock)).unwrap();
        assert_eq!(
            broken.claim("term", "page", ip(1), false).err(),
            Some(OwnershipError::InvalidClock)
        );
        let (registry, _) = setup(1);
        let token = claim(&registry, "term", "page", ip(1), false);
        registry.state.lock().unwrap().next_connection = u64::MAX;
        assert_eq!(
            registry.bind("term", "page", &token).err(),
            Some(OwnershipError::ConnectionIdsExhausted)
        );
        assert!(
            registry
                .release_reservation("term", "page", &token)
                .unwrap()
        );
    }

    #[test]
    fn concurrent_claims_without_force_have_exactly_one_winner() {
        let (registry, _) = setup(1);
        let barrier = Arc::new(Barrier::new(16));
        let workers: Vec<_> = (0..16)
            .map(|index| {
                let registry = registry.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    registry
                        .claim("term", &format!("page-{index}"), ip(1), false)
                        .unwrap()
                        .status()
                })
            })
            .collect();
        let statuses: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(statuses.iter().filter(|status| **status == 200).count(), 1);
        assert_eq!(statuses.iter().filter(|status| **status == 409).count(), 15);
    }

    #[test]
    fn concurrent_binds_consume_a_reservation_once() {
        let (registry, _) = setup(1);
        let token = claim(&registry, "term", "page", ip(1), false);
        let barrier = Arc::new(Barrier::new(16));
        let workers: Vec<_> = (0..16)
            .map(|_| {
                let registry = registry.clone();
                let barrier = barrier.clone();
                let token = token.clone();
                thread::spawn(move || {
                    barrier.wait();
                    registry.bind("term", "page", &token)
                })
            })
            .collect();
        let mut bound = None;
        let mut rejected = 0;
        for worker in workers {
            match worker.join().unwrap() {
                Ok(winner) => {
                    assert!(bound.is_none());
                    bound = Some(winner);
                }
                Err(error) => {
                    assert_eq!(error, OwnershipError::AlreadyBound);
                    rejected += 1;
                }
            }
        }
        assert_eq!(rejected, 15);
        assert!(registry.release(&bound.unwrap()).unwrap());
    }

    #[test]
    fn concurrent_old_cleanup_cannot_remove_replacement() {
        for _ in 0..16 {
            let (registry, _) = setup(1);
            let token = claim(&registry, "term", "old-page", ip(1), false);
            let old = registry.bind("term", "old-page", &token).unwrap();
            let barrier = Arc::new(Barrier::new(3));
            let releasing = {
                let registry = registry.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    registry.release(&old).unwrap()
                })
            };
            let replacing = {
                let registry = registry.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    claim(&registry, "term", "new-page", ip(2), true)
                })
            };
            barrier.wait();
            releasing.join().unwrap();
            let token = replacing.join().unwrap();
            assert_eq!(registry.owner("term").unwrap().unwrap().ip, ip(2));
            let new = registry.bind("term", "new-page", &token).unwrap();
            assert!(registry.is_current(&new).unwrap());
        }
    }

    #[tokio::test]
    async fn revocation_signal_wakes_an_async_consumer() {
        let (registry, _) = setup(1);
        let token = claim(&registry, "term", "page-a", ip(1), false);
        let bound = registry.bind("term", "page-a", &token).unwrap();
        let mut revoked = bound.revocations();
        let task = tokio::spawn(async move {
            revoked.changed().await.unwrap();
            revoked.borrow_and_update().clone().unwrap()
        });
        claim(&registry, "term", "page-b", ip(2), true);
        let signal = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(signal.reason, RevocationReason::Replaced);
        assert_eq!(signal.new_ip, Some(ip(2)));
        assert!(signal.notify);
    }
}
