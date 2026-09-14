//! Synthetic local TCP peer only; no shell, CLI, or native history discovery.
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

const NAME: &str = "pending-terminal";
const INSTANCE: &str = "synthetic-instance-original";
const LAUNCH: &str = "synthetic-launch-original";

struct Peer {
    directory: TempDir,
    record: Arc<AsyncMutex<Value>>,
    infos: Arc<AtomicUsize>,
    attaches: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
    no_ack: Arc<AtomicBool>,
    legacy: Arc<AtomicBool>,
    forbidden_controls: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

fn frame(kind: u8, bytes: &[u8]) -> Vec<u8> {
    let mut frame = vec![kind];
    frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(bytes);
    frame
}

impl Peer {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let value = json!({"name":NAME,"host_pid":10,"pid":11,"created":12,"cols":80,"rows":24,
            "port":listener.local_addr().unwrap().port(),"token":"SYNTHETIC_HOST_SECRET",
            "meta":{"source":"codex","instance_id":INSTANCE,"launch_id":LAUNCH}});
        tokio::fs::write(
            directory.path().join(format!("{NAME}.json")),
            value.to_string(),
        )
        .await
        .unwrap();
        let record = Arc::new(AsyncMutex::new(value));
        let infos = Arc::new(AtomicUsize::new(0));
        let attaches = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(AtomicUsize::new(0));
        let no_ack = Arc::new(AtomicBool::new(false));
        let legacy = Arc::new(AtomicBool::new(false));
        let forbidden_controls = Arc::new(AtomicUsize::new(0));
        let task = {
            let forbidden_controls = forbidden_controls.clone();
            let legacy = legacy.clone();
            let (record, infos, attaches, writes, no_ack) = (
                record.clone(),
                infos.clone(),
                attaches.clone(),
                writes.clone(),
                no_ack.clone(),
            );
            tokio::spawn(async move {
                let mut jobs = JoinSet::new();
                loop {
                    tokio::select! {
                        result=listener.accept() => {
                            let (mut stream,_) = result.unwrap();
                            let forbidden_controls = forbidden_controls.clone();
                            let legacy = legacy.clone();
                            let (record,infos,attaches,writes,no_ack) = (record.clone(),infos.clone(),attaches.clone(),writes.clone(),no_ack.clone());
                            jobs.spawn(async move {
                                let mut bytes = Vec::new();
                                loop { let Ok(byte)=stream.read_u8().await else {return}; if byte==b'\n' {break} bytes.push(byte); assert!(bytes.len()<8192); }
                                let request:Value = serde_json::from_slice(&bytes).unwrap();
                                assert_eq!(request["token"],"SYNTHETIC_HOST_SECRET");
                                let record = record.lock().await.clone();
                                if request["op"]=="info" {
                                    infos.fetch_add(1,Ordering::SeqCst);
                                    let capabilities=if legacy.load(Ordering::SeqCst) {json!({})} else {json!({"instance_guard":1,"launch_guard":1})};
                                    let reply=json!({"ok":true,"info":record,"exited":false,"capabilities":capabilities});
                                    let _=stream.write_all(format!("{reply}\n").as_bytes()).await;
                                    return;
                                }
                                if request["op"]!="launch_guard_v1" || request["request"]["op"]!="attach" {
                                    forbidden_controls.fetch_add(1,Ordering::SeqCst);
                                    return;
                                }
                                if request["expected_instance_id"]!=record["meta"]["instance_id"]
                                    || request["expected_launch_id"]!=record["meta"]["launch_id"]
                                    || request["expected_source"]!=record["meta"]["source"] {
                                    let _=stream.write_all(b"{\"ok\":false}\n").await; return;
                                }
                                assert_eq!(request["request"]["op"],"attach");
                                attaches.fetch_add(1,Ordering::SeqCst);
                                let mut reply=json!({"ok":true,"cols":80,"rows":24});
                                if !no_ack.load(Ordering::SeqCst) {
                                    reply["launch_guard"]=json!({"version":1,"source":"codex","instance_id":record["meta"]["instance_id"],"launch_id":record["meta"]["launch_id"]});
                                }
                                let mut output=format!("{reply}\n").into_bytes();
                                output.extend(frame(1,b"PENDING_READY"));
                                if stream.write_all(&output).await.is_err() {return}
                                while let Ok(kind)=stream.read_u8().await {
                                    let Ok(length)=stream.read_u32().await else {break}; assert!(length<8192);
                                    let mut bytes=vec![0;length as usize]; if stream.read_exact(&mut bytes).await.is_err(){break}
                                    writes.fetch_add(1,Ordering::SeqCst);
                                    if kind==1 {
                                        if stream.write_all(&frame(1,&bytes)).await.is_err(){break}
                                        if bytes==b"exit" {
                                            let _=stream.write_all(&frame(3,b"{\"code\":0,\"output_complete\":true}")).await;
                                            break;
                                        }
                                    }
                                }
                            });
                        }
                        _=jobs.join_next(), if !jobs.is_empty()=>{}
                    }
                }
            })
        };
        Self {
            directory,
            record,
            infos,
            attaches,
            writes,
            no_ack,
            legacy,
            forbidden_controls,
            task,
        }
    }

