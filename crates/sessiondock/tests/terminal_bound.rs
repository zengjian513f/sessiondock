//! HTTP/WS guarded transport against synthetic native inventory and fake hosts.
//! No shell, native home, process discovery, or paid CLI is used here.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sessiondock::{app_with_shutdown, config::Config, sessions::SessionRoots};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{self, Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const NAME: &str = "guarded-terminal";
const INSTANCE: &str = "synthetic-instance-original";
const SECRET: &str = "SYNTHETIC_GUARDED_HOST_SECRET";
const NO_ACK: u8 = 1;
const LEGACY: u8 = 2;
const OFFLINE: u8 = 3;
const EXITED: u8 = 4;
type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct FakeHost {
    path: PathBuf,
    record: Arc<Mutex<Value>>,
    mode: Arc<AtomicU8>,
    frames: Arc<AtomicUsize>,
    attached_resizes: Arc<AtomicUsize>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl FakeHost {
    async fn new(directory: &Path, name: &str, uid: &str) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let value = json!({"name":name,"host_pid":41,"pid":42,"created":10,"cols":80,"rows":24,
            "port":listener.local_addr().unwrap().port(),"token":SECRET,"argv":["PRIVATE_ARGV"],
            "meta":{"source":"codex","sid":"synthetic-codex","uid":uid,"instance_id":INSTANCE,"secret":"PRIVATE_META"}});
        let path = directory.join(format!("{name}.json"));
        tokio::fs::write(&path, serde_json::to_vec(&value).unwrap())
            .await
            .unwrap();
        let record = Arc::new(Mutex::new(value));
        let mode = Arc::new(AtomicU8::new(0));
        let frames = Arc::new(AtomicUsize::new(0));
        let attached_resizes = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let task = {
            let (record, mode, frames, attached_resizes, cancel) = (
                record.clone(),
                mode.clone(),
                frames.clone(),
                attached_resizes.clone(),
                cancel.clone(),
            );
            tokio::spawn(async move {
                let mut jobs = JoinSet::new();
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        result = listener.accept() => {
                            let (mut socket,_) = result.unwrap();
                            let (record,mode,frames,attached_resizes) = (record.clone(),mode.clone(),frames.clone(),attached_resizes.clone());
                            jobs.spawn(async move {
                                let mut line = Vec::new();
                                while let Ok(byte) = socket.read_u8().await { if byte == b'\n' { break } line.push(byte); assert!(line.len()<8192); }
                                let Ok(request) = serde_json::from_slice::<Value>(&line) else { return };
                                assert_eq!(request["token"],SECRET);
                                let mode = mode.load(Ordering::SeqCst);
                                let record = record.lock().await.clone();
                                if mode == OFFLINE { return }
                                if request["op"] == "info" {
                                    let caps = if mode == LEGACY { json!({}) } else { json!({"instance_guard":1}) };
                                    let bytes = format!("{}\n",json!({"ok":true,"info":record,"exited":mode==EXITED,"capabilities":caps}));
                                    let _ = socket.write_all(bytes.as_bytes()).await; return;
                                }
                                assert_eq!(request["op"],"guarded_v1","no bound request may downgrade");
                                if mode == LEGACY || request["expected_instance_id"] != record["meta"]["instance_id"]
                                    || request["expected_source"] != record["meta"]["source"]
                                    || request["expected_sid"] != record["meta"]["sid"]
                                    || request["expected_uid"] != record["meta"]["uid"] {
                                    let _ = socket.write_all(b"{\"ok\":false,\"error\":\"guard rejected\"}\n").await; return;
                                }
                                assert_eq!(request["request"]["op"],"attach");
                                // A valid receiving guard can already resize the
                                // child before its acknowledgement is delivered.
                                attached_resizes.fetch_add(1,Ordering::SeqCst);
                                let mut reply = json!({"ok":true,"cols":80,"rows":24});
                                if mode != NO_ACK { reply["instance_guard"] = json!({"version":1,"instance_id":record["meta"]["instance_id"]}); }
                                let mut output = format!("{reply}\n").into_bytes();
                                output.extend(frame(1,b"GUARDED_READY"));
                                if socket.write_all(&output).await.is_err() { return }
                                while let Ok(kind) = socket.read_u8().await {
                                    let Ok(length) = socket.read_u32().await else { break }; assert!(length<65536);
                                    let mut bytes = vec![0;length as usize]; if socket.read_exact(&mut bytes).await.is_err() { break }
                                    frames.fetch_add(1,Ordering::SeqCst);
                                    if kind == 1 && socket.write_all(&frame(1,&bytes)).await.is_err() { break }
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
            path,
            record,
            mode,
            frames,
            attached_resizes,
            cancel,
            task,
        }
    }

    async fn set_meta(&self, key: &str, value: Value, disk: bool) {
        let mut record = self.record.lock().await;
        record["meta"][key] = value;
        if disk {
            tokio::fs::write(&self.path, serde_json::to_vec(&*record).unwrap())
                .await
                .unwrap();
        }
    }
}

impl Drop for FakeHost {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

fn frame(kind: u8, bytes: &[u8]) -> Vec<u8> {
    let mut output = vec![kind];
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    output.extend_from_slice(bytes);
    output
}

struct Harness {
    _directory: TempDir,
    hosts: PathBuf,
    app: Router,
    address: SocketAddr,
    uid: String,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl Harness {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let native = directory.path().join("native");
        let hosts = directory.path().join("hosts");
        std::fs::create_dir(&native).unwrap();
        std::fs::create_dir(&hosts).unwrap();
        std::fs::write(
            native.join("rollout-synthetic-codex.jsonl"),
            include_bytes!("fixtures/codex/2026/09/11/rollout-synthetic-codex.jsonl"),
        )
        .unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let cancel = CancellationToken::new();
        let app = app_with_shutdown(
            Config {
                bind: address,
                web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
                roots: SessionRoots {
                    claude: None,
                    codex: Some(native),
                    grok: None,
                },
                ptyhost_dir: Some(hosts.clone()),
                ..Default::default()
            },
            cancel.clone(),
        )
        .unwrap();
        let task = {
            let (app, cancel) = (app.clone(), cancel.clone());
            tokio::spawn(async move {
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(cancel.cancelled_owned())
                .await
                .unwrap();
            })
        };
        let mut harness = Self {
            _directory: directory,
            hosts,
            app,
            address,
            uid: String::new(),
            cancel,
            task,
        };
        let (_, body) = harness.request("GET", "/api/sessions", Value::Null).await;
        harness.uid = body["sessions"][0]["uid"].as_str().unwrap().into();
        harness
    }

    async fn request(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", self.address.to_string())
            .header("content-type", "application/json")
            .body(if method == "GET" {
                Body::empty()
            } else {
                Body::from(body.to_string())
            })
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        for secret in [SECRET, "PRIVATE_ARGV", "PRIVATE_META"] {
            assert!(!body.to_string().contains(secret));
        }
        (status, body)
    }

    async fn claim(&self, page: &str, force: bool) -> String {
        let (status,body) = self.request("POST","/api/term/claim",json!({"name":NAME,"page":page,"force":force,"uid":self.uid,"instance_id":INSTANCE})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["token"].as_str().unwrap().into()
    }

    async fn connect(
        &self,
        page: &str,
        token: &str,
        binding: bool,
    ) -> Result<Ws, tungstenite::Error> {
        let scope = if binding {
            format!("&uid={}&instance_id={INSTANCE}", self.uid)
        } else {
            String::new()
        };
        let uri = format!(
            "ws://{}/api/term/attach?name={NAME}&page={page}&token={token}&cols=80&rows=24{scope}",
            self.address
        );
        let mut request = uri.into_client_request().unwrap();
        request.headers_mut().insert(
            "origin",
            format!("http://{}", self.address).parse().unwrap(),
        );
        timeout(Duration::from_secs(3), connect_async(request))
            .await
            .unwrap()
            .map(|(ws, _)| ws)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}
async fn next(ws: &mut Ws) -> Message {
    timeout(Duration::from_secs(3), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}
async fn closed(ws: &mut Ws) -> u16 {
    loop {
        if let Message::Close(frame) = next(ws).await {
            return frame.unwrap().code.into();
        }
    }
}

#[tokio::test]
async fn exact_catalog_claim_and_guarded_ws_preserve_bytes_and_refuse_raw_downgrade() {
    let h = Harness::new().await;
    let host = FakeHost::new(&h.hosts, NAME, &h.uid).await;
    let token = h.claim("page", false).await;
    let mut ws = h.connect("page", &token, true).await.unwrap();
    assert_eq!(
        next(&mut ws).await,
        Message::Binary(b"GUARDED_READY".to_vec().into())
    );
    let (status, _) = h
        .request(
            "POST",
            "/api/term/claim",
            json!({"name":NAME,"page":"page","force":true}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let error = h.connect("page", &token, false).await.err().unwrap();
    assert!(
        matches!(error,tungstenite::Error::Http(ref response) if response.status()==StatusCode::CONFLICT)
    );
    ws.send(Message::Binary(vec![b'X', 0xff, b'Y'].into()))
        .await
        .unwrap();
    assert_eq!(
        next(&mut ws).await,
        Message::Binary(vec![b'X', 0xff, b'Y'].into())
    );
    assert_eq!(host.frames.load(Ordering::SeqCst), 1);
    ws.close(None).await.unwrap();
}

#[tokio::test]
async fn claim_then_same_name_replacement_rejects_input_and_resize_before_new_host_effects() {
    for disk in [true, false] {
        let h = Harness::new().await;
        let host = FakeHost::new(&h.hosts, NAME, &h.uid).await;
        let token = h.claim("page", false).await;
        host.set_meta("instance_id", json!("synthetic-instance-replacement"), disk)
            .await;
        // Fresh native observation rejects replacement before a writable upgrade.
        let error = h.connect("page", &token, true).await.err().unwrap();
        assert!(
            matches!(error,tungstenite::Error::Http(ref response) if response.status()==StatusCode::CONFLICT)
        );
        assert_eq!(host.frames.load(Ordering::SeqCst), 0);
        assert_eq!(host.attached_resizes.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn guardless_or_missing_ack_replacement_is_not_usable_or_silently_retried() {
    for mode in [LEGACY, NO_ACK] {
        let h = Harness::new().await;
        let host = FakeHost::new(&h.hosts, NAME, &h.uid).await;
        let token = h.claim("page", false).await;
        host.mode.store(mode, Ordering::SeqCst);
        if mode == LEGACY {
            // Missing capability is known before upgrade; no attach is attempted.
            let error = h.connect("page", &token, true).await.err().unwrap();
            assert!(
                matches!(error,tungstenite::Error::Http(ref response) if response.status()==StatusCode::CONFLICT)
            );
        } else {
            // A missing attach ACK is only discovered after the guarded request.
            let mut ws = h.connect("page", &token, true).await.unwrap();
            let _ = ws
                .send(Message::Binary(b"no forward".to_vec().into()))
                .await;
            let closed = timeout(Duration::from_secs(3), ws.next())
                .await
                .unwrap()
                .unwrap();
            match closed {
                Ok(Message::Close(frame)) => assert_eq!(u16::from(frame.unwrap().code), 1011),
                #[cfg(windows)]
                Err(tungstenite::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                other => panic!("missing-ack connection did not close: {other:?}"),
            }
        }
        assert_eq!(host.frames.load(Ordering::SeqCst), 0);
        // No-ack is ambiguous: a compliant guard could already have resized.
        assert_eq!(
            host.attached_resizes.load(Ordering::SeqCst),
            usize::from(mode == NO_ACK)
        );
    }
}

#[tokio::test]
async fn wrong_sid_uid_duplicate_offline_exited_or_guardless_hosts_cannot_claim() {
    for scenario in 0..7 {
        let h = Harness::new().await;
        let host = FakeHost::new(&h.hosts, NAME, &h.uid).await;
        let _duplicate = match scenario {
            0 => {
                host.set_meta("sid", json!("different-native-session"), true)
                    .await;
                None
            }
            1 => {
                host.set_meta("uid", json!("codex:ffffffffffffffff"), true)
                    .await;
                None
            }
            2 => Some(FakeHost::new(&h.hosts, "duplicate", &h.uid).await),
            3 => {
                host.mode.store(OFFLINE, Ordering::SeqCst);
                None
            }
            4 => {
                host.mode.store(EXITED, Ordering::SeqCst);
                None
            }
            5 => {
                host.mode.store(LEGACY, Ordering::SeqCst);
                None
            }
            _ => None,
        };
        let uid = if scenario == 6 {
            "codex:bad-query-uid"
        } else {
            &h.uid
        };
        let (status, list) = h.request("GET", "/api/term/list", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            list["enabled"],
            scenario == 6,
            "scenario {scenario}: {list}"
        );
        if scenario == 6 {
            assert_eq!(list["sessions"][0]["uid"], h.uid);
            assert_eq!(list["sessions"][0]["instance_id"], INSTANCE);
            assert_eq!(list["sessions"][0]["sid"], "synthetic-codex");
        } else {
            assert_eq!(list["sessions"], json!([]));
        }
        assert_eq!(list["sources"], json!({}));
        assert_eq!(list["pending"], json!([]));
        let (status, _) = h
            .request(
                "POST",
                "/api/term/claim",
                json!({"name":NAME,"page":"page","uid":uid,"instance_id":INSTANCE}),
            )
            .await;
        assert!(
            matches!(status, StatusCode::FORBIDDEN | StatusCode::CONFLICT),
            "scenario {scenario}: {status}"
        );
        assert_eq!(host.frames.load(Ordering::SeqCst), 0);
        assert_eq!(host.attached_resizes.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn force_takeover_revokes_old_bound_socket_and_old_cleanup_preserves_replacement() {
    let h = Harness::new().await;
    let host = FakeHost::new(&h.hosts, NAME, &h.uid).await;
    let first = h.claim("first", false).await;
    let mut old = h.connect("first", &first, true).await.unwrap();
    let _ = next(&mut old).await;
    let second = h.claim("second", true).await;
    assert!(
        matches!(next(&mut old).await,Message::Text(text) if serde_json::from_str::<Value>(&text).unwrap()["t"]=="revoked")
    );
    assert_eq!(closed(&mut old).await, 4001);
    drop(old);
    let mut new = h.connect("second", &second, true).await.unwrap();
    let _ = next(&mut new).await;
    new.send(Message::Binary(b"new owner".to_vec().into()))
        .await
        .unwrap();
    assert_eq!(
        next(&mut new).await,
        Message::Binary(b"new owner".to_vec().into())
    );
    assert_eq!(host.frames.load(Ordering::SeqCst), 1);
    new.close(None).await.unwrap();
}
