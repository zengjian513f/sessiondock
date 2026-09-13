use super::*;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::{JoinHandle, JoinSet},
};

const INSTANCE: &str = "synthetic-instance-original";
const UID: &str = "codex:0123456789abcdef";
const SID: &str = "complete-synthetic-session";
const NO_ACK: u8 = 1;
const LEGACY: u8 = 2;

struct Peer {
    directory: TempDir,
    record: Arc<AsyncMutex<Value>>,
    mode: Arc<AtomicU8>,
    infos: Arc<AtomicUsize>,
    attaches: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl Peer {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let value = json!({"name":"terminal","host_pid":10,"pid":11,"created":12,"cols":80,"rows":24,
            "port":listener.local_addr().unwrap().port(),"token":"TEST_HOST_SECRET","meta":{
                "source":"codex","sid":SID,"uid":UID,"instance_id":INSTANCE}});
        tokio::fs::write(
            directory.path().join("terminal.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .await
        .unwrap();
        let record = Arc::new(AsyncMutex::new(value));
        let mode = Arc::new(AtomicU8::new(0));
        let infos = Arc::new(AtomicUsize::new(0));
        let attaches = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let task = {
            let (record, mode, infos, attaches, writes, cancel) = (
                record.clone(),
                mode.clone(),
                infos.clone(),
                attaches.clone(),
                writes.clone(),
                cancel.clone(),
            );
            tokio::spawn(async move {
                let mut jobs = JoinSet::new();
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        result = listener.accept() => {
                            let (mut stream, _) = result.unwrap();
                            let (record, mode, infos, attaches, writes) = (record.clone(), mode.clone(), infos.clone(), attaches.clone(), writes.clone());
                            jobs.spawn(async move {
                                let mut bytes = Vec::new();
                                loop { let Ok(byte) = stream.read_u8().await else { return }; if byte == b'\n' { break } bytes.push(byte); assert!(bytes.len() < 8192); }
                                let request: Value = serde_json::from_slice(&bytes).unwrap();
                                assert_eq!(request["token"],"TEST_HOST_SECRET");
                                let record = record.lock().await.clone();
                                let mode = mode.load(Ordering::SeqCst);
                                if request["op"] == "info" {
                                    infos.fetch_add(1,Ordering::SeqCst);
                                    let caps = if mode == LEGACY { json!({}) } else { json!({"instance_guard":1}) };
                                    let bytes = format!("{}\n",json!({"ok":true,"info":record,"exited":false,"capabilities":caps}));
                                    let _ = stream.write_all(bytes.as_bytes()).await;
                                } else {
                                    assert_eq!(request["op"],"guarded_v1","bound bridge must never downgrade to attach");
                                    if mode == LEGACY || request["expected_instance_id"] != record["meta"]["instance_id"] {
                                        let _ = stream.write_all(b"{\"ok\":false,\"error\":\"guard rejected\"}\n").await; return;
                                    }
                                    assert_eq!(request["request"]["op"],"attach");
                                    attaches.fetch_add(1,Ordering::SeqCst);
                                    let mut response = json!({"ok":true,"cols":80,"rows":24});
                                    if mode != NO_ACK { response["instance_guard"] = json!({"version":1,"instance_id":record["meta"]["instance_id"]}); }
                                    let bytes = format!("{response}\n"); if stream.write_all(bytes.as_bytes()).await.is_err() { return }
                                    while let Ok(kind) = stream.read_u8().await {
                                        let length = stream.read_u32().await.unwrap() as usize; assert!(length < 8192);
                                        let mut data = vec![0;length]; stream.read_exact(&mut data).await.unwrap();
                                        assert!(kind == 1 || kind == 2);
                                        writes.fetch_add(1,Ordering::SeqCst);
                                    }
                                }
                            });
                        },
                        _ = jobs.join_next(), if !jobs.is_empty() => {},
                    }
                }
                jobs.abort_all();
                while jobs.join_next().await.is_some() {}
            })
        };
        Self {
            directory,
            record,
            mode,
            infos,
            attaches,
            writes,
            cancel,
            task,
        }
    }

    fn service(&self) -> Arc<TerminalService> {
        Arc::new(TerminalService::new(self.directory.path().to_owned()).unwrap())
    }

    async fn target(&self) -> BoundTarget {
        let observed = HostClient::new(self.directory.path(), Limits::default())
            .unwrap()
            .probe("terminal")
            .await
            .unwrap();
        BoundTarget::from_observation(&observed, ptyhost_client::Source::Codex, SID, UID).unwrap()
    }

