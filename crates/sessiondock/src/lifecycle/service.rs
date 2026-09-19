//! Isolated lifecycle coordinator. HTTP cancellation never owns a spawn.

use futures_util::{StreamExt, stream};
use ptyhost_client::{BoundTarget, ControlOp, HostClient, LaunchTarget, NativeBindingState};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::Child,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{
    launcher::{self, Launcher},
    model::{
        BindingMethod, BindingSpec, BindingState, Failure, Launch, LaunchSpec, Record, Source,
        State,
    },
    store::{
        self, BindingEvidence, BindingObservation, LifecycleStore, Observation,
        ObservationEvidence, StartAuthority,
    },
};
use crate::terminal::TerminalService;

static OPEN_WORKERS: Semaphore = Semaphore::const_new(2);

#[derive(Clone, Copy, Debug)]
pub struct ServiceLimits {
    pub capacity: usize,
    pub readiness_timeout: Duration,
    pub cancel_timeout: Duration,
    pub probe_timeout: Duration,
    pub poll_interval: Duration,
}
impl Default for ServiceLimits {
    fn default() -> Self {
        Self {
            capacity: 8,
            readiness_timeout: Duration::from_secs(5),
            cancel_timeout: Duration::from_secs(3),
            probe_timeout: Duration::from_secs(1),
            poll_interval: Duration::from_millis(50),
        }
    }
}
impl ServiceLimits {
    fn validate(self) -> Result<Self, Error> {
        if self.capacity == 0
            || self.readiness_timeout.is_zero()
            || self.cancel_timeout.is_zero()
            || self.probe_timeout.is_zero()
            || self.poll_interval.is_zero()
        {
            Err(Error::InvalidLimits)
        } else {
            Ok(self)
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Store(store::Error),
    Launcher(launcher::Error),
    Busy,
    Closed,
    WorkerFailed,
    InvalidLimits,
    NotReady,
    IdentityConflict,
    RetirementFailed,
    ReaperUnavailable,
    InvalidBinding,
    BindingUnsupported,
    BindingConflict,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "lifecycle service: {self:?}")
    }
}
impl std::error::Error for Error {}

/// Caller must obtain NativeScope from its current validated native catalog and
/// independently require explicit operator confirmation, or supply the
/// server's own process evidence (`from_process_evidence`). Never serde authority.
pub struct VerifiedNativeBinding {
    record_id: String,
    launch_id: String,
    instance_id: String,
    spec: BindingSpec,
    method: BindingMethod,
    evidence: Option<String>,
}
impl VerifiedNativeBinding {
    pub fn from_scope(
        scope: &crate::sessions::NativeScope,
        receipt: &Record,
        operator_confirmed: bool,
    ) -> Result<Self, Error> {
        if !operator_confirmed {
            return Err(Error::InvalidBinding);
        }
        Self::verified(scope, receipt, BindingMethod::Operator, None)
    }
    /// A binding asserted by process evidence (the launched host's child
    /// process holds the native record open / is the CLI main process of
    /// exactly one indexed session). `evidence` is the private note persisted
    /// with the receipt.
    pub fn from_process_evidence(
        scope: &crate::sessions::NativeScope,
        receipt: &Record,
        evidence: String,
    ) -> Result<Self, Error> {
        Self::verified(scope, receipt, BindingMethod::Process, Some(evidence))
    }
    fn verified(
        scope: &crate::sessions::NativeScope,
        receipt: &Record,
        method: BindingMethod,
        evidence: Option<String>,
    ) -> Result<Self, Error> {
        if scope.agent_id.is_some() {
            return Err(Error::BindingUnsupported);
        }
        let source = match scope.source.as_str() {
            "claude" => Source::Claude,
            "codex" => Source::Codex,
            "grok" => Source::Grok,
            _ => return Err(Error::InvalidBinding),
        };
        if source != receipt.spec().source()
            || receipt.cancel_requested()
            || receipt.state() != State::Running
        {
            return Err(Error::InvalidBinding);
        }
        let spec = BindingSpec::new(source, scope.session_id.clone(), scope.uid.clone())
            .map_err(|_| Error::InvalidBinding)?;
        Ok(Self {
            record_id: receipt.record_id().into(),
            launch_id: receipt.launch_id().into(),
            instance_id: receipt.instance_id().into(),
            spec,
            method,
            evidence,
        })
    }
    pub fn record_id(&self) -> &str {
        &self.record_id
    }
    pub fn uid(&self) -> &str {
        self.spec.uid()
    }
}

/// Two EOF (`C-d`) attempts of 1.2 s each before
/// the host-performed stop. The Web process never signals a PID itself.
pub const GRACEFUL_ATTEMPTS: u8 = 2;
pub const GRACEFUL_WAIT: Duration = Duration::from_millis(1200);

/// Which stage ended the managed instance, or why nothing could be ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopStage {
    /// The CLI exited after an EOF key sent through the guarded host input.
    Graceful,
    /// The host-performed guarded stop (`term/kill` path) observed the exit.
    Stopped,
    /// The exact instance had already exited before anything was sent.
    AlreadyExited,
    /// Keys and the guarded stop were issued but exit was not observed in time.
    Uncertain,
    /// A record names this session but its host state is unknown; nothing sent.
    Unknown,
    /// No managed instance declares this session; external CLIs are not probed.
    NoInstance,
}

