//! Reliable-send executor.
//!
//! Drives `DeliveryEngine` dispatch batches for Claude and Codex main sessions
//! on managed instances: every terminal mutation follows a durable commit
//! (persist → inspect → persist → paste → persist → Enter), the text and the
//! Enter are two separately persisted steps, and a crash between them leaves
//! the receipt uncertain instead of re-injecting. Native acknowledgment comes
//! only from the session's native file through the provider adapter; screen
//! text alone never confirms anything. Retry exists only for receipts the
//! domain calls retryable (a local waiter or a pre-write failure that was never
//! written); discard/dismiss retires a row without resending.
//!
//! The two providers share the lease, composer, paste/Enter and tracking
//! machinery; only the domain commands, the composer model and the native
//! adapter differ (`Provider`).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::{
    claude::{
        self, Command, Composer, ComposerState, Cursor, Payload, Prepared, Receipt, Request, Scope,
        State, Target,
    },
    claude_adapter as adapter, codex,
    codex_adapter::{self, Boundary, Delivered, Outcome, ReadPlan, ReplayClock, ReplayPolicy},
    driver::{
        ComposerKind, ComposerView, DeliveryTarget, DriverError, LeaseHandle, PageLease,
        TerminalDriver,
    },
    engine::{self, Action, ClaudeAction, CodexAction, DeliveryEngine, ReceiptInfo},
    service::{self, DeliveryService},
};
use crate::{
    sessions::{NativeFence, NativeInputRead},
    state::Reader,
};

/// `Target.ownership_epoch` for every managed-instance delivery: the guard
/// protocol generation, not a lease secret. The live lease is re-authorized by
/// the host at each write; this field only makes the persisted target explicit.
pub const OWNERSHIP_EPOCH: &str = "guarded_v1";

#[derive(Clone, Copy, Debug)]
pub struct ExecutorLimits {
    /// Concurrent HTTP send/draft/retry operations across all sessions.
    pub workers: usize,
    /// After this long without acknowledgment the
    /// fixed confirmation fence is re-read, at most once per interval.
    pub confirm_timeout: Duration,
    /// Automatic native tracking stops after this window; the receipt keeps
    /// its state and is annotated once with the domain's timeout issue.
    pub tracking_window: Duration,
    pub tick: Duration,
    /// Queued rows that could not be dispatched (unknown composer, lease held
    /// elsewhere) are re-dispatched no more often than this.
    pub redispatch_interval: Duration,
    /// How long paste verification waits for the composer to show the text.
    pub prepare_timeout: Duration,
}

impl Default for ExecutorLimits {
    fn default() -> Self {
        Self {
            workers: 4,
            confirm_timeout: Duration::from_secs(8),
            tracking_window: Duration::from_secs(3600),
            tick: Duration::from_millis(500),
            redispatch_interval: Duration::from_secs(3),
            prepare_timeout: Duration::from_millis(2500),
        }
    }
}

impl ExecutorLimits {
    /// The Codex adapter's replay schedule expressed in these limits.
    fn replay_policy(&self) -> ReplayPolicy {
        ReplayPolicy {
            confirm_timeout_ms: self.confirm_timeout.as_millis() as u64,
            replay_interval_ms: self.confirm_timeout.as_millis() as u64,
            track_window_ms: self.tracking_window.as_millis() as u64,
        }
    }
}

/// Resolves the unique managed instance bound to a native UID. Production
/// resolution walks the runtime catalog and lifecycle bindings; tests fake it.
pub trait TargetResolver: Send + Sync {
    fn resolve<'a>(&'a self, uid: &'a str) -> BoxFuture<'a, Result<DeliveryTarget, Failure>>;
}

/// Production resolver: the unique guard-capable managed host whose verified
/// native association names this UID (runtime catalog), authorized through
/// the lifecycle binding when the instance originates from a launch receipt.
/// Duplicate, unmatched, exited or unauthorized instances are "not linked".
/// It is provider-neutral: the bound target's own source decides nothing here
/// beyond the identity the runtime already verified.
pub struct ManagedResolver {
    pub runtime: Arc<crate::runtime::ManagedRuntime>,
    pub reader: Reader,
    pub lifecycle: Option<Arc<crate::lifecycle::service::LifecycleService>>,
    pub probes: Arc<Semaphore>,
}

fn unlinked() -> Failure {
    Failure::new(
        409,
        "terminal_unlinked",
        "终端会话未连接：该会话没有唯一关联的运行中受管实例",
    )
}

impl TargetResolver for ManagedResolver {
    fn resolve<'a>(&'a self, uid: &'a str) -> BoxFuture<'a, Result<DeliveryTarget, Failure>> {
        Box::pin(async move {
            let _permit = self
                .probes
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| Failure::new(503, "runtime_unavailable", "受控进程观察已关闭"))?;
            let catalog = self
                .reader
                .run(|store| store.native_catalog())
                .await
                .map_err(api_failure)?;
            let observed = self.runtime.observe(&catalog).await.map_err(|_| {
                Failure::new(
                    503,
                    "runtime_unavailable",
                    "受控 host 目录不可用或超出观察预算",
                )
            })?;
            let mut matches = observed
                .hosts
                .iter()
                .filter_map(|host| host.bound_target())
                .filter(|target| target.uid() == uid);
            let Some(target) = matches.next() else {
                return Err(unlinked());
            };
            if matches.next().is_some() {
                return Err(unlinked());
            }
            if target.origin_launch_id().is_some() {
                let lifecycle = self.lifecycle.as_ref().ok_or_else(unlinked)?;
                lifecycle
                    .authorize_native(target)
                    .await
                    .map_err(|_| unlinked())?;
            }
            Ok(DeliveryTarget {
                name: target.name().to_owned(),
                uid: uid.to_owned(),
                instance_id: target.instance_id().to_owned(),
                bound: Arc::new(target.clone()),
            })
        })
    }
}

/// Failure: `{error, code, ...extra}` with an HTTP status.
#[derive(Clone, Debug)]
pub struct Failure {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub extra: Value,
}

impl Failure {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            extra: Value::Null,
        }
    }
    pub fn reply(self) -> Reply {
        let mut body = json!({"error": self.message, "code": self.code});
        merge(&mut body, self.extra);
        Reply {
            status: self.status,
            body,
        }
    }
}

impl From<DriverError> for Failure {
    fn from(error: DriverError) -> Self {
        let mut failure = Failure::new(error.status, error.code, error.message);
        if let Some(ip) = error.owner_ip {
            failure.extra = json!({"owner": {"ip": ip}});
        }
        failure
    }
}

impl From<service::Error> for Failure {
    fn from(error: service::Error) -> Self {
        match error {
            service::Error::Closed => Failure::new(503, "delivery_closed", "发送账本服务已关闭"),
            service::Error::Engine(engine::Error::Frozen) => Failure::new(
                503,
                "delivery_unavailable",
                "发送账本已冻结；需要重新打开服务，不会返回空队列代替错误",
            ),
            service::Error::Engine(engine::Error::Claude(claude::Error::Capacity))
            | service::Error::Engine(engine::Error::Codex(codex::Error::Capacity)) => Failure::new(
                429,
                "delivery_capacity",
                "发送账本已满或该会话待确认消息过多",
            ),
            service::Error::Engine(engine::Error::Codex(codex::Error::WrongState)) => Failure::new(
                409,
                "delivery_busy",
                "该会话有另一条消息正处于注入边界；请稍后再发送",
            ),
            other => Failure::new(
                503,
                "delivery_unavailable",
                format!("发送账本操作失败：{other}"),
            ),
        }
    }
}

impl From<engine::Error> for Failure {
    fn from(error: engine::Error) -> Self {
        service::Error::Engine(error).into()
    }
}

/// Reader failures keep their code; an unknown session is a 400.
fn api_failure(error: crate::error::ApiError) -> Failure {
    if error.code == "session_error" && error.status.as_u16() == 404 {
        return Failure::new(
            400,
            "session_error",
            "服务端发送账本只用于已有 Claude/Codex 会话",
        );
    }
    Failure::new(error.status.as_u16(), error.code, error.message)
}

