use super::*;
use crate::delivery::{codex, store};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{Condvar, Mutex},
    time::Duration,
};

static TEST_GATE: Semaphore = Semaphore::const_new(1);

pub(super) struct Pause {
    started: tokio::sync::Notify,
    released: Mutex<bool>,
    wake: Condvar,
}
impl Pause {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: tokio::sync::Notify::new(),
            released: Mutex::new(false),
            wake: Condvar::new(),
        })
    }
    pub(super) fn block(&self) {
        self.started.notify_one();
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.wake.wait(released).unwrap();
        }
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}
// Never leave a blocking test worker parked after an assertion panic.
struct ReleaseOnDrop(Arc<Pause>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

fn scope() -> claude::Scope {
    claude::Scope {
        uid: "claude:synthetic".into(),
        session_id: "native-session".into(),
        agent_id: None,
    }
}
async fn fixture() -> (tokio::sync::SemaphorePermit<'static>, tempfile::TempDir) {
    let gate = TEST_GATE.acquire().await.unwrap();
    let dir = tokio::task::spawn_blocking(|| {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut engine = DeliveryEngine::initialize(dir.path()).unwrap();
        engine
            .apply_codex(
                codex::Command::Submit {
                    request: codex::Request {
                        request_id: "codex-request".into(),
                        payload: codex::Payload {
                            uid: "codex:synthetic".into(),
                            target: codex::Target {
                                host_instance: "synthetic-instance".into(),
                                session_id: "synthetic-terminal".into(),
                                ownership_epoch: "synthetic-owner".into(),
                            },
                            text: "private fixture prompt".into(),
                            media: vec![],
                        },
                    },
                    now_ms: 1234,
                },
                None,
            )
            .unwrap()
            .claim()
            .unwrap();
        for id in ["claude-request", "claude-tombstone"] {
            engine
                .apply_claude(claude::Command::Enqueue {
                    request: claude::Request {
                        id: id.into(),
                        payload: claude::Payload {
                            scope: scope(),
                            target: claude::Target {
                                host_instance: "synthetic-instance".into(),
                                terminal_id: "synthetic-claude".into(),
                                ownership_epoch: "synthetic-owner".into(),
                            },
                            text: "private fixture prompt".into(),
                            attachments: vec![],
                        },
                    },
                    now_ms: 2345,
                })
                .unwrap()
                .claim()
                .unwrap();
        }
        engine
            .apply_claude(claude::Command::Cancel {
                id: "claude-tombstone".into(),
                scope: scope(),
            })
            .unwrap()
            .claim()
            .unwrap();
        drop(engine);
        dir
    })
    .await
    .unwrap();
    (gate, dir)
}
async fn open(dir: &tempfile::TempDir) -> DeliveryService {
    open_with(dir, Limits::default(), CancellationToken::new()).await
}
/// `OPEN_WORKERS` is process-global: with hundreds of lib tests in flight all
/// eight permits can be taken for a moment. `Busy` is admission, not a verdict
/// on this fixture, so wait it out like the bad-open test does.
async fn open_with(
    dir: &tempfile::TempDir,
    limits: Limits,
    shutdown: CancellationToken,
) -> DeliveryService {
    let mut attempt = 0;
    loop {
        match DeliveryService::open(dir.path().into(), limits, shutdown.clone()).await {
            Err(Error::Busy) if attempt < 200 => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            result => return result.unwrap(),
        }
    }
}
/// `open_inner` with a read-pause hook, waiting out `Busy` like `open_with`.
async fn open_paused(
    dir: &tempfile::TempDir,
    capacity: usize,
    pause: Arc<Pause>,
) -> DeliveryService {
    let mut attempt = 0;
    loop {
        match DeliveryService::open_inner(
            dir.path().into(),
            Limits {
                capacity,
                ..Limits::default()
            },
            CancellationToken::new(),
            Hooks {
                read_pause: Some(pause.clone()),
            },
        )
        .await
        {
            Err(Error::Busy) if attempt < 200 => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            result => return result.unwrap(),
        }
    }
}
fn value(json: EncodedJson) -> serde_json::Value {
    serde_json::from_slice(json.as_bytes()).unwrap()
}
fn codex_query() -> Query {
    Query::CodexOutbox {
        uid: "codex:synthetic".into(),
        agent_id: None,
    }
}
async fn assert_locked(path: PathBuf) {
    assert!(
        tokio::task::spawn_blocking(move || matches!(
            DeliveryEngine::open(&path),
            Err(engine::Error::Store(store::Error::WriterLocked))
        ))
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn reads_existing_wire_retains_tombstones_and_never_writes_after_open() {
    let (_gate, dir) = fixture().await;
    let service = open(&dir).await;
    let ledger = dir.path().join(store::LEDGER_FILENAME);
    let baseline = fs::read(&ledger).unwrap();
    let codex = value(
        service
            .codex_outbox("codex:synthetic".into(), None)
            .await
            .unwrap(),
    );
    assert_eq!(codex["outbox"][0]["state"], "failed");
    assert_eq!(codex["outbox"][0]["created"], 1234);
    assert_eq!(codex["outbox"][0]["server"], true);
    assert!(codex["outbox"][0].get("afterTs").is_none());
    let claude = value(service.claude_outbox(scope()).await.unwrap());
    assert_eq!(claude["outbox"].as_array().unwrap().len(), 1);
    assert_eq!(claude["outbox"][0]["state"], "persisted");
    let receipts = value(service.receipts(Provider::Claude, 0, 128).await.unwrap());
    assert_eq!(receipts.as_array().unwrap().len(), 2);
    assert!(!receipts.to_string().contains("private fixture prompt"));
    assert!(
        !value(service.logs(0, 128).await.unwrap())
            .to_string()
            .contains("private fixture prompt")
    );
    service.shutdown().await.unwrap();
    service.shutdown().await.unwrap();
    assert_eq!(fs::read(ledger).unwrap(), baseline);
}

#[tokio::test]
async fn exact_scope_passes_through_and_child_codex_is_rejected() {
    let (_gate, dir) = fixture().await;
    let service = open(&dir).await;
    let mut child = scope();
    child.agent_id = Some("synthetic-child".into());
    assert_eq!(
        value(service.claude_outbox(child).await.unwrap())["outbox"],
        serde_json::json!([])
    );
    let mut other = scope();
    other.session_id = "different-native".into();
    assert_eq!(
        value(service.claude_outbox(other).await.unwrap())["outbox"],
        serde_json::json!([])
    );
    assert!(matches!(
        service
            .codex_outbox("codex:synthetic".into(), Some("child".into()))
            .await,
        Err(Error::Engine(engine::Error::UnsupportedAgent))
    ));
    assert!(matches!(
        service.receipts(Provider::Claude, 0, 129).await,
        Err(Error::Engine(engine::Error::InvalidLimit))
    ));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn slow_read_and_dropped_http_response_retain_capacity_and_lock() {
    let (_gate, dir) = fixture().await;
    let pause = Pause::new();
    let _release = ReleaseOnDrop(pause.clone());
    let service = open_paused(&dir, 2, pause.clone()).await;
    let response = service.admit(codex_query()).unwrap();
    pause.started.notified().await;
    let queued = service
        .admit(Query::Logs {
            after_sequence: 0,
            limit: 1,
        })
        .unwrap();
    drop(response);
    assert!(matches!(service.admit(codex_query()), Err(Error::Busy)));
    assert_eq!(service.admission.available_permits(), 0);
    assert_locked(dir.path().into()).await;
    // A single-thread Tokio test still advances while the worker is parked.
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::time::sleep(Duration::from_millis(10)),
    )
    .await
    .unwrap();
    assert_eq!(service.admission.available_permits(), 0);
    pause.release();
    queued.await.unwrap().unwrap();
    assert_eq!(service.admission.available_permits(), 2);
    service
        .codex_outbox("codex:synthetic".into(), None)
        .await
        .unwrap();
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_waits_for_active_read_and_rejects_not_started_reads() {
    let (_gate, dir) = fixture().await;
    let pause = Pause::new();
    let _release = ReleaseOnDrop(pause.clone());
    let service = Arc::new(open_paused(&dir, 2, pause.clone()).await);
    let active = service.admit(codex_query()).unwrap();
    pause.started.notified().await;
    let queued = service.admit(codex_query()).unwrap();
    let clone = service.clone();
    let mut shutdown = tokio::spawn(async move { clone.shutdown().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut shutdown)
            .await
            .is_err()
    );
    assert!(matches!(service.admit(codex_query()), Err(Error::Closed)));
    assert_eq!(service.admission.available_permits(), 0);
    assert_locked(dir.path().into()).await;
    pause.release();
    active.await.unwrap().unwrap();
    assert!(matches!(queued.await.unwrap(), Err(Error::Closed)));
    shutdown.await.unwrap().unwrap();
    let reopened = open(&dir).await;
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn last_handle_drop_releases_lock_after_worker_without_channel_cycle() {
    let (_gate, dir) = fixture().await;
    let pause = Pause::new();
    let _release = ReleaseOnDrop(pause.clone());
    let service = open_paused(&dir, Limits::default().capacity, pause.clone()).await;
    let response = service.admit(codex_query()).unwrap();
    pause.started.notified().await;
    let mut done = service.done.clone();
    drop(response);
    drop(service);
    assert_locked(dir.path().into()).await;
    pause.release();
    tokio::time::timeout(
        Duration::from_secs(1),
        done.wait_for(|state| state.is_some()),
    )
    .await
    .unwrap()
    .unwrap();
    let reopened = open(&dir).await;
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn external_shutdown_does_not_cancel_parent_and_closes_admission() {
    let (_gate, dir) = fixture().await;
    let parent = CancellationToken::new();
    let service = open_with(&dir, Limits::default(), parent.clone()).await;
    service.shutdown().await.unwrap();
    assert!(!parent.is_cancelled());
    let service = open_with(&dir, Limits::default(), parent.clone()).await;
    parent.cancel();
    assert!(matches!(service.admit(codex_query()), Err(Error::Closed)));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn external_change_freezes_reads_but_logs_remain_available_until_recovery() {
    let (_gate, dir) = fixture().await;
    let service = open(&dir).await;
    let old = value(
        service
            .codex_outbox("codex:synthetic".into(), None)
            .await
            .unwrap(),
    );
    let path = dir.path().join(store::LEDGER_FILENAME);
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b' ');
    fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        service.codex_outbox("codex:synthetic".into(), None).await,
        Err(Error::Engine(engine::Error::Store(store::Error::Changed)))
    ));
    assert!(matches!(
        service.claude_outbox(scope()).await,
        Err(Error::Engine(engine::Error::Frozen))
    ));
    let logs = value(service.logs(0, 128).await.unwrap());
    assert_eq!(logs.as_array().unwrap().last().unwrap()["event"], "frozen");
    assert_eq!(fs::read(&path).unwrap(), bytes);
    service.shutdown().await.unwrap();
    let reopened = open(&dir).await;
    let next = value(
        reopened
            .codex_outbox("codex:synthetic".into(), None)
            .await
            .unwrap(),
    );
    assert_ne!(
        next["outbox_version"]["epoch"],
        old["outbox_version"]["epoch"]
    );
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn reopening_recovers_both_epochs_and_keeps_original_requests() {
    let (_gate, dir) = fixture().await;
    let first = open(&dir).await;
    let old_c = value(
        first
            .codex_outbox("codex:synthetic".into(), None)
            .await
            .unwrap(),
    );
    let old_h = value(first.claude_outbox(scope()).await.unwrap());
    first.shutdown().await.unwrap();
    let next = open(&dir).await;
    let new_c = value(
        next.codex_outbox("codex:synthetic".into(), None)
            .await
            .unwrap(),
    );
    let new_h = value(next.claude_outbox(scope()).await.unwrap());
    assert_ne!(
        old_c["outbox_version"]["epoch"],
        new_c["outbox_version"]["epoch"]
    );
    assert_ne!(
        old_h["outbox_version"]["epoch"],
        new_h["outbox_version"]["epoch"]
    );
    assert_eq!(old_c["outbox"], new_c["outbox"]);
    assert_eq!(old_h["outbox"], new_h["outbox"]);
    next.shutdown().await.unwrap();
}

#[tokio::test]
async fn bad_open_releases_lock_and_never_initializes_missing_data() {
    let (_gate, dir) = fixture().await;
    let path = dir.path().join(store::LEDGER_FILENAME);
    let original = fs::read(&path).unwrap();
    fs::write(&path, b"invalid").unwrap();
    // `OPEN_WORKERS` is process-global: other test binaries' opens may hold
    // all eight permits for a moment, which is `Busy`, not a verdict on the
    // ledger. Only the engine error proves the invalid file was rejected.
    let mut attempt = 0;
    loop {
        match DeliveryService::open(
            dir.path().into(),
            Limits::default(),
            CancellationToken::new(),
        )
        .await
        {
            Err(Error::Engine(_)) => break,
            Err(Error::Busy) if attempt < 50 => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            other => panic!("expected an engine error, got {:?}", other.err()),
        }
    }
    assert_eq!(fs::read(&path).unwrap(), b"invalid");
    fs::write(&path, original).unwrap();
    let service = open(&dir).await;
    service.shutdown().await.unwrap();
    let empty = tempfile::tempdir().unwrap();
    fs::set_permissions(empty.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        DeliveryService::open(
            empty.path().into(),
            Limits::default(),
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read_dir(empty.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn response_limit_is_explicit_and_does_not_truncate_or_freeze_ledger() {
    let (_gate, dir) = fixture().await;
    let service = open_with(
        &dir,
        Limits {
            max_json_bytes: 128,
            ..Limits::default()
        },
        CancellationToken::new(),
    )
    .await;
    let path = dir.path().join(store::LEDGER_FILENAME);
    let bytes = fs::read(&path).unwrap();
    assert!(matches!(
        service.codex_outbox("codex:synthetic".into(), None).await,
        Err(Error::ResponseLimit)
    ));
    assert_eq!(
        value(service.logs(0, 1).await.unwrap()),
        serde_json::json!([])
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
    service.shutdown().await.unwrap();
}

#[test]
fn bounded_writer_checks_encoded_bytes_before_appending() {
    assert!(matches!(encode(&"\n\n\n", 7), Err(Error::ResponseLimit)));
    assert_eq!(encode(&"\n\n\n", 8).unwrap().as_bytes(), b"\"\\n\\n\\n\"");
    let mut writer = BoundedWriter {
        bytes: vec![b'x'; 4],
        max_bytes: 5,
        exceeded: false,
    };
    assert!(writer.write_all(b"too-long").is_err());
    assert_eq!(writer.bytes.len(), 4);
    assert!(writer.exceeded);
    assert_eq!(
        Limits {
            capacity: 17,
            ..Limits::default()
        }
        .validate()
        .unwrap_err(),
        Error::InvalidLimits
    );
    assert_eq!(
        Limits {
            max_json_bytes: MAX_JSON_BYTES + 1,
            ..Limits::default()
        }
        .validate()
        .unwrap_err(),
        Error::InvalidLimits
    );
}

#[tokio::test]
async fn completed_response_and_body_guard_keep_aggregate_response_capacity() {
    let (_gate, dir) = fixture().await;
    let service = open_with(
        &dir,
        Limits {
            capacity: 1,
            ..Limits::default()
        },
        CancellationToken::new(),
    )
    .await;
    let response = service
        .codex_outbox("codex:synthetic".into(), None)
        .await
        .unwrap();
    assert_eq!(service.admission.available_permits(), 0);
    assert!(matches!(service.logs(0, 1).await, Err(Error::Busy)));
    let (bytes, guard) = response.into_parts();
    assert!(!bytes.is_empty());
    assert!(matches!(service.logs(0, 1).await, Err(Error::Busy)));
    // Shutdown waits for workers/lock, not a caller still holding response bytes.
    service.shutdown().await.unwrap();
    let reopened = open(&dir).await;
    reopened.shutdown().await.unwrap();
    assert_eq!(service.admission.available_permits(), 0);
    drop(guard);
    assert_eq!(service.admission.available_permits(), 1);
}
