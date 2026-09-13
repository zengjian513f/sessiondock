use std::time::Duration;

use ptyhost_client::{
    AssociationState, BoundTarget, ControlOp, ControlReply, Error, HostClient, HostEvent,
    HostObservation, LaunchState, LaunchTarget, Limits, Source, TerminalSize,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};

const INSTANCE: &str = "synthetic-instance-original";
const LAUNCH: &str = "synthetic-launch-original";
const SECRET: &str = "PRIVATE_LAUNCH_HOST_TOKEN";

#[tokio::test]
async fn exited_launch_status_requires_guard_ack_and_never_grants_writable_target() {
    for acknowledge in [false, true] {
        let (_directory, listener, client, record) = peer(meta()).await;
        let server = tokio::spawn(async move {
            let (mut stream, body) = request(&listener).await;
            assert_eq!(body["op"], "info");
            reply(
                &mut stream,
                json!({"ok":true,"info":record,"exited":true,
                "capabilities":{"launch_guard":1}}),
            )
            .await;
            let (mut stream, body) = request(&listener).await;
            assert_eq!(body["op"], "launch_guard_v1");
            assert_eq!(body["request"], json!({"op":"info"}));
            assert_eq!(body["expected_launch_id"], LAUNCH);
            let mut value =
                json!({"ok":true,"info":record,"exited":true,"capabilities":{"launch_guard":1}});
            if acknowledge {
                value["launch_guard"] = ack();
            }
            reply(&mut stream, value).await;
        });
        let status = client
            .status_launch("pending", Source::Codex, LAUNCH, INSTANCE)
            .await;
        if acknowledge {
            let status = status.unwrap();
            assert!(status.exited);
            assert!(matches!(
                LaunchTarget::from_observation(&status, Source::Codex, LAUNCH, INSTANCE),
                Err(Error::InvalidLaunchBinding)
            ));
        } else {
            assert!(matches!(status, Err(Error::LaunchGuardNotAcknowledged)));
        }
        server.await.unwrap();
    }
}

fn meta() -> Value {
    json!({"source":"codex","instance_id":INSTANCE,"launch_id":LAUNCH})
}
fn ack() -> Value {
    json!({"version":1,"instance_id":INSTANCE,"source":"codex","launch_id":LAUNCH})
}

fn native_binding() -> Value {
    json!({"version":1,"source":"codex","instance_id":INSTANCE,"launch_id":LAUNCH,
        "sid":"native-thread","uid":"codex:verified-native","method":"operator"})
}

#[tokio::test]
async fn guarded_status_uses_binding_from_guarded_reply_not_earlier_probe() {
    use ptyhost_client::NativeBindingState;
    let (_directory, listener, client, record) = peer(meta()).await;
    let server = tokio::spawn(async move {
        for binding in [Value::Null, native_binding()] {
            let (mut stream, body) = request(&listener).await;
            let mut reply_value = json!({"ok":true,"info":record,"exited":false,
                "capabilities":{"instance_guard":1,"launch_guard":1,"launch_bind":1},"native_binding":binding});
            if body["op"] == "launch_guard_v1" {
                reply_value["launch_guard"] = ack();
            }
            reply(&mut stream, reply_value).await;
        }
    });
    let status = client
        .status_launch("pending", Source::Codex, LAUNCH, INSTANCE)
        .await
        .unwrap();
    let NativeBindingState::Bound(binding) = &status.native_binding else {
        panic!("guarded state missing")
    };
    assert_eq!(binding.sid(), "native-thread");
    assert_eq!(status.association, AssociationState::Invalid);
    let bound = BoundTarget::from_observation(
        &status,
        Source::Codex,
        "native-thread",
        "codex:verified-native",
    )
    .unwrap();
    assert_eq!(bound.origin_launch_id(), Some(LAUNCH));
    assert!(
        BoundTarget::from_observation(
            &status,
            Source::Codex,
            "display-alias",
            "codex:verified-native"
        )
        .is_err()
    );
    server.await.unwrap();
}

