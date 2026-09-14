//! No synthetic NativeBinding constructor: every target below comes from a
//! real HostClient::probe exchange with a bounded, private loopback fake peer.
use super::*;
use ptyhost_client::Source;
use serde_json::{Value, json};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
};
use tokio_tungstenite::tungstenite::Message as ClientMessage;

const NAME: &str = "native-terminal";
const INSTANCE: &str = "synthetic-instance-original";
const LAUNCH: &str = "synthetic-launch-original";
const SID: &str = "complete-native-id";
const UID: &str = "codex:complete-native-uid";

struct Peer {
    directory: TempDir,
    record: Arc<AsyncMutex<Value>>,
    bound: Arc<AtomicBool>,
    attaches: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
    forbidden: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}
fn frame(bytes: &[u8]) -> Vec<u8> {
    let mut output = vec![1];
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    output.extend_from_slice(bytes);
    output
}
impl Peer {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let value = json!({"name":NAME,"host_pid":10,"pid":11,"created":12,"cols":80,"rows":24,
            "port":listener.local_addr().unwrap().port(),"token":"SYNTHETIC_SECRET",
            "meta":{"source":"codex","instance_id":INSTANCE,"launch_id":LAUNCH}});
        tokio::fs::write(
            directory.path().join(format!("{NAME}.json")),
            value.to_string(),
        )
        .await
        .unwrap();
        let record = Arc::new(AsyncMutex::new(value));
        let bound = Arc::new(AtomicBool::new(false));
        let attaches = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(AtomicUsize::new(0));
        let forbidden = Arc::new(AtomicUsize::new(0));
        let task = {
            let (record, bound, attaches, writes, forbidden) = (
                record.clone(),
                bound.clone(),
                attaches.clone(),
                writes.clone(),
                forbidden.clone(),
            );
            tokio::spawn(async move {
                let mut jobs = JoinSet::new();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let (mut stream, _) = accepted.unwrap();
                            let (record, bound, attaches, writes, forbidden) =
                                (record.clone(), bound.clone(), attaches.clone(), writes.clone(), forbidden.clone());
                            jobs.spawn(async move {
                                let mut bytes = Vec::new();
                                loop {
                                    let Ok(byte) = stream.read_u8().await else { return };
                                    if byte == b'\n' { break; }
                                    bytes.push(byte); assert!(bytes.len() < 8192);
                                }
                                let request: Value = serde_json::from_slice(&bytes).unwrap();
                                assert_eq!(request["token"], "SYNTHETIC_SECRET");
                                let record = record.lock().await.clone();
                                let bound = bound.load(Ordering::SeqCst);
                                let binding = if bound {
                                    json!({"version":1,"instance_id":record["meta"]["instance_id"],
                                        "source":"codex","launch_id":record["meta"]["launch_id"],
                                        "sid":SID,"uid":UID,"method":"operator"})
                                } else { Value::Null };
                                if request["op"] == "info" {
                                    let reply = json!({"ok":true,"info":record,"exited":false,
                                        "capabilities":{"instance_guard":1,"launch_guard":1,"launch_bind":1},
                                        "native_binding":binding});
                                    let _ = stream.write_all(format!("{reply}\n").as_bytes()).await;
                                    return;
                                }
                                let launch = request["op"] == "launch_guard_v1";
                                let native = request["op"] == "guarded_v1";
                                if !(launch || native) || request["request"]["op"] != "attach" {
                                    forbidden.fetch_add(1, Ordering::SeqCst); return;
                                }
                                if request["expected_instance_id"] != record["meta"]["instance_id"]
                                    || request["expected_source"] != "codex"
                                    || (launch && request["expected_launch_id"] != record["meta"]["launch_id"])
                                    || (native && (!bound || request["expected_sid"] != SID || request["expected_uid"] != UID)) {
                                    let _ = stream.write_all(b"{\"ok\":false}\n").await; return;
                                }
                                attaches.fetch_add(1, Ordering::SeqCst);
                                let mut reply = json!({"ok":true,"cols":80,"rows":24});
                                if launch {
                                    reply["launch_guard"] = json!({"version":1,"source":"codex",
                                        "instance_id":record["meta"]["instance_id"],"launch_id":record["meta"]["launch_id"]});
                                } else {
                                    reply["instance_guard"] = json!({"version":1,"instance_id":record["meta"]["instance_id"]});
                                }
                                let mut output = format!("{reply}\n").into_bytes();
                                output.extend(frame(b"READY"));
                                if stream.write_all(&output).await.is_err() { return; }
                                while let Ok(kind) = stream.read_u8().await {
                                    let Ok(len) = stream.read_u32().await else { break };
                                    assert!(len < 8192);
                                    let mut bytes = vec![0;len as usize];
                                    if stream.read_exact(&mut bytes).await.is_err() { break; }
                                    assert!(kind == 1 || kind == 2);
                                    writes.fetch_add(1, Ordering::SeqCst);
                                    if kind == 1 && stream.write_all(&frame(&bytes)).await.is_err() { break; }
                                }
                            });
                        }
                        result = jobs.join_next(), if !jobs.is_empty() => { result.unwrap().unwrap(); }
                    }
                }
            })
        };
        Self {
            directory,
            record,
            bound,
            attaches,
            writes,
            forbidden,
            task,
        }
    }
    fn service(&self) -> Arc<TerminalService> {
        Arc::new(TerminalService::new(self.directory.path().into()).unwrap())
    }
    async fn targets(&self) -> (Arc<LaunchTarget>, Option<Arc<BoundTarget>>) {
        let client = HostClient::new(self.directory.path(), Limits::default()).unwrap();
        let observation = client.probe(NAME).await.unwrap();
        let LaunchState::Declared(launch) = &observation.launch else {
            panic!("missing launch")
        };
        let target = Arc::new(
            LaunchTarget::from_observation(
                &observation,
                Source::Codex,
                &launch.launch_id,
                observation.instance_id.as_deref().unwrap(),
            )
            .unwrap(),
        );
        let native = if self.bound.load(Ordering::SeqCst) {
            let native =
                BoundTarget::from_observation(&observation, Source::Codex, SID, UID).unwrap();
            assert_eq!(native.origin_launch_id(), Some(target.launch_id()));
            Some(Arc::new(native))
        } else {
            None
        };
        (target, native)
    }
    async fn replace(&self, launch: &str, instance: &str) {
        let mut record = self.record.lock().await;
        record["meta"]["launch_id"] = json!(launch);
        record["meta"]["instance_id"] = json!(instance);
        tokio::fs::write(
            self.directory.path().join(format!("{NAME}.json")),
            record.to_string(),
        )
        .await
        .unwrap();
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
        if !std::thread::panicking() {
            assert_eq!(
                self.forbidden.load(Ordering::SeqCst),
                0,
                "no raw attach, bind, send, or kill"
            );
        }
    }
}
fn ip() -> IpAddr {
    "192.0.2.1".parse().unwrap()
}
fn size() -> TerminalSize {
    TerminalSize::new(80, 24).unwrap()
}
fn token(claim: ClaimResponse) -> String {
    claim.into_api_json()["token"].as_str().unwrap().into()
}
async fn prepare_native(
    service: &Arc<TerminalService>,
    target: &BoundTarget,
    page: &str,
) -> PreparedAttachment {
    let token = token(
        service
            .claim_bound(target, page, ip(), false)
            .await
            .unwrap(),
    );
    service
        .prepare_bound(NAME, page, &token, UID, target.instance_id())
        .unwrap()
}
type TestSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;
struct BridgeTask(JoinHandle<()>);
impl Drop for BridgeTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn bridge(prepared: PreparedAttachment) -> (TestSocket, BridgeTask) {
    // Only an already authorized prepared lease enters this private route.
    let prepared = Arc::new(AsyncMutex::new(Some(prepared)));
    let router = axum::Router::new().route(
        "/ws",
        axum::routing::get(move |upgrade: axum::extract::WebSocketUpgrade| {
            let prepared = prepared.clone();
            async move {
                let prepared = prepared.lock().await.take().unwrap();
                upgrade.on_upgrade(move |socket| {
                    prepared.run(socket, size(), CancellationToken::new())
                })
            }
        }),
    );
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = BridgeTask(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .unwrap();
    (socket, task)
}
async fn next(socket: &mut TestSocket) -> ClientMessage {
    timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn pending_native_force_never_crosses_kinds_and_explicit_release_allows_a_new_native_claim() {
    let peer = Peer::new().await;
    let service = peer.service();
    let (launch, _) = peer.targets().await;
    let token = token(
        service
            .claim_launch(launch.clone(), "page", ip(), false)
            .await
            .unwrap(),
    );
    let pending = service
        .prepare_launch(NAME, "page", &token, LAUNCH, INSTANCE)
        .unwrap();
    peer.bound.store(true, Ordering::SeqCst);
    let (_, native) = peer.targets().await;
    let native = native.unwrap();
    for force in [false, true] {
        assert_eq!(
            service
                .claim_bound(&native, "page", ip(), force)
                .await
                .err()
                .unwrap()
                .status,
            409
        );
        assert_eq!(
            service
                .claim(NAME, "page", ip(), force)
                .await
                .err()
                .unwrap()
                .status,
            409
        );
    }
    assert!(service.registry.is_current(&pending.guard.bound).unwrap());
    assert!(pending.guard.bound.revocations().borrow().is_none());
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 0);
    // Explicit release, not a silent kind conversion. Stale guard cleanup must
    // not remove the new lease even when page/name are identical.
    assert!(service.registry.release(&pending.guard.bound).unwrap());
    let native_lease = prepare_native(&service, &native, "page").await;
    drop(pending);
    assert!(
        service
            .registry
            .is_current(&native_lease.guard.bound)
            .unwrap()
    );
    for force in [false, true] {
        assert_eq!(
            service
                .claim_launch(launch.clone(), "page", ip(), force)
                .await
                .err()
                .unwrap()
                .status,
            409
        );
    }
    assert!(native_lease.guard.bound.revocations().borrow().is_none());
    let (mut socket, _bridge) = bridge(native_lease).await;
    assert_eq!(
        next(&mut socket).await,
        ClientMessage::Binary(b"READY".to_vec().into())
    );
    socket
        .send(ClientMessage::Binary(b"native input".to_vec().into()))
        .await
        .unwrap();
    assert_eq!(
        next(&mut socket).await,
        ClientMessage::Binary(b"native input".to_vec().into())
    );
    socket.close(None).await.unwrap();
    assert_eq!(peer.writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retiring_the_exact_launch_closes_derived_native_websocket_and_blocks_every_reclaim_path() {
    let peer = Peer::new().await;
    peer.bound.store(true, Ordering::SeqCst);
    let service = peer.service();
    let (launch, native) = peer.targets().await;
    let native = native.unwrap();
    let prepared = prepare_native(&service, &native, "page").await;
    let (mut socket, _bridge) = bridge(prepared).await;
    assert_eq!(
        next(&mut socket).await,
        ClientMessage::Binary(b"READY".to_vec().into())
    );
    socket
        .send(ClientMessage::Binary(b"before retire".to_vec().into()))
        .await
        .unwrap();
    assert_eq!(
        next(&mut socket).await,
        ClientMessage::Binary(b"before retire".to_vec().into())
    );
    service.retire_launch(launch.clone()).await.unwrap();
    let ClientMessage::Close(Some(close)) = next(&mut socket).await else {
        panic!("retired WS must close without notice")
    };
    assert_eq!(u16::from(close.code), 4002);
    assert_eq!(close.reason, "launch retired");
    assert!(service.registry.owner(NAME).unwrap().is_none());
    for force in [false, true] {
        assert_eq!(
            service
                .claim_bound(&native, "late", ip(), force)
                .await
                .err()
                .unwrap()
                .status,
            409
        );
        assert_eq!(
            service
                .claim_launch(launch.clone(), "late", ip(), force)
                .await
                .err()
                .unwrap()
                .status,
            409
        );
        assert_eq!(
            service
                .claim(NAME, "late", ip(), force)
                .await
                .err()
                .unwrap()
                .status,
            409
        );
    }
    assert_eq!(peer.writes.load(Ordering::SeqCst), 1);
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retirement_invalidates_prepared_native_input_and_queued_reclaim_under_the_same_gate() {
    let peer = Peer::new().await;
    peer.bound.store(true, Ordering::SeqCst);
    let service = peer.service();
    let (launch, native) = peer.targets().await;
    let native = native.unwrap();
    let prepared = prepare_native(&service, &native, "page").await;
    let attachment = prepared
        .attach_current(size())
        .await
        .unwrap_or_else(|_| panic!("attach"));
    let (mut reader, mut writer) = attachment.into_split();
    assert_eq!(
        reader.next().await.unwrap(),
        Some(HostEvent::Data(b"READY".to_vec()))
    );
    let held = prepared.guard.gate.lock().await;
    let mut retiring = {
        let (service, launch) = (service.clone(), launch.clone());
        tokio::spawn(async move { service.retire_launch(launch).await })
    };
    assert!(
        timeout(Duration::from_millis(25), &mut retiring)
            .await
            .is_err()
    );
    let mut reclaiming = {
        let (service, native) = (service.clone(), native.clone());
        tokio::spawn(async move { service.claim_bound(&native, "late", ip(), true).await })
    };
    assert!(
        timeout(Duration::from_millis(25), &mut reclaiming)
            .await
            .is_err()
    );
    assert!(service.registry.is_current(&prepared.guard.bound).unwrap());
    drop(held);
    retiring.await.unwrap().unwrap();
    assert_eq!(reclaiming.await.unwrap().err().unwrap().status, 409);
    assert_eq!(
        prepared
            .guard
            .bound
            .revocations()
            .borrow()
            .as_ref()
            .unwrap()
            .reason,
        ownership::RevocationReason::LaunchRetired
    );
    assert_eq!(
        gated_write(
            &prepared.guard,
            &mut writer,
            HostInput::Data(Bytes::from_static(b"forbidden")),
        )
        .await
        .err()
        .unwrap()
        .code,
        4002
    );
    assert_eq!(
        prepared.attach_current(size()).await.err().unwrap().code,
        4002
    );
    assert_eq!(peer.writes.load(Ordering::SeqCst), 0);
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stale_launch_retirement_does_not_revoke_replacement_native_lease_or_its_websocket() {
    // Each nonce is independently part of the retirement key. Deliberately
    // reuse one component in these adversarial fixtures, never in a launcher.
    for (replacement_launch, replacement_instance) in [
        (LAUNCH, "synthetic-instance-replacement"),
        ("synthetic-launch-replacement", INSTANCE),
    ] {
        let peer = Peer::new().await;
        peer.bound.store(true, Ordering::SeqCst);
        let service = peer.service();
        let (old_launch, old_native) = peer.targets().await;
        let old = prepare_native(&service, &old_native.unwrap(), "old").await;
        service.retire_launch(old_launch.clone()).await.unwrap();
        peer.replace(replacement_launch, replacement_instance).await;
        let (_, replacement) = peer.targets().await;
        let replacement = replacement.unwrap();
        let new = prepare_native(&service, &replacement, "new").await;
        service.retire_launch(old_launch).await.unwrap();
        drop(old);
        assert!(service.registry.is_current(&new.guard.bound).unwrap());
        assert!(new.guard.bound.revocations().borrow().is_none());
        let (mut socket, _bridge) = bridge(new).await;
        assert_eq!(
            next(&mut socket).await,
            ClientMessage::Binary(b"READY".to_vec().into())
        );
        socket
            .send(ClientMessage::Binary(b"replacement alive".to_vec().into()))
            .await
            .unwrap();
        assert_eq!(
            next(&mut socket).await,
            ClientMessage::Binary(b"replacement alive".to_vec().into())
        );
        socket.close(None).await.unwrap();
        assert_eq!(peer.writes.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn fresh_claim_rejects_changed_launch_origin_even_when_native_tuple_and_instance_match() {
    let peer = Peer::new().await;
    peer.bound.store(true, Ordering::SeqCst);
    let service = peer.service();
    let (_, original) = peer.targets().await;
    let original = original.unwrap();
    let old = prepare_native(&service, &original, "old").await;
    peer.replace("synthetic-launch-different", INSTANCE).await;
    let error = service
        .claim_bound(&original, "new", ip(), true)
        .await
        .err()
        .unwrap();
    assert_eq!(error.status, 409);
    assert!(service.registry.is_current(&old.guard.bound).unwrap());
    assert!(old.guard.bound.revocations().borrow().is_none());
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 0);
}
