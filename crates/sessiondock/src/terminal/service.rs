//! Opt-in local transport: an explicit host directory, bounded forwarding, and
//! no process creation, kill, send ledger, native inventory reads, or defaults.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use ptyhost_client::{
    AttachReader, AttachWriter, BoundTarget, CaptureKind, CaptureReply, ControlOp, ControlReply,
    HostClient, HostEvent, LaunchState, LaunchTarget, Limits, SessionSummary, TerminalSize,
};
use serde::Deserialize;
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::input;
use super::ownership::{
    self, BoundLease, ClaimResponse, ExpectedTarget, LeaseTarget, Registry, Revocation,
};

const OUTPUT_CHUNK: usize = 32 * 1024;
const OUTPUT_QUEUE: usize = 16;
const CLOSE_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone, Debug)]
pub struct BridgeLimits {
    pub max_input_bytes: usize,
    pub max_host_frame_bytes: usize,
    pub operation_timeout: Duration,
    pub partial_frame_timeout: Option<Duration>,
}

impl Default for BridgeLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 1024 * 1024, // ptyhost's guarded send/paste ceiling
            max_host_frame_bytes: 8 * 1024 * 1024,
            operation_timeout: Duration::from_secs(10),
            partial_frame_timeout: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TerminalError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl TerminalError {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TerminalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TerminalError {}

impl From<ownership::OwnershipError> for TerminalError {
    fn from(error: ownership::OwnershipError) -> Self {
        Self::new(error.status(), "terminal_ownership", error.to_string())
    }
}

fn host_error(error: ptyhost_client::Error) -> TerminalError {
    // Never render peer errors, arbitrary I/O paths, or token-bearing requests.
    match error {
        ptyhost_client::Error::NotFound => {
            TerminalError::new(404, "terminal_missing", "指定目录中没有这个终端 host")
        }
        ptyhost_client::Error::Timeout => {
            TerminalError::new(504, "terminal_timeout", "终端 host 响应超时")
        }
        ptyhost_client::Error::GuardUnsupported => TerminalError::new(
            501,
            "terminal_guard_required",
            "终端 host 尚不支持实例保护协议，不能连接原生会话",
        ),
        ptyhost_client::Error::LaunchGuardUnsupported => TerminalError::new(
            501,
            "terminal_launch_guard_required",
            "终端 host 尚不支持启动实例保护协议，不能连接待关联终端",
        ),
        ptyhost_client::Error::InvalidBinding
        | ptyhost_client::Error::InvalidLaunchBinding
        | ptyhost_client::Error::IdentityChanged => TerminalError::new(
            409,
            "terminal_identity",
            "终端会话或进程实例已改变，请重新选择",
        ),
        ptyhost_client::Error::LineTooLarge => TerminalError::new(
            413,
            "terminal_input_too_large",
            "单次终端输入超过宿主控制通道上限",
        ),
        _ => TerminalError::new(
            503,
            "terminal_unavailable",
            "终端 host 不可达或本地协议无效",
        ),
    }
}

fn exited() -> TerminalError {
    TerminalError::new(
        410,
        "terminal_exited",
        "终端 host 已退出或其记录已被移除，无法再输入",
    )
}

/// Input-path mapping: a lease was granted against a live host, so a record
/// that is now missing means that exact instance is gone (410, as the bridge
/// reports host exit), and an ambiguous submission is reported as such.
fn input_error(error: ptyhost_client::Error) -> TerminalError {
    match error {
        ptyhost_client::Error::NotFound => exited(),
        ptyhost_client::Error::Timeout
        | ptyhost_client::Error::GuardNotAcknowledged
        | ptyhost_client::Error::LaunchGuardNotAcknowledged
        | ptyhost_client::Error::UnexpectedEof => TerminalError::new(
            504,
            "terminal_input_ambiguous",
            "终端 host 未确认这次输入；它可能已经生效，不会自动重试",
        ),
        ptyhost_client::Error::Rejected => TerminalError::new(
            409,
            "terminal_identity",
            "终端 host 拒绝了这次受保护输入，会话或进程实例可能已改变",
        ),
        other => host_error(other),
    }
}

pub struct TerminalService {
    client: HostClient,
    registry: Arc<Registry>,
    limits: BridgeLimits,
    gates: Mutex<BTreeMap<String, Weak<AsyncMutex<()>>>>,
}

/// One HTTP input request after JSON validation. Text is UTF-8 only because
/// the host's `send` operation carries a JSON string; split or invalid bytes
/// stay on the binary WebSocket path.
pub enum InputPayload {
    Text(String),
    /// Host key names already resolved by [`input::map_keys`].
    Keys(Vec<String>),
    /// The host wraps text in bracketed-paste markers; no Enter is implied.
    Paste(String),
}

/// Decoded paste bytes, matching ptyhost's send/paste ceiling. The serialized
/// request must also fit the host's 4 MiB JSON control-line budget.
pub const MAX_PASTE_BYTES: usize = 1024 * 1024;

impl InputPayload {
    pub fn bytes(&self) -> usize {
        match self {
            Self::Text(text) | Self::Paste(text) => text.len(),
            Self::Keys(keys) => keys
                .iter()
                .map(|key| input::map_key(key).map_or(0, |mapped| mapped.bytes))
                .sum(),
        }
    }
    fn limit(&self) -> usize {
        match self {
            Self::Paste(_) => MAX_PASTE_BYTES,
            _ => input::MAX_INPUT_BYTES,
        }
    }
}

/// The host acknowledged the PTY write. Nothing here observes whether the CLI
/// consumed the bytes; that would be reliable delivery, which stays unavailable.
#[derive(Clone, Copy, Debug)]
pub struct InputReceipt {
    pub bytes: usize,
}

/// The pinned instance a lease-less HTTP input writes to. Raw name-only hosts
/// are deliberately absent: without a lease there is no downgrade by name.
pub enum UnleasedTarget<'a> {
    Native(&'a BoundTarget),
    Launch(&'a LaunchTarget),
}

impl UnleasedTarget<'_> {
    fn name(&self) -> &str {
        match self {
            Self::Native(target) => target.name(),
            Self::Launch(target) => target.name(),
        }
    }
}

/// Shared payload validation of the leased and lease-less HTTP input paths.
fn input_operation(payload: InputPayload) -> Result<(usize, ControlOp), TerminalError> {
    let bytes = payload.bytes();
    if bytes == 0 {
        return Err(TerminalError::new(
            400,
            "terminal_input_empty",
            "终端输入为空",
        ));
    }
    if bytes > payload.limit() {
        return Err(TerminalError::new(
            413,
            "terminal_input_too_large",
            "单次终端输入不能超过 1 MiB",
        ));
    }
    let operation = match payload {
        InputPayload::Text(text) => ControlOp::Send { text },
        InputPayload::Keys(keys) => ControlOp::Keys { keys },
        InputPayload::Paste(text) => ControlOp::Paste {
            text,
            bracketed: true,
        },
    };
    Ok((bytes, operation))
}

impl TerminalService {
    pub fn new(directory: PathBuf) -> Result<Self, TerminalError> {
        Self::with_limits(directory, BridgeLimits::default())
    }

    pub fn with_limits(directory: PathBuf, limits: BridgeLimits) -> Result<Self, TerminalError> {
        if directory.as_os_str().is_empty() {
            return Err(TerminalError::new(
                503,
                "terminal_config",
                "必须显式指定隔离的 ptyhost 目录",
            ));
        }
        let directory = directory.canonicalize().map_err(|_| {
            TerminalError::new(503, "terminal_config", "显式 ptyhost 目录不存在或不可访问")
        })?;
        if !directory.is_dir()
            || limits.max_input_bytes == 0
            || limits.max_input_bytes > 1024 * 1024
            || limits.max_host_frame_bytes == 0
            || limits.max_host_frame_bytes > 64 * 1024 * 1024
            || limits.operation_timeout.is_zero()
            || limits.operation_timeout > Duration::from_secs(30)
            || limits
                .partial_frame_timeout
                .is_some_and(|timeout| timeout.is_zero())
        {
            return Err(TerminalError::new(
                503,
                "terminal_config",
                "终端传输配置无效",
            ));
        }
        let client = HostClient::new(
            directory,
            Limits {
                max_line_bytes: 4 * 1024 * 1024, // ptyhost protocol::MAX_LINE; a 1 MiB send plus its guard envelope
                // The host wire carries a u32 length and Python applies no
                // smaller attachment-frame policy.
                max_frame_bytes: u32::MAX as usize,
                operation_timeout: limits.operation_timeout,
                partial_frame_timeout: limits.partial_frame_timeout,
            },
        )
        .map_err(host_error)?;
        Ok(Self {
            client,
            registry: Arc::new(Registry::new()?),
            limits,
            gates: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn limits(&self) -> &BridgeLimits {
        &self.limits
    }

    fn gate(&self, name: &str) -> Result<Arc<AsyncMutex<()>>, TerminalError> {
        let mut gates = self
            .gates
            .lock()
            .map_err(|_| TerminalError::new(503, "terminal_unavailable", "终端写入锁不可用"))?;
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(name).and_then(Weak::upgrade) {
            return Ok(gate);
        }
        // Strong references are held only by active connections and operations;
        // expired name entries cannot grow this map.
        let gate = Arc::new(AsyncMutex::new(()));
        gates.insert(name.to_owned(), Arc::downgrade(&gate));
        Ok(gate)
    }

    pub async fn hosts(&self) -> Result<Vec<SessionSummary>, TerminalError> {
        Ok(self
            .client
            .discover()
            .await
            .map_err(host_error)?
            .into_iter()
            .filter(|host| ownership::validate_name(&host.name).is_ok())
            .collect())
    }

    /// The claim itself never attaches/resizes the PTY. Probe the exact indexed
    /// endpoint first; absent/exited/unreachable hosts cannot consume leases.
    pub async fn claim(
        &self,
        name: &str,
        page: &str,
        ip: IpAddr,
        force: bool,
    ) -> Result<ClaimResponse, TerminalError> {
        ownership::validate_name(name)?;
        ownership::validate_page(page)?;
        let observation = self.client.probe(name).await.map_err(host_error)?;
        if observation.launch != LaunchState::Missing {
            return Err(ownership::OwnershipError::BindingMismatch.into());
        }
        if observation.exited {
            return Err(TerminalError::new(
                409,
                "terminal_exited",
                "终端 host 中的进程已退出",
            ));
        }
        let gate = self.gate(name)?;
        let _gate = gate.lock().await;
        Ok(self.registry.claim(name, page, ip, force)?)
    }

    pub fn cancel_reservation(&self, name: &str, page: &str, token: &str) {
        let _ = self.registry.release_reservation(name, page, token);
    }

    /// The caller supplies a uniquely catalog-resolved target. While holding
    /// the same per-name gate as attach/input, re-probe immutable identity and
    /// guard capability before publishing a lease. No native inventory I/O.
    pub async fn claim_bound(
        &self,
        target: &BoundTarget,
        page: &str,
        ip: IpAddr,
        force: bool,
    ) -> Result<ClaimResponse, TerminalError> {
        ownership::validate_name(target.name())?;
        ownership::validate_page(page)?;
        let gate = self.gate(target.name())?;
        let _gate = gate.lock().await;
        let observation = self.client.probe(target.name()).await.map_err(host_error)?;
        let fresh = BoundTarget::from_observation(
            &observation,
            target.source(),
            target.sid(),
            target.uid(),
        )
        .map_err(host_error)?;
        if fresh.name() != target.name()
            || fresh.instance_id() != target.instance_id()
            || fresh.origin_launch_id() != target.origin_launch_id()
        {
            return Err(host_error(ptyhost_client::Error::IdentityChanged));
        }
        Ok(self
            .registry
            .claim_bound(Arc::new(fresh), page, ip, force)?)
    }

    /// Only a trusted lifecycle receipt supplies this target. A launch lease
    /// is never a native-session lease, including when both metadata exist.
    pub async fn claim_launch(
        &self,
        target: Arc<LaunchTarget>,
        page: &str,
        ip: IpAddr,
        force: bool,
    ) -> Result<ClaimResponse, TerminalError> {
        ownership::validate_name(target.name())?;
        ownership::validate_page(page)?;
        let gate = self.gate(target.name())?;
        let _gate = gate.lock().await;
        self.registry.check_launch(&target)?;
        let observation = self.client.probe(target.name()).await.map_err(host_error)?;
        let fresh = LaunchTarget::from_observation(
            &observation,
            target.source(),
            target.launch_id(),
            target.instance_id(),
        )
        .map_err(host_error)?;
        Ok(self
            .registry
            .claim_launch(Arc::new(fresh), page, ip, force)?)
    }

    /// Permanently block this exact tuple for this service lifetime and revoke
    /// its input lease under the same gate as attach/write/claim. No host I/O or
    /// kill: even an offline old instance can have its local authority retired.
    /// Retired launch identities are retained for the service lifetime.
    pub async fn retire_launch(&self, target: Arc<LaunchTarget>) -> Result<(), TerminalError> {
        ownership::validate_name(target.name())?;
        let gate = self.gate(target.name())?;
        let _gate = gate.lock().await;
        Ok(self.registry.retire_launch(&target)?)
    }

    /// Whether a host record with this name exists in the explicit directory.
    /// This reads only the local record; it is not liveness or authorization.
    pub async fn has_host(&self, name: &str) -> Result<bool, TerminalError> {
        ownership::validate_name(name)?;
        Ok(self
            .client
            .session(name)
            .await
            .map_err(host_error)?
            .is_some())
    }

    /// Raw HTTP input under the same authority and per-name gate as the
    /// WebSocket write path: the caller's exact lease (reserved or bound), the
    /// pinned instance target sent through `guarded_v1`/launch guard, and no
    /// new write after a successful force claim. A raw lease re-probes like
    /// raw attach does. Timeout or transport failure after submission is
    /// ambiguous and is never retried; success is only the host's write ack.
    pub async fn send_input(
        &self,
        name: &str,
        page: &str,
        token: &str,
        expected: ExpectedTarget<'_>,
        payload: InputPayload,
    ) -> Result<InputReceipt, TerminalError> {
        ownership::validate_name(name)?;
        ownership::validate_page(page)?;
        let (bytes, operation) = input_operation(payload)?;
        self.request_under_lease(name, page, token, expected, operation)
            .await
            .map(|_| InputReceipt { bytes })
    }

    /// Raw HTTP input from a page that holds no lease for this terminal: a
    /// conversation view whose console is open elsewhere or not at all (the
    /// composer's Esc, a question card, a Grok text submit). It follows the
    /// delivery executor's ordinary-claimant rule instead of the Python
    /// backend's unauthenticated `send-keys`: under the per-name gate, any
    /// current lease — another page's console, a reservation, or a server
    /// send in flight — is the documented ownership conflict, and otherwise
    /// the write goes through the pinned instance exactly like a leased
    /// input. Nothing is reserved or minted: the gate already serializes this
    /// write against claims and leased input, so no token can leak or linger.
    pub async fn send_input_unleased(
        &self,
        target: UnleasedTarget<'_>,
        payload: InputPayload,
    ) -> Result<InputReceipt, TerminalError> {
        let name = target.name();
        ownership::validate_name(name)?;
        let (bytes, operation) = input_operation(payload)?;
        let gate = self.gate(name)?;
        let _gate = gate.lock().await;
        if let Some(owner) = self.registry.owner(name)? {
            return Err(TerminalError::new(
                409,
                "terminal_ownership",
                format!(
                    "终端控制权正由其他页面持有（{}）；请从持有控制台的页面发送，或先释放/接管该控制台",
                    owner.ip
                ),
            ));
        }
        let observation = self.client.probe(name).await.map_err(host_error)?;
        let result = match target {
            UnleasedTarget::Native(target) => {
                let fresh = BoundTarget::from_observation(
                    &observation,
                    target.source(),
                    target.sid(),
                    target.uid(),
                )
                .map_err(host_error)?;
                if fresh.name() != target.name()
                    || fresh.instance_id() != target.instance_id()
                    || fresh.origin_launch_id() != target.origin_launch_id()
                {
                    return Err(host_error(ptyhost_client::Error::IdentityChanged));
                }
                self.client.request_bound(&fresh, operation).await
            }
            UnleasedTarget::Launch(target) => {
                self.registry.check_launch(target)?;
                let fresh = LaunchTarget::from_observation(
                    &observation,
                    target.source(),
                    target.launch_id(),
                    target.instance_id(),
                )
                .map_err(host_error)?;
                self.client.request_launch(&fresh, operation).await
            }
        };
        result.map_err(input_error).map(|_| InputReceipt { bytes })
    }

    /// Batch 31 delivery driver: the host's screen model plus cursor and
    /// health counters, read under the exact lease and per-name gate like an
    /// input. The host first lets the model
    /// catch up with pending output; a nonzero `lag` means it did not.
    pub async fn capture_screen(
        &self,
        name: &str,
        page: &str,
        token: &str,
        expected: ExpectedTarget<'_>,
    ) -> Result<CaptureReply, TerminalError> {
        ownership::validate_name(name)?;
        ownership::validate_page(page)?;
        let reply = self
            .request_under_lease(
                name,
                page,
                token,
                expected,
                ControlOp::Capture {
                    kind: CaptureKind::Screen,
                    styled: true,
                    join: false,
                    lines: 0,
                },
            )
            .await?;
        match reply {
            ControlReply::Capture(capture) => Ok(capture),
            _ => Err(TerminalError::new(
                503,
                "terminal_unavailable",
                "终端 host 未返回屏幕捕获",
            )),
        }
    }

    /// Shared body of the HTTP input and delivery driver paths: operation
    /// permit, per-name gate, exact current lease, then
    /// one guarded host request whose failure after submission is ambiguous.
    async fn request_under_lease(
        &self,
        name: &str,
        page: &str,
        token: &str,
        expected: ExpectedTarget<'_>,
        operation: ControlOp,
    ) -> Result<ControlReply, TerminalError> {
        let gate = self.gate(name)?;
        let _gate = gate.lock().await;
        let target = self.registry.authorize_input(name, page, token, expected)?;
        let result = match &target {
            LeaseTarget::Native(bound) => self.client.request_bound(bound, operation).await,
            LeaseTarget::Launch(launch) => self.client.request_launch(launch, operation).await,
            LeaseTarget::Raw => {
                let observed = self.client.probe(name).await.map_err(input_error)?;
                if observed.launch != LaunchState::Missing {
                    return Err(ownership::OwnershipError::BindingMismatch.into());
                }
                if observed.exited {
                    return Err(exited());
                }
                self.client.request(name, operation).await
            }
        };
        result.map_err(input_error)
    }

    pub fn prepare(
        self: &Arc<Self>,
        name: &str,
        page: &str,
        token: &str,
    ) -> Result<PreparedAttachment, TerminalError> {
        self.prepare_target(name, page, token, ExpectedTarget::Raw)
    }

    pub fn prepare_bound(
        self: &Arc<Self>,
        name: &str,
        page: &str,
        token: &str,
        uid: &str,
        instance: &str,
    ) -> Result<PreparedAttachment, TerminalError> {
        self.prepare_target(name, page, token, ExpectedTarget::Native { uid, instance })
    }

    /// Consume only a launch reservation with both complete nonces. This does
    /// no host I/O; the existing bridge verifies launch_guard_v1 before exposing
    /// replay or sending input. It cannot consume raw/native reservations.
    pub fn prepare_launch(
        self: &Arc<Self>,
        name: &str,
        page: &str,
        token: &str,
        launch_id: &str,
        instance_id: &str,
    ) -> Result<PreparedAttachment, TerminalError> {
        self.prepare_target(
            name,
            page,
            token,
            ExpectedTarget::Launch {
                launch: launch_id,
                instance: instance_id,
            },
        )
    }

    fn prepare_target(
        self: &Arc<Self>,
        name: &str,
        page: &str,
        token: &str,
        expected: ExpectedTarget<'_>,
    ) -> Result<PreparedAttachment, TerminalError> {
        ownership::validate_name(name)?;
        let gate = self.gate(name)?;
        let bound = match expected {
            ExpectedTarget::Raw => self.registry.bind(name, page, token)?,
            ExpectedTarget::Native { uid, instance } => {
                self.registry.bind_bound(name, page, token, uid, instance)?
            }
            ExpectedTarget::Launch { launch, instance } => self
                .registry
                .bind_launch(name, page, token, launch, instance)?,
        };
        Ok(PreparedAttachment {
            service: self.clone(),
            guard: LeaseGuard {
                registry: self.registry.clone(),
                bound,
                gate,
            },
        })
    }
}

struct LeaseGuard {
    registry: Arc<Registry>,
    bound: BoundLease,
    gate: Arc<AsyncMutex<()>>,
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let _ = self.registry.release(&self.bound);
    }
}

/// Captured by the upgrade callback. If upgrading fails, dropping that callback
/// releases the bound lease without host I/O.
pub struct PreparedAttachment {
    service: Arc<TerminalService>,
    guard: LeaseGuard,
}

impl PreparedAttachment {
    pub async fn run(self, socket: WebSocket, size: TerminalSize, shutdown: CancellationToken) {
        let mut revoked = self.guard.bound.revocations();
        let attachment = tokio::select! {
            biased;
            signal = revocation(&mut revoked) => Err(Close::revoked(signal)),
            _ = shutdown.cancelled() => Err(Close::shutdown()),
            attachment = self.attach_current(size) => attachment,
        };
        let (mut sink, stream) = socket.split();
        let close = match attachment {
            Err(close) => close,
            Ok(attachment) => {
                {
                    let (reader, writer) = attachment.into_split();
                    let (send, receive) = mpsc::channel(OUTPUT_QUEUE);
                    // These futures stay inside one scope. Exiting it drops both
                    // host halves before attempting the browser close handshake.
                    let output = host_output(reader, send.clone());
                    let input = browser_input(stream, writer, send, &self.guard);
                    let sending = browser_output(&mut sink, receive);
                    tokio::pin!(output, input, sending);
                    tokio::select! {
                        biased;
                        signal = revocation(&mut revoked) => Close::revoked(signal),
                        _ = shutdown.cancelled() => Close::shutdown(),
                        close = output => close,
                        close = input => close,
                        close = sending => close,
                    }
                }
            }
        };
        // Do not hold ownership until the peer acknowledges its close. The
        // exact credential guard is safe even after a newer claim was installed.
        let _ = self.guard.registry.release(&self.guard.bound);
        close_browser(&mut sink, close).await;
    }

    async fn attach_current(
        &self,
        size: TerminalSize,
    ) -> Result<ptyhost_client::Attachment, Close> {
        let _gate = self.guard.gate.lock().await;
        current(&self.guard)?;
        let attachment = match self.guard.bound.lease_target() {
            LeaseTarget::Native(target) => {
                self.service.client.attach_bound(target, size, true).await
            }
            LeaseTarget::Launch(target) => {
                self.service.client.attach_launch(target, size, true).await
            }
            LeaseTarget::Raw => {
                // A previously raw reservation cannot attach a newly replaced
                // pending launch. Metadata absence at claim is not permanent.
                let observed = self
                    .service
                    .client
                    .probe(self.guard.bound.name())
                    .await
                    .map_err(|_| Close::new(1011, "attach failed: local host unavailable"))?;
                if observed.launch != LaunchState::Missing || observed.exited {
                    return Err(Close::new(
                        1011,
                        "attach failed: launch identity requires its own lease",
                    ));
                }
                self.service
                    .client
                    .attach(self.guard.bound.name(), size, true)
                    .await
            }
        };
        attachment.map_err(|_| {
            Close::new(
                1011,
                "attach failed: local host unavailable or identity changed",
            )
        })
    }
}

pub fn terminal_size(cols: u16, rows: u16) -> Result<TerminalSize, TerminalError> {
    if cols == 0 || rows == 0 {
        return Err(TerminalError::new(
            400,
            "terminal_size",
            "终端尺寸须为正整数",
        ));
    }
    TerminalSize::new(cols, rows).map_err(host_error)
}

struct Close {
    code: u16,
    reason: String,
    /// The `revoked` notice label for the replaced page: the new claimant's
    /// display address, or empty when it would only repeat the page's own.
    notice: Option<String>,
}

impl Close {
    fn new(code: u16, reason: &str) -> Self {
        Self {
            code,
            reason: reason.to_owned(),
            notice: None,
        }
    }
    fn shutdown() -> Self {
        Self::new(1001, "server shutdown")
    }
    fn revoked(signal: Option<Revocation>) -> Self {
        if signal
            .as_ref()
            .is_some_and(|signal| signal.reason == ownership::RevocationReason::LaunchRetired)
        {
            return Self::new(4002, "launch retired");
        }
        let Some(signal) = signal.filter(|signal| signal.notify) else {
            return Self::new(4001, "replaced");
        };
        let label = signal.new_ip.map(|ip| ip.to_string()).unwrap_or_default();
        Self {
            code: 4001,
            reason: format!("revoked:{label}"),
            notice: Some(label),
        }
    }
}

async fn revocation(receiver: &mut watch::Receiver<Option<Revocation>>) -> Option<Revocation> {
    loop {
        if let Some(signal) = receiver.borrow_and_update().clone() {
            return Some(signal);
        }
        if receiver.changed().await.is_err() {
            return None;
        }
    }
}

enum BrowserOutput {
    Data(Bytes),
    Flush,
    Finish(Close),
}

async fn enqueue(sender: &mpsc::Sender<BrowserOutput>, item: BrowserOutput) -> Result<(), Close> {
    sender
        .send(item)
        .await
        .map_err(|_| Close::new(1001, "browser disconnected"))
}

async fn host_output(mut reader: AttachReader, sender: mpsc::Sender<BrowserOutput>) -> Close {
    loop {
        match reader.next().await {
            Ok(Some(HostEvent::Data(bytes))) => {
                for chunk in bytes.chunks(OUTPUT_CHUNK) {
                    if let Err(close) =
                        enqueue(&sender, BrowserOutput::Data(Bytes::copy_from_slice(chunk))).await
                    {
                        return close;
                    }
                }
            }
            Ok(Some(HostEvent::Exit {
                output_complete: Some(false),
                reason,
                ..
            })) => {
                let reason = match reason {
                    Some(ptyhost_client::ExitReason::PtyDrainTimeout) => {
                        "host output incomplete: PTY drain timeout"
                    }
                    Some(ptyhost_client::ExitReason::PtyReadError) => {
                        "host output incomplete: PTY read failed"
                    }
                    _ => "host output incomplete",
                };
                return finish_output(&sender, Close::new(1011, reason)).await;
            }
            Ok(Some(HostEvent::Exit { .. })) => {
                return finish_output(&sender, Close::new(1000, "host exited")).await;
            }
            Ok(None) => {
                return finish_output(
                    &sender,
                    Close::new(1011, "host stream closed without exit marker"),
                )
                .await;
            }
            Err(_) => {
                return finish_output(&sender, Close::new(1011, "local host protocol failed"))
                    .await;
            }
        }
    }
}

async fn finish_output(sender: &mpsc::Sender<BrowserOutput>, close: Close) -> Close {
    if let Err(close) = enqueue(sender, BrowserOutput::Finish(close)).await {
        return close;
    }
    // Let the one browser writer drain all prior output before returning the
    // close result to the coordinator. Immediate EOF must not discard replay.
    std::future::pending().await
}

#[derive(Deserialize)]
#[serde(tag = "t")]
enum BrowserControl {
    #[serde(rename = "resize")]
    Resize { cols: u16, rows: u16 },
}

async fn browser_input(
    mut stream: SplitStream<WebSocket>,
    mut writer: AttachWriter,
    sender: mpsc::Sender<BrowserOutput>,
    guard: &LeaseGuard,
) -> Close {
    while let Some(message) = stream.next().await {
        let message = match message {
            Ok(message) => message,
            Err(_) => return Close::new(1009, "invalid or oversized browser frame"),
        };
        let write = match message {
            Message::Binary(bytes) => {
                if bytes.is_empty() {
                    continue;
                }
                HostInput::Data(bytes)
            }
            Message::Text(text) => {
                match serde_json::from_str::<BrowserControl>(&text) {
                    Ok(BrowserControl::Resize { cols, rows }) => match terminal_size(cols, rows) {
                        Ok(size) => HostInput::Resize(size),
                        Err(_) => HostInput::Data(Bytes::copy_from_slice(text.as_bytes())),
                    },
                    // Python treats every other text frame, including JSON
                    // that is not a valid resize, as literal PTY input.
                    Err(_) => HostInput::Data(Bytes::copy_from_slice(text.as_bytes())),
                }
            }
            Message::Close(_) => return Close::new(1000, "client closed"),
            Message::Ping(_) => {
                // Tungstenite queued the matching pong; flush via the one sink
                // owner instead of allowing independent concurrent WS writes.
                if let Err(close) = enqueue(&sender, BrowserOutput::Flush).await {
                    return close;
                }
                continue;
            }
            Message::Pong(_) => continue,
        };
        if let Err(close) = gated_write(guard, &mut writer, write).await {
            return close;
        }
    }
    Close::new(1000, "client disconnected")
}

enum HostInput {
    Data(Bytes),
    Resize(TerminalSize),
}

fn current(guard: &LeaseGuard) -> Result<(), Close> {
    if guard.registry.is_current(&guard.bound).unwrap_or(false) {
        return Ok(());
    }
    Err(Close::revoked(guard.bound.revocations().borrow().clone()))
}

async fn gated_write(
    guard: &LeaseGuard,
    writer: &mut AttachWriter,
    input: HostInput,
) -> Result<(), Close> {
    let _gate = guard.gate.lock().await;
    // The same async gate covers claim publication, attach and every write.
    // No new old-lease write can begin after a successful force claim.
    current(guard)?;
    let result = match input {
        HostInput::Data(bytes) => writer.send_data(&bytes).await,
        HostInput::Resize(size) => writer.resize(size).await,
    };
    if result.is_ok() {
        return Ok(());
    }
    // A frame prefix may have reached the host. Drop; do not retry or reuse.
    Err(Close::new(1011, "host input write failed"))
}

async fn browser_output(
    sink: &mut SplitSink<WebSocket, Message>,
    mut receiver: mpsc::Receiver<BrowserOutput>,
) -> Close {
    while let Some(item) = receiver.recv().await {
        let result = match item {
            BrowserOutput::Data(bytes) => sink.send(Message::Binary(bytes)).await,
            BrowserOutput::Flush => sink.flush().await,
            BrowserOutput::Finish(close) => return close,
        };
        if result.is_err() {
            return Close::new(1001, "browser disconnected");
        }
    }
    Close::new(1000, "host output closed")
}

async fn close_browser(sink: &mut SplitSink<WebSocket, Message>, close: Close) {
    let _ = timeout(CLOSE_TIMEOUT, async {
        if let Some(label) = close.notice {
            let notice = serde_json::json!({"t":"revoked","ip":label}).to_string();
            let _ = sink.send(Message::Text(notice.into())).await;
        }
        let _ = sink
            .send(Message::Close(Some(CloseFrame {
                code: close.code,
                reason: close.reason.into(),
            })))
            .await;
    })
    .await;
}

#[cfg(test)]
#[path = "service_bound_tests.rs"]
mod bound_tests;

#[cfg(test)]
#[path = "service_launch_tests.rs"]
mod launch_tests;

#[cfg(test)]
#[path = "service_native_binding_tests.rs"]
mod native_binding_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    fn token(registry: &Registry, name: &str, page: &str) -> String {
        registry
            .claim(name, page, "127.0.0.1".parse().unwrap(), false)
            .unwrap()
            .into_api_json()["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn gates_are_per_name_and_weak_entries_are_bounded() {
        let temporary = tempfile::tempdir().unwrap();
        let service = TerminalService::new(temporary.path().to_owned()).unwrap();
        let first = service.gate("first").unwrap();
        assert!(Arc::ptr_eq(&first, &service.gate("first").unwrap()));
        let second = service.gate("second").unwrap();
        let _held = first.lock().await;
        assert!(second.try_lock().is_ok());
        for index in 0..10_000 {
            drop(service.gate(&format!("expired-{index}")).unwrap());
        }
        assert!(service.gates.lock().unwrap().len() <= 3);
    }

    #[tokio::test]
    async fn prepared_connections_have_no_process_wide_quota() {
        let temporary = tempfile::tempdir().unwrap();
        let service = Arc::new(
            TerminalService::with_limits(temporary.path().to_owned(), BridgeLimits::default())
                .unwrap(),
        );
        let first = token(&service.registry, "first", "page");
        let second = token(&service.registry, "second", "page");
        let prepared = service.prepare("first", "page", &first).unwrap();
        let second_prepared = service.prepare("second", "page", &second).unwrap();
        drop(prepared);
        assert!(service.registry.owner("first").unwrap().is_none());
        drop(second_prepared);
        assert!(service.registry.owner("second").unwrap().is_none());
    }

    #[tokio::test]
    async fn force_claim_waits_for_the_same_io_gate_before_publishing() {
        let temporary = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let record = serde_json::json!({"name":"terminal","host_pid":42,"pid":43,
            "cols":80,"rows":24,"port":listener.local_addr().unwrap().port(),"token":"SYNTHETIC_ONLY"});
        tokio::fs::write(temporary.path().join("terminal.json"), record.to_string())
            .await
            .unwrap();
        let (probed, probe) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut line = String::new();
            BufReader::new(&mut stream)
                .read_line(&mut line)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&line).unwrap()["op"],
                "info"
            );
            let response = serde_json::json!({"ok":true,"info":record,"exited":false});
            stream
                .write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap();
            probed.send(()).unwrap();
        });
        let service = Arc::new(TerminalService::new(temporary.path().to_owned()).unwrap());
        let old = token(&service.registry, "terminal", "old-page");
        let prepared = service.prepare("terminal", "old-page", &old).unwrap();
        let gate = service.gate("terminal").unwrap();
        let in_flight_write = gate.lock().await;
        let mut claiming = {
            let service = service.clone();
            tokio::spawn(async move {
                service
                    .claim("terminal", "new-page", "192.0.2.2".parse().unwrap(), true)
                    .await
            })
        };
        probe.await.unwrap();
        assert!(
            timeout(Duration::from_millis(25), &mut claiming)
                .await
                .is_err()
        );
        assert!(service.registry.is_current(&prepared.guard.bound).unwrap());
        drop(in_flight_write);
        assert_eq!(claiming.await.unwrap().unwrap().status(), 200);
        assert!(!service.registry.is_current(&prepared.guard.bound).unwrap());
        // A stale attach fails under the same gate before contacting any host.
        assert_eq!(
            prepared
                .attach_current(TerminalSize::new(80, 24).unwrap())
                .await
                .err()
                .unwrap()
                .code,
            4001
        );
        peer.await.unwrap();
    }
}