/// What the transport resolved for the UID from one fresh runtime observation.
/// Never a name, cwd, time or PID guess: `Instance` is a guarded bound target.
pub enum StopCandidate {
    Instance(Box<BoundTarget>),
    Exited,
    Unknown,
    NoInstance,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct StopOutcome {
    pub uid: String,
    pub stage: StopStage,
    /// True only when the exact instance is confirmed not running.
    pub stopped: bool,
    pub name: Option<String>,
    pub instance_id: Option<String>,
    pub record_id: Option<String>,
    pub graceful_attempts: u8,
}
impl StopOutcome {
    fn new(uid: &str, stage: StopStage) -> Self {
        Self {
            uid: uid.to_owned(),
            stage,
            stopped: matches!(
                stage,
                StopStage::Graceful | StopStage::Stopped | StopStage::AlreadyExited
            ),
            name: None,
            instance_id: None,
            record_id: None,
            graceful_attempts: 0,
        }
    }
}

enum Command {
    Create(String, LaunchSpec),
    Get(String),
    List(usize, usize),
    Target(String),
    Cancel(String, String),
    Bind(VerifiedNativeBinding),
    AuthorizeNative(BoundTarget),
    Stop(String, StopCandidate),
    Discard(String, String),
}
impl Command {
    /// Commands that may change what a receipt list or a host observation
    /// shows (a spawn, a kill, a binding, a tombstone), so the display
    /// caches keyed on [`LifecycleService::generation`] are invalidated
    /// after them whatever their outcome — a failed cancel may still have
    /// recorded its intent.
    fn mutates(&self) -> bool {
        !matches!(self, Self::Get(_) | Self::List(..) | Self::Target(_))
    }
}
enum Answer {
    Record(Box<Record>),
    Records(Vec<Record>),
    Target(Arc<LaunchTarget>),
    Stop(Box<StopOutcome>),
}
struct Response {
    answer: Result<Answer, Error>,
    _permit: OwnedSemaphorePermit,
}
struct Request {
    command: Command,
    reply: oneshot::Sender<Response>,
    permit: OwnedSemaphorePermit,
}

pub struct LifecycleService {
    tx: mpsc::Sender<Request>,
    admission: Arc<Semaphore>,
    stop: CancellationToken,
    done: watch::Receiver<Option<Result<(), Error>>>,
    /// Shared read-only allowlist handles for completion and catalog queries;
    /// spawning still happens only inside the coordinator.
    launcher: Arc<Launcher>,
    /// Incremented by the coordinator after every mutating command
    /// (`Command::mutates`), from whichever caller (HTTP, autobind, the
    /// bug-report worker). Display caches (`/api/live`, `/api/term/list`,
    /// the shared managed observation) compare it and drop their entries.
    generation: Arc<AtomicU64>,
}
impl LifecycleService {
    pub async fn open(
        directory: PathBuf,
        config: launcher::Config,
        terminal: Arc<TerminalService>,
        limits: ServiceLimits,
        shutdown: CancellationToken,
    ) -> Result<Self, Error> {
        let limits = limits.validate()?;
        if shutdown.is_cancelled() {
            return Err(Error::Closed);
        }
        let stop = shutdown.child_token();
        let opening_stop = stop.clone();
        let opening_permit = tokio::select! {
            _ = opening_stop.cancelled() => return Err(Error::Closed),
            permit = OPEN_WORKERS.acquire() => permit.map_err(|_| Error::Closed)?,
        };
        // Configuration validation and recovery fsync happen off the reactor.
        let (store, launcher, client) = tokio::task::spawn_blocking(move || {
            let _opening_permit = opening_permit;
            if opening_stop.is_cancelled() {
                return Err(Error::Closed);
            }
            let launcher = Launcher::new(config).map_err(Error::Launcher)?;
            let client = HostClient::new(
                launcher.host_dir(),
                ptyhost_client::Limits {
                    operation_timeout: limits.probe_timeout,
                    max_line_bytes: 4 * 1024 * 1024, // ptyhost protocol::MAX_LINE; a 1 MiB send plus its guard envelope
                    ..Default::default()
                },
            )
            .map_err(|_| Error::NotReady)?;
            let store = LifecycleStore::open(&directory).map_err(Error::Store)?;
            if opening_stop.is_cancelled() {
                return Err(Error::Closed);
            }
            Ok((store, launcher, client))
        })
        .await
        .map_err(|_| Error::WorkerFailed)??;
        let (tx, rx) = mpsc::channel(limits.capacity);
        let admission = Arc::new(Semaphore::new(limits.capacity));
        let (done_tx, done) = watch::channel(None);
        let launcher = Arc::new(launcher);
        let generation = Arc::new(AtomicU64::new(1));
        let core = Core {
            store: Arc::new(Mutex::new(store)),
            launcher: launcher.clone(),
            client,
            terminal,
            children: BTreeMap::new(),
            targets: BTreeMap::new(),
            bindings: BTreeMap::new(),
            limits,
            stop: stop.clone(),
        };
        tokio::spawn(coordinate(
            core,
            rx,
            admission.clone(),
            stop.clone(),
            done_tx,
            generation.clone(),
        ));
        Ok(Self {
            tx,
            admission,
            stop,
            done,
            launcher,
            generation,
        })
    }
    /// The mutation counter (see the field): equal values mean no create,
    /// cancel, bind, authorization, stop or discard completed in between.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    /// Configured adapter/profile IDs with their source and capabilities.
    pub fn entries(&self) -> &[launcher::Entry] {
        self.launcher.entries()
    }
    /// Fixed source table: the one configured CLI of `source`
    /// (resume-capable when `resume`), `None` when the source has none or
    /// more than one. `term/create` and the bug-report worker both select
    /// through this, so a worker runs exactly what the picker would start.
    pub fn entry_for(&self, source: Source, resume: bool) -> Option<&launcher::Entry> {
        let mut candidates = self
            .entries()
            .iter()
            .filter(|entry| entry.source == source && (!resume || entry.resume));
        let first = candidates.next()?;
        candidates.next().is_none().then_some(first)
    }
    /// Absolute directory completion. It uses the same
    /// admission path as other requests and does not touch ledger or host state.
    pub async fn complete_directories(
        &self,
        text: String,
        limit: usize,
    ) -> Result<Vec<String>, Error> {
        if self.stop.is_cancelled() {
            return Err(Error::Closed);
        }
        let permit = tokio::select! {
            _ = self.stop.cancelled() => return Err(Error::Closed),
            permit = self.admission.clone().acquire_owned() => permit.map_err(|_| Error::Closed)?,
        };
        let launcher = self.launcher.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            launcher
                .complete_directories(&text, limit)
                .map_err(Error::Launcher)
        })
        .await
        .map_err(|_| Error::WorkerFailed)?
    }
    pub async fn create(&self, request_id: String, spec: LaunchSpec) -> Result<Record, Error> {
        match self.ask(Command::Create(request_id, spec)).await? {
            Answer::Record(record) => Ok(*record),
            _ => Err(Error::WorkerFailed),
        }
    }
    pub async fn get(&self, record_id: String) -> Result<Record, Error> {
        match self.ask(Command::Get(record_id)).await? {
            Answer::Record(record) => Ok(*record),
            _ => Err(Error::WorkerFailed),
        }
    }
    pub async fn list(&self, offset: usize, limit: usize) -> Result<Vec<Record>, Error> {
        match self.ask(Command::List(offset, limit)).await? {
            Answer::Records(records) => Ok(records),
            _ => Err(Error::WorkerFailed),
        }
    }
    pub async fn target(&self, record_id: String) -> Result<Arc<LaunchTarget>, Error> {
        match self.ask(Command::Target(record_id)).await? {
            Answer::Target(target) => Ok(target),
            _ => Err(Error::WorkerFailed),
        }
    }
    pub async fn bind(&self, verified: VerifiedNativeBinding) -> Result<Record, Error> {
        match self.ask(Command::Bind(verified)).await? {
            Answer::Record(record) => Ok(*record),
            _ => Err(Error::WorkerFailed),
        }
    }
    pub async fn authorize_native(&self, target: &BoundTarget) -> Result<Record, Error> {
        match self.ask(Command::AuthorizeNative(target.clone())).await? {
            Answer::Record(record) => Ok(*record),
            _ => Err(Error::WorkerFailed),
        }
    }
    pub async fn cancel(
        &self,
        record_id: String,
        expected_instance_id: String,
    ) -> Result<Record, Error> {
        match self
            .ask(Command::Cancel(record_id, expected_instance_id))
            .await?
        {
            Answer::Record(record) => Ok(*record),
            _ => Err(Error::WorkerFailed),
        }
    }
    /// Drop a finished or durably cancelled receipt from the pending view.
    /// The receipt stays queryable; a
    /// receipt whose instance may still run is refused, never killed.
    pub async fn discard(
        &self,
        record_id: String,
        expected_instance_id: String,
    ) -> Result<Record, Error> {
        match self
            .ask(Command::Discard(record_id, expected_instance_id))
            .await?
        {
            Answer::Record(record) => Ok(*record),
            _ => Err(Error::WorkerFailed),
        }
    }
    /// Stop the managed instance the caller resolved for `uid`
    /// (host-only): EOF keys through the guarded input
    /// path, then the same guarded stop `cancel` performs for launched
    /// receipts, or the host's own `kill` for a guarded instance without a
    /// receipt.
    pub async fn stop_session(
        &self,
        uid: String,
        candidate: StopCandidate,
    ) -> Result<StopOutcome, Error> {
        match self.ask(Command::Stop(uid, candidate)).await? {
            Answer::Stop(outcome) => Ok(*outcome),
            _ => Err(Error::WorkerFailed),
        }
    }
    async fn ask(&self, command: Command) -> Result<Answer, Error> {
        let response = self.admit(command).await?;
        response.await.map_err(|_| Error::WorkerFailed)?.answer
    }
    async fn admit(&self, command: Command) -> Result<oneshot::Receiver<Response>, Error> {
        if self.stop.is_cancelled() {
            return Err(Error::Closed);
        }
        let permit = tokio::select! {
            _ = self.stop.cancelled() => return Err(Error::Closed),
            permit = self.admission.clone().acquire_owned() => permit.map_err(|_| Error::Closed)?,
        };
        let (reply, response) = oneshot::channel();
        tokio::select! {
            _ = self.stop.cancelled() => return Err(Error::Closed),
            result = self.tx.send(Request {
                command,
                reply,
                permit,
            }) => result.map_err(|_| Error::Closed)?,
        }
        Ok(response)
    }
    pub async fn shutdown(&self) -> Result<(), Error> {
        self.stop.cancel();
        self.admission.close();
        let mut done = self.done.clone();
        loop {
            if let Some(status) = done.borrow_and_update().clone() {
                return status;
            }
            done.changed().await.map_err(|_| Error::WorkerFailed)?;
        }
    }
}
impl Drop for LifecycleService {
    fn drop(&mut self) {
        self.admission.close();
        self.stop.cancel();
    }
}

