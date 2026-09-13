use std::time::Duration;

use ptyhost_client::{
    BoundTarget, ControlOp, ControlReply, Error, HostClient, HostEvent, HostObservation, Limits,
    Source, TerminalSize,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};

const INSTANCE: &str = "synthetic-instance-original";
const UID: &str = "codex:0123456789abcdef";
const SID: &str = "complete-synthetic-session";
const SECRET: &str = "PRIVATE_GUARDED_HOST_TOKEN";

async fn request(listener: &TcpListener) -> (TcpStream, Value) {
    let (mut stream, _) = listener.accept().await.unwrap();
    let mut bytes = Vec::new();
    loop {
        let byte = stream.read_u8().await.unwrap();
        if byte == b'\n' {
            break;
        }
        bytes.push(byte);
        assert!(bytes.len() < 8192);
    }
    (stream, serde_json::from_slice(&bytes).unwrap())
}

async fn reply(stream: &mut TcpStream, value: Value) {
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    stream.write_all(&bytes).await.unwrap();
}

async fn fixture(
    capability: Value,
    meta: Value,
) -> (TempDir, TcpListener, HostClient, HostObservation, Value) {
    let directory = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let record = json!({"name":"synthetic","host_pid":10,"pid":11,"created":12,"cols":80,"rows":24,
        "port":listener.local_addr().unwrap().port(),"token":SECRET,"meta":meta});
    tokio::fs::write(
        directory.path().join("synthetic.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .await
    .unwrap();
    let client = HostClient::new(
        directory.path(),
        Limits {
            operation_timeout: Duration::from_millis(150),
            ..Limits::default()
        },
    )
    .unwrap();
    let server = {
        let record = record.clone();
        tokio::spawn(async move {
            let (mut stream, body) = request(&listener).await;
            assert_eq!(body["op"], "info");
            reply(&mut stream, json!({"ok":true,"info":record,"exited":false,"capabilities":{"instance_guard":capability}})).await;
            listener
        })
    };
    let observation = client.probe("synthetic").await.unwrap();
    (
        directory,
        server.await.unwrap(),
        client,
        observation,
        record,
    )
}

fn meta() -> Value {
    json!({"source":"codex","sid":SID,"uid":UID,"instance_id":INSTANCE})
}
fn target(observed: &HostObservation) -> BoundTarget {
    BoundTarget::from_observation(observed, Source::Codex, SID, UID).unwrap()
}
fn ack() -> Value {
    json!({"version":1,"instance_id":INSTANCE})
}

#[tokio::test]
async fn binding_requires_explicit_guard_capability_strong_instance_and_exact_native_ids() {
    for capability in [
        json!(null),
        json!(false),
        json!(true),
        json!("1"),
        json!(0),
        json!(2),
    ] {
        let (_directory, _listener, _client, observed, _) = fixture(capability, meta()).await;
        assert!(!observed.instance_guard_v1);
        assert!(matches!(
            BoundTarget::from_observation(&observed, Source::Codex, SID, UID),
            Err(Error::GuardUnsupported)
        ));
    }
    let (_directory, _listener, _client, observed, _) = fixture(json!(1), meta()).await;
    let bound = target(&observed);
    assert_eq!(
        (
            bound.name(),
            bound.source(),
            bound.sid(),
            bound.uid(),
            bound.instance_id()
        ),
        ("synthetic", Source::Codex, SID, UID, INSTANCE)
    );
    for (source, sid, uid) in [
        (Source::Claude, SID, UID),
        (Source::Codex, "complete", UID),
        (Source::Codex, SID, "codex:wrong"),
        (Source::Codex, "../path", UID),
    ] {
        assert!(matches!(
            BoundTarget::from_observation(&observed, source, sid, uid),
            Err(Error::InvalidBinding)
        ));
    }
    for mode in [0, 1, 2] {
        let mut modified = observed.clone();
        match mode {
            0 => modified.instance_id = None,
            1 => modified.instance_id = Some("tiny".into()),
            _ => modified.exited = true,
        }
        assert!(matches!(
            BoundTarget::from_observation(&modified, Source::Codex, SID, UID),
            Err(Error::InvalidBinding)
        ));
    }
}

#[tokio::test]
async fn bound_send_uses_new_envelope_and_exact_guard_ack_without_public_credentials() {
    let (_directory, listener, client, observed, _) = fixture(json!(1), meta()).await;
    let bound = target(&observed);
    let server = tokio::spawn(async move {
        let (mut stream, request) = request(&listener).await;
        assert_eq!(
            request,
            json!({"op":"guarded_v1","token":SECRET,"expected_instance_id":INSTANCE,"expected_source":"codex",
            "expected_sid":SID,"expected_uid":UID,"request":{"op":"send","text":"synthetic input"}})
        );
        reply(&mut stream, json!({"ok":true,"instance_guard":ack()})).await;
    });
    assert!(matches!(
        client
            .request_bound(
                &bound,
                ControlOp::Send {
                    text: "synthetic input".into()
                }
            )
            .await,
        Ok(ControlReply::Ack)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn changed_record_instance_is_rejected_before_any_connection_or_write() {
    let (directory, listener, client, observed, mut record) = fixture(json!(1), meta()).await;
    let bound = target(&observed);
    record["meta"]["instance_id"] = json!("synthetic-instance-replacement");
    tokio::fs::write(
        directory.path().join("synthetic.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .await
    .unwrap();
    assert!(matches!(
        client
            .request_bound(
                &bound,
                ControlOp::Send {
                    text: "must not send".into()
                }
            )
            .await,
        Err(Error::IdentityChanged)
    ));
    assert!(matches!(
        client
            .attach_bound(&bound, TerminalSize::new(80, 24).unwrap(), true)
            .await,
        Err(Error::IdentityChanged)
    ));
    assert!(
        timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn replacement_after_probe_and_unchanged_stale_record_is_rejected_by_guarded_peer() {
    let (_directory, listener, client, observed, _) = fixture(json!(1), meta()).await;
    let bound = target(&observed);
    let server = tokio::spawn(async move {
        let (mut stream, body) = request(&listener).await;
        // The endpoint now belongs to a different instance, while the record
        // still has the old values. Only the receiving host can reject atomically.
        let current_instance = "synthetic-instance-replacement";
        assert_eq!(body["op"], "guarded_v1");
        assert_ne!(body["expected_instance_id"], current_instance);
        reply(&mut stream, json!({"ok":false,"error":"instance mismatch"})).await;
        // No request dispatch or PTY write is performed on the mismatch branch.
        assert!(
            timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
    });
    assert!(matches!(
        client
            .request_bound(
                &bound,
                ControlOp::Send {
                    text: "must not send".into()
                }
            )
            .await,
        Err(Error::Rejected)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn replaced_legacy_peer_rejects_envelope_and_client_never_downgrades() {
    let (_directory, listener, client, observed, _) = fixture(json!(1), meta()).await;
    let bound = target(&observed);
    let server = tokio::spawn(async move {
        let (mut stream, body) = request(&listener).await;
        assert_eq!(body["op"], "guarded_v1");
        reply(&mut stream, json!({"ok":false,"error":"unknown operation"})).await;
        assert!(
            timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
    });
    assert!(matches!(
        client
            .attach_bound(&bound, TerminalSize::new(80, 24).unwrap(), true)
            .await,
        Err(Error::Rejected)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn missing_malformed_or_wrong_guard_ack_is_not_success_and_is_never_retried() {
    for guard in [
        Value::Null,
        json!({"version":1}),
        json!({"version":2,"instance_id":INSTANCE}),
        json!({"version":1,"instance_id":"OTHER_PRIVATE_INSTANCE"}),
        json!({"version":1,"instance_id":INSTANCE,"extra":true}),
    ] {
        let (_directory, listener, client, observed, _) = fixture(json!(1), meta()).await;
        let bound = target(&observed);
        let server = tokio::spawn(async move {
            let (mut stream, _) = request(&listener).await;
            reply(
                &mut stream,
                json!({"ok":true,"instance_guard":guard,"peer_error":SECRET}),
            )
            .await;
            assert!(
                timeout(Duration::from_millis(30), listener.accept())
                    .await
                    .is_err()
            );
        });
        let error = client
            .request_bound(
                &bound,
                ControlOp::Keys {
                    keys: vec!["Enter".into()],
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(error, Error::GuardNotAcknowledged));
        assert!(!format!("{error:?} {error}").contains(SECRET));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn guarded_attach_preserves_coalesced_replay_and_emits_frames_on_same_connection() {
    let (_directory, listener, client, observed, _) = fixture(json!(1), meta()).await;
    let bound = target(&observed);
    let server = tokio::spawn(async move {
        let (mut stream, body) = request(&listener).await;
        assert_eq!(body["op"], "guarded_v1");
        assert_eq!(body["request"]["op"], "attach");
        let mut bytes =
            serde_json::to_vec(&json!({"ok":true,"cols":80,"rows":24,"instance_guard":ack()}))
                .unwrap();
        bytes.push(b'\n');
        bytes.push(1);
        bytes.extend_from_slice(&3u32.to_be_bytes());
        bytes.extend_from_slice(&[b'A', 0xff, b'B']);
        stream.write_all(&bytes).await.unwrap();
        assert_eq!(stream.read_u8().await.unwrap(), 1);
        assert_eq!(stream.read_u32().await.unwrap(), 2);
        let mut input = [0; 2];
        stream.read_exact(&mut input).await.unwrap();
        assert_eq!(&input, b"ok");
    });
    let mut attached = client
        .attach_bound(&bound, TerminalSize::new(80, 24).unwrap(), true)
        .await
        .unwrap();
    assert_eq!(
        attached.reader.next().await.unwrap(),
        Some(HostEvent::Data(vec![b'A', 0xff, b'B']))
    );
    attached.writer.send_data(b"ok").await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn attach_without_guard_ack_drops_connection_before_exposing_replay_or_writable_handle() {
    let (_directory, listener, client, observed, _) = fixture(json!(1), meta()).await;
    let bound = target(&observed);
    let server = tokio::spawn(async move {
        let (mut stream, _) = request(&listener).await;
        stream
            .write_all(b"{\"ok\":true,\"cols\":80,\"rows\":24}\n\x01\x00\x00\x00\x03BAD")
            .await
            .unwrap();
        assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
    });
    assert!(matches!(
        client
            .attach_bound(&bound, TerminalSize::new(80, 24).unwrap(), true)
            .await,
        Err(Error::GuardNotAcknowledged)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn only_declared_native_keys_are_sent_and_operation_timeout_stays_ambiguous() {
    let mut metadata = meta();
    metadata.as_object_mut().unwrap().remove("uid");
    let (_directory, listener, client, observed, _) = fixture(json!(1), metadata).await;
    let bound = target(&observed);
    let server = tokio::spawn(async move {
        let (mut stream, body) = request(&listener).await;
        assert_eq!(body["expected_sid"], SID);
        assert!(body.get("expected_uid").is_none());
        assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
    });
    assert!(matches!(
        client
            .request_bound(
                &bound,
                ControlOp::Paste {
                    text: "synthetic".into(),
                    bracketed: true
                }
            )
            .await,
        Err(Error::Timeout)
    ));
    server.await.unwrap();
}