    async fn replace(&self, disk: bool) {
        let mut record = self.record.lock().await;
        record["meta"]["instance_id"] = json!("synthetic-instance-replacement");
        record["pid"] = json!(22);
        if disk {
            tokio::fs::write(
                self.directory.path().join("terminal.json"),
                serde_json::to_vec(&*record).unwrap(),
            )
            .await
            .unwrap();
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}
fn ip() -> IpAddr {
    "127.0.0.1".parse().unwrap()
}
fn size() -> TerminalSize {
    TerminalSize::new(80, 24).unwrap()
}
fn token(claim: ClaimResponse) -> String {
    claim.into_api_json()["token"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn bound_claim_reprobes_and_raw_prepare_wrong_uid_and_force_downgrade_are_rejected() {
    let peer = Peer::new().await;
    let service = peer.service();
    let target = peer.target().await;
    let lease = token(
        service
            .claim_bound(&target, "page", ip(), false)
            .await
            .unwrap(),
    );
    assert_eq!(peer.infos.load(Ordering::SeqCst), 2);
    assert_eq!(
        service
            .prepare("terminal", "page", &lease)
            .err()
            .unwrap()
            .status,
        409
    );
    assert_eq!(
        service
            .prepare_bound("terminal", "page", &lease, "codex:wrong", INSTANCE)
            .err()
            .unwrap()
            .status,
        409
    );
    assert_eq!(
        service
            .claim("terminal", "page", ip(), true)
            .await
            .err()
            .unwrap()
            .status,
        409
    );
    let prepared = service
        .prepare_bound("terminal", "page", &lease, UID, INSTANCE)
        .unwrap();
    let attached = prepared
        .attach_current(size())
        .await
        .unwrap_or_else(|_| panic!("guarded attach failed"));
    let (_reader, mut writer) = attached.into_split();
    gated_write(
        &prepared.guard,
        &mut writer,
        HostInput::Data(Bytes::from_static(b"ok")),
    )
    .await
    .unwrap_or_else(|_| panic!("current lease write failed"));
    timeout(Duration::from_secs(1), async {
        while peer.writes.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(prepared);
    assert!(service.registry.owner("terminal").unwrap().is_none());
}

#[tokio::test]
async fn changed_instance_between_catalog_and_claim_cannot_publish_lease() {
    let peer = Peer::new().await;
    let service = peer.service();
    let target = peer.target().await;
    peer.replace(true).await;
    assert_eq!(
        service
            .claim_bound(&target, "page", ip(), false)
            .await
            .err()
            .unwrap()
            .status,
        409
    );
    assert!(service.registry.owner("terminal").unwrap().is_none());
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn replacement_between_claim_and_attach_is_rejected_before_any_new_instance_write() {
    for disk in [false, true] {
        let peer = Peer::new().await;
        let service = peer.service();
        let target = peer.target().await;
        let lease = token(
            service
                .claim_bound(&target, "page", ip(), false)
                .await
                .unwrap(),
        );
        let prepared = service
            .prepare_bound("terminal", "page", &lease, UID, INSTANCE)
            .unwrap();
        peer.replace(disk).await;
        assert_eq!(
            prepared.attach_current(size()).await.err().unwrap().code,
            1011
        );
        assert_eq!(peer.attaches.load(Ordering::SeqCst), 0);
        assert_eq!(peer.writes.load(Ordering::SeqCst), 0);
        drop(prepared);
        assert!(service.registry.owner("terminal").unwrap().is_none());
    }
}

#[tokio::test]
async fn legacy_or_missing_ack_after_bound_claim_never_produces_a_writer() {
    for mode in [NO_ACK, LEGACY] {
        let peer = Peer::new().await;
        let service = peer.service();
        let target = peer.target().await;
        let lease = token(
            service
                .claim_bound(&target, "page", ip(), false)
                .await
                .unwrap(),
        );
        let prepared = service
            .prepare_bound("terminal", "page", &lease, UID, INSTANCE)
            .unwrap();
        peer.mode.store(mode, Ordering::SeqCst);
        assert_eq!(
            prepared.attach_current(size()).await.err().unwrap().code,
            1011
        );
        assert_eq!(peer.writes.load(Ordering::SeqCst), 0);
        drop(prepared);
        assert!(service.registry.owner("terminal").unwrap().is_none());
    }
}

#[tokio::test]
async fn force_bound_claim_waits_before_fresh_probe_and_old_cleanup_does_not_remove_new_target() {
    let peer = Peer::new().await;
    let service = peer.service();
    let target = peer.target().await;
    let lease = token(
        service
            .claim_bound(&target, "first", ip(), false)
            .await
            .unwrap(),
    );
    let old = service
        .prepare_bound("terminal", "first", &lease, UID, INSTANCE)
        .unwrap();
    let attachment = old
        .attach_current(size())
        .await
        .unwrap_or_else(|_| panic!("old attach failed"));
    let (_reader, mut writer) = attachment.into_split();
    let held = old.guard.gate.lock().await;
    let infos = peer.infos.load(Ordering::SeqCst);
    let mut force = {
        let service = service.clone();
        tokio::spawn(async move { service.claim_bound(&target, "second", ip(), true).await })
    };
    assert!(
        timeout(Duration::from_millis(30), &mut force)
            .await
            .is_err()
    );
    assert_eq!(peer.infos.load(Ordering::SeqCst), infos);
    assert!(service.registry.is_current(&old.guard.bound).unwrap());
    drop(held);
    let lease = token(force.await.unwrap().unwrap());
    let error = gated_write(
        &old.guard,
        &mut writer,
        HostInput::Data(Bytes::from_static(b"stale write")),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error.code, 4001);
    assert_eq!(peer.writes.load(Ordering::SeqCst), 0);
    assert_eq!(old.attach_current(size()).await.err().unwrap().code, 4001);
    drop(old);
    let new = service
        .prepare_bound("terminal", "second", &lease, UID, INSTANCE)
        .unwrap();
    assert_eq!(new.guard.bound.target().unwrap().instance_id(), INSTANCE);
    assert!(service.registry.is_current(&new.guard.bound).unwrap());
}

#[tokio::test]
async fn fresh_guard_capability_failure_during_force_preserves_existing_lease() {
    let peer = Peer::new().await;
    let service = peer.service();
    let target = peer.target().await;
    let lease = token(
        service
            .claim_bound(&target, "first", ip(), false)
            .await
            .unwrap(),
    );
    let old = service
        .prepare_bound("terminal", "first", &lease, UID, INSTANCE)
        .unwrap();
    peer.mode.store(LEGACY, Ordering::SeqCst);
    let error = service
        .claim_bound(&target, "second", ip(), true)
        .await
        .err()
        .unwrap();
    assert_eq!(error.status, 501);
    assert!(service.registry.is_current(&old.guard.bound).unwrap());
    assert!(old.guard.bound.revocations().borrow().is_none());
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 0);
    assert_eq!(peer.writes.load(Ordering::SeqCst), 0);
}