async fn coordinate(
    mut core: Core,
    mut rx: mpsc::Receiver<Request>,
    admission: Arc<Semaphore>,
    stop: CancellationToken,
    done: watch::Sender<Option<Result<(), Error>>>,
    generation: Arc<AtomicU64>,
) {
    loop {
        let request = tokio::select! { biased; _ = stop.cancelled() => break, request = rx.recv() => match request { Some(request) => request, None => break } };
        let mutates = request.command.mutates();
        let answer = core.execute(request.command).await;
        if mutates {
            generation.fetch_add(1, Ordering::AcqRel);
        }
        let _ = request.reply.send(Response {
            answer,
            _permit: request.permit,
        });
    }
    admission.close();
    rx.close();
    while let Ok(request) = rx.try_recv() {
        let _ = request.reply.send(Response {
            answer: Err(Error::Closed),
            _permit: request.permit,
        });
    }
    // Core owns no Child. Reaper ownership survives coordinator/HTTP/runtime drops.
    let result = tokio::task::spawn_blocking(move || drop(core))
        .await
        .map_err(|_| Error::WorkerFailed);
    let _ = done.send(Some(result));
}

/// Host round trips a list refresh runs concurrently for live receipts.
const LIST_PARALLEL_PROBES: usize = 8;

/// What the host said about one receipt (see `Core::probe_remote`).
enum RemoteProbe {
    Observed(Box<ptyhost_client::HostObservation>),
    /// The host record was retired because its local process is dead.
    Retired,
    Unavailable,
}