#[tokio::test]
async fn malformed_capability_binding_and_predeclared_metadata_cannot_form_native_target() {
    use ptyhost_client::NativeBindingState;
    let mut cases = vec![
        (json!(true), native_binding(), meta()),
        (json!(2), native_binding(), meta()),
        (json!(1), json!({}), meta()),
        (json!(1), json!({"sid":"short"}), meta()),
    ];
    for (field, value) in [
        ("version", json!(2)),
        ("instance_id", json!("wrong")),
        ("source", json!("claude")),
        ("launch_id", json!("wrong")),
        ("method", json!("automatic")),
        ("uid", json!("claude:foreign")),
        ("sid", json!("../bad")),
        ("secret", json!(SECRET)),
    ] {
        let mut binding = native_binding();
        binding[field] = value;
        cases.push((json!(1), binding, meta()));
    }
    for field in ["sid", "uid"] {
        let mut metadata = meta();
        metadata[field] = Value::Null;
        cases.push((json!(1), native_binding(), metadata));
    }
    for (capability, binding, metadata) in cases {
        let (_directory, listener, client, record) = peer(metadata).await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = request(&listener).await;
            reply(&mut stream,json!({"ok":true,"info":record,"exited":false,
                "capabilities":{"instance_guard":1,"launch_guard":1,"launch_bind":capability},"native_binding":binding})).await;
        });
        let observed = client.probe("pending").await.unwrap();
        assert_eq!(observed.native_binding, NativeBindingState::Invalid);
        assert!(
            BoundTarget::from_observation(
                &observed,
                Source::Codex,
                "native-thread",
                "codex:verified-native"
            )
            .is_err()
        );
        assert!(!format!("{observed:?}").contains(SECRET));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn bind_is_a_distinct_outer_operation_and_never_retries_a_missing_ack() {
    for acknowledge in [false, true] {
        let (_directory, listener, client, observed, record) = fixture(json!(1), meta()).await;
        let target = target(&observed);
        let server = tokio::spawn(async move {
            for op in ["info", "launch_guard_v1", "launch_bind_v1"] {
                let (mut stream, body) = request(&listener).await;
                assert_eq!(body["op"], op);
                if op == "launch_bind_v1" {
                    assert_eq!(
                        body["native"],
                        json!({"sid":"native-thread","uid":"codex:verified-native"})
                    );
                    assert_eq!(body["expected_launch_id"], LAUNCH);
                    assert!(body.get("request").is_none());
                    let mut result = json!({"ok":true,"native_binding":native_binding()});
                    if acknowledge {
                        result["launch_guard"] = ack();
                    }
                    reply(&mut stream, result).await;
                } else {
                    let mut result = json!({"ok":true,"info":record,"exited":false,
                        "capabilities":{"instance_guard":1,"launch_guard":1,"launch_bind":1},"native_binding":null});
                    if op == "launch_guard_v1" {
                        result["launch_guard"] = ack();
                    }
                    reply(&mut stream, result).await;
                }
            }
            assert!(
                timeout(Duration::from_millis(30), listener.accept())
                    .await
                    .is_err()
            );
        });
        let result = client
            .bind_launch(&target, "native-thread", "codex:verified-native")
            .await;
        if acknowledge {
            assert_eq!(result.unwrap().uid(), "codex:verified-native");
        } else {
            assert!(matches!(result, Err(Error::LaunchGuardNotAcknowledged)));
        }
        server.await.unwrap();
    }
}
fn target(observed: &HostObservation) -> LaunchTarget {
    LaunchTarget::from_observation(observed, Source::Codex, LAUNCH, INSTANCE).unwrap()
}
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
async fn peer(metadata: Value) -> (TempDir, TcpListener, HostClient, Value) {
    let directory = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let record = json!({"name":"pending","host_pid":10,"pid":11,"created":12,"cols":80,"rows":24,
        "port":listener.local_addr().unwrap().port(),"token":SECRET,"meta":metadata,
        "argv":["PRIVATE_COMMAND_ARGUMENT"]});
    tokio::fs::write(
        directory.path().join("pending.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .await
    .unwrap();
    let client = HostClient::new(
        directory.path(),
        Limits {
            operation_timeout: Duration::from_millis(200),
            ..Default::default()
        },
    )
    .unwrap();
    (directory, listener, client, record)
}
async fn fixture(
    capability: Value,
    metadata: Value,
) -> (TempDir, TcpListener, HostClient, HostObservation, Value) {
    let (directory, listener, client, record) = peer(metadata).await;
    let server = {
        let record = record.clone();
        tokio::spawn(async move {
            let (mut stream, body) = request(&listener).await;
            assert_eq!(body, json!({"op":"info","token":SECRET}));
            reply(
                &mut stream,
                json!({"ok":true,"info":record,"exited":false,
                "capabilities":{"instance_guard":1,"launch_guard":capability}}),
            )
            .await;
            listener
        })
    };
    let observed = client.probe("pending").await.unwrap();
    (directory, server.await.unwrap(), client, observed, record)
}

#[tokio::test]
async fn pending_launch_is_independent_of_native_association_and_drops_unreviewed_secrets() {
    let mut metadata = meta();
    metadata["secret"] = json!("PRIVATE_META_SECRET");
    metadata["token"] = json!(SECRET);
    let (_directory, _listener, _client, observed, _) = fixture(json!(1), metadata).await;
    assert_eq!(observed.association, AssociationState::Invalid); // Existing source-only semantics.
    let LaunchState::Declared(identity) = &observed.launch else {
        panic!("missing launch identity");
    };
    assert_eq!(identity.source, Source::Codex);
    assert_eq!(identity.launch_id, LAUNCH);
    assert!(observed.launch_guard_v1);
    let target = target(&observed);
    assert_eq!(
        (
            target.name(),
            target.source(),
            target.launch_id(),
            target.instance_id()
        ),
        ("pending", Source::Codex, LAUNCH, INSTANCE)
    );
    assert!(matches!(
        BoundTarget::from_observation(&observed, Source::Codex, "fake-sid", "codex:fake"),
        Err(Error::InvalidBinding)
    ));
    let encoded = format!("{} {observed:?}", serde_json::to_string(&observed).unwrap());
    for secret in [
        SECRET,
        "PRIVATE_META_SECRET",
        "PRIVATE_COMMAND_ARGUMENT",
        "\"token\"",
        "\"port\"",
        "\"argv\"",
    ] {
        assert!(!encoded.contains(secret));
    }
    let mut invalid_launch = meta();
    invalid_launch["launch_id"] = json!("tiny");
    invalid_launch["sid"] = json!("real-native-sid");
    let (_directory, _listener, _client, observed, _) = fixture(json!(1), invalid_launch).await;
    assert_eq!(observed.launch, LaunchState::Invalid);
    assert!(matches!(
        observed.association,
        AssociationState::Declared(_)
    ));
}

#[tokio::test]
async fn launch_binding_requires_exact_integer_capability_nonces_source_and_liveness() {
    for capability in [
        json!(null),
        json!(false),
        json!(true),
        json!("1"),
        json!(1.0),
        json!(0),
        json!(2),
    ] {
        let (_directory, _listener, _client, observed, _) = fixture(capability, meta()).await;
        assert!(!observed.launch_guard_v1);
        assert!(matches!(
            LaunchTarget::from_observation(&observed, Source::Codex, LAUNCH, INSTANCE),
            Err(Error::LaunchGuardUnsupported)
        ));
    }
    let (_directory, _listener, _client, observed, _) = fixture(json!(1), meta()).await;
    for (source, launch, instance) in [
        (Source::Claude, LAUNCH, INSTANCE),
        (Source::Codex, "synthetic-launch-other", INSTANCE),
        (Source::Codex, LAUNCH, "synthetic-instance-other"),
        (Source::Codex, "synthetic-launch", INSTANCE),
        (Source::Codex, LAUNCH, "tiny"),
    ] {
        assert!(matches!(
            LaunchTarget::from_observation(&observed, source, launch, instance),
            Err(Error::InvalidLaunchBinding)
        ));
    }
    let mut exited = observed.clone();
    exited.exited = true;
    assert!(matches!(
        LaunchTarget::from_observation(&exited, Source::Codex, LAUNCH, INSTANCE),
        Err(Error::InvalidLaunchBinding)
    ));
    for metadata in [
        json!({}),
        json!(null),
        json!({"source":"codex","launch_id":LAUNCH}),
        json!({"source":"codex","launch_id":LAUNCH,"instance_id":"tiny"}),
        json!({"source":"codex","launch_id":42,"instance_id":INSTANCE}),
        json!({"source":"codex","launch_id":"tiny","instance_id":INSTANCE}),
        json!({"source":"codex","launch_id":"x".repeat(129),"instance_id":INSTANCE}),
        json!({"source":"unknown","launch_id":LAUNCH,"instance_id":INSTANCE}),
        json!({"source":"codex","launch_id":"../invalid-launch-id","instance_id":INSTANCE}),
    ] {
        let (_directory, _listener, _client, observed, _) = fixture(json!(1), metadata).await;
        assert!(!matches!(observed.launch, LaunchState::Declared(_)));
        assert!(matches!(
            LaunchTarget::from_observation(&observed, Source::Codex, LAUNCH, INSTANCE),
            Err(Error::InvalidLaunchBinding)
        ));
    }
}

#[tokio::test]
async fn pending_requests_have_only_the_launch_envelope_and_exact_ack() {
    let (_directory, listener, client, observed, _) = fixture(json!(1), meta()).await;
    let target = target(&observed);
    let server = tokio::spawn(async move {
        let (mut stream, body) = request(&listener).await;
        assert_eq!(
            body,
            json!({"op":"launch_guard_v1","token":SECRET,"expected_instance_id":INSTANCE,
            "expected_source":"codex","expected_launch_id":LAUNCH,"request":{"op":"kill","force":false}})
        );
        reply(&mut stream, json!({"ok":true,"launch_guard":ack()})).await;
    });
    assert!(matches!(
        client
            .request_launch(&target, ControlOp::Kill { force: false })
            .await,
        Ok(ControlReply::Ack)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn launch_record_replacement_is_rejected_before_connecting_for_request_or_attach() {
    for (field, changed) in [
        ("launch_id", "synthetic-launch-other"),
        ("instance_id", "synthetic-instance-other"),
        ("source", "grok"),
    ] {
        let (directory, listener, client, observed, mut record) = fixture(json!(1), meta()).await;
        let target = target(&observed);
        record["meta"][field] = json!(changed);
        tokio::fs::write(
            directory.path().join("pending.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .await
        .unwrap();
        assert!(matches!(
            client
                .request_launch(
                    &target,
                    ControlOp::Send {
                        text: "never send".into()
                    }
                )
                .await,
            Err(Error::IdentityChanged)
        ));
        assert!(matches!(
            client
                .attach_launch(&target, TerminalSize::new(80, 24).unwrap(), true)
                .await,
            Err(Error::IdentityChanged)
        ));
        assert!(
            timeout(Duration::from_millis(25), listener.accept())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn probe_checks_launch_metadata_in_both_reply_and_finishing_record() {
    for change_disk in [false, true] {
        let (directory, listener, client, record) = peer(meta()).await;
        let path = directory.path().join("pending.json");
        let server = tokio::spawn(async move {
            let (mut stream, _) = request(&listener).await;
            let mut changed = record.clone();
            changed["meta"]["launch_id"] = json!("synthetic-launch-other");
            let sent = if change_disk {
                tokio::fs::write(path, serde_json::to_vec(&changed).unwrap())
                    .await
                    .unwrap();
                record
            } else {
                changed
            };
            reply(
                &mut stream,
                json!({"ok":true,"info":sent,"exited":false,"capabilities":{"launch_guard":1}}),
            )
            .await;
        });
        assert!(matches!(
            client.probe("pending").await,
            Err(Error::IdentityChanged)
        ));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn missing_bad_or_wrong_launch_ack_never_succeeds_or_retries() {
    for guard in [
        Value::Null,
        json!({}),
        json!({"version":1,"instance_id":INSTANCE,"launch_id":LAUNCH}),
        json!({"version":2,"instance_id":INSTANCE,"source":"codex","launch_id":LAUNCH}),
        json!({"version":1,"instance_id":"other-instance","source":"codex","launch_id":LAUNCH}),
        json!({"version":1,"instance_id":INSTANCE,"source":"grok","launch_id":LAUNCH}),
        json!({"version":1,"instance_id":INSTANCE,"source":"codex","launch_id":"other-launch"}),
        json!({"version":1,"instance_id":INSTANCE,"source":"codex","launch_id":LAUNCH,"secret":SECRET}),
    ] {
        let (_directory, listener, client, observed, _) = fixture(json!(1), meta()).await;
        let target = target(&observed);
        let server = tokio::spawn(async move {
            let (mut stream, _) = request(&listener).await;
            reply(
                &mut stream,
                json!({"ok":true,"launch_guard":guard,"error":SECRET}),
            )
            .await;
            assert!(
                timeout(Duration::from_millis(25), listener.accept())
                    .await
                    .is_err()
            );
        });
        let error = client
            .request_launch(
                &target,
                ControlOp::Keys {
                    keys: vec!["Enter".into()],
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(error, Error::LaunchGuardNotAcknowledged));
        assert!(!format!("{error:?} {error}").contains(SECRET));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn launch_attach_preserves_coalesced_replay_and_raw_input_after_verified_ack() {
    let (_directory, listener, client, observed, _) = fixture(json!(1), meta()).await;
    let target = target(&observed);
    let server = tokio::spawn(async move {
        let (mut stream, body) = request(&listener).await;
        assert_eq!(body["op"], "launch_guard_v1");
        assert_eq!(body["expected_launch_id"], LAUNCH);
        assert_eq!(
            body["request"],
            json!({"op":"attach","cols":80,"rows":24,"replay":true})
        );
        let mut bytes =
            serde_json::to_vec(&json!({"ok":true,"cols":80,"rows":24,"launch_guard":ack()}))
                .unwrap();
        bytes.extend_from_slice(b"\n\x01\x00\x00\x00\x03A\xffB");
        stream.write_all(&bytes).await.unwrap();
        assert_eq!(stream.read_u8().await.unwrap(), 1);
        assert_eq!(stream.read_u32().await.unwrap(), 2);
        let mut input = [0; 2];
        stream.read_exact(&mut input).await.unwrap();
        assert_eq!(&input, b"ok");
    });
    let mut attached = client
        .attach_launch(&target, TerminalSize::new(80, 24).unwrap(), true)
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
async fn launch_attach_without_exact_ack_exposes_neither_replay_nor_writer() {
    let (_directory, listener, client, observed, _) = fixture(json!(1), meta()).await;
    let target = target(&observed);
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
            .attach_launch(&target, TerminalSize::new(80, 24).unwrap(), true)
            .await,
        Err(Error::LaunchGuardNotAcknowledged)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn replacement_or_legacy_peer_rejection_never_downgrades_and_redacts_peer_text() {
    let (_directory, listener, client, observed, _) = fixture(json!(1), meta()).await;
    let target = target(&observed);
    let server = tokio::spawn(async move {
        let (mut stream, body) = request(&listener).await;
        assert_eq!(body["op"], "launch_guard_v1");
        reply(&mut stream, json!({"ok":false,"error":SECRET})).await;
        assert!(
            timeout(Duration::from_millis(25), listener.accept())
                .await
                .is_err()
        );
    });
    let error = client
        .request_launch(
            &target,
            ControlOp::Send {
                text: "not authorized on replacement".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Rejected));
    assert!(!format!("{error:?} {error}").contains(SECRET));
    server.await.unwrap();
}

#[tokio::test]
async fn launch_write_timeout_and_eof_are_ambiguous_without_retry() {
    for close in [false, true] {
        let (_directory, listener, client, observed, _) = fixture(json!(1), meta()).await;
        let target = target(&observed);
        let server = tokio::spawn(async move {
            let (mut stream, body) = request(&listener).await;
            assert_eq!(body["request"]["op"], "send");
            if close {
                drop(stream);
            } else {
                assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
            }
            assert!(
                timeout(Duration::from_millis(25), listener.accept())
                    .await
                    .is_err()
            );
        });
        let error = client
            .request_launch(
                &target,
                ControlOp::Send {
                    text: "possibly applied once".into(),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Timeout | Error::UnexpectedEof));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn guarded_info_cannot_forge_launch_identity_inside_a_correct_outer_ack() {
    let (_directory, listener, client, observed, mut record) = fixture(json!(1), meta()).await;
    let target = target(&observed);
    record["meta"]["launch_id"] = json!("synthetic-launch-forged");
    let server = tokio::spawn(async move {
        let (mut stream, _) = request(&listener).await;
        reply(
            &mut stream,
            json!({"ok":true,"info":record,"exited":false,"launch_guard":ack()}),
        )
        .await;
    });
    assert!(matches!(
        client.request_launch(&target, ControlOp::Info).await,
        Err(Error::IdentityChanged)
    ));
    server.await.unwrap();
}
