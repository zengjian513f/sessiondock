use super::super::model::Source;
use super::*;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

static TEST_GATE: Semaphore = Semaphore::const_new(1);
static BLOCKING_PAUSE: Mutex<Option<Arc<BlockingPause>>> = Mutex::new(None);
#[derive(Default)]
struct BlockingPause {
    entered: tokio::sync::Notify,
    released: Mutex<bool>,
    changed: std::sync::Condvar,
}
pub(super) fn pause_blocking_work() {
    let pause = BLOCKING_PAUSE.lock().unwrap().take();
    if let Some(pause) = pause {
        pause.entered.notify_one();
        let mut released = pause.released.lock().unwrap();
        while !*released {
            released = pause.changed.wait(released).unwrap();
        }
    }
}
impl BlockingPause {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_one();
    }
}
struct Fixture {
    temp: tempfile::TempDir,
    config: launcher::Config,
    ledger: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        for name in ["host", "work", "bin", "ledger"] {
            fs::create_dir(temp.path().join(name)).unwrap();
            fs::set_permissions(temp.path().join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        for name in ["host", "adapter"] {
            fs::write(
                temp.path().join("bin").join(name),
                b"invalid synthetic executable",
            )
            .unwrap();
            fs::set_permissions(
                temp.path().join("bin").join(name),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        let config = launcher::Config {
            schema: 1,
            host_binary: temp.path().join("bin/host"),
            host_dir: temp.path().join("host"),
            adapters: vec![launcher::Adapter {
                id: "shell-v1".into(),
                source: Source::Codex,
                executable: temp.path().join("bin/adapter"),
                args: vec![],
                env: BTreeMap::new(),
            }],
            profiles: vec![],
        };
        let ledger = temp.path().join("ledger");
        drop(LifecycleStore::initialize(&ledger).unwrap());
        Self {
            temp,
            config,
            ledger,
        }
    }
    fn spec(&self) -> LaunchSpec {
        LaunchSpec::new(
            Source::Codex,
            "shell-v1".into(),
            &self.temp.path().join("work"),
        )
        .unwrap()
    }
    fn seed(&self, request: &str, state: State) -> Record {
        let mut store = LifecycleStore::open(&self.ledger).unwrap();
        let outcome = store.create(request, &self.spec()).unwrap();
        if state == State::Prepared {
            return outcome.record;
        }
        let start = store.begin_start(outcome.prepared.unwrap()).unwrap();
        if state == State::Starting {
            return start.record().clone();
        }
        let running = store.mark_running(start).unwrap();
        if state == State::CancelRequested {
            return store
                .request_cancel(running.record_id(), running.instance_id())
                .unwrap()
                .record;
        }
        running
    }
    async fn open(&self, limits: ServiceLimits) -> LifecycleService {
        LifecycleService::open(
            self.ledger.clone(),
            self.config.clone(),
            Arc::new(TerminalService::new(self.config.host_dir.clone()).unwrap()),
            limits,
            CancellationToken::new(),
        )
        .await
        .unwrap()
    }
}
fn limits() -> ServiceLimits {
    ServiceLimits {
        readiness_timeout: Duration::from_millis(250),
        cancel_timeout: Duration::from_millis(250),
        probe_timeout: Duration::from_millis(100),
        poll_interval: Duration::from_millis(5),
        ..Default::default()
    }
}
async fn fixture() -> (tokio::sync::SemaphorePermit<'static>, Fixture) {
    let gate = TEST_GATE.acquire().await.unwrap();
    (
        gate,
        tokio::task::spawn_blocking(Fixture::new).await.unwrap(),
    )
}

struct Peer {
    state: Arc<Mutex<Value>>,
    exited: Arc<AtomicBool>,
    ack: Arc<AtomicBool>,
    kill_exits: Arc<AtomicBool>,
    kills: Arc<AtomicUsize>,
    binding: Arc<Mutex<Value>>,
    binds: Arc<AtomicUsize>,
    bind_ack: Arc<AtomicBool>,
    bind_apply: Arc<AtomicBool>,
    bind_pause: Arc<AtomicBool>,
    binding_capable: Arc<AtomicBool>,
    info_fail: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    stop: CancellationToken,
    path: PathBuf,
}
impl Peer {
    async fn new(f: &Fixture, record: &Record) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let process = std::process::id();
        let raw = json!({"name":record.host_name(),"host_pid":process,"pid":process,"created":0,"cols":80,"rows":24,"port":listener.local_addr().unwrap().port(),"token":"PRIVATE_HOST_TOKEN","meta":{"source":"codex","launch_id":record.launch_id(),"instance_id":record.instance_id()},"argv":["PRIVATE_ARG"]});
        let path = f
            .config
            .host_dir
            .join(format!("{}.json", record.host_name()));
        fs::write(&path, raw.to_string()).unwrap();
        let peer = Self {
            state: Arc::new(Mutex::new(raw)),
            exited: Arc::new(AtomicBool::new(false)),
            ack: Arc::new(AtomicBool::new(true)),
            kill_exits: Arc::new(AtomicBool::new(true)),
            kills: Arc::new(AtomicUsize::new(0)),
            binding: Arc::new(Mutex::new(Value::Null)),
            binds: Arc::new(AtomicUsize::new(0)),
            bind_ack: Arc::new(AtomicBool::new(true)),
            bind_apply: Arc::new(AtomicBool::new(true)),
            bind_pause: Arc::new(AtomicBool::new(false)),
            binding_capable: Arc::new(AtomicBool::new(true)),
            info_fail: Arc::new(AtomicBool::new(false)),
            pause: Arc::new(AtomicBool::new(false)),
            entered: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
            stop: CancellationToken::new(),
            path,
        };
        let (state, exited, ack, kill_exits, kills, pause, entered, release, stop) = (
            peer.state.clone(),
            peer.exited.clone(),
            peer.ack.clone(),
            peer.kill_exits.clone(),
            peer.kills.clone(),
            peer.pause.clone(),
            peer.entered.clone(),
            peer.release.clone(),
            peer.stop.clone(),
        );
        let (binding, binds, bind_ack, bind_apply, bind_pause, binding_capable, info_fail) = (
            peer.binding.clone(),
            peer.binds.clone(),
            peer.bind_ack.clone(),
            peer.bind_apply.clone(),
            peer.bind_pause.clone(),
            peer.binding_capable.clone(),
            peer.info_fail.clone(),
        );
        tokio::spawn(async move {
            loop {
                let mut stream = tokio::select! { _=stop.cancelled()=>break, accepted=listener.accept()=>match accepted {Ok((stream,_))=>stream,Err(_)=>break} };
                let mut bytes = Vec::new();
                loop {
                    match stream.read_u8().await {
                        Ok(b'\n') => break,
                        Ok(b) if bytes.len() < 8192 => bytes.push(b),
                        _ => break,
                    }
                }
                let Ok(request) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                if pause.swap(false, Ordering::SeqCst) {
                    entered.notify_one();
                    tokio::select! { _=stop.cancelled()=>break,_=release.notified()=>{} }
                }
                let guarded = request["op"] == "launch_guard_v1";
                let operation = if guarded {
                    &request["request"]["op"]
                } else {
                    &request["op"]
                };
                if operation == "launch_bind_v1" && bind_pause.swap(false, Ordering::SeqCst) {
                    entered.notify_one();
                    tokio::select! { _=stop.cancelled()=>break,_=release.notified()=>{} }
                }
                let raw = state.lock().unwrap().clone();
                if operation == "kill" {
                    kills.fetch_add(1, Ordering::SeqCst);
                    if kill_exits.load(Ordering::SeqCst) {
                        exited.store(true, Ordering::SeqCst);
                    }
                }
                let mut response = if operation == "info" {
                    json!({"ok":true,"info":raw,"exited":exited.load(Ordering::SeqCst),"capabilities":{"launch_guard":1,"instance_guard":1}})
                } else {
                    json!({"ok":true})
                };
                if operation == "info" && binding_capable.load(Ordering::SeqCst) {
                    response["capabilities"]["launch_bind"] = json!(1);
                    response["native_binding"] = binding.lock().unwrap().clone();
                }
                if operation == "info" && info_fail.load(Ordering::SeqCst) {
                    response = json!({"ok":false,"error":"synthetic unavailable"});
                }
                if operation == "launch_bind_v1" {
                    binds.fetch_add(1, Ordering::SeqCst);
                    let desired = json!({"version":1,"method":"operator","source":"codex","sid":request["native"]["sid"],"uid":request["native"]["uid"],"launch_id":raw["meta"]["launch_id"],"instance_id":raw["meta"]["instance_id"]});
                    let mut current = binding.lock().unwrap();
                    if request["expected_launch_id"] != raw["meta"]["launch_id"]
                        || request["expected_instance_id"] != raw["meta"]["instance_id"]
                        || (!current.is_null() && *current != desired)
                    {
                        response = json!({"ok":false,"code":"native_binding_conflict"});
                    } else {
                        if bind_apply.load(Ordering::SeqCst) {
                            *current = desired.clone();
                        }
                        response["native_binding"] = desired;
                    }
                }
                if guarded && ack.load(Ordering::SeqCst) {
                    response["launch_guard"] = json!({"version":1,"instance_id":raw["meta"]["instance_id"],"source":"codex","launch_id":raw["meta"]["launch_id"]});
                }
                if operation == "launch_bind_v1" && bind_ack.load(Ordering::SeqCst) {
                    response["launch_guard"] = json!({"version":1,"instance_id":raw["meta"]["instance_id"],"source":"codex","launch_id":raw["meta"]["launch_id"]});
                }
                let _ = stream.write_all(format!("{response}\n").as_bytes()).await;
            }
        });
        peer
    }
    fn replace_instance(&self) {
        let mut raw = self.state.lock().unwrap();
        raw["meta"]["instance_id"] = json!("replacement-instance-000000");
        fs::write(&self.path, raw.to_string()).unwrap();
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.stop.cancel();
        self.release.notify_waiters();
    }
}

#[tokio::test]
async fn historical_running_requires_guarded_observation_and_target_never_guesses_native_identity()
{
    let (_gate, f) = fixture().await;
    let record = f.seed("request-running", State::Running);
    let peer = Peer::new(&f, &record).await;
    let service = f.open(limits()).await;
    assert_eq!(
        service
            .get(record.record_id().into())
            .await
            .unwrap()
            .state(),
        State::Running
    );
    let target = service.target(record.record_id().into()).await.unwrap();
    assert_eq!(target.launch_id(), record.launch_id());
    assert_eq!(target.instance_id(), record.instance_id());
    peer.ack.store(false, Ordering::SeqCst);
    assert_eq!(
        service
            .get(record.record_id().into())
            .await
            .unwrap()
            .state(),
        State::Uncertain
    );
    assert!(matches!(
        service.target(record.record_id().into()).await,
        Err(Error::NotReady)
    ));
    peer.ack.store(true, Ordering::SeqCst);
    peer.exited.store(true, Ordering::SeqCst);
    assert_eq!(
        service
            .get(record.record_id().into())
            .await
            .unwrap()
            .state(),
        State::Exited
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancel_is_durable_before_effect_and_dropped_response_never_loses_work() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-cancel", State::Running);
    let peer = Peer::new(&f, &record).await;
    peer.pause.store(true, Ordering::SeqCst);
    let service = f.open(limits()).await;
    let response = service
        .admit(Command::Cancel(
            record.record_id().into(),
            record.instance_id().into(),
        ))
        .await
        .unwrap();
    peer.entered.notified().await;
    let raw: Value =
        serde_json::from_slice(&fs::read(f.ledger.join(store::LEDGER_FILENAME)).unwrap()).unwrap();
    assert_eq!(
        raw["records"][record.record_id()]["state"],
        "cancel_requested"
    );
    assert_eq!(peer.kills.load(Ordering::SeqCst), 0);
    drop(response);
    peer.release.notify_one();
    let after = service.get(record.record_id().into()).await.unwrap();
    assert_eq!(after.state(), State::Exited);
    assert!(after.cancel_requested());
    assert_eq!(peer.kills.load(Ordering::SeqCst), 1);
    service
        .cancel(record.record_id().into(), record.instance_id().into())
        .await
        .unwrap();
    assert_eq!(peer.kills.load(Ordering::SeqCst), 1);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn kill_ack_without_observed_exit_stays_uncertain_and_is_not_repeated() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-ack-only", State::Running);
    let peer = Peer::new(&f, &record).await;
    peer.kill_exits.store(false, Ordering::SeqCst);
    let service = f.open(limits()).await;
    let cancelled = service
        .cancel(record.record_id().into(), record.instance_id().into())
        .await
        .unwrap();
    assert_eq!(cancelled.state(), State::Uncertain);
    assert!(cancelled.cancel_requested());
    assert!(matches!(
        service.target(record.record_id().into()).await,
        Err(Error::NotReady)
    ));
    service
        .cancel(record.record_id().into(), record.instance_id().into())
        .await
        .unwrap();
    assert_eq!(peer.kills.load(Ordering::SeqCst), 1);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn wrong_instance_and_same_name_replacement_never_receive_kill() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-replaced", State::Running);
    let peer = Peer::new(&f, &record).await;
    let service = f.open(limits()).await;
    assert!(matches!(
        service
            .cancel(record.record_id().into(), "wrong-instance".into())
            .await,
        Err(Error::Store(store::Error::Conflict))
    ));
    service.target(record.record_id().into()).await.unwrap();
    peer.replace_instance();
    let result = service
        .cancel(record.record_id().into(), record.instance_id().into())
        .await
        .unwrap();
    assert_eq!(result.state(), State::Uncertain);
    assert!(result.cancel_requested());
    assert_eq!(peer.kills.load(Ordering::SeqCst), 0);
    service
        .cancel(record.record_id().into(), record.instance_id().into())
        .await
        .unwrap();
    assert_eq!(peer.kills.load(Ordering::SeqCst), 0);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn starting_and_cancel_crash_windows_only_reconcile_never_respawn_or_rekill() {
    let (_gate, f) = fixture().await;
    let started = f.seed("request-start-crash", State::Starting);
    let cancelled = f.seed("request-cancel-crash", State::CancelRequested);
    let _one = Peer::new(&f, &started).await;
    let two = Peer::new(&f, &cancelled).await;
    let service = f.open(limits()).await;
    assert_eq!(
        service
            .get(started.record_id().into())
            .await
            .unwrap()
            .state(),
        State::Running
    );
    let record = service
        .cancel(cancelled.record_id().into(), cancelled.instance_id().into())
        .await
        .unwrap();
    assert_eq!(record.state(), State::Uncertain);
    assert!(record.cancel_requested());
    assert_eq!(two.kills.load(Ordering::SeqCst), 0);
    assert!(matches!(
        service.target(cancelled.record_id().into()).await,
        Err(Error::NotReady)
    ));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn queued_work_waits_and_shutdown_keeps_started_work_owned_until_complete() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-queue", State::Running);
    let peer = Peer::new(&f, &record).await;
    peer.pause.store(true, Ordering::SeqCst);
    let service = Arc::new(
        f.open(ServiceLimits {
            capacity: 2,
            ..limits()
        })
        .await,
    );
    let first = service
        .admit(Command::Get(record.record_id().into()))
        .await
        .unwrap();
    peer.entered.notified().await;
    let queued = service
        .admit(Command::Get(record.record_id().into()))
        .await
        .unwrap();
    let waiting_service = service.clone();
    let record_id = record.record_id().to_owned();
    let waiting = tokio::spawn(async move { waiting_service.admit(Command::Get(record_id)).await });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());
    drop(first);
    let child = service.clone();
    let shutdown = tokio::spawn(async move { child.shutdown().await });
    peer.release.notify_one();
    shutdown.await.unwrap().unwrap();
    assert!(matches!(waiting.await.unwrap(), Err(Error::Closed)));
    assert!(matches!(queued.await.unwrap().answer, Err(Error::Closed)));
    assert_eq!(peer.kills.load(Ordering::SeqCst), 0);
    drop(LifecycleStore::open(&f.ledger).unwrap());
}

#[tokio::test]
async fn spawn_failure_and_duplicate_create_never_turn_into_false_running() {
    let (_gate, f) = fixture().await;
    let service = f.open(limits()).await;
    let first = service
        .create("request-failed-spawn".into(), f.spec())
        .await
        .unwrap();
    assert_eq!(first.state(), State::Failed);
    let duplicate = service
        .create("request-failed-spawn".into(), f.spec())
        .await
        .unwrap();
    assert_eq!(first.record_id(), duplicate.record_id());
    assert_eq!(first.revision(), duplicate.revision());
    let wrong = LaunchSpec::new(Source::Claude, "shell-v1".into(), f.spec().cwd()).unwrap();
    assert!(matches!(
        service.create("request-failed-spawn".into(), wrong).await,
        Err(Error::Store(store::Error::Conflict))
    ));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn already_exited_exact_host_finishes_cancel_without_a_kill_or_cached_target() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-already-exited", State::Running);
    let peer = Peer::new(&f, &record).await;
    peer.exited.store(true, Ordering::SeqCst);
    let service = f.open(limits()).await;
    let cancelled = service
        .cancel(record.record_id().into(), record.instance_id().into())
        .await
        .unwrap();
    assert_eq!(cancelled.state(), State::Exited);
    assert!(cancelled.cancel_requested());
    assert_eq!(peer.kills.load(Ordering::SeqCst), 0);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancel_replay_retries_local_retirement_after_busy_without_retrying_kill() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-retire-retry", State::Running);
    let peer = Peer::new(&f, &record).await;
    let terminal = Arc::new(
        TerminalService::with_limits(
            f.config.host_dir.clone(),
            crate::terminal::BridgeLimits {
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let service = LifecycleService::open(
        f.ledger.clone(),
        f.config.clone(),
        terminal.clone(),
        limits(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let target = service.target(record.record_id().into()).await.unwrap();
    peer.pause.store(true, Ordering::SeqCst);
    let claim = {
        let terminal = terminal.clone();
        let target = target.clone();
        tokio::spawn(async move {
            terminal
                .claim_launch(
                    target,
                    "before-cancel",
                    "127.0.0.1".parse::<std::net::IpAddr>().unwrap(),
                    false,
                )
                .await
        })
    };
    peer.entered.notified().await;
    let first = service
        .cancel(record.record_id().into(), record.instance_id().into())
        .await
        .unwrap();
    assert_eq!(first.state(), State::Uncertain);
    assert!(first.cancel_requested());
    assert_eq!(peer.kills.load(Ordering::SeqCst), 0);
    peer.release.notify_one();
    assert!(claim.await.unwrap().is_err());
    let cancelled = service
        .cancel(record.record_id().into(), record.instance_id().into())
        .await
        .unwrap();
    assert_eq!(cancelled.state(), State::Uncertain);
    assert!(cancelled.cancel_requested());
    let error = terminal
        .claim_launch(
            target,
            "after-cancel",
            "127.0.0.1".parse::<std::net::IpAddr>().unwrap(),
            true,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.status, 409);
    assert_eq!(peer.kills.load(Ordering::SeqCst), 0);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn slow_blocking_work_keeps_capacity_and_store_lock_after_http_response_drop() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-blocked-read", State::Prepared);
    let service = Arc::new(
        f.open(ServiceLimits {
            capacity: 2,
            ..limits()
        })
        .await,
    );
    let pause = Arc::new(BlockingPause::default());
    *BLOCKING_PAUSE.lock().unwrap() = Some(pause.clone());
    let response = service
        .admit(Command::Get(record.record_id().into()))
        .await
        .unwrap();
    pause.entered.notified().await;
    drop(response);
    let queued = service.admit(Command::List(0, 1)).await.unwrap();
    let waiting_service = service.clone();
    let waiting = tokio::spawn(async move { waiting_service.admit(Command::List(0, 1)).await });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());
    drop(LifecycleStore::open(&f.ledger).unwrap());
    let worker = service.clone();
    let shutdown = tokio::spawn(async move { worker.shutdown().await });
    tokio::task::yield_now().await;
    assert!(!shutdown.is_finished());
    assert_eq!(service.admission.available_permits(), 0);
    pause.release();
    shutdown.await.unwrap().unwrap();
    assert!(matches!(waiting.await.unwrap(), Err(Error::Closed)));
    assert!(matches!(queued.await.unwrap().answer, Err(Error::Closed)));
    drop(LifecycleStore::open(&f.ledger).unwrap());
}

fn native_scope() -> crate::sessions::NativeScope {
    crate::sessions::NativeScope {
        source: "codex".into(),
        uid: "codex:0123456789abcdef".into(),
        session_id: "full.native.session".into(),
        agent_id: None,
    }
}
fn verified(record: &Record) -> VerifiedNativeBinding {
    VerifiedNativeBinding::from_scope(&native_scope(), record, true).unwrap()
}
async fn native_target(f: &Fixture, record: &Record) -> BoundTarget {
    let client = HostClient::new(&f.config.host_dir, Default::default()).unwrap();
    let observation = client.probe(record.host_name()).await.unwrap();
    BoundTarget::from_observation(
        &observation,
        ptyhost_client::Source::Codex,
        &native_scope().session_id,
        &native_scope().uid,
    )
    .unwrap()
}

#[tokio::test]
async fn binding_is_durable_before_host_call_and_response_drop_preserves_confirmation() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-native-binding", State::Running);
    let peer = Peer::new(&f, &record).await;
    let host_before = fs::read(&peer.path).unwrap();
    peer.bind_pause.store(true, Ordering::SeqCst);
    let service = f.open(limits()).await;
    let fresh = service.get(record.record_id().into()).await.unwrap();
    let response = service
        .admit(Command::Bind(verified(&fresh)))
        .await
        .unwrap();
    peer.entered.notified().await;
    let raw: Value =
        serde_json::from_slice(&fs::read(f.ledger.join(store::LEDGER_FILENAME)).unwrap()).unwrap();
    assert_eq!(
        raw["records"][record.record_id()]["binding"]["state"],
        "intent"
    );
    assert_eq!(
        raw["records"][record.record_id()]["binding"]["spec"]["sid"],
        native_scope().session_id
    );
    assert_eq!(peer.binds.load(Ordering::SeqCst), 0);
    drop(response);
    peer.release.notify_one();
    let confirmed = service.get(record.record_id().into()).await.unwrap();
    assert_eq!(
        confirmed.binding().unwrap().state(),
        BindingState::Confirmed
    );
    assert_eq!(peer.binds.load(Ordering::SeqCst), 1);
    let duplicate = service.bind(verified(&confirmed)).await.unwrap();
    assert_eq!(
        duplicate.binding().unwrap().state(),
        BindingState::Confirmed
    );
    assert_eq!(peer.binds.load(Ordering::SeqCst), 1);
    assert_eq!(fs::read(&peer.path).unwrap(), host_before);
    let target = native_target(&f, &record).await;
    assert_eq!(
        service.authorize_native(&target).await.unwrap().record_id(),
        record.record_id()
    );
    service.shutdown().await.unwrap();
    let reopened = f.open(limits()).await;
    assert_eq!(
        reopened
            .get(record.record_id().into())
            .await
            .unwrap()
            .binding()
            .unwrap()
            .state(),
        BindingState::Confirmed
    );
    assert_eq!(peer.binds.load(Ordering::SeqCst), 1);
    assert!(reopened.authorize_native(&target).await.is_ok());
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn binding_lost_ack_and_unavailable_info_remain_uncertain_until_read_only_reconciliation() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-binding-lost-ack", State::Running);
    let peer = Peer::new(&f, &record).await;
    peer.bind_ack.store(false, Ordering::SeqCst);
    peer.bind_pause.store(true, Ordering::SeqCst);
    let service = f.open(limits()).await;
    let response = service
        .admit(Command::Bind(verified(&record)))
        .await
        .unwrap();
    peer.entered.notified().await;
    peer.info_fail.store(true, Ordering::SeqCst);
    peer.release.notify_one();
    let Answer::Record(uncertain) = response.await.unwrap().answer.unwrap() else {
        panic!("record response")
    };
    assert_eq!(
        uncertain.binding().unwrap().state(),
        BindingState::Uncertain
    );
    assert_eq!(uncertain.state(), State::Uncertain);
    assert_eq!(peer.binds.load(Ordering::SeqCst), 1);
    service.shutdown().await.unwrap();
    peer.info_fail.store(false, Ordering::SeqCst);
    let reopened = f.open(limits()).await;
    let confirmed = reopened.get(record.record_id().into()).await.unwrap();
    assert_eq!(
        confirmed.binding().unwrap().state(),
        BindingState::Confirmed
    );
    assert_eq!(confirmed.state(), State::Running);
    assert_eq!(
        peer.binds.load(Ordering::SeqCst),
        1,
        "recovery only observes, never binds"
    );
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn binding_ack_without_info_match_requires_explicit_same_value_retry() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-binding-retry", State::Running);
    let peer = Peer::new(&f, &record).await;
    peer.bind_apply.store(false, Ordering::SeqCst);
    let service = f.open(limits()).await;
    let uncertain = service.bind(verified(&record)).await.unwrap();
    assert_eq!(uncertain.state(), State::Running);
    assert_eq!(
        uncertain.binding().unwrap().state(),
        BindingState::Uncertain
    );
    assert_eq!(
        service.list(0, 128).await.unwrap()[0]
            .binding()
            .unwrap()
            .state(),
        BindingState::Uncertain
    );
    assert_eq!(peer.binds.load(Ordering::SeqCst), 1);
    peer.bind_apply.store(true, Ordering::SeqCst);
    let confirmed = service.bind(verified(&uncertain)).await.unwrap();
    assert_eq!(
        confirmed.binding().unwrap().state(),
        BindingState::Confirmed
    );
    assert_eq!(peer.binds.load(Ordering::SeqCst), 2);
    let mut wrong = native_scope();
    wrong.session_id = "PRIVATE_DIFFERENT_SESSION".into();
    let error = service
        .bind(VerifiedNativeBinding::from_scope(&wrong, &confirmed, true).unwrap())
        .await
        .err()
        .unwrap();
    assert_eq!(error, Error::BindingConflict);
    assert!(!format!("{error} {error:?}").contains("PRIVATE"));
    assert_eq!(peer.binds.load(Ordering::SeqCst), 2);
    peer.binding.lock().unwrap()["sid"] = json!("different-host-session");
    assert!(matches!(
        service.get(record.record_id().into()).await,
        Err(Error::BindingConflict)
    ));
    let raw: Value =
        serde_json::from_slice(&fs::read(f.ledger.join(store::LEDGER_FILENAME)).unwrap()).unwrap();
    assert_eq!(
        raw["records"][record.record_id()]["binding"]["spec"]["sid"],
        native_scope().session_id
    );
    assert_eq!(
        raw["records"][record.record_id()]["binding"]["state"],
        "uncertain"
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_binding_cannot_authorize_native_after_web_restart_or_rebind() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-native-cancel", State::Running);
    let peer = Peer::new(&f, &record).await;
    peer.kill_exits.store(false, Ordering::SeqCst);
    let service = f.open(limits()).await;
    let confirmed = service.bind(verified(&record)).await.unwrap();
    let stale_confirmation = verified(&confirmed);
    let target = native_target(&f, &record).await;
    assert!(service.authorize_native(&target).await.is_ok());
    let cancelled = service
        .cancel(record.record_id().into(), record.instance_id().into())
        .await
        .unwrap();
    assert!(cancelled.cancel_requested());
    assert!(service.bind(stale_confirmation).await.is_err());
    assert!(service.authorize_native(&target).await.is_err());
    assert_eq!(peer.binds.load(Ordering::SeqCst), 1);
    service.shutdown().await.unwrap();
    // A fresh empty TerminalService registry is not enough to regain authority.
    let reopened = f.open(limits()).await;
    assert!(reopened.authorize_native(&target).await.is_err());
    assert!(
        reopened
            .get(record.record_id().into())
            .await
            .unwrap()
            .cancel_requested()
    );
    assert_eq!(peer.binds.load(Ordering::SeqCst), 1);
    assert_eq!(peer.kills.load(Ordering::SeqCst), 1);
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_authorization_requires_ledger_intent_not_only_a_host_binding() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-unowned-binding", State::Running);
    let peer = Peer::new(&f, &record).await;
    *peer.binding.lock().unwrap() = json!({"version":1,"method":"operator","source":"codex","sid":native_scope().session_id,"uid":native_scope().uid,"launch_id":record.launch_id(),"instance_id":record.instance_id()});
    let target = native_target(&f, &record).await;
    let service = f.open(limits()).await;
    assert!(matches!(
        service.authorize_native(&target).await,
        Err(Error::NotReady)
    ));
    assert!(
        service
            .get(record.record_id().into())
            .await
            .unwrap()
            .binding()
            .is_none()
    );
    assert_eq!(peer.binds.load(Ordering::SeqCst), 0);
    let confirmed = service.bind(verified(&record)).await.unwrap();
    assert_eq!(
        confirmed.binding().unwrap().state(),
        BindingState::Confirmed
    );
    assert_eq!(
        peer.binds.load(Ordering::SeqCst),
        0,
        "already matching host needs only durable operator intent and fresh Info"
    );
    assert!(service.authorize_native(&target).await.is_ok());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn binding_confirmation_rejects_unsupported_scope_capability_and_stale_instance() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-bind-validation", State::Running);
    assert!(matches!(
        VerifiedNativeBinding::from_scope(&native_scope(), &record, false),
        Err(Error::InvalidBinding)
    ));
    let mut child = native_scope();
    child.agent_id = Some("full-child-agent".into());
    assert!(matches!(
        VerifiedNativeBinding::from_scope(&child, &record, true),
        Err(Error::BindingUnsupported)
    ));
    // A Grok scope is bindable, but only for a Grok receipt (this one
    // is Codex) — source disagreement stays an invalid binding.
    let mut grok = native_scope();
    grok.source = "grok".into();
    grok.uid = "grok:0123456789abcdef".into();
    assert!(matches!(
        VerifiedNativeBinding::from_scope(&grok, &record, true),
        Err(Error::InvalidBinding)
    ));
    assert!(
        VerifiedNativeBinding::from_process_evidence(&native_scope(), &record, "x".repeat(513))
            .is_ok()
    );
    let mut wrong = native_scope();
    wrong.source = "claude".into();
    assert!(matches!(
        VerifiedNativeBinding::from_scope(&wrong, &record, true),
        Err(Error::InvalidBinding)
    ));
    let peer = Peer::new(&f, &record).await;
    peer.binding_capable.store(false, Ordering::SeqCst);
    let service = f.open(limits()).await;
    assert!(matches!(
        service.bind(verified(&record)).await,
        Err(Error::BindingUnsupported)
    ));
    assert!(
        service
            .get(record.record_id().into())
            .await
            .unwrap()
            .binding()
            .is_none()
    );
    peer.binding_capable.store(true, Ordering::SeqCst);
    let mut stale = verified(&record);
    stale.instance_id = "f".repeat(32);
    assert!(matches!(
        service.bind(stale).await,
        Err(Error::IdentityConflict)
    ));
    assert_eq!(peer.binds.load(Ordering::SeqCst), 0);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn offline_status_keeps_association_but_revokes_live_authority() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-binding-offline", State::Running);
    let peer = Peer::new(&f, &record).await;
    let service = f.open(limits()).await;
    let confirmed = service.bind(verified(&record)).await.unwrap();
    assert_eq!(
        confirmed.binding().unwrap().state(),
        BindingState::Confirmed
    );
    let target = native_target(&f, &record).await;
    assert!(service.authorize_native(&target).await.is_ok());
    peer.info_fail.store(true, Ordering::SeqCst);
    let offline = service.get(record.record_id().into()).await.unwrap();
    assert_eq!(offline.state(), State::Uncertain);
    assert_eq!(offline.binding().unwrap().state(), BindingState::Confirmed);
    assert!(offline.binding().unwrap().spec() == confirmed.binding().unwrap().spec());
    assert!(matches!(
        service.authorize_native(&target).await,
        Err(Error::NotReady)
    ));
    assert!(matches!(
        service.target(record.record_id().into()).await,
        Err(Error::NotReady)
    ));
    assert_eq!(peer.binds.load(Ordering::SeqCst), 1);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn binding_intent_recovery_only_observes_and_missing_ledger_cannot_authorize() {
    let (_gate, f) = fixture().await;
    let record = f.seed("request-binding-crash", State::Running);
    let spec =
        BindingSpec::new(Source::Codex, native_scope().session_id, native_scope().uid).unwrap();
    let mut store = LifecycleStore::open(&f.ledger).unwrap();
    let _abandoned = store.begin_binding(&record, &spec).unwrap();
    drop(store);
    let peer = Peer::new(&f, &record).await;
    let service = f.open(limits()).await;
    let recovered = service.get(record.record_id().into()).await.unwrap();
    assert_eq!(
        recovered.binding().unwrap().state(),
        BindingState::Uncertain
    );
    assert_eq!(peer.binds.load(Ordering::SeqCst), 0);
    service.bind(verified(&recovered)).await.unwrap();
    let target = native_target(&f, &record).await;
    service.shutdown().await.unwrap();
    let other = tokio::task::spawn_blocking(Fixture::new).await.unwrap();
    let unrelated = LifecycleService::open(
        other.ledger.clone(),
        f.config.clone(),
        Arc::new(TerminalService::new(f.config.host_dir.clone()).unwrap()),
        limits(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(matches!(
        unrelated.authorize_native(&target).await,
        Err(Error::NotReady)
    ));
    assert!(unrelated.list(0, 128).await.unwrap().is_empty());
    assert_eq!(peer.binds.load(Ordering::SeqCst), 1);
    unrelated.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_local_hosts_become_exited_and_last_handle_drop_releases_store_without_kill() {
    let (_gate, f) = fixture().await;
    for index in 0..5 {
        f.seed(&format!("request-list-{index}"), State::Running);
    }
    let prepared = f.seed("request-prepared", State::Prepared);
    let service = f.open(limits()).await;
    let started = Instant::now();
    let records = service.list(0, 128).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(records.len(), 6);
    assert!(
        records
            .iter()
            .filter(|r| r.record_id() != prepared.record_id())
            .all(|r| r.state() == State::Exited)
    );
    assert_eq!(
        service
            .cancel(prepared.record_id().into(), prepared.instance_id().into())
            .await
            .unwrap()
            .failure(),
        Some(Failure::PreparationCancelled)
    );
    let mut done = service.done.clone();
    drop(service);
    tokio::time::timeout(
        Duration::from_secs(1),
        done.wait_for(|value| value.is_some()),
    )
    .await
    .unwrap()
    .unwrap();
    drop(LifecycleStore::open(&f.ledger).unwrap());
}

#[tokio::test]
async fn dead_resume_receipt_does_not_block_a_new_resume_attempt() {
    let (_gate, mut f) = fixture().await;
    f.config.schema = 2;
    f.config.profiles.push(launcher::CliProfile {
        id: "profile-v1".into(),
        source: Source::Codex,
        executable: f.temp.path().join("bin/adapter"),
        args: vec![],
        new_args: vec![],
        resume_args: vec![],
        env: BTreeMap::new(),
        env_remove: vec![],
    });
    let spec = LaunchSpec::resume(
        Source::Codex,
        "profile-v1".into(),
        &f.temp.path().join("work"),
        "native-session-1".into(),
        "codex:native-session-1".into(),
    )
    .unwrap();
    let old = {
        let mut store = LifecycleStore::open(&f.ledger).unwrap();
        let created = store.create("old-resume-request", &spec).unwrap();
        let starting = store.begin_start(created.prepared.unwrap()).unwrap();
        store.mark_running(starting).unwrap()
    };

    let service = f.open(limits()).await;
    let replacement = service
        .create("new-resume-request".into(), spec)
        .await
        .unwrap();
    assert_ne!(replacement.record_id(), old.record_id());
    // The synthetic executable cannot run; reaching Failed proves the old
    // missing instance was reconciled and did not return launch_conflict.
    assert_eq!(replacement.state(), State::Failed);
    assert_eq!(
        service.get(old.record_id().into()).await.unwrap().state(),
        State::Exited
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "explicit isolated free-shell integration; requires test binary paths"]
async fn explicit_free_shell_creation_survives_response_drop_and_shutdown_then_cancels_exact_host()
{
    let (_gate, mut f) = fixture().await;
    f.config.host_binary = PathBuf::from(
        std::env::var_os("SESSIONDOCK_TEST_PTYHOST_BINARY").expect("explicit built ptyhost binary"),
    );
    f.config.adapters[0].executable = PathBuf::from(
        std::env::var_os("SESSIONDOCK_TEST_FREE_SHELL_BINARY").expect("explicit free shell binary"),
    );
    f.config.adapters[0].args = vec!["-c".into(), "read synthetic_line".into()];
    let service = f.open(ServiceLimits::default()).await;
    let response = service
        .admit(Command::Create("request-free-shell".into(), f.spec()))
        .await
        .unwrap();
    drop(response);
    let records = service.list(0, 128).await.unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.state(), State::Running);
    let duplicate = service
        .create("request-free-shell".into(), f.spec())
        .await
        .unwrap();
    assert_eq!(duplicate.record_id(), record.record_id());
    service.shutdown().await.unwrap();
    let reopened = f.open(ServiceLimits::default()).await;
    assert_eq!(
        reopened
            .get(record.record_id().into())
            .await
            .unwrap()
            .state(),
        State::Running
    );
    let cancelled = reopened
        .cancel(record.record_id().into(), record.instance_id().into())
        .await
        .unwrap();
    assert_eq!(cancelled.state(), State::Exited);
    reopened.shutdown().await.unwrap();
}