struct Core {
    store: Arc<Mutex<LifecycleStore>>,
    launcher: Arc<Launcher>,
    client: HostClient,
    terminal: Arc<TerminalService>,
    children: BTreeMap<String, watch::Receiver<ChildState>>,
    targets: BTreeMap<String, Arc<LaunchTarget>>,
    bindings: BTreeMap<String, NativeBindingState>,
    limits: ServiceLimits,
    stop: CancellationToken,
}
enum Created {
    Existing(Record),
    Started(StartAuthority, watch::Receiver<ChildState>),
    Failed(Record),
}
impl Core {
    async fn work<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut LifecycleStore) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            #[cfg(all(test, unix))]
            tests::pause_blocking_work();
            let mut store = store.lock().map_err(|_| Error::WorkerFailed)?;
            work(&mut store)
        })
        .await
        .map_err(|_| Error::WorkerFailed)?
    }
    async fn execute(&mut self, command: Command) -> Result<Answer, Error> {
        match command {
            Command::Create(id, spec) => self
                .create(id, spec)
                .await
                .map(|r| Answer::Record(Box::new(r))),
            Command::Get(id) => self.get(id).await.map(|r| Answer::Record(Box::new(r))),
            Command::List(offset, limit) => {
                let records = self
                    .work(move |store| store.list(offset, limit).map_err(Error::Store))
                    .await?;
                let deadline = Instant::now() + self.limits.readiness_timeout;
                self.refresh_all(records, deadline)
                    .await
                    .map(Answer::Records)
            }
            Command::Target(id) => {
                let record = self.get(id.clone()).await?;
                if record.state() != State::Running || record.cancel_requested() {
                    return Err(Error::NotReady);
                }
                self.targets
                    .get(&id)
                    .cloned()
                    .map(Answer::Target)
                    .ok_or(Error::NotReady)
            }
            Command::Cancel(id, instance) => self
                .cancel(id, instance)
                .await
                .map(|r| Answer::Record(Box::new(r))),
            Command::Bind(verified) => self
                .bind(verified)
                .await
                .map(|r| Answer::Record(Box::new(r))),
            Command::AuthorizeNative(target) => self
                .authorize_native(target)
                .await
                .map(|r| Answer::Record(Box::new(r))),
            Command::Stop(uid, candidate) => self
                .stop_session(uid, candidate)
                .await
                .map(|outcome| Answer::Stop(Box::new(outcome))),
            Command::Discard(id, instance) => {
                // Fresh evidence first: a receipt still Running (or one that
                // just recovered) is not discardable.
                let record = self.get(id).await?;
                if record.instance_id() != instance {
                    return Err(Error::IdentityConflict);
                }
                if !record.discardable() {
                    return Err(Error::NotReady);
                }
                let id = record.record_id().to_owned();
                self.work(move |store| store.discard(&id).map_err(Error::Store))
                    .await
                    .map(|r| Answer::Record(Box::new(r)))
            }
        }
    }
    /// At most one managed live instance per resumed native session. A fresh
    /// Running receipt for the same full SID/UID is returned instead of a
    /// second `--resume`; an unresolved Uncertain one conflicts until it is
    /// explicitly cancelled. Exited/failed/cancelled receipts do not block.
    async fn existing_resume(
        &mut self,
        id: &str,
        spec: &LaunchSpec,
    ) -> Result<Option<Record>, Error> {
        let Launch::Resume { .. } = spec.launch() else {
            return Ok(None);
        };
        let records = self
            .work(|store| store.list(0, usize::MAX).map_err(Error::Store))
            .await?;
        let deadline = Instant::now() + self.limits.readiness_timeout;
        for record in records {
            if record.request_id() == id
                || record.spec().source() != spec.source()
                || record.spec().launch() != spec.launch()
                || record.cancel_requested()
                || !matches!(
                    record.state(),
                    State::Running | State::Uncertain | State::Starting
                )
            {
                continue;
            }
            let refreshed = self.refresh(record, deadline).await?;
            match refreshed.state() {
                State::Running if !refreshed.cancel_requested() => return Ok(Some(refreshed)),
                State::Uncertain | State::Starting => {
                    return Err(Error::Store(store::Error::Conflict));
                }
                _ => {}
            }
        }
        Ok(None)
    }
    async fn create(&mut self, id: String, spec: LaunchSpec) -> Result<Record, Error> {
        if let Some(existing) = self.existing_resume(&id, &spec).await? {
            return Ok(existing);
        }
        let launcher = self.launcher.clone();
        let stop = self.stop.clone();
        let created = self
            .work(move |store| {
                // HTTP specs are deserialized before this blocking worker.
                // Normalize a live cwd here.
                // Keep the original spelling when it no longer exists so an
                // exact replay can still return its durable receipt.
                let spec = LaunchSpec::with_launch(
                    spec.source(),
                    spec.adapter_id().into(),
                    spec.cwd(),
                    spec.launch().clone(),
                )
                .unwrap_or(spec);
                if let Some(record) = store.lookup_request(&id, &spec).map_err(Error::Store)? {
                    return Ok(Created::Existing(record));
                }
                if stop.is_cancelled() {
                    return Err(Error::Closed);
                }
                launcher.validate_spec(&spec).map_err(Error::Launcher)?;
                let reaper = Reaper::global()?;
                let created = store.create(&id, &spec).map_err(Error::Store)?;
                let Some(prepared) = created.prepared else {
                    return Ok(Created::Existing(created.record));
                };
                if stop.is_cancelled() {
                    return store
                        .cancel_prepared(created.record.record_id())
                        .map(Created::Failed)
                        .map_err(Error::Store);
                }
                let authority = store.begin_start(prepared).map_err(Error::Store)?;
                // Never timeout this call: spawn success must transfer exact Child
                // ownership even if HTTP or service shutdown occurs concurrently.
                match launcher.launch(authority) {
                    Ok(started) => {
                        let identity = ReapIdentity {
                            host_dir: launcher.host_dir().to_path_buf(),
                            record: started.authority.record().clone(),
                        };
                        let observation = reaper.adopt(started.child, identity);
                        Ok(Created::Started(started.authority, observation))
                    }
                    Err(failed) => store
                        .mark_failed(failed.authority, Failure::LaunchRejected)
                        .map(Created::Failed)
                        .map_err(Error::Store),
                }
            })
            .await?;
        let (authority, observation) = match created {
            Created::Existing(record) => {
                return self
                    .refresh(record, Instant::now() + self.limits.readiness_timeout)
                    .await;
            }
            Created::Failed(record) => return Ok(record),
            Created::Started(authority, observation) => (authority, observation),
        };
        let record = authority.record().clone();
        self.children.insert(record.record_id().into(), observation);
        let deadline = Instant::now() + self.limits.readiness_timeout;
        loop {
            if self.child_exited(record.record_id()) {
                return self
                    .work(move |store| {
                        store
                            .mark_failed(authority, Failure::LaunchRejected)
                            .map_err(Error::Store)
                    })
                    .await;
            }
            if self.stop.is_cancelled() || Instant::now() >= deadline {
                return self
                    .work(move |store| store.mark_uncertain(authority).map_err(Error::Store))
                    .await;
            }
            match self.probe(&record, deadline).await {
                Observation::Running => {
                    return self
                        .work(move |store| store.mark_running(authority).map_err(Error::Store))
                        .await;
                }
                Observation::Exited => {
                    return self
                        .work(move |store| {
                            store
                                .mark_failed(authority, Failure::LaunchRejected)
                                .map_err(Error::Store)
                        })
                        .await;
                }
                Observation::Unavailable => {}
            }
            self.pause(deadline).await;
        }
    }
    async fn get(&mut self, id: String) -> Result<Record, Error> {
        let record = self
            .work(move |store| store.get(&id).map_err(Error::Store))
            .await?;
        self.refresh(record, Instant::now() + self.limits.readiness_timeout)
            .await
    }
    fn child_exited(&self, id: &str) -> bool {
        self.children
            .get(id)
            .is_some_and(|state| *state.borrow() == ChildState::Exited)
    }
    async fn probe(&mut self, record: &Record, deadline: Instant) -> Observation {
        if let Some(observation) = self.probe_local(record, deadline) {
            return observation;
        }
        let outcome = Self::probe_remote(self.client.clone(), record, deadline).await;
        self.apply_remote(record, outcome)
    }
    /// Evidence that needs no host round trip: the owned child's exit state and
    /// the deadline. `Some` settles the receipt; `None` means ask the host.
    fn probe_local(&mut self, record: &Record, deadline: Instant) -> Option<Observation> {
        self.bindings.remove(record.record_id());
        if !self.children.contains_key(record.record_id())
            && let Some(reaper) = REAPER.get().and_then(|result| result.as_ref().ok())
            && let Some(state) = reaper.observe(record, self.launcher.host_dir())
        {
            self.children.insert(record.record_id().into(), state);
        }
        if self.child_exited(record.record_id()) {
            self.targets.remove(record.record_id());
            return Some(Observation::Exited);
        }
        if self.stop.is_cancelled() || Instant::now() >= deadline {
            return Some(Observation::Unavailable);
        }
        None
    }
    /// The host round trips for one receipt, without `&mut self` so a list can
    /// run several at once. Mutations happen afterwards in `apply_remote`.
    async fn probe_remote(client: HostClient, record: &Record, deadline: Instant) -> RemoteProbe {
        let source = host_source(record.spec().source());
        let observation = tokio::time::timeout_at(
            deadline,
            client.status_launch(
                record.host_name(),
                source,
                record.launch_id(),
                record.instance_id(),
            ),
        )
        .await;
        match observation {
            Ok(Ok(observation)) => RemoteProbe::Observed(Box::new(observation)),
            _ if record.state() != State::Starting => {
                match client
                    .retire_if_local_process_dead(record.host_name())
                    .await
                {
                    Ok(true) => RemoteProbe::Retired,
                    _ => RemoteProbe::Unavailable,
                }
            }
            // A freshly spawned ptyhost has not necessarily written its record
            // before the first readiness probe. The owned live child remains
            // authoritative while Starting; missing metadata is not an exit.
            _ => RemoteProbe::Unavailable,
        }
    }
    fn apply_remote(&mut self, record: &Record, outcome: RemoteProbe) -> Observation {
        let source = host_source(record.spec().source());
        match outcome {
            RemoteProbe::Observed(observation) if observation.exited => {
                self.bindings
                    .insert(record.record_id().into(), observation.native_binding);
                self.targets.remove(record.record_id());
                Observation::Exited
            }
            RemoteProbe::Observed(observation) => {
                self.bindings.insert(
                    record.record_id().into(),
                    observation.native_binding.clone(),
                );
                match LaunchTarget::from_observation(
                    &observation,
                    source,
                    record.launch_id(),
                    record.instance_id(),
                ) {
                    Ok(target) => {
                        self.targets
                            .insert(record.record_id().into(), Arc::new(target));
                        Observation::Running
                    }
                    Err(_) => Observation::Unavailable,
                }
            }
            RemoteProbe::Retired => {
                self.targets.remove(record.record_id());
                Observation::Exited
            }
            RemoteProbe::Unavailable => Observation::Unavailable,
        }
    }
    /// The list refresh. Settled receipts (Prepared, Failed, Exited) are
    /// returned as stored: nothing about them can change any more, and `exited`
    /// receipts accumulate, so probing each one made `term/list` grow linearly
    /// with ledger history (73 receipts ≈ 0.5–0.7 s). Live receipts get their
    /// host round trips in parallel, then one store pass persists every
    /// observation after a single ledger reload. Order is preserved.
    async fn refresh_all(
        &mut self,
        records: Vec<Record>,
        deadline: Instant,
    ) -> Result<Vec<Record>, Error> {
        let mut slots: Vec<Option<Record>> = Vec::with_capacity(records.len());
        let mut observed: Vec<(usize, Record, Observation)> = Vec::new();
        let mut remote: Vec<(usize, Record)> = Vec::new();
        for record in records {
            let slot = slots.len();
            if matches!(
                record.state(),
                State::Prepared | State::Failed | State::Exited
            ) {
                slots.push(Some(record));
                continue;
            }
            slots.push(None);
            match self.probe_local(&record, deadline) {
                Some(observation) => observed.push((slot, record, observation)),
                None => remote.push((slot, record)),
            }
        }
        let client = self.client.clone();
        let outcomes: Vec<(usize, Record, RemoteProbe)> = stream::iter(remote)
            .map(|(slot, record)| {
                let client = client.clone();
                async move {
                    let outcome = Self::probe_remote(client, &record, deadline).await;
                    (slot, record, outcome)
                }
            })
            .buffer_unordered(LIST_PARALLEL_PROBES)
            .collect()
            .await;
        for (slot, record, outcome) in outcomes {
            let observation = self.apply_remote(&record, outcome);
            observed.push((slot, record, observation));
        }
        let items: Vec<(usize, ObservationEvidence, BindingObservation)> = observed
            .into_iter()
            .map(|(slot, record, observation)| {
                let binding = binding_observation(&record, self.bindings.get(record.record_id()));
                (
                    slot,
                    ObservationEvidence::new(&record, observation),
                    binding,
                )
            })
            .collect();
        let refreshed = self
            .work(move |store| store.refresh_many(items).map_err(binding_store_error))
            .await?;
        for (slot, record) in refreshed {
            slots[slot] = Some(record);
        }
        Ok(slots.into_iter().flatten().collect())
    }
    async fn refresh(&mut self, record: Record, deadline: Instant) -> Result<Record, Error> {
        if matches!(record.state(), State::Prepared | State::Failed) {
            return Ok(record);
        }
        let observation = self.probe(&record, deadline).await;
        let binding = binding_observation(&record, self.bindings.get(record.record_id()));
        let evidence = ObservationEvidence::new(&record, observation);
        self.work(move |store| {
            let record = store.observe(evidence).map_err(Error::Store)?;
            store
                .observe_binding(BindingEvidence::new(&record, binding))
                .map_err(binding_store_error)
        })
        .await
    }
    async fn authorize_native(&mut self, target: BoundTarget) -> Result<Record, Error> {
        let launch = target.origin_launch_id().ok_or(Error::InvalidBinding)?;
        let records = self
            .work(|store| store.list(0, usize::MAX).map_err(Error::Store))
            .await?;
        let mut matching = records.into_iter().filter(|record| {
            record.host_name() == target.name()
                && record.launch_id() == launch
                && record.instance_id() == target.instance_id()
                && host_source(record.spec().source()) == target.source()
        });
        let record = matching.next().ok_or(Error::NotReady)?;
        if matching.next().is_some() {
            return Err(Error::IdentityConflict);
        }
        let record = self
            .refresh(record, Instant::now() + self.limits.readiness_timeout)
            .await?;
        if record.state() != State::Running
            || record.cancel_requested()
            || !record.binding().is_some_and(|binding| {
                binding.state() == BindingState::Confirmed
                    && binding.spec().sid() == target.sid()
                    && binding.spec().uid() == target.uid()
                    && host_source(binding.spec().source()) == target.source()
            })
        {
            return Err(Error::NotReady);
        }
        Ok(record)
    }
    async fn bind(&mut self, verified: VerifiedNativeBinding) -> Result<Record, Error> {
        let record = self.get(verified.record_id.clone()).await?;
        if record.launch_id() != verified.launch_id
            || record.instance_id() != verified.instance_id
            || record.spec().source() != verified.spec.source()
        {
            return Err(Error::IdentityConflict);
        }
        if record.state() != State::Running || record.cancel_requested() {
            return Err(Error::NotReady);
        }
        if record
            .binding()
            .is_some_and(|binding| binding.spec() != &verified.spec)
        {
            return Err(Error::BindingConflict);
        }
        let already_bound = match self.bindings.get(record.record_id()) {
            Some(NativeBindingState::Unsupported) => return Err(Error::BindingUnsupported),
            Some(NativeBindingState::Invalid) => return Err(Error::BindingConflict),
            Some(NativeBindingState::Bound(binding)) => {
                if binding.sid() != verified.spec.sid()
                    || binding.uid() != verified.spec.uid()
                    || binding.source() != host_source(verified.spec.source())
                {
                    return Err(Error::BindingConflict);
                }
                true
            }
            Some(NativeBindingState::Unbound) => false,
            None => return Err(Error::NotReady),
        };
        if already_bound
            && record
                .binding()
                .is_some_and(|binding| binding.state() == BindingState::Confirmed)
        {
            return Ok(record);
        }
        let target = self
            .targets
            .get(record.record_id())
            .cloned()
            .ok_or(Error::NotReady)?;
        let spec = verified.spec;
        let (method, evidence) = (verified.method, verified.evidence);
        let authority = self
            .work(move |store| {
                store
                    .begin_binding_with(&record, &spec, method, evidence)
                    .map_err(binding_store_error)
            })
            .await?;
        let record = authority.record().clone();
        let deadline = Instant::now() + self.limits.readiness_timeout;
        if !already_bound && !self.stop.is_cancelled() {
            let spec = record.binding().ok_or(Error::WorkerFailed)?.spec();
            let _ = tokio::time::timeout_at(
                deadline,
                self.client.bind_launch(&target, spec.sid(), spec.uid()),
            )
            .await;
        }
        // ACK alone cannot confirm a persisted association; require a current
        // same-instance guarded Info, including after a lost bind ACK.
        let observation = self.probe(&record, deadline).await;
        let binding = binding_observation(&record, self.bindings.get(record.record_id()));
        self.work(move |store| {
            let record = store
                .finish_binding(authority, binding)
                .map_err(binding_store_error)?;
            store
                .observe(ObservationEvidence::new(&record, observation))
                .map_err(Error::Store)
        })
        .await
    }
    async fn cancel(&mut self, id: String, expected_instance: String) -> Result<Record, Error> {
        let outcome = self
            .work(move |store| {
                store
                    .request_cancel(&id, &expected_instance)
                    .map_err(Error::Store)
            })
            .await?;
        let Some(authority) = outcome.authority else {
            // Local retirement is idempotent and may be retried after transient
            // gate/admission failure. A replay never regains kill authority.
            if outcome.record.cancel_requested() {
                if !self.targets.contains_key(outcome.record.record_id()) {
                    self.probe(&outcome.record, Instant::now() + self.limits.cancel_timeout)
                        .await;
                }
                if let Some(target) = self.targets.get(outcome.record.record_id()).cloned() {
                    self.terminal
                        .retire_launch(target)
                        .await
                        .map_err(|_| Error::RetirementFailed)?;
                }
            }
            return self
                .refresh(outcome.record, Instant::now() + self.limits.cancel_timeout)
                .await;
        };
        let record = authority.record().clone();
        let deadline = Instant::now() + self.limits.cancel_timeout;
        // A cached target pins identity even if the host is now unreachable, so
        // local browser ownership can still be retired before any kill attempt.
        let initial = if !self.targets.contains_key(record.record_id()) {
            self.probe(&record, deadline).await
        } else {
            Observation::Unavailable
        };
        let Some(target) = self.targets.get(record.record_id()).cloned() else {
            let observed =
                if initial == Observation::Exited || self.child_exited(record.record_id()) {
                    Observation::Exited
                } else {
                    Observation::Unavailable
                };
            return self
                .work(move |store| {
                    store
                        .finish_cancel(authority, observed)
                        .map_err(Error::Store)
                })
                .await;
        };
        if self.terminal.retire_launch(target.clone()).await.is_err() {
            self.work(move |store| {
                store
                    .finish_cancel(authority, Observation::Unavailable)
                    .map_err(Error::Store)
            })
            .await?;
            return Err(Error::RetirementFailed);
        }
        if self.stop.is_cancelled() || Instant::now() >= deadline {
            return self
                .work(move |store| {
                    store
                        .finish_cancel(authority, Observation::Unavailable)
                        .map_err(Error::Store)
                })
                .await;
        }
        // SSH/shell gets the same graceful stage a session stop gives a CLI:
        // EOF first (a shell at its prompt exits at once), the guarded stop
        // only if it is still there after [`GRACEFUL_WAIT`].
        if record.spec().source() == Source::Shell {
            let grace = (Instant::now() + GRACEFUL_WAIT).min(deadline);
            let _ = tokio::time::timeout_at(
                grace,
                self.client.request_launch(
                    &target,
                    ControlOp::Keys {
                        keys: vec!["C-d".into()],
                    },
                ),
            )
            .await;
            let exited = loop {
                if self.stop.is_cancelled() || Instant::now() >= grace {
                    break false;
                }
                match self.probe(&record, grace).await {
                    Observation::Exited => break true,
                    Observation::Unavailable | Observation::Running => self.pause(grace).await,
                }
            };
            if exited {
                return self
                    .work(move |store| {
                        store
                            .finish_cancel(authority, Observation::Exited)
                            .map_err(Error::Store)
                    })
                    .await;
            }
        }
        // Exactly one kill attempt per durable cancel intent. Even a timeout or
        // missing ACK can follow an applied kill; later requests only observe.
        let _ = tokio::time::timeout_at(
            deadline,
            self.client
                .request_launch(&target, ControlOp::Kill { force: false }),
        )
        .await;
        let observed = loop {
            if self.stop.is_cancelled() || Instant::now() >= deadline {
                break Observation::Unavailable;
            }
            match self.probe(&record, deadline).await {
                Observation::Exited => break Observation::Exited,
                Observation::Unavailable | Observation::Running => self.pause(deadline).await,
            }
        };
        self.work(move |store| {
            store
                .finish_cancel(authority, observed)
                .map_err(Error::Store)
        })
        .await
    }
    async fn pause(&self, deadline: Instant) {
        tokio::select! { _ = self.stop.cancelled() => {}, _ = tokio::time::sleep_until((Instant::now() + self.limits.poll_interval).min(deadline)) => {} }
    }

    // ------------------------------------------------------------ session stop
    async fn stop_session(
        &mut self,
        uid: String,
        candidate: StopCandidate,
    ) -> Result<StopOutcome, Error> {
        let outcome = match candidate {
            StopCandidate::Instance(target) => self.stop_instance(&uid, *target).await?,
            StopCandidate::Exited => StopOutcome::new(&uid, StopStage::AlreadyExited),
            StopCandidate::Unknown => StopOutcome::new(&uid, StopStage::Unknown),
            StopCandidate::NoInstance => {
                // A retained receipt that declared or bound exactly this UID and
                // is durably Exited still answers "already exited" after the host
                // record was cleaned (also off Linux, where identity memory is
                // unavailable). Anything else is not evidence of any kind.
                let records = self
                    .work(|store| store.list(0, usize::MAX).map_err(Error::Store))
                    .await?;
                let exited = records.iter().find(|record| {
                    record.state() == State::Exited
                        && (record.declared_uid() == Some(uid.as_str())
                            || record
                                .binding()
                                .is_some_and(|binding| binding.spec().uid() == uid))
                });
                match exited {
                    Some(record) => StopOutcome {
                        name: Some(record.host_name().to_owned()),
                        instance_id: Some(record.instance_id().to_owned()),
                        record_id: Some(record.record_id().to_owned()),
                        ..StopOutcome::new(&uid, StopStage::AlreadyExited)
                    },
                    None => StopOutcome::new(&uid, StopStage::NoInstance),
                }
            }
        };
        Ok(outcome)
    }

    /// The receipt this Web process holds for the exact host instance, if any.
    /// Declared identities (Claude `--session-id`, resume) carry no origin
    /// launch id on the bound target, so name + instance + source identify
    /// the receipt; operator-bound targets additionally pin their launch id.
    fn receipt_for(records: Vec<Record>, target: &BoundTarget) -> Option<Record> {
        records.into_iter().find(|record| {
            record.host_name() == target.name()
                && record.instance_id() == target.instance_id()
                && host_source(record.spec().source()) == target.source()
                && target
                    .origin_launch_id()
                    .is_none_or(|launch| launch == record.launch_id())
        })
    }

    /// Exit evidence for the exact instance only. With a receipt this is the
    /// same launch-guarded status (plus the owned Child reaper) `cancel`
    /// trusts; without one, the host's own record: a cleaned record after a
    /// live guarded probe means that host finished, a replaced instance never
    /// counts as exit.
    async fn instance_exited(
        &mut self,
        record: Option<&Record>,
        target: &BoundTarget,
        deadline: Instant,
    ) -> bool {
        if let Some(record) = record {
            return self.probe(record, deadline).await == Observation::Exited;
        }
        match tokio::time::timeout_at(deadline, self.client.probe(target.name())).await {
            Ok(Ok(observation)) => {
                observation.exited
                    && observation.instance_id.as_deref() == Some(target.instance_id())
            }
            Ok(Err(ptyhost_client::Error::NotFound)) => true,
            _ => false,
        }
    }

    async fn wait_exit(
        &mut self,
        record: Option<&Record>,
        target: &BoundTarget,
        deadline: Instant,
    ) -> bool {
        loop {
            if self.instance_exited(record, target, deadline).await {
                return true;
            }
            if self.stop.is_cancelled() || Instant::now() >= deadline {
                return false;
            }
            self.pause(deadline).await;
        }
    }

    /// Record the observed exit on the receipt now instead of at the next
    /// list/get refresh. A fresh deadline: an expired one would read as
    /// Unavailable and downgrade the receipt to Uncertain.
    async fn persist_exit(&mut self, record: Option<&Record>) {
        let Some(record) = record else {
            return;
        };
        // The exact instance's exit was just observed (guarded Info
        // `exited`, or the owned Child reaped). Persist that observation
        // directly: re-probing here can race the host's own cleanup (record
        // and socket gone before the reaper tick) and would downgrade the
        // receipt to Uncertain right after a confirmed stop.
        let evidence = ObservationEvidence::new(record, Observation::Exited);
        let persisted = self
            .work(move |store| store.observe(evidence).map_err(Error::Store))
            .await;
        if persisted.is_err() {
            let _ = self
                .refresh(record.clone(), Instant::now() + self.limits.probe_timeout)
                .await;
        }
        self.targets.remove(record.record_id());
    }

    async fn stop_instance(
        &mut self,
        uid: &str,
        target: BoundTarget,
    ) -> Result<StopOutcome, Error> {
        let records = self
            .work(|store| store.list(0, usize::MAX).map_err(Error::Store))
            .await?;
        let record = Self::receipt_for(records, &target);
        let mut outcome = StopOutcome {
            name: Some(target.name().to_owned()),
            instance_id: Some(target.instance_id().to_owned()),
            record_id: record.as_ref().map(|record| record.record_id().to_owned()),
            ..StopOutcome::new(uid, StopStage::Uncertain)
        };
        let finish = |mut outcome: StopOutcome, stage: StopStage| {
            outcome.stage = stage;
            outcome.stopped = matches!(
                stage,
                StopStage::Graceful | StopStage::Stopped | StopStage::AlreadyExited
            );
            outcome
        };
        let probe = Instant::now() + self.limits.probe_timeout;
        if self.instance_exited(record.as_ref(), &target, probe).await {
            self.persist_exit(record.as_ref()).await;
            return Ok(finish(outcome, StopStage::AlreadyExited));
        }
        // Graceful stage: EOF through the guarded input path,
        // sending `C-d`, bounded per attempt. A failed send
        // is not exit evidence; only the exact instance's status is.
        for _ in 0..GRACEFUL_ATTEMPTS {
            if self.stop.is_cancelled() {
                return Ok(outcome);
            }
            outcome.graceful_attempts += 1;
            let _ = tokio::time::timeout(
                self.limits.probe_timeout,
                self.client.request_bound(
                    &target,
                    ControlOp::Keys {
                        keys: vec!["C-d".into()],
                    },
                ),
            )
            .await;
            let deadline = Instant::now() + GRACEFUL_WAIT;
            if self.wait_exit(record.as_ref(), &target, deadline).await {
                self.persist_exit(record.as_ref()).await;
                return Ok(finish(outcome, StopStage::Graceful));
            }
        }
        // Guarded stop: for a receipt this is the durable `term/kill` path
        // (persist cancel → retire leases → one host-performed HUP → exact
        // exit evidence); without one, the host's own guarded `kill`. Neither
        // signals a PID from here.
        match &record {
            Some(record) => {
                let cancelled = self
                    .cancel(record.record_id().into(), record.instance_id().into())
                    .await?;
                if cancelled.state() == State::Exited {
                    return Ok(finish(outcome, StopStage::Stopped));
                }
                Ok(outcome)
            }
            None => {
                let deadline = Instant::now() + self.limits.cancel_timeout;
                let _ = tokio::time::timeout_at(
                    deadline,
                    self.client
                        .request_bound(&target, ControlOp::Kill { force: false }),
                )
                .await;
                if self.wait_exit(None, &target, deadline).await {
                    return Ok(finish(outcome, StopStage::Stopped));
                }
                Ok(outcome)
            }
        }
    }
}