    fn service(&self) -> Arc<TerminalService> {
        Arc::new(TerminalService::new(self.directory.path().to_owned()).unwrap())
    }
    async fn target(&self) -> Arc<LaunchTarget> {
        let client = HostClient::new(self.directory.path(), Limits::default()).unwrap();
        let observed = client.probe(NAME).await.unwrap();
        let LaunchState::Declared(identity) = &observed.launch else {
            panic!()
        };
        Arc::new(
            LaunchTarget::from_observation(
                &observed,
                Source::Codex,
                &identity.launch_id,
                observed.instance_id.as_deref().unwrap(),
            )
            .unwrap(),
        )
    }
    async fn metadata(&self, metadata: Value, disk: bool) {
        let mut record = self.record.lock().await;
        record["meta"] = metadata;
        if disk {
            tokio::fs::write(
                self.directory.path().join(format!("{NAME}.json")),
                record.to_string(),
            )
            .await
            .unwrap();
        }
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
        if !std::thread::panicking() {
            assert_eq!(
                self.forbidden_controls.load(Ordering::SeqCst),
                0,
                "launch bridge may not send raw/native control or kill"
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
fn token(response: ClaimResponse) -> String {
    response.into_api_json()["token"].as_str().unwrap().into()
}
async fn prepare(
    service: &Arc<TerminalService>,
    target: Arc<LaunchTarget>,
    page: &str,
    force: bool,
) -> PreparedAttachment {
    let token = token(
        service
            .claim_launch(target.clone(), page, ip(), force)
            .await
            .unwrap(),
    );
    service
        .prepare_launch(NAME, page, &token, target.launch_id(), target.instance_id())
        .unwrap()
}

#[tokio::test]
async fn pending_raw_claim_is_rejected_before_any_lease_and_bad_bind_does_not_consume_launch() {
    let peer = Peer::new().await;
    let service = peer.service();
    assert_eq!(
        service
            .claim(NAME, "raw", ip(), true)
            .await
            .err()
            .unwrap()
            .status,
        409
    );
    assert!(service.registry.owner(NAME).unwrap().is_none());
    let target = peer.target().await;
    let lease = token(
        service
            .claim_launch(target, "page", ip(), false)
            .await
            .unwrap(),
    );
    assert_eq!(
        service.prepare(NAME, "page", &lease).err().unwrap().status,
        409
    );
    assert_eq!(
        service
            .prepare_bound(NAME, "page", &lease, "codex:fake-native", INSTANCE)
            .err()
            .unwrap()
            .status,
        409
    );
    assert_eq!(
        service
            .prepare_launch(NAME, "page", &lease, LAUNCH, "synthetic-instance-wrong")
            .err()
            .unwrap()
            .status,
        409
    );
    assert_eq!(
        service
            .prepare_launch(NAME, "page", &lease, "synthetic-launch-short", INSTANCE)
            .err()
            .unwrap()
            .status,
        409
    );
    let prepared = service
        .prepare_launch(NAME, "page", &lease, LAUNCH, INSTANCE)
        .unwrap();
    assert!(service.registry.is_current(&prepared.guard.bound).unwrap());
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_launch_or_null_meta_never_enable_raw_control() {
    let peer = Peer::new().await;
    let service = peer.service();
    for metadata in [
        Value::Null,
        json!({"launch_id":null}),
        json!({"launch_id":"tiny"}),
        json!({"source":"codex","instance_id":INSTANCE,"launch_id":42}),
    ] {
        peer.metadata(metadata, true).await;
        assert_eq!(
            service
                .claim(NAME, "raw", ip(), true)
                .await
                .err()
                .unwrap()
                .status,
            409
        );
        assert!(service.registry.owner(NAME).unwrap().is_none());
    }
    // Unknown, non-launch metadata remains legal for the existing raw path.
    peer.metadata(json!({"unreviewed":"ignored"}), true).await;
    assert_eq!(
        service
            .claim(NAME, "raw", ip(), false)
            .await
            .unwrap()
            .status(),
        200
    );
}

#[tokio::test]
async fn raw_reservation_cannot_attach_host_that_now_declares_launch() {
    let peer = Peer::new().await;
    let service = peer.service();
    peer.metadata(json!({}), true).await;
    let lease = token(service.claim(NAME, "page", ip(), false).await.unwrap());
    let prepared = service.prepare(NAME, "page", &lease).unwrap();
    peer.metadata(
        json!({"source":"codex","instance_id":INSTANCE,"launch_id":LAUNCH}),
        true,
    )
    .await;
    assert_eq!(
        prepared.attach_current(size()).await.err().unwrap().code,
        1011
    );
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn launch_replacement_and_missing_ack_fail_without_raw_retry_or_input() {
    for disk in [false, true] {
        let peer = Peer::new().await;
        let service = peer.service();
        let target = peer.target().await;
        let prepared = prepare(&service, target.clone(), "page", false).await;
        peer.metadata(json!({"source":"codex","instance_id":"synthetic-instance-new","launch_id":"synthetic-launch-new"}),disk).await;
        assert_eq!(
            prepared.attach_current(size()).await.err().unwrap().code,
            1011
        );
        assert_eq!(
            service
                .claim_launch(target, "new", ip(), true)
                .await
                .err()
                .unwrap()
                .status,
            409
        );
        assert!(service.registry.is_current(&prepared.guard.bound).unwrap());
        assert_eq!(peer.attaches.load(Ordering::SeqCst), 0);
        assert_eq!(peer.writes.load(Ordering::SeqCst), 0);
    }
    let peer = Peer::new().await;
    let service = peer.service();
    let prepared = prepare(&service, peer.target().await, "page", false).await;
    peer.no_ack.store(true, Ordering::SeqCst);
    assert_eq!(
        prepared.attach_current(size()).await.err().unwrap().code,
        1011
    );
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 1);
    assert_eq!(peer.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn retirement_serializes_with_claim_and_input_and_blocks_only_the_exact_tuple() {
    let peer = Peer::new().await;
    let service = peer.service();
    let target = peer.target().await;
    let prepared = prepare(&service, target.clone(), "page", false).await;
    let attachment = prepared
        .attach_current(size())
        .await
        .unwrap_or_else(|_| panic!("attach"));
    let (mut reader, mut writer) = attachment.into_split();
    assert_eq!(
        reader.next().await.unwrap(),
        Some(HostEvent::Data(b"PENDING_READY".to_vec()))
    );
    gated_write(
        &prepared.guard,
        &mut writer,
        HostInput::Data(Bytes::from_static(b"before")),
    )
    .await
    .unwrap_or_else(|_| panic!("write"));
    assert_eq!(
        reader.next().await.unwrap(),
        Some(HostEvent::Data(b"before".to_vec()))
    );
    let held = prepared.guard.gate.lock().await;
    let mut retiring = {
        let service = service.clone();
        let target = target.clone();
        tokio::spawn(async move { service.retire_launch(target).await })
    };
    assert!(
        timeout(Duration::from_millis(25), &mut retiring)
            .await
            .is_err()
    );
    assert!(service.registry.is_current(&prepared.guard.bound).unwrap());
    // FIFO mutex: this claimant queues after retirement, so no probe or lease
    // publication is permitted when the in-flight input gate is released.
    let infos = peer.infos.load(Ordering::SeqCst);
    let mut claiming = {
        let service = service.clone();
        let target = target.clone();
        tokio::spawn(async move { service.claim_launch(target, "late", ip(), true).await })
    };
    assert!(
        timeout(Duration::from_millis(25), &mut claiming)
            .await
            .is_err()
    );
    drop(held);
    retiring.await.unwrap().unwrap();
    assert_eq!(claiming.await.unwrap().err().unwrap().status, 409);
    assert_eq!(peer.infos.load(Ordering::SeqCst), infos);
    assert!(!service.registry.is_current(&prepared.guard.bound).unwrap());
    assert_eq!(
        gated_write(
            &prepared.guard,
            &mut writer,
            HostInput::Data(Bytes::from_static(b"after")),
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
    assert_eq!(peer.writes.load(Ordering::SeqCst), 1);
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 1);
    peer.metadata(
        json!({"source":"codex","instance_id":"synthetic-instance-new","launch_id":LAUNCH}),
        true,
    )
    .await;
    let new = prepare(&service, peer.target().await, "new", true).await;
    service.retire_launch(target).await.unwrap();
    drop(prepared);
    assert!(service.registry.is_current(&new.guard.bound).unwrap());
}

#[tokio::test]
async fn claim_already_queued_before_retirement_is_revoked_after_publication() {
    let peer = Peer::new().await;
    let service = peer.service();
    let target = peer.target().await;
    let gate = service.gate(NAME).unwrap();
    let held = gate.lock().await;
    let mut claiming = {
        let service = service.clone();
        let target = target.clone();
        tokio::spawn(async move { service.claim_launch(target, "early", ip(), false).await })
    };
    assert!(
        timeout(Duration::from_millis(25), &mut claiming)
            .await
            .is_err()
    );
    let mut retiring = {
        let service = service.clone();
        let target = target.clone();
        tokio::spawn(async move { service.retire_launch(target).await })
    };
    assert!(
        timeout(Duration::from_millis(25), &mut retiring)
            .await
            .is_err()
    );
    drop(held);
    let token = token(claiming.await.unwrap().unwrap());
    retiring.await.unwrap().unwrap();
    assert!(service.registry.owner(NAME).unwrap().is_none());
    assert_eq!(
        service
            .prepare_launch(NAME, "early", &token, LAUNCH, INSTANCE)
            .err()
            .unwrap()
            .status,
        409
    );
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 0);
}

type TestSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;
#[tokio::test]
async fn force_with_missing_fresh_launch_capability_preserves_the_previous_lease() {
    let peer = Peer::new().await;
    let service = peer.service();
    let target = peer.target().await;
    let prepared = prepare(&service, target.clone(), "page", false).await;
    peer.legacy.store(true, Ordering::SeqCst);
    let error = service
        .claim_launch(target, "new", ip(), true)
        .await
        .err()
        .unwrap();
    assert_eq!(error.status, 501);
    assert_eq!(error.code, "terminal_launch_guard_required");
    assert!(service.registry.is_current(&prepared.guard.bound).unwrap());
    assert!(prepared.guard.bound.revocations().borrow().is_none());
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 0);
}

struct BridgeTask(JoinHandle<()>);
impl Drop for BridgeTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn bridge(prepared: PreparedAttachment) -> (TestSocket, BridgeTask) {
    // This private test route consumes an already authorized prepared lease.
    // It is not an application HTTP endpoint or an alternative claim path.
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

async fn next(socket: &mut TestSocket) -> tokio_tungstenite::tungstenite::Message {
    timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn launch_websocket_bridge_preserves_replay_input_echo_and_tail_before_exit_close() {
    use tokio_tungstenite::tungstenite::Message as ClientMessage;
    let peer = Peer::new().await;
    let service = peer.service();
    let prepared = prepare(&service, peer.target().await, "page", false).await;
    let (mut socket, _bridge) = bridge(prepared).await;
    assert_eq!(
        next(&mut socket).await,
        ClientMessage::Binary(b"PENDING_READY".to_vec().into())
    );
    socket
        .send(ClientMessage::Binary(vec![0, 255, b'X'].into()))
        .await
        .unwrap();
    assert_eq!(
        next(&mut socket).await,
        ClientMessage::Binary(vec![0, 255, b'X'].into())
    );
    socket
        .send(ClientMessage::Binary(b"exit".to_vec().into()))
        .await
        .unwrap();
    assert_eq!(
        next(&mut socket).await,
        ClientMessage::Binary(b"exit".to_vec().into())
    );
    let ClientMessage::Close(Some(close)) = next(&mut socket).await else {
        panic!("missing close")
    };
    assert_eq!(u16::from(close.code), 1000);
    assert_eq!(close.reason, "host exited");
    assert!(service.registry.owner(NAME).unwrap().is_none());
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 1);
    assert_eq!(peer.writes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn launch_force_and_retire_close_live_websockets_without_killing_or_releasing_new_lease() {
    use tokio_tungstenite::tungstenite::Message as ClientMessage;
    let peer = Peer::new().await;
    let service = peer.service();
    let target = peer.target().await;
    let prepared = prepare(&service, target.clone(), "page", false).await;
    let (mut old, _old_bridge) = bridge(prepared).await;
    assert!(matches!(next(&mut old).await, ClientMessage::Binary(_)));
    let new = prepare(&service, target.clone(), "new", true).await;
    // Other-page force takeover may send its established IP notice first.
    loop {
        match next(&mut old).await {
            ClientMessage::Text(_) => {}
            ClientMessage::Close(Some(close)) => {
                assert_eq!(u16::from(close.code), 4001);
                break;
            }
            other => panic!("unexpected revoke message: {other:?}"),
        }
    }
    assert!(service.registry.is_current(&new.guard.bound).unwrap());
    let (mut new, _new_bridge) = bridge(new).await;
    assert!(matches!(next(&mut new).await, ClientMessage::Binary(_)));
    service.retire_launch(target).await.unwrap();
    let ClientMessage::Close(Some(close)) = next(&mut new).await else {
        panic!("missing retire close")
    };
    assert_eq!(u16::from(close.code), 4002);
    assert_eq!(close.reason, "launch retired");
    assert!(service.registry.owner(NAME).unwrap().is_none());
    assert_eq!(peer.writes.load(Ordering::SeqCst), 0);
    assert_eq!(peer.attaches.load(Ordering::SeqCst), 2);
}
