use std::time::Duration;

use ptyhost_client::{AssociationState, Error, HostClient, Limits, Source};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const SECRET: &str = "TEST_ONLY_HOST_CREDENTIAL";

async fn peer(meta: Value) -> (TempDir, TcpListener, HostClient, Value) {
    let directory = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let record = json!({"name":"synthetic","host_pid":10,"pid":11,"created":123,
        "cols":80,"rows":24,"port":listener.local_addr().unwrap().port(),"token":SECRET,
        "meta":meta,"argv":["PRIVATE_COMMAND_ARGUMENT"]});
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
    (directory, listener, client, record)
}

async fn accept(listener: TcpListener) -> TcpStream {
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
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value, json!({"op":"info","token":SECRET}));
    stream
}

async fn reply(stream: &mut TcpStream, record: Value, exited: bool) {
    let mut bytes = serde_json::to_vec(&json!({"ok":true,"info":record,"exited":exited})).unwrap();
    bytes.push(b'\n');
    // JSON control parsing must tolerate arbitrary transport boundaries.
    stream.write_all(&bytes[..9]).await.unwrap();
    tokio::task::yield_now().await;
    stream.write_all(&bytes[9..]).await.unwrap();
}

#[tokio::test]
async fn reviewed_fields_survive_and_all_other_meta_and_credentials_are_dropped() {
    let meta = json!({"source":"claude","sid":"full-session-identifier","uid":"claude:1234567890abcdef",
        "instance_id":"synthetic-instance-0001","secret":"PRIVATE_META","token":SECRET,"argv":["PRIVATE_ARGV"]});
    let (_directory, listener, client, record) = peer(meta).await;
    let server = tokio::spawn(async move {
        let mut stream = accept(listener).await;
        reply(&mut stream, record, false).await;
    });
    let observed = client.probe("synthetic").await.unwrap();
    let AssociationState::Declared(association) = &observed.association else {
        panic!("expected declared association")
    };
    assert_eq!(association.source, Source::Claude);
    assert_eq!(association.sid.as_deref(), Some("full-session-identifier"));
    assert_eq!(
        observed.instance_id.as_deref(),
        Some("synthetic-instance-0001")
    );
    assert!(!observed.exited);
    let encoded = format!("{} {observed:?}", serde_json::to_string(&observed).unwrap());
    for secret in [
        SECRET,
        "PRIVATE_META",
        "PRIVATE_ARGV",
        "PRIVATE_COMMAND_ARGUMENT",
        "\"port\"",
        "\"token\"",
        "\"meta\"",
    ] {
        assert!(!encoded.contains(secret));
    }
    server.await.unwrap();
}