fn host_source(source: Source) -> ptyhost_client::Source {
    match source {
        Source::Claude => ptyhost_client::Source::Claude,
        Source::Codex => ptyhost_client::Source::Codex,
        Source::Grok => ptyhost_client::Source::Grok,
        Source::Shell => ptyhost_client::Source::Shell,
    }
}
fn binding_observation(
    record: &Record,
    binding: Option<&NativeBindingState>,
) -> BindingObservation {
    match binding {
        Some(NativeBindingState::Bound(binding))
            if binding.source() == host_source(record.spec().source())
                && binding.launch_id() == record.launch_id()
                && binding.instance_id() == record.instance_id() =>
        {
            match BindingSpec::new(
                record.spec().source(),
                binding.sid().into(),
                binding.uid().into(),
            ) {
                Ok(spec) => BindingObservation::Confirmed(spec),
                Err(_) => BindingObservation::Conflict,
            }
        }
        Some(NativeBindingState::Bound(_) | NativeBindingState::Invalid) => {
            BindingObservation::Conflict
        }
        _ => BindingObservation::Unavailable,
    }
}
fn binding_store_error(error: store::Error) -> Error {
    if error == store::Error::Conflict {
        Error::BindingConflict
    } else {
        Error::Store(error)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChildState {
    Alive,
    Exited,
    Unknown,
}
struct ReapJob {
    identity: ReapIdentity,
    child: Child,
    state: watch::Sender<ChildState>,
}
struct ReapIdentity {
    host_dir: PathBuf,
    record: Record,
}
struct Reaper {
    jobs: Mutex<Vec<ReapJob>>,
}
static REAPER: OnceLock<Result<Arc<Reaper>, ()>> = OnceLock::new();
impl Reaper {
    fn global() -> Result<Arc<Self>, Error> {
        REAPER
            .get_or_init(|| {
                let reaper = Arc::new(Self {
                    jobs: Mutex::new(Vec::new()),
                });
                let worker = reaper.clone();
                std::thread::Builder::new()
                    .name("lifecycle-child-reaper".into())
                    .spawn(move || {
                        loop {
                            {
                                let mut jobs = worker
                                    .jobs
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                let mut index = 0;
                                while index < jobs.len() {
                                    match jobs[index].child.try_wait() {
                                        Ok(Some(_)) => {
                                            let job = jobs.swap_remove(index);
                                            let _ = job.state.send(ChildState::Exited);
                                        }
                                        Ok(None) => index += 1,
                                        Err(_) => {
                                            let _ = jobs[index].state.send(ChildState::Unknown);
                                            index += 1;
                                        }
                                    }
                                }
                            }
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    })
                    .map_err(|_| ())?;
                Ok(reaper)
            })
            .as_ref()
            .cloned()
            .map_err(|_| Error::ReaperUnavailable)
    }
    fn adopt(&self, child: Child, identity: ReapIdentity) -> watch::Receiver<ChildState> {
        let (state, receiver) = watch::channel(ChildState::Alive);
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(ReapJob {
                identity,
                child,
                state,
            });
        receiver
    }
    fn observe(
        &self,
        record: &Record,
        host_dir: &std::path::Path,
    ) -> Option<watch::Receiver<ChildState>> {
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|job| {
                let owned = &job.identity.record;
                job.identity.host_dir == host_dir
                    && owned.record_id() == record.record_id()
                    && owned.host_name() == record.host_name()
                    && owned.spec().source() == record.spec().source()
                    && owned.launch_id() == record.launch_id()
                    && owned.instance_id() == record.instance_id()
            })
            .map(|job| job.state.subscribe())
    }
}

#[cfg(all(test, unix))]
#[path = "service_tests.rs"]
mod tests;