pub struct Reply {
    pub status: u16,
    pub body: Value,
}

fn merge(body: &mut Value, extra: Value) {
    if let (Some(target), Some(fields)) = (body.as_object_mut(), extra.as_object()) {
        for (key, value) in fields {
            target.insert(key.clone(), value.clone());
        }
    }
}

#[derive(Clone, Debug)]
pub struct SendRequest {
    pub uid: String,
    pub agent: String,
    pub name: String,
    pub text: String,
    pub media: Vec<Value>,
    pub request_id: String,
    pub overwrite_draft: String,
    pub page_lease: Option<PageLease>,
}

/// The delivery domain a session belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    fn label(self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::Codex => "Codex",
        }
    }
    fn composer(self) -> ComposerKind {
        match self {
            Provider::Claude => ComposerKind::Claude,
            Provider::Codex => ComposerKind::Codex,
        }
    }
}

/// A main-session send target resolved from the opened main view: the native
/// identity the domain scopes receipts to, never a display SID or name.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Session {
    provider: Provider,
    uid: String,
    session_id: String,
    agent_id: Option<String>,
}

impl Session {
    fn claude_scope(&self) -> Scope {
        Scope {
            uid: self.uid.clone(),
            session_id: self.session_id.clone(),
            agent_id: self.agent_id.clone(),
        }
    }
}

struct Applied {
    actions: Vec<Action>,
    replays: Vec<ReceiptInfo>,
}

#[derive(Default)]
struct Tracking {
    submitted: HashMap<String, TrackEntry>,
    redispatch: HashMap<String, Instant>,
    /// Codex receipts follow the adapter's `ReplayClock`:
    /// one clock per tracked receipt, dropped on retirement.
    codex: HashMap<String, CodexTrack>,
}

struct TrackEntry {
    since: Instant,
    last_replay: Option<Instant>,
    timed_out: bool,
}

#[derive(Default)]
struct CodexTrack {
    clock: ReplayClock,
    expired: bool,
}

pub struct DeliveryExecutor {
    service: Arc<DeliveryService>,
    driver: Arc<dyn TerminalDriver>,
    resolver: Arc<dyn TargetResolver>,
    reader: Reader,
    limits: ExecutorLimits,
    scopes: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    admission: Arc<Semaphore>,
    tracking: Mutex<Tracking>,
    stop: CancellationToken,
}

