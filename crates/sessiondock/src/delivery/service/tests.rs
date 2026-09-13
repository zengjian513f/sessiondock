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
/// Open a fixture service; process-wide open concurrency waits internally.
async fn open_with(
    dir: &tempfile::TempDir,
    limits: Limits,
    shutdown: CancellationToken,
) -> DeliveryService {
    DeliveryService::open(dir.path().into(), limits, shutdown)
        .await
        .unwrap()
}
/// `open_inner` with a read-pause hook.
async fn open_paused(
    dir: &tempfile::TempDir,
    capacity: usize,
    pause: Arc<Pause>,
) -> DeliveryService {
    DeliveryService::open_inner(
        dir.path().into(),
        Limits { capacity },
        CancellationToken::new(),
        Hooks {
            read_pause: Some(pause.clone()),
        },
    )
    .await
    .unwrap()
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
    assert!(
        service
            .codex_outbox("codex:synthetic".into(), Some("child".into()))
            .await
            .is_ok()
    );
    assert!(service.receipts(Provider::Claude, 0, 129).await.is_ok());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn slow_read_and_dropped_http_response_retain_the_worker() {
    let (_gate, dir) = fixture().await;
    let pause = Pause::new();
    let _release = ReleaseOnDrop(pause.clone());
    let service = open_paused(&dir, 2, pause.clone()).await;
    let response = service.admit(codex_query()).await.unwrap();
    pause.started.notified().await;
    let queued = service
        .admit(Query::Logs {
            after_sequence: 0,
            limit: 1,
        })
        .await
        .unwrap();
    drop(response);
    // A single-thread Tokio test still advances while the worker is parked.
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::time::sleep(Duration::from_millis(10)),
    )
    .await
    .unwrap();
    pause.release();
    queued.await.unwrap().unwrap();
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
    let active = service.admit(codex_query()).await.unwrap();
    pause.started.notified().await;
    let queued = service.admit(codex_query()).await.unwrap();
    let clone = service.clone();
    let mut shutdown = tokio::spawn(async move { clone.shutdown().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut shutdown)
            .await
            .is_err()
    );
    assert!(matches!(
        service.admit(codex_query()).await,
        Err(Error::Closed)
    ));
    pause.release();
    active.await.unwrap().unwrap();
    assert!(matches!(queued.await.unwrap(), Err(Error::Closed)));
    shutdown.await.unwrap().unwrap();
    let reopened = open(&dir).await;
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn last_handle_drop_stops_worker_without_channel_cycle() {
    let (_gate, dir) = fixture().await;
    let pause = Pause::new();
    let _release = ReleaseOnDrop(pause.clone());
    let service = open_paused(&dir, Limits::default().capacity, pause.clone()).await;
    let response = service.admit(codex_query()).await.unwrap();
    pause.started.notified().await;
    let mut done = service.done.clone();
    drop(response);
    drop(service);
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
    assert!(matches!(
        service.admit(codex_query()).await,
        Err(Error::Closed)
    ));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn external_syntactic_change_reloads_without_freezing_the_service() {
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
    let reloaded = value(
        service
            .codex_outbox("codex:synthetic".into(), None)
            .await
            .unwrap(),
    );
    assert_eq!(reloaded, old);
    assert!(service.claude_outbox(scope()).await.is_ok());
    let logs = value(service.logs(0, 128).await.unwrap());
    assert!(logs.as_array().unwrap().is_empty());
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
async fn corrupt_and_missing_ledgers_initialize_as_empty() {
    let (_gate, dir) = fixture().await;
    let path = dir.path().join(store::LEDGER_FILENAME);
    fs::write(&path, b"invalid").unwrap();
    let reset = DeliveryService::open(
        dir.path().into(),
        Limits::default(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(
        value(
            reset
                .codex_outbox("codex:synthetic".into(), None)
                .await
                .unwrap()
        )["outbox"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    reset.shutdown().await.unwrap();
    assert_ne!(fs::read(&path).unwrap(), b"invalid");
    let empty = tempfile::tempdir().unwrap();
    fs::set_permissions(empty.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let initialized = DeliveryService::open(
        empty.path().into(),
        Limits::default(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    initialized.shutdown().await.unwrap();
    assert!(Limits { capacity: 17 }.validate().is_ok());
}

#[test]
fn default_outbox_encoding_accepts_more_than_the_old_32_mib_ceiling() {
    let text = "x".repeat(32 * 1024 * 1024 + 1);
    let encoded = encode(&text).unwrap();
    assert_eq!(encoded.as_bytes().len(), text.len() + 2);
}

#[tokio::test]
async fn completed_response_does_not_block_another_read() {
    let (_gate, dir) = fixture().await;
    let service = open_with(&dir, Limits { capacity: 1 }, CancellationToken::new()).await;
    let response = service
        .codex_outbox("codex:synthetic".into(), None)
        .await
        .unwrap();
    assert!(service.logs(0, 1).await.is_ok());
    let bytes = response.into_bytes();
    assert!(!bytes.is_empty());
    assert!(service.logs(0, 1).await.is_ok());
    // Shutdown waits for workers, not a caller still holding response bytes.
    service.shutdown().await.unwrap();
    let reopened = open(&dir).await;
    reopened.shutdown().await.unwrap();
}