#[tokio::test]
async fn missing_or_bad_association_does_not_hide_an_observable_host() {
    for (meta, valid_missing) in [
        (json!({}), true),
        (json!({"unreviewed":"PRIVATE"}), true),
        (json!({"instance_id":"synthetic-instance-0001"}), true),
        (json!(null), false),
        (json!({"source":"unknown","sid":"full-id"}), false),
        (json!({"source":"codex"}), false),
        (json!({"source":"codex","sid":"../path"}), false),
        (json!({"source":"codex","sid":null}), false),
        (
            json!({"source":"codex","uid":"claude:1234567890abcdef"}),
            false,
        ),
        (
            json!({"source":"codex","sid":"full-id","instance_id":"tiny"}),
            false,
        ),
    ] {
        let (_directory, listener, client, record) = peer(meta).await;
        let server = tokio::spawn(async move {
            let mut stream = accept(listener).await;
            reply(&mut stream, record, true).await;
        });
        let observed = client.probe("synthetic").await.unwrap();
        assert_eq!(
            observed.association,
            if valid_missing {
                AssociationState::Missing
            } else {
                AssociationState::Invalid
            }
        );
        assert!(observed.exited);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn info_identity_must_match_record_not_just_requested_name() {
    for (field, value) in [
        ("name", json!("replacement")),
        ("pid", json!(12)),
        ("host_pid", json!(12)),
        ("created", json!(124)),
        ("port", json!(1)),
        ("token", json!("OTHER_PRIVATE_TOKEN")),
        ("meta", json!({"source":"grok","sid":"other-session"})),
    ] {
        let (_directory, listener, client, mut record) =
            peer(json!({"source":"grok","sid":"full-session"})).await;
        record[field] = value;
        let server = tokio::spawn(async move {
            let mut stream = accept(listener).await;
            reply(&mut stream, record, false).await;
        });
        let error = client.probe("synthetic").await.unwrap_err();
        assert!(matches!(error, Error::IdentityChanged));
        assert!(!format!("{error:?} {error}").contains(SECRET));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn concurrent_record_replacement_or_deletion_invalidates_observation() {
    for delete in [false, true] {
        let (directory, listener, client, record) =
            peer(json!({"source":"codex","sid":"full-session"})).await;
        let path = directory.path().join("synthetic.json");
        let server = tokio::spawn(async move {
            let mut stream = accept(listener).await;
            if delete {
                tokio::fs::remove_file(path).await.unwrap();
            } else {
                let mut replacement = record.clone();
                replacement["created"] = json!(999);
                tokio::fs::write(path, serde_json::to_vec(&replacement).unwrap())
                    .await
                    .unwrap();
            }
            reply(&mut stream, record, false).await;
        });
        assert!(matches!(
            client.probe("synthetic").await,
            Err(Error::IdentityChanged)
        ));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn attached_and_terminal_dimensions_can_change_without_instance_change() {
    let (_directory, listener, client, mut record) = peer(json!({})).await;
    record["attached"] = json!(true);
    record["cols"] = json!(100);
    record["rows"] = json!(40);
    let server = tokio::spawn(async move {
        let mut stream = accept(listener).await;
        reply(&mut stream, record, false).await;
    });
    let observed = client.probe("synthetic").await.unwrap();
    assert_eq!((observed.summary.cols, observed.summary.rows), (100, 40));
    assert!(observed.summary.attached);
    assert!(observed.instance_id.is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn auth_rejection_and_timeout_do_not_leak_credentials_or_claim_exit() {
    let (_directory, listener, client, _record) = peer(json!({})).await;
    let server = tokio::spawn(async move {
        let mut stream = accept(listener).await;
        stream
            .write_all(format!("{{\"ok\":false,\"error\":\"{SECRET}\"}}\n").as_bytes())
            .await
            .unwrap();
    });
    let error = client.probe("synthetic").await.unwrap_err();
    assert!(matches!(error, Error::Rejected));
    assert!(!format!("{error:?} {error}").contains(SECRET));
    server.await.unwrap();
    let (_directory, _listener, client, _record) = peer(json!({})).await;
    assert!(matches!(
        client.probe("synthetic").await,
        Err(Error::Timeout)
    ));
}

/// Executable boundary regression, not a claim that plain attach is bound:
/// an earlier observation must never be accepted as future control authority.
#[tokio::test]
async fn prior_probe_does_not_bind_a_later_name_only_attach_to_that_instance() {
    use ptyhost_client::{HostEvent, TerminalSize};

    let (directory, listener, client, record) = peer(json!({
        "source":"codex","sid":"full-session","instance_id":"synthetic-instance-old"
    }))
    .await;
    let original = record.clone();
    let server = tokio::spawn(async move {
        let mut stream = accept(listener).await;
        reply(&mut stream, original, false).await;
    });
    let observed = client.probe("synthetic").await.unwrap();
    assert_eq!(
        observed.instance_id.as_deref(),
        Some("synthetic-instance-old")
    );
    server.await.unwrap();

    // A replacement can own the same name after a completely valid probe.
    let replacement = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let mut record = record;
    record["port"] = json!(replacement.local_addr().unwrap().port());
    record["meta"]["instance_id"] = json!("synthetic-instance-new");
    record["pid"] = json!(12);
    tokio::fs::write(
        directory.path().join("synthetic.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .await
    .unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = replacement.accept().await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let byte = stream.read_u8().await.unwrap();
            if byte == b'\n' {
                break;
            }
            bytes.push(byte);
        }
        let request: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(request["op"], "attach");
        assert!(request.get("expected_instance_id").is_none());
        let mut reply = b"{\"ok\":true,\"cols\":80,\"rows\":24}\n".to_vec();
        reply.push(1);
        reply.extend_from_slice(&12u32.to_be_bytes());
        reply.extend_from_slice(b"NEW_INSTANCE");
        stream.write_all(&reply).await.unwrap();
    });
    let mut attached = client
        .attach("synthetic", TerminalSize::new(80, 24).unwrap(), true)
        .await
        .unwrap();
    assert_eq!(
        attached.reader.next().await.unwrap(),
        Some(HostEvent::Data(b"NEW_INSTANCE".to_vec()))
    );
    server.await.unwrap();
    // This API is intentionally raw-name transport. The observation above is
    // insufficient: a future bound API must reject before replacement effects.
}

/// Why optional fields plus a post-effect acknowledgement are insufficient:
/// existing peers dispatch on `op` and ignore new JSON keys.
#[tokio::test]
async fn old_peer_can_ignore_expected_instance_on_known_op_but_rejects_new_envelope() {
    for guarded_envelope in [false, true] {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let byte = stream.read_u8().await.unwrap();
                if byte == b'\n' {
                    break;
                }
                bytes.push(byte);
            }
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            // Matches the legacy host's auth -> op dispatch boundary. It has no
            // knowledge of the proposed expected-instance field or inner op.
            let executed = value["op"] == "send";
            let response = if executed {
                json!({"ok":true})
            } else {
                json!({"ok":false,"error":"unknown operation"})
            };
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            stream.write_all(&bytes).await.unwrap();
            executed
        });
        let mut stream = TcpStream::connect(address).await.unwrap();
        let request = if guarded_envelope {
            json!({"op":"guarded_v1","expected_instance_id":"synthetic-instance-old","request":{"op":"send","text":"synthetic input"}})
        } else {
            json!({"op":"send","expected_instance_id":"synthetic-instance-old","text":"synthetic input"})
        };
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        stream.write_all(&bytes).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let response: Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(response["ok"], !guarded_envelope);
        assert_eq!(peer.await.unwrap(), !guarded_envelope);
    }
}

#[tokio::test]
async fn declared_metadata_reads_the_record_without_connecting_or_claiming_liveness() {
    let (directory, listener, client, _record) = peer(json!({"source":"codex",
        "uid":"codex:0123456789abcdef","instance_id":"synthetic-instance-0001","secret":"PRIVATE_META"}))
    .await;
    // Nobody accepts on the listener: a declared read never opens the endpoint.
    let declared = client.declared("synthetic").await.unwrap().unwrap();
    assert_eq!(declared.summary.name, "synthetic");
    assert_eq!(
        declared.instance_id.as_deref(),
        Some("synthetic-instance-0001")
    );
    match &declared.association {
        AssociationState::Declared(identity) => {
            assert_eq!(identity.source, Source::Codex);
            assert_eq!(identity.uid.as_deref(), Some("codex:0123456789abcdef"));
            assert!(identity.sid.is_none());
        }
        other => panic!("{other:?}"),
    }
    let encoded = serde_json::to_string(&declared).unwrap();
    for private in [
        "PRIVATE_META",
        SECRET,
        "PRIVATE_COMMAND_ARGUMENT",
        "\"port\"",
        "\"token\"",
    ] {
        assert!(!encoded.contains(private), "{private}");
    }
    assert!(client.declared("absent").await.unwrap().is_none());
    assert!(matches!(
        client.declared("../x").await,
        Err(Error::InvalidName)
    ));
    tokio::fs::write(directory.path().join("synthetic.json"), b"{not json")
        .await
        .unwrap();
    assert!(matches!(
        client.declared("synthetic").await,
        Err(Error::InvalidMetadata)
    ));
    drop(listener);
}