impl DeliveryExecutor {
    /// Builds the executor. `spawn_tracker` must be called from an async
    /// context to start native tracking; it stops with `shutdown`.
    pub fn start(
        service: Arc<DeliveryService>,
        driver: Arc<dyn TerminalDriver>,
        resolver: Arc<dyn TargetResolver>,
        reader: Reader,
        limits: ExecutorLimits,
        shutdown: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            service,
            driver,
            resolver,
            reader,
            limits,
            scopes: Mutex::new(HashMap::new()),
            admission: Arc::new(Semaphore::new(limits.workers.clamp(1, 16))),
            tracking: Mutex::new(Tracking::default()),
            stop: shutdown.child_token(),
        })
    }

    /// Spawns the background native-acknowledgment tracker on the current
    /// Tokio runtime; it stops when `shutdown` is cancelled. Call once from an
    /// async context after construction.
    pub fn spawn_tracker(self: &Arc<Self>) {
        tokio::spawn(track(Arc::downgrade(self)));
    }

    pub fn shutdown(&self) {
        self.stop.cancel();
    }

    fn scope_lock(&self, uid: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut scopes = self.scopes.lock().unwrap_or_else(|p| p.into_inner());
        scopes.retain(|_, lock| Arc::strong_count(lock) > 1);
        scopes.entry(uid.to_owned()).or_default().clone()
    }

    async fn admit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, Failure> {
        if self.stop.is_cancelled() {
            return Err(Failure::new(503, "shutdown", "服务正在关闭"));
        }
        self.admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Failure::new(503, "shutdown", "服务正在关闭"))
    }

    /// Apply one Claude domain command. The outer error is a service failure;
    /// the inner one is the engine/domain verdict (stale, busy, unproven...).
    /// The batch is claimed inside the exclusive closure so no other engine
    /// caller can observe a pending batch; every claimed action is then
    /// handled or reported by this executor.
    async fn try_apply(&self, command: Command) -> Result<Result<Applied, engine::Error>, Failure> {
        Ok(self
            .service
            .with_engine(move |engine| -> Result<Applied, engine::Error> {
                let batch = engine.apply_claude(command)?;
                let replays = batch.replays().to_vec();
                let actions = batch.claim()?;
                Ok(Applied { actions, replays })
            })
            .await?)
    }

    async fn apply(&self, command: Command) -> Result<Applied, Failure> {
        Ok(self.try_apply(command).await??)
    }

    /// Same as `try_apply` for the Codex domain (main session only).
    async fn try_apply_codex(
        &self,
        command: codex::Command,
    ) -> Result<Result<Applied, engine::Error>, Failure> {
        Ok(self
            .service
            .with_engine(move |engine| -> Result<Applied, engine::Error> {
                let batch = engine.apply_codex(command, None)?;
                let replays = batch.replays().to_vec();
                let actions = batch.claim()?;
                Ok(Applied { actions, replays })
            })
            .await?)
    }

    async fn apply_codex(&self, command: codex::Command) -> Result<Applied, Failure> {
        Ok(self.try_apply_codex(command).await??)
    }

    async fn engine<T, F>(&self, work: F) -> Result<T, Failure>
    where
        T: Send + 'static,
        F: FnOnce(&mut DeliveryEngine) -> Result<T, engine::Error> + Send + 'static,
    {
        Ok(self.service.with_engine(work).await??)
    }

    /// The provider and native identity of a send target, from the same
    /// fresh, restamped inventory history reads use. Only Claude and Codex
    /// main sessions are executor targets; Grok and child agents are typed
    /// `501`s.
    async fn session(&self, uid: &str, agent: &str) -> Result<Session, Failure> {
        let scope = {
            let (uid, agent) = (uid.to_owned(), agent.to_owned());
            self.reader
                .run(move |store| store.native_scope(&uid, &agent))
                .await
                .map_err(api_failure)?
        };
        let provider = match scope.source.as_str() {
            "claude" => Provider::Claude,
            "codex" => Provider::Codex,
            _ => {
                return Err(Failure::new(
                    501,
                    "delivery_source_unsupported",
                    "此来源的可靠发送尚未接入 Rust 执行器",
                ));
            }
        };
        Ok(Session {
            provider,
            uid: scope.uid,
            session_id: scope.session_id,
            agent_id: scope.agent_id,
        })
    }

    /// Claude native read: the current fence and the human inputs after it.
    async fn native(
        &self,
        uid: &str,
        from: Option<NativeFence>,
    ) -> Result<NativeInputRead, Failure> {
        let session = self.session(uid, "").await?;
        if session.provider != Provider::Claude {
            return Err(Failure::new(
                501,
                "delivery_source_unsupported",
                "此来源的可靠发送尚未接入 Rust 执行器",
            ));
        }
        let uid = uid.to_owned();
        self.reader
            .run(move |store| store.claude_native_inputs(&uid, from.as_ref()))
            .await
            .map_err(api_failure)
    }

    /// Codex fixed-boundary capture: the committed checkpoint of the frozen
    /// main-session view (`ViewSnapshot::native_checkpoint`), taken before
    /// any terminal write.
    async fn codex_checkpoint(&self, uid: &str) -> Result<codex::NativeCursor, Failure> {
        let uid = uid.to_owned();
        let checkpoint = self
            .reader
            .run(move |store| Ok(store.snapshot(&uid, "")?.native_checkpoint()))
            .await
            .map_err(api_failure)?;
        if checkpoint.source != "codex" || checkpoint.agent_id.is_some() {
            return Err(Failure::new(
                501,
                "delivery_source_unsupported",
                "可靠发送只支持 Codex 主会话",
            ));
        }
        Ok(codex::NativeCursor {
            source_identity: checkpoint.source_identity,
            position: checkpoint.position,
            head: checkpoint.head,
            anchor: checkpoint.anchor,
        })
    }

    async fn outbox(&self, scope: &Scope) -> Result<Value, Failure> {
        let scope = scope.clone();
        let outbox = self
            .engine(move |engine| engine.claude_outbox(&scope))
            .await?;
        serde_json::to_value(outbox)
            .map_err(|_| Failure::new(500, "delivery_encoding", "发送账本无法编码"))
    }

    async fn codex_outbox(&self, uid: &str) -> Result<Value, Failure> {
        let uid = uid.to_owned();
        let outbox = self
            .engine(move |engine| engine.codex_outbox(&uid, None))
            .await?;
        serde_json::to_value(outbox)
            .map_err(|_| Failure::new(500, "delivery_encoding", "发送账本无法编码"))
    }

    async fn session_outbox(&self, session: &Session) -> Result<Value, Failure> {
        match session.provider {
            Provider::Claude => self.outbox(&session.claude_scope()).await,
            Provider::Codex => self.codex_outbox(&session.uid).await,
        }
    }

    async fn item(&self, id: &str, outbox: &Value) -> Result<Value, Failure> {
        if let Some(row) = outbox["outbox"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"] == id))
        {
            return Ok(row.clone());
        }
        let id = id.to_owned();
        let receipt = self
            .engine(move |engine| engine.claude_receipt(&id))
            .await?
            .ok_or_else(|| Failure::new(404, "delivery_missing", "待核对消息不存在"))?;
        Ok(hidden_item(&receipt))
    }

    async fn codex_item(&self, id: &str, outbox: &Value) -> Result<Value, Failure> {
        if let Some(row) = outbox["outbox"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"] == id))
        {
            return Ok(row.clone());
        }
        let id = id.to_owned();
        let receipt = self
            .engine(move |engine| engine.codex_receipt(&id))
            .await?
            .ok_or_else(|| Failure::new(404, "delivery_missing", "待发送消息不存在"))?;
        Ok(hidden_codex_item(&receipt))
    }

    async fn session_item(&self, session: &Session, id: &str) -> Result<(Value, Value), Failure> {
        let outbox = self.session_outbox(session).await?;
        let item = match session.provider {
            Provider::Claude => self.item(id, &outbox).await?,
            Provider::Codex => self.codex_item(id, &outbox).await?,
        };
        Ok((item, outbox))
    }

    async fn reply_with_outbox(
        &self,
        status: u16,
        mut body: Value,
        session: &Session,
    ) -> Result<Reply, Failure> {
        let outbox = self.session_outbox(session).await?;
        merge(&mut body, outbox);
        Ok(Reply { status, body })
    }

    async fn reply_item(&self, session: &Session, id: &str) -> Result<Reply, Failure> {
        let (item, outbox) = self.session_item(session, id).await?;
        let mut body = json!({"ok": true, "item": item});
        merge(&mut body, outbox);
        Ok(Reply { status: 200, body })
    }

    // ---- HTTP operations -------------------------------------------------

    pub async fn send(&self, request: SendRequest) -> Reply {
        let permit = match self.admit().await {
            Ok(permit) => permit,
            Err(failure) => return failure.reply(),
        };
        let lock = self.scope_lock(&request.uid);
        let _scope = lock.lock().await;
        let _permit = permit;
        match self.send_inner(request).await {
            Ok(reply) => reply,
            Err(failure) => failure.reply(),
        }
    }

    async fn send_inner(&self, request: SendRequest) -> Result<Reply, Failure> {
        let request_id = normalize_request_id(&request.request_id)?;
        if request.text.trim().is_empty() {
            return Err(Failure::new(
                400,
                "invalid_send",
                "缺少 uid、终端名或消息正文",
            ));
        }
        let session = self.session(&request.uid, &request.agent).await?;
        let target = self.resolver.resolve(&session.uid).await?;
        if target.name != request.name {
            return Err(Failure::new(
                409,
                "terminal_unlinked",
                format!(
                    "{} 终端会话未连接：请求的终端名不是该会话唯一关联的受管实例",
                    session.provider.label()
                ),
            ));
        }
        match session.provider {
            Provider::Claude => self.send_claude(session, target, request, request_id).await,
            Provider::Codex => self.send_codex(session, target, request, request_id).await,
        }
    }

    async fn send_claude(
        &self,
        session: Session,
        target: DeliveryTarget,
        request: SendRequest,
        request_id: String,
    ) -> Result<Reply, Failure> {
        let scope = session.claude_scope();
        let ledger_target = Target {
            host_instance: target.instance_id.clone(),
            terminal_id: target.name.clone(),
            ownership_epoch: OWNERSHIP_EPOCH.into(),
        };
        let payload = Payload {
            scope: scope.clone(),
            target: ledger_target,
            text: request.text.clone(),
            attachments: request
                .media
                .iter()
                .cloned()
                .map(claude::Attachment)
                .collect(),
        };
        let existing = {
            let id = request_id.clone();
            self.engine(move |engine| engine.claude_receipt(&id))
                .await?
        };
        let mut consent_pending = false;
        if let Some(receipt) = existing {
            if receipt.request.payload.scope != scope
                || receipt.request.payload.text != request.text
            {
                return Err(Failure::new(
                    400,
                    "request_conflict",
                    "重复发送 ID 对应了不同消息",
                ));
            }
            if receipt.state == State::DraftConflict
                && !request.overwrite_draft.is_empty()
                && receipt.draft_token.as_deref() == Some(request.overwrite_draft.as_str())
            {
                consent_pending = true;
            } else {
                // A repeated HTTP request after a lost response is a status
                // lookup, never permission to paste the prompt again.
                return self.reply_item(&session, &request_id).await;
            }
        }
        self.cancel_stale_conflicts(&scope, &request_id).await?;
        let lease = self
            .driver
            .acquire(&target, request.page_lease.as_ref())
            .await?;
        let result = self
            .send_under_lease(
                &lease,
                &session,
                payload,
                &request,
                &request_id,
                consent_pending,
            )
            .await;
        self.driver.release(lease).await;
        result
    }

    async fn send_under_lease(
        &self,
        lease: &LeaseHandle,
        session: &Session,
        payload: Payload,
        request: &SendRequest,
        request_id: &str,
        consent_pending: bool,
    ) -> Result<Reply, Failure> {
        let scope = session.claude_scope();
        let request_id = request_id.to_owned();
        let first = if consent_pending {
            Command::ConfirmDraft {
                id: request_id.clone(),
                scope: scope.clone(),
                token: request.overwrite_draft.clone(),
            }
        } else {
            // Consent is checked and the approved
            // draft cleared before anything is persisted, so a refused or
            // changed draft leaves no receipt behind.
            if let Some(conflict) = self
                .overwrite_draft(ComposerKind::Claude, lease, &request.overwrite_draft)
                .await?
            {
                return self.reply_with_outbox(409, conflict, session).await;
            }
            let applied = self
                .apply(Command::Enqueue {
                    request: Request {
                        id: request_id.clone(),
                        payload,
                    },
                    now_ms: now_ms(),
                })
                .await?;
            if !applied.replays.is_empty() {
                return self.reply_item(session, &request_id).await;
            }
            Command::DispatchNext {
                scope: scope.clone(),
            }
        };
        let conflict = self
            .dispatch(lease, &scope, &request.overwrite_draft, first)
            .await?;
        if let Some(token) = conflict {
            return self
                .reply_with_outbox(
                    409,
                    json!({"draft_state": "editing", "draft_conflict": true, "draft_token": token}),
                    session,
                )
                .await;
        }
        self.reply_item(session, &request_id).await
    }

    /// Returns the 409 body when the
    /// composer holds an unapproved draft or the approved one could not be
    /// cleared; `None` means the composer is empty/unknown and sending may go on.
    async fn overwrite_draft(
        &self,
        kind: ComposerKind,
        lease: &LeaseHandle,
        expected_token: &str,
    ) -> Result<Option<Value>, Failure> {
        let view = self.inspect(kind, lease).await?;
        if view.state != ComposerState::Editing {
            return Ok(None);
        }
        if expected_token.is_empty() || expected_token != view.screen_token {
            return Ok(Some(draft_conflict(&view)));
        }
        self.driver.keys(lease, &["C-u", "C-k"]).await?;
        for delay in [30, 50, 80, 130, 210] {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if self.inspect(kind, lease).await?.state == ComposerState::Empty {
                return Ok(None);
            }
        }
        Ok(Some(json!({"error": "未能确认终端草稿已清空，消息未发送"})))
    }

    async fn inspect(
        &self,
        kind: ComposerKind,
        lease: &LeaseHandle,
    ) -> Result<ComposerView, Failure> {
        let capture = self.driver.capture(lease).await?;
        Ok(super::driver::inspect_for(kind, &capture))
    }

    async fn cancel_stale_conflicts(&self, scope: &Scope, keep: &str) -> Result<(), Failure> {
        // A draft conflict the page never resolved would block every later
        // request in this scope (FIFO); a new submission abandons it.
        let rows = {
            let scope = scope.clone();
            self.engine(move |engine| engine.claude_receipts(Some(&scope)))
                .await?
        };
        for row in rows {
            if row.request.id != keep && !row.attempted && row.state == State::DraftConflict {
                let _ = self
                    .apply(Command::Cancel {
                        id: row.request.id.clone(),
                        scope: scope.clone(),
                    })
                    .await;
            }
        }
        Ok(())
    }

    /// One FIFO dispatch of the scope's oldest local waiter under the lease,
    /// starting from `first` (DispatchNext, or ConfirmDraft when the request
    /// carries the consent token of an existing conflict). Returns the new
    /// draft token when the composer holds an unapproved draft.
    async fn dispatch(
        &self,
        lease: &LeaseHandle,
        scope: &Scope,
        approved: &str,
        first: Command,
    ) -> Result<Option<String>, Failure> {
        let kind = ComposerKind::Claude;
        let mut next = Some(first);
        for _ in 0..3 {
            let Some(command) = next.take() else {
                return Ok(None);
            };
            let applied = match self.try_apply(command).await? {
                Ok(applied) => applied,
                // Nothing dispatchable (no local waiter, or another critical
                // section on this terminal): not a failure of this request.
                Err(engine::Error::Claude(claude::Error::Missing | claude::Error::Busy)) => {
                    return Ok(None);
                }
                Err(engine::Error::Claude(claude::Error::Conflict)) => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            let Some(Action::Claude(ClaudeAction::InspectComposer { operation, target })) =
                applied.actions.into_iter().next()
            else {
                return Ok(None);
            };
            let id = operation.request_id.clone();
            let view = match self.inspect(kind, lease).await {
                Ok(view) => view,
                Err(_) => unknown_view(),
            };
            let cursor = match self.native(&scope.uid, None).await {
                Ok(read) => adapter::cursor_of(&read.current),
                Err(_) => Cursor {
                    source_identity: "unavailable".into(),
                    offset: 0,
                    head: "unavailable".into(),
                    anchor: "unavailable".into(),
                },
            };
            let state = if cursor.head == "unavailable" {
                ComposerState::Unknown
            } else {
                view.state
            };
            let applied = self
                .apply(Command::ComposerObserved {
                    operation,
                    composer: Composer {
                        target,
                        state,
                        frame_token: view.screen_token.clone(),
                        native_cursor: cursor,
                    },
                })
                .await?;
            let Some(action) = applied.actions.into_iter().next() else {
                let receipt = {
                    let id = id.clone();
                    self.engine(move |engine| engine.claude_receipt(&id))
                        .await?
                };
                let Some(receipt) = receipt else {
                    return Ok(None);
                };
                if receipt.state != State::DraftConflict {
                    self.note_redispatch(&id);
                    return Ok(None);
                }
                let token = receipt.draft_token.clone().unwrap_or_default();
                if !approved.is_empty() && approved == token {
                    next = Some(Command::ConfirmDraft {
                        id,
                        scope: scope.clone(),
                        token,
                    });
                    continue;
                }
                return Ok(Some(token));
            };
            let Action::Claude(ClaudeAction::Prepare {
                operation,
                payload,
                expected_frame,
                overwrite,
            }) = action
            else {
                return Ok(None);
            };
            match self
                .prepare(
                    kind,
                    lease,
                    &view,
                    &payload.text,
                    &expected_frame,
                    overwrite,
                )
                .await
            {
                Err(reason) => {
                    self.apply(Command::InjectionFailed { operation, reason })
                        .await?;
                    self.track(&id);
                    return Ok(None);
                }
                Ok((frame_token, prepared_view)) => {
                    let evidence = Prepared {
                        target: payload.target.clone(),
                        observed_text: payload.text.clone(),
                        observed_attachments: payload.attachments.clone(),
                        frame_token,
                    };
                    let applied = self
                        .apply(Command::Prepared {
                            operation,
                            evidence,
                        })
                        .await?;
                    let Some(Action::Claude(ClaudeAction::Enter {
                        operation,
                        expected_frame,
                        ..
                    })) = applied.actions.into_iter().next()
                    else {
                        // Prepared evidence was rejected: the row is uncertain.
                        self.track(&id);
                        return Ok(None);
                    };
                    let finished = self
                        .enter(kind, lease, &prepared_view, &expected_frame)
                        .await;
                    let command = match finished {
                        Ok(transport_returned) => Command::InjectionFinished {
                            operation,
                            transport_returned,
                        },
                        Err(reason) => Command::InjectionFailed { operation, reason },
                    };
                    self.apply(command).await?;
                    self.track(&id);
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    /// Conditional preparation: recheck the frame under the lease, clear only
    /// an approved draft, paste, then require the composer to visibly hold the
    /// exact text (soft wraps ignored) before reporting `Prepared`. Returns the
    /// observed frame token and the prepared view.
    async fn prepare(
        &self,
        kind: ComposerKind,
        lease: &LeaseHandle,
        previous: &ComposerView,
        text: &str,
        expected_frame: &str,
        overwrite: bool,
    ) -> Result<(String, ComposerView), String> {
        let label = match kind {
            ComposerKind::Claude => "Claude",
            ComposerKind::Codex => "Codex",
        };
        let view = self
            .inspect(kind, lease)
            .await
            .map_err(|failure| format!("写入前无法读取终端画面：{}", failure.message))?;
        if !frame_ok(expected_frame, previous, &view) {
            return Err(format!("写入前 {label} 编辑器画面已变化，未粘贴"));
        }
        let mut previous = view;
        if overwrite {
            self.driver
                .keys(lease, &["C-u", "C-k"])
                .await
                .map_err(|error| format!("清空已确认草稿失败：{}", error.message))?;
            let mut cleared = false;
            for delay in [30, 50, 80, 130, 210] {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                let view = self
                    .inspect(kind, lease)
                    .await
                    .map_err(|failure| failure.message)?;
                if view.state == ComposerState::Empty {
                    previous = view;
                    cleared = true;
                    break;
                }
            }
            if !cleared {
                return Err("未能确认已批准的草稿被清空；未粘贴".into());
            }
        }
        self.driver.paste(lease, text).await.map_err(|error| {
            if error.ambiguous {
                format!("粘贴结果不明：{}", error.message)
            } else {
                format!("粘贴失败：{}", error.message)
            }
        })?;
        let deadline = Instant::now() + self.limits.prepare_timeout;
        loop {
            tokio::time::sleep(Duration::from_millis(60)).await;
            let view = self
                .inspect(kind, lease)
                .await
                .map_err(|failure| failure.message)?;
            if !view.lagging
                && !view.pasting
                && view.dropped == previous.dropped
                && view.state == ComposerState::Editing
                && view.text.as_deref().is_some_and(|observed| {
                    super::driver::same_text_ignoring_whitespace(observed, text)
                })
            {
                let frame_token = view.screen_token.clone();
                return Ok((frame_token, view));
            }
            if Instant::now() >= deadline {
                return Err(format!("未观察到完整正文进入 {label} 编辑器；未发送 Enter"));
            }
        }
    }

    /// Conditional Enter: the composer must still show the prepared frame.
    /// `Ok(false)` means the host did not acknowledge (write may have happened).
    async fn enter(
        &self,
        kind: ComposerKind,
        lease: &LeaseHandle,
        previous: &ComposerView,
        expected_frame: &str,
    ) -> Result<bool, String> {
        let view = self
            .inspect(kind, lease)
            .await
            .map_err(|failure| format!("发送 Enter 前无法读取终端画面：{}", failure.message))?;
        if view.pasting {
            return Err("发送 Enter 前仍在粘贴；未发送 Enter".into());
        }
        if !frame_ok(expected_frame, previous, &view) {
            return Err("发送 Enter 前编辑器画面已变化；未发送 Enter".into());
        }
        match self.driver.keys(lease, &["Enter"]).await {
            Ok(()) => Ok(true),
            Err(error) if error.ambiguous => Ok(false),
            Err(error) => Err(format!("发送 Enter 失败：{}", error.message)),
        }
    }

    // ---- Codex ---------------------------------------------------------------

    /// Replay lookup, draft consent, then
    /// `Submit` (persisted before any terminal access) followed by the
    /// inspect → prepare → Enter chain under the lease. Codex owns follow-up
    /// queueing while working, so nothing here waits for an idle TUI.
    async fn send_codex(
        &self,
        session: Session,
        target: DeliveryTarget,
        request: SendRequest,
        request_id: String,
    ) -> Result<Reply, Failure> {
        let ledger_target = codex::Target {
            host_instance: target.instance_id.clone(),
            session_id: session.session_id.clone(),
            ownership_epoch: OWNERSHIP_EPOCH.into(),
        };
        let payload = codex::Payload {
            uid: session.uid.clone(),
            target: ledger_target,
            text: request.text.clone(),
            media: request.media.iter().cloned().map(codex::MediaRef).collect(),
        };
        let existing = {
            let id = request_id.clone();
            self.engine(move |engine| engine.codex_receipt(&id)).await?
        };
        let mut consent_pending = false;
        if let Some(receipt) = existing {
            if receipt.request.payload.uid != session.uid
                || receipt.request.payload.text != request.text
            {
                return Err(Failure::new(
                    400,
                    "request_conflict",
                    "重复发送 ID 对应了不同消息",
                ));
            }
            if receipt.state == codex::State::DraftConflict
                && !request.overwrite_draft.is_empty()
                && receipt.draft_token.as_deref() == Some(request.overwrite_draft.as_str())
            {
                consent_pending = true;
            } else {
                // A replay after a lost HTTP response is only a status read;
                // the first request already crossed (or approached) the host.
                return self.reply_item(&session, &request_id).await;
            }
        }
        let lease = self
            .driver
            .acquire(&target, request.page_lease.as_ref())
            .await?;
        let result = self
            .send_codex_under_lease(
                &lease,
                &session,
                payload,
                &request,
                &request_id,
                consent_pending,
            )
            .await;
        self.driver.release(lease).await;
        result
    }

    async fn send_codex_under_lease(
        &self,
        lease: &LeaseHandle,
        session: &Session,
        payload: codex::Payload,
        request: &SendRequest,
        request_id: &str,
        consent_pending: bool,
    ) -> Result<Reply, Failure> {
        let first = if consent_pending {
            codex::Command::ConfirmDraft {
                request_id: request_id.to_owned(),
                uid: session.uid.clone(),
                token: request.overwrite_draft.clone(),
            }
        } else {
            if let Some(conflict) = self
                .overwrite_draft(ComposerKind::Codex, lease, &request.overwrite_draft)
                .await?
            {
                return self.reply_with_outbox(409, conflict, session).await;
            }
            codex::Command::Submit {
                request: codex::Request {
                    request_id: request_id.to_owned(),
                    payload,
                },
                now_ms: now_ms(),
            }
        };
        let conflict = self
            .dispatch_codex(lease, session, &request.overwrite_draft, first)
            .await?;
        if let Some(token) = conflict {
            return self
                .reply_with_outbox(
                    409,
                    json!({"draft_state": "editing", "draft_conflict": true, "draft_token": token}),
                    session,
                )
                .await;
        }
        self.reply_item(session, request_id).await
    }

    /// Drive one Codex receipt from `first` (Submit, Retry or ConfirmDraft)
    /// through the domain's `InspectComposer` → `InjectPrepare` →
    /// `InjectEnter` actions exactly like the Claude chain: each step is
    /// persisted before its terminal write, and the fixed confirmation cursor
    /// is the frozen view's checkpoint captured at inspection, before any
    /// paste. Returns the draft token when the composer holds an unapproved
    /// draft.
    async fn dispatch_codex(
        &self,
        lease: &LeaseHandle,
        session: &Session,
        approved: &str,
        first: codex::Command,
    ) -> Result<Option<String>, Failure> {
        let kind = ComposerKind::Codex;
        let mut next = Some(first);
        for _ in 0..3 {
            let Some(command) = next.take() else {
                return Ok(None);
            };
            let applied = match self.try_apply_codex(command).await? {
                Ok(applied) => applied,
                Err(engine::Error::Codex(codex::Error::Missing)) => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            if !applied.replays.is_empty() {
                return Ok(None);
            }
            let Some(Action::Codex(CodexAction::InspectComposer { operation, target })) =
                applied.actions.into_iter().next()
            else {
                return Ok(None);
            };
            let id = operation.request_id.clone();
            let view = match self.inspect(kind, lease).await {
                Ok(view) => view,
                Err(_) => unknown_view(),
            };
            let (state, native_cursor) = match self.codex_checkpoint(&session.uid).await {
                Ok(cursor) => (view.state, cursor),
                Err(_) => (
                    ComposerState::Unknown,
                    codex::NativeCursor {
                        source_identity: "unavailable".into(),
                        position: 0,
                        head: "unavailable".into(),
                        anchor: "unavailable".into(),
                    },
                ),
            };
            let applied = self
                .apply_codex(codex::Command::DraftObserved {
                    operation,
                    observation: codex::DraftObservation {
                        target,
                        state: match state {
                            ComposerState::Empty => codex::DraftState::Empty,
                            ComposerState::Editing => codex::DraftState::Editing,
                            ComposerState::Unknown => codex::DraftState::Unknown,
                        },
                        token: view.screen_token.clone(),
                        native_cursor,
                    },
                })
                .await?;
            let Some(action) = applied.actions.into_iter().next() else {
                let receipt = {
                    let id = id.clone();
                    self.engine(move |engine| engine.codex_receipt(&id)).await?
                };
                let Some(receipt) = receipt else {
                    return Ok(None);
                };
                if receipt.state != codex::State::DraftConflict {
                    // Unknown composer: the domain persisted a pre-write
                    // failure (retryable, attempts 0); nothing was pasted.
                    return Ok(None);
                }
                let token = receipt.draft_token.clone().unwrap_or_default();
                if !approved.is_empty() && approved == token {
                    next = Some(codex::Command::ConfirmDraft {
                        request_id: id,
                        uid: session.uid.clone(),
                        token,
                    });
                    continue;
                }
                return Ok(Some(token));
            };
            let Action::Codex(CodexAction::Prepare {
                operation,
                payload,
                expected_frame,
                overwrite,
            }) = action
            else {
                return Ok(None);
            };
            match self
                .prepare(
                    kind,
                    lease,
                    &view,
                    &payload.text,
                    &expected_frame,
                    overwrite,
                )
                .await
            {
                Err(reason) => {
                    self.apply_codex(codex::Command::PrepareFailed { operation, reason })
                        .await?;
                    self.track_codex(&id);
                    return Ok(None);
                }
                Ok((frame_token, prepared_view)) => {
                    let evidence = codex::PreparedEvidence {
                        target: payload.target.clone(),
                        observed_text: payload.text.clone(),
                        observed_media: payload.media.clone(),
                        frame_token,
                    };
                    let applied = self
                        .apply_codex(codex::Command::Prepared {
                            operation,
                            evidence,
                        })
                        .await?;
                    let Some(Action::Codex(CodexAction::Enter {
                        operation,
                        expected_frame,
                        ..
                    })) = applied.actions.into_iter().next()
                    else {
                        self.track_codex(&id);
                        return Ok(None);
                    };
                    let result = match self
                        .enter(kind, lease, &prepared_view, &expected_frame)
                        .await
                    {
                        Ok(true) => codex::EnterResult::TransportReturned,
                        Ok(false) => {
                            codex::EnterResult::Unknown("终端调用结果不明；禁止重新注入".into())
                        }
                        Err(reason) => codex::EnterResult::Unknown(reason),
                    };
                    self.apply_codex(codex::Command::EnterFinished { operation, result })
                        .await?;
                    self.track_codex(&id);
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    pub async fn draft_status(
        &self,
        uid: &str,
        name: &str,
        page_lease: Option<PageLease>,
    ) -> Reply {
        let permit = match self.admit().await {
            Ok(permit) => permit,
            Err(failure) => return failure.reply(),
        };
        let lock = self.scope_lock(uid);
        let _scope = lock.lock().await;
        let _permit = permit;
        match self.draft_status_inner(uid, name, page_lease).await {
            Ok(reply) => reply,
            Err(failure) => failure.reply(),
        }
    }

    async fn draft_status_inner(
        &self,
        uid: &str,
        name: &str,
        page_lease: Option<PageLease>,
    ) -> Result<Reply, Failure> {
        let session = self.session(uid, "").await?;
        let target = self.resolver.resolve(uid).await?;
        if target.name != name {
            return Err(Failure::new(
                409,
                "terminal_unlinked",
                format!(
                    "{} 终端会话未连接：请求的终端名不是该会话唯一关联的受管实例",
                    session.provider.label()
                ),
            ));
        }
        let lease = self.driver.acquire(&target, page_lease.as_ref()).await?;
        let probe = self.inspect(session.provider.composer(), &lease).await;
        self.driver.release(lease).await;
        // A failed capture is `unknown`, not an error.
        let mut body = json!({"ok": true, "draft_state": "unknown"});
        if let Ok(view) = probe {
            body = json!({"ok": true});
            merge(&mut body, draft_conflict(&view));
        }
        Ok(Reply { status: 200, body })
    }

    pub async fn retry(
        &self,
        uid: &str,
        id: &str,
        overwrite_draft: &str,
        page_lease: Option<PageLease>,
    ) -> Reply {
        let permit = match self.admit().await {
            Ok(permit) => permit,
            Err(failure) => return failure.reply(),
        };
        let lock = self.scope_lock(uid);
        let _scope = lock.lock().await;
        let _permit = permit;
        match self.retry_inner(uid, id, overwrite_draft, page_lease).await {
            Ok(reply) => reply,
            Err(failure) => failure.reply(),
        }
    }

    async fn retry_inner(
        &self,
        uid: &str,
        id: &str,
        overwrite_draft: &str,
        page_lease: Option<PageLease>,
    ) -> Result<Reply, Failure> {
        let session = self.session(uid, "").await?;
        match session.provider {
            Provider::Claude => {
                self.retry_claude(session, id, overwrite_draft, page_lease)
                    .await
            }
            Provider::Codex => {
                self.retry_codex(session, id, overwrite_draft, page_lease)
                    .await
            }
        }
    }

    async fn retry_claude(
        &self,
        session: Session,
        id: &str,
        overwrite_draft: &str,
        page_lease: Option<PageLease>,
    ) -> Result<Reply, Failure> {
        let scope = session.claude_scope();
        let receipt = {
            let id = id.to_owned();
            self.engine(move |engine| engine.claude_receipt(&id))
                .await?
        };
        let Some(receipt) =
            receipt.filter(|r| r.request.payload.scope == scope && r.visible_in_outbox())
        else {
            return Err(Failure::new(404, "delivery_missing", "待核对消息不存在"));
        };
        // The domain's only retry: a local waiter that was never written may be
        // dispatched again (fresh inspection). Anything after a write is refused.
        if receipt.attempted || !matches!(receipt.state, State::Queued | State::DraftConflict) {
            return self
                .reply_with_outbox(
                    409,
                    json!({"error": "消息已经提交到终端，禁止盲目重发；请等待原生记录或打开终端检查"}),
                    &session,
                )
                .await;
        }
        let target = self.resolver.resolve(&session.uid).await?;
        let lease = self.driver.acquire(&target, page_lease.as_ref()).await?;
        let first = match (&receipt.state, receipt.draft_token.as_deref()) {
            (State::DraftConflict, Some(token))
                if !overwrite_draft.is_empty() && token == overwrite_draft =>
            {
                Command::ConfirmDraft {
                    id: id.to_owned(),
                    scope: scope.clone(),
                    token: token.to_owned(),
                }
            }
            (State::DraftConflict, Some(token)) => {
                self.driver.release(lease).await;
                return self
                    .reply_with_outbox(
                        409,
                        json!({"draft_state": "editing", "draft_conflict": true, "draft_token": token}),
                        &session,
                    )
                    .await;
            }
            _ => Command::DispatchNext {
                scope: scope.clone(),
            },
        };
        let result = self.dispatch(&lease, &scope, overwrite_draft, first).await;
        self.driver.release(lease).await;
        match result? {
            Some(token) => {
                self.reply_with_outbox(
                    409,
                    json!({"draft_state": "editing", "draft_conflict": true, "draft_token": token}),
                    &session,
                )
                .await
            }
            None => {
                self.reply_with_outbox(200, json!({"ok": true}), &session)
                    .await
            }
        }
    }

    /// Only a `failed` row with
    /// `attempts == 0` (the domain's `retryable()`, plus an unresolved draft
    /// conflict carrying its consent token) may be re-inspected; anything that
    /// reached the terminal is refused with the fixed message.
    async fn retry_codex(
        &self,
        session: Session,
        id: &str,
        overwrite_draft: &str,
        page_lease: Option<PageLease>,
    ) -> Result<Reply, Failure> {
        let receipt = {
            let id = id.to_owned();
            self.engine(move |engine| engine.codex_receipt(&id)).await?
        };
        let Some(receipt) =
            receipt.filter(|r| r.request.payload.uid == session.uid && r.visible_in_outbox())
        else {
            return Err(Failure::new(404, "delivery_missing", "待发送消息不存在"));
        };
        if !(receipt.retryable()
            || (receipt.state == codex::State::DraftConflict && receipt.attempts == 0))
        {
            return self
                .reply_with_outbox(
                    409,
                    json!({"error": "消息已经写入终端或仍在确认，禁止重复发送"}),
                    &session,
                )
                .await;
        }
        let target = self.resolver.resolve(&session.uid).await?;
        let lease = self.driver.acquire(&target, page_lease.as_ref()).await?;
        let first = match (&receipt.state, receipt.draft_token.as_deref()) {
            (codex::State::DraftConflict, Some(token))
                if !overwrite_draft.is_empty() && token == overwrite_draft =>
            {
                codex::Command::ConfirmDraft {
                    request_id: id.to_owned(),
                    uid: session.uid.clone(),
                    token: token.to_owned(),
                }
            }
            (codex::State::DraftConflict, Some(token)) => {
                self.driver.release(lease).await;
                return self
                    .reply_with_outbox(
                        409,
                        json!({"draft_state": "editing", "draft_conflict": true, "draft_token": token}),
                        &session,
                    )
                    .await;
            }
            _ => {
                // The console draft is checked before retry.
                match self
                    .overwrite_draft(ComposerKind::Codex, &lease, overwrite_draft)
                    .await
                {
                    Ok(None) => codex::Command::Retry {
                        request_id: id.to_owned(),
                        uid: session.uid.clone(),
                    },
                    Ok(Some(conflict)) => {
                        self.driver.release(lease).await;
                        return self.reply_with_outbox(409, conflict, &session).await;
                    }
                    Err(failure) => {
                        self.driver.release(lease).await;
                        return Err(failure);
                    }
                }
            }
        };
        let result = self
            .dispatch_codex(&lease, &session, overwrite_draft, first)
            .await;
        self.driver.release(lease).await;
        match result? {
            Some(token) => {
                self.reply_with_outbox(
                    409,
                    json!({"draft_state": "editing", "draft_conflict": true, "draft_token": token}),
                    &session,
                )
                .await
            }
            None => {
                self.reply_with_outbox(200, json!({"ok": true}), &session)
                    .await
            }
        }
    }

    pub async fn discard(&self, uid: &str, id: &str) -> Reply {
        let permit = match self.admit().await {
            Ok(permit) => permit,
            Err(failure) => return failure.reply(),
        };
        // Dismissal is a serialized ledger transition, not a terminal write.
        // It must stay available while that session's paste/Enter is pending.
        let _permit = permit;
        match self.discard_inner(uid, id).await {
            Ok(reply) => reply,
            Err(failure) => failure.reply(),
        }
    }

    async fn discard_inner(&self, uid: &str, id: &str) -> Result<Reply, Failure> {
        let session = self.session(uid, "").await?;
        match session.provider {
            Provider::Claude => {
                let scope = session.claude_scope();
                let receipt = {
                    let id = id.to_owned();
                    self.engine(move |engine| engine.claude_receipt(&id))
                        .await?
                };
                if !receipt
                    .is_some_and(|r| r.request.payload.scope == scope && r.visible_in_outbox())
                {
                    return Err(Failure::new(404, "delivery_missing", "待核对消息不存在"));
                }
                // Dismiss hides the row and cancels an unwritten waiter; it never
                // cancels Claude or forgets the deduplication identity.
                self.apply(Command::Dismiss {
                    id: id.to_owned(),
                    scope: scope.clone(),
                })
                .await?;
                self.untrack(id);
            }
            Provider::Codex => {
                let receipt = {
                    let id = id.to_owned();
                    self.engine(move |engine| engine.codex_receipt(&id)).await?
                };
                // Discard for Codex is idempotent: a row
                // another tab already removed answers `ok` with the snapshot.
                if receipt
                    .is_some_and(|r| r.request.payload.uid == session.uid && r.visible_in_outbox())
                {
                    // Dismissing a row is permitted while paste/Enter is in
                    // flight. Dismissal only hides it; the durable operation and
                    // deduplication identity survive until its callback settles.
                    self.apply_codex(codex::Command::Dismiss {
                        request_id: id.to_owned(),
                        uid: session.uid.clone(),
                    })
                    .await?;
                    self.untrack(id);
                }
            }
        }
        self.reply_with_outbox(200, json!({"ok": true, "uid": uid}), &session)
            .await
    }

    // ---- tracking -----------------------------------------------------------

    fn track(&self, id: &str) {
        let mut tracking = self.tracking.lock().unwrap_or_else(|p| p.into_inner());
        tracking.submitted.insert(
            id.to_owned(),
            TrackEntry {
                since: Instant::now(),
                last_replay: None,
                timed_out: false,
            },
        );
        tracking.redispatch.remove(id);
    }
    fn track_codex(&self, id: &str) {
        let mut tracking = self.tracking.lock().unwrap_or_else(|p| p.into_inner());
        tracking.codex.insert(id.to_owned(), CodexTrack::default());
    }
    fn untrack(&self, id: &str) {
        let mut tracking = self.tracking.lock().unwrap_or_else(|p| p.into_inner());
        tracking.submitted.remove(id);
        tracking.redispatch.remove(id);
        tracking.codex.remove(id);
    }
    fn note_redispatch(&self, id: &str) {
        let mut tracking = self.tracking.lock().unwrap_or_else(|p| p.into_inner());
        tracking.redispatch.insert(id.to_owned(), Instant::now());
    }

    /// One tracker pass: observe every attempted-but-unacknowledged receipt
    /// from its watch cursor (or, when overdue and rate-limited, from its fixed
    /// confirmation fence), and re-dispatch Claude local waiters whose earlier
    /// inspection could not authorize a write.
    pub async fn tick(&self) -> Result<(), Failure> {
        let rows = self
            .engine(move |engine| engine.claude_receipts(None))
            .await?;
        let now = Instant::now();
        let mut redispatch = Vec::new();
        for row in rows {
            // A dismissed row is retired from automatic tracking (tombstoned
            // on discard): a later identical record then
            // acknowledges the next receipt with that text, not the hidden one.
            if row.attempted && tracked_state(row.state) && !row.dismissed {
                self.observe_row(&row, now).await;
            } else if row.state == State::Queued && !row.attempted && !row.dismissed {
                let due = {
                    let tracking = self.tracking.lock().unwrap_or_else(|p| p.into_inner());
                    tracking
                        .redispatch
                        .get(&row.request.id)
                        .is_none_or(|at| now.duration_since(*at) >= self.limits.redispatch_interval)
                };
                if due {
                    redispatch.push(row);
                }
            }
        }
        for row in redispatch {
            self.redispatch(&row).await;
        }
        let rows = self
            .engine(move |engine| engine.codex_receipts(None))
            .await?;
        for row in rows {
            if row.state == codex::State::Uncertain && !row.dismissed {
                self.observe_codex_row(&row).await;
            }
        }
        Ok(())
    }

    async fn observe_row(&self, row: &Receipt, now: Instant) {
        let id = row.request.id.clone();
        let scope = row.request.payload.scope.clone();
        let (overdue, replay, timed_out) = {
            let mut tracking = self.tracking.lock().unwrap_or_else(|p| p.into_inner());
            let entry = tracking
                .submitted
                .entry(id.clone())
                .or_insert_with(|| TrackEntry {
                    // After a Web restart the durable creation time bounds the
                    // window; the in-memory Enter instant is gone.
                    since: now.checked_sub(age_since(row.created_ms)).unwrap_or(now),
                    last_replay: None,
                    timed_out: false,
                });
            let elapsed = now.duration_since(entry.since);
            if elapsed >= self.limits.tracking_window {
                let first = !entry.timed_out;
                entry.timed_out = true;
                (true, false, Some(first))
            } else {
                let overdue = elapsed >= self.limits.confirm_timeout;
                let replay = overdue
                    && entry
                        .last_replay
                        .is_none_or(|at| now.duration_since(at) >= self.limits.confirm_timeout);
                if replay {
                    entry.last_replay = Some(now);
                }
                (overdue, replay, None)
            }
        };
        if let Some(first) = timed_out {
            if first {
                let _ = self
                    .apply(Command::Timeout {
                        id: id.clone(),
                        scope: scope.clone(),
                    })
                    .await;
            }
            return;
        }
        let _ = overdue;
        let Ok(applied) = self
            .apply(Command::InspectNative {
                id: id.clone(),
                scope: scope.clone(),
                replay,
            })
            .await
        else {
            return;
        };
        let Some(Action::Claude(ClaudeAction::InspectNative {
            operation,
            scope,
            from,
            confirmation,
        })) = applied.actions.into_iter().next()
        else {
            return;
        };
        let Ok(read) = self.native_wait(&scope.uid, adapter::fence_of(&from)).await else {
            return;
        };
        match adapter::observe(row, &scope, &from, &read) {
            adapter::Observation::Accepted(mut evidence) => {
                // Confirmation is by the native text record. Media is outbox
                // preview metadata; it has no separate native representation.
                evidence.attachments = row.request.payload.attachments.clone();
                if matches!(
                    self.try_apply(Command::ObserveUser {
                        id: id.clone(),
                        evidence: *evidence,
                    })
                    .await,
                    Ok(Ok(_))
                ) {
                    self.untrack(&id);
                }
            }
            adapter::Observation::Nothing { next: Some(next) } => {
                let _ = self
                    .apply(Command::AdvanceWatch {
                        operation,
                        previous: from,
                        next,
                        confirmation,
                    })
                    .await;
            }
            adapter::Observation::Nothing { next: None } | adapter::Observation::FenceInvalid => {}
        }
    }

    async fn native_wait(&self, uid: &str, from: NativeFence) -> Result<NativeInputRead, Failure> {
        let uid = uid.to_owned();
        self.reader
            .run_wait(&self.stop, move |store| {
                store.claude_native_inputs(&uid, Some(&from))
            })
            .await
            .map_err(|error| Failure::new(error.status.as_u16(), error.code, error.message))
    }

    /// For one Codex receipt: the adapter's replay clock
    /// decides a cheap watch tick, a rate-limited replay from the fixed
    /// boundary, or expiry after the tracking window (the row keeps its state
    /// and is not retryable). Outcomes map to the domain as documented in
    /// `docs/delivery-codex-executor.md`; a causal matching native user record
    /// after the boundary retires the receipt.
    async fn observe_codex_row(&self, row: &codex::Receipt) {
        let id = row.request.request_id.clone();
        let uid = row.request.payload.uid.clone();
        let Some(enter) = row.enter_operation.clone() else {
            // A prepare that never reached Enter has nothing to observe.
            return;
        };
        let policy = self.limits.replay_policy();
        // The durable Submit clock bounds the window and the timestamp check;
        // the physical fence is the real boundary.
        let delivered_ms = row.created_ms;
        let plan = {
            let mut tracking = self.tracking.lock().unwrap_or_else(|p| p.into_inner());
            let entry = tracking.codex.entry(id.clone()).or_default();
            if entry.expired {
                return;
            }
            let plan = entry.clock.plan(&policy, delivered_ms, now_ms());
            if plan == ReadPlan::Expired {
                entry.expired = true;
            }
            plan
        };
        if plan == ReadPlan::Expired {
            return;
        }
        let replay = plan == ReadPlan::Replay;
        let Ok(applied) = self
            .apply_codex(codex::Command::InspectNative {
                request_id: id.clone(),
                uid: uid.clone(),
                replay,
            })
            .await
        else {
            return;
        };
        let Some(Action::Codex(CodexAction::InspectNative {
            operation,
            from,
            confirmation,
            replay,
        })) = applied.actions.into_iter().next()
        else {
            return;
        };
        let boundary = Boundary {
            confirmation: confirmation.clone(),
            sequence: enter.revision,
            delivered_ms,
        };
        let delivered = Delivered {
            uid: uid.clone(),
            text: row.request.payload.text.clone(),
            media: row.request.payload.media.clone(),
        };
        let watch_from = from.clone();
        let observation = {
            let uid = uid.clone();
            self.reader
                .run_wait(&self.stop, move |store| {
                    let snapshot = store.snapshot(&uid, "")?;
                    // A watch tick observes only when the committed end moved
                    // past the watch cursor; a replay observes regardless.
                    let checkpoint = snapshot.native_checkpoint();
                    if !replay
                        && checkpoint.source_identity == watch_from.source_identity
                        && checkpoint.position <= watch_from.position
                    {
                        return Ok(None);
                    }
                    Ok(Some(codex_adapter::observe(
                        &snapshot, &boundary, &delivered,
                    )))
                })
                .await
        };
        let Ok(Some(observation)) = observation else {
            return;
        };
        match observation.outcome {
            Outcome::Possible(found) => {
                let acknowledged = matches!(
                    self.try_apply_codex(codex::Command::NativeAck {
                        request_id: id.clone(),
                        evidence: found.evidence.clone(),
                    })
                    .await,
                    Ok(Ok(_))
                );
                if acknowledged {
                    self.untrack(&id);
                    if let Some(completion) = found.completion.clone() {
                        let _ = self
                            .try_apply_codex(codex::Command::NativeCompletion {
                                request_id: id.clone(),
                                evidence: completion,
                            })
                            .await;
                    }
                }
            }
            Outcome::Absent(_) => {
                if let Some(next) = observation.watch.filter(|next| next != &from) {
                    let _ = self
                        .apply_codex(codex::Command::AdvanceWatch {
                            operation,
                            previous: from,
                            next,
                            validated_confirmation: confirmation,
                        })
                        .await;
                }
            }
            Outcome::Uncertain(codex_adapter::Uncertainty::CheckpointMismatch) => {
                // The file was rewritten or truncated below the fixed fence:
                // annotate once; the fence never moves and nothing is retried.
                let reason = "原生记录在确认边界以下被重写或截断；回执保持待核对".to_owned();
                if row.issue.as_deref() != Some(reason.as_str()) {
                    let _ = self
                        .apply_codex(codex::Command::NativeReset { operation, reason })
                        .await;
                }
            }
            // Ambiguous duplicates, foreign scope, media, blank text or an
            // unreadable view: the receipt stays uncertain, unannotated.
            Outcome::Uncertain(_) => {}
        }
    }

    async fn redispatch(&self, row: &Receipt) {
        let id = row.request.id.clone();
        let scope = row.request.payload.scope.clone();
        self.note_redispatch(&id);
        let Ok(permit) = self.admit().await else {
            return;
        };
        let lock = self.scope_lock(&scope.uid);
        let _guard = lock.lock().await;
        let _permit = permit;
        let Ok(target) = self.resolver.resolve(&scope.uid).await else {
            return;
        };
        if target.instance_id != row.request.payload.target.host_instance {
            // The receipt names a different process instance; never inject
            // into a replacement. It stays a visible persisted row.
            return;
        }
        let Ok(lease) = self.driver.acquire(&target, None).await else {
            return;
        };
        let _ = self
            .dispatch(
                &lease,
                &scope,
                "",
                Command::DispatchNext {
                    scope: scope.clone(),
                },
            )
            .await;
        self.driver.release(lease).await;
    }
}

async fn track(executor: std::sync::Weak<DeliveryExecutor>) {
    loop {
        let (stop, tick) = match executor.upgrade() {
            Some(strong) => (strong.stop.clone(), strong.limits.tick),
            None => return,
        };
        tokio::select! {
            biased;
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(tick) => {}
        }
        let Some(strong) = executor.upgrade() else {
            return;
        };
        if strong.stop.is_cancelled() {
            return;
        }
        let _ = strong.tick().await;
    }
}

fn tracked_state(state: State) -> bool {
    matches!(
        state,
        State::Uncertain
            | State::NativeQueued
            | State::NativeDequeued
            | State::NativeRemoved
            | State::Restored
            | State::Interrupted
    )
}

fn frame_ok(expected_frame: &str, previous: &ComposerView, current: &ComposerView) -> bool {
    !current.lagging
        && current.dropped == previous.dropped
        && (current.screen_token == expected_frame
            || (current.composer_token.is_some()
                && current.composer_token == previous.composer_token))
}

fn unknown_view() -> ComposerView {
    ComposerView {
        state: ComposerState::Unknown,
        screen_token: "unavailable".into(),
        composer_token: None,
        text: None,
        busy: false,
        pasting: false,
        lagging: true,
        dropped: None,
    }
}

/// Body: `draft_state`, plus `draft_conflict` and the
/// consent token while editing.
fn draft_conflict(view: &ComposerView) -> Value {
    match view.state {
        ComposerState::Editing => json!({
            "draft_state": "editing", "draft_conflict": true, "draft_token": view.screen_token,
        }),
        ComposerState::Empty => json!({"draft_state": "empty"}),
        ComposerState::Unknown => json!({"draft_state": "unknown"}),
    }
}

/// A confirmed/retired row: no text or media.
fn hidden_item(receipt: &Receipt) -> Value {
    json!({
        "id": receipt.request.id,
        "uid": receipt.request.payload.scope.uid,
        "created": receipt.created_ms,
        "state": "confirmed",
        "attempts": u32::from(receipt.attempted),
        "server": true,
    })
}

fn hidden_codex_item(receipt: &codex::Receipt) -> Value {
    json!({
        "id": receipt.request.request_id,
        "uid": receipt.request.payload.uid,
        "created": receipt.created_ms,
        "state": "confirmed",
        "attempts": receipt.attempts,
        "server": true,
    })
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn age_since(created_ms: u64) -> Duration {
    Duration::from_millis(now_ms().saturating_sub(created_ms))
}

/// Mint a UUID for an empty ID, otherwise keep the
/// first 128 Unicode characters, including whitespace and punctuation.
/// Retry and discard use the returned ID exactly, without normalization.
pub fn normalize_request_id(raw: &str) -> Result<String, Failure> {
    if raw.is_empty() {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes)
            .map_err(|_| Failure::new(503, "random_unavailable", "安全随机数暂不可用"))?;
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        return Ok(format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        ));
    }
    Ok(raw.chars().take(128).collect())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod codex_tests;
