use std::path::Path;
use std::time::Duration;

use ptyhost_client::{
    CaptureKind, ControlOp, ControlReply, Error, HostClient, HostEvent, Limits, TerminalSize,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const NAME: &str = "sessiondock-fake";
const TOKEN: &str = "TEST_ONLY_LOCAL_HOST_TOKEN";

fn record(name: &str, port: u16) -> Value {
    json!({
        "name": name, "host_pid": 42, "pid": 43, "created": 123,
        "cwd": "/synthetic", "cmd": "fake", "cols": 80, "rows": 24,
        "attached": false, "backend": "ptyhost", "port": port, "token": TOKEN,
        "argv": ["NEVER_PUBLIC_ARGV"], "meta": {"credential": "NEVER_PUBLIC_META"},
        // A remote-looking extra field must never be used for routing.
        "address": "203.0.113.1"
    })
}

async fn put_record(directory: &Path, name: &str, value: Value) {
    tokio::fs::write(
        directory.join(format!("{name}.json")),
        serde_json::to_vec(&value).unwrap(),
    )
    .await
    .unwrap();
}

async fn tcp_peer(limits: Limits) -> (TempDir, TcpListener, HostClient) {
    let directory = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    put_record(
        directory.path(),
        NAME,
        record(NAME, listener.local_addr().unwrap().port()),
    )
    .await;
    let client = HostClient::new(directory.path(), limits).unwrap();
    (directory, listener, client)
}

async fn read_json<R: AsyncRead + Unpin>(stream: &mut R) -> Value {
    let mut line = Vec::new();
    loop {
        let byte = stream.read_u8().await.unwrap();
        if byte == b'\n' {
            break;
        }
        line.push(byte);
        assert!(line.len() < 8192);
    }
    serde_json::from_slice(&line).unwrap()
}

fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

async fn read_frame<R: AsyncRead + Unpin>(stream: &mut R) -> (u8, Vec<u8>) {
    let kind = stream.read_u8().await.unwrap();
    let length = stream.read_u32().await.unwrap() as usize;
    assert!(length < 8192);
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).await.unwrap();
    (kind, payload)
}

fn size() -> TerminalSize {
    TerminalSize::new(80, 24).unwrap()
}

#[tokio::test]
async fn discovery_is_read_only_and_redacts_private_metadata() {
    let (directory, _listener, client) = tcp_peer(Limits::default()).await;
    // This is an arbitrary synthetic PID, not a real liveness assertion.
    put_record(directory.path(), "other", record("other", 54321)).await;
    put_record(directory.path(), "mismatch", record("wrong-name", 54321)).await;
    tokio::fs::write(directory.path().join("broken.json"), b"not JSON")
        .await
        .unwrap();
    let rows = client.discover().await.unwrap();
    assert_eq!(
        rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        [NAME, "other"]
    );
    assert_eq!(rows[0].server, "ptyhost");
    let public = serde_json::to_string(&rows).unwrap();
    for secret in [
        TOKEN,
        "NEVER_PUBLIC_ARGV",
        "NEVER_PUBLIC_META",
        "token",
        "meta",
        "port",
    ] {
        assert!(
            !public.contains(secret),
            "unexpected private field: {secret}"
        );
    }
    assert!(!format!("{client:?}").contains(TOKEN));
    assert!(directory.path().join("broken.json").exists());
    assert!(directory.path().join("mismatch.json").exists());
    assert!(directory.path().join(format!("{NAME}.json")).exists());
    assert!(client.session("missing").await.unwrap().is_none());
    let missing = HostClient::new(directory.path().join("absent"), Limits::default()).unwrap();
    assert!(missing.discover().await.unwrap().is_empty());
}

#[tokio::test]
async fn invalid_names_and_limits_are_rejected_before_io() {
    let directory = tempfile::tempdir().unwrap();
    let client = HostClient::new(directory.path(), Limits::default()).unwrap();
    for name in [
        "",
        "..",
        "../escape",
        "a/b",
        "a\\b",
        "a:stream",
        "NUL",
        "COM1.json",
        "trailing.",
    ] {
        assert!(matches!(
            client.session(name).await,
            Err(Error::InvalidName)
        ));
    }
    assert!(matches!(TerminalSize::new(0, 1), Err(Error::InvalidSize)));
    assert!(matches!(
        HostClient::new("", Limits::default()),
        Err(Error::InvalidDirectory)
    ));
    let limits = Limits {
        operation_timeout: Duration::ZERO,
        ..Limits::default()
    };
    assert!(matches!(
        HostClient::new(directory.path(), limits),
        Err(Error::InvalidLimits)
    ));
    assert!(matches!(
        client
            .request(
                "missing",
                ControlOp::Rename {
                    to: "../escape".into()
                }
            )
            .await,
        Err(Error::InvalidName)
    ));
}

#[tokio::test]
async fn discovery_has_no_entry_quota_and_keeps_metadata_size_budget() {
    let (directory, _listener, _client) = tcp_peer(Limits::default()).await;
    tokio::fs::write(directory.path().join("second.json"), b"{}")
        .await
        .unwrap();
    let limits = Limits {
        ..Limits::default()
    };
    let client = HostClient::new(directory.path(), limits).unwrap();
    assert_eq!(client.discover().await.unwrap().len(), 1);
    let limits = Limits {
        max_line_bytes: 4,
        ..Limits::default()
    };
    let client = HostClient::new(directory.path(), limits).unwrap();
    assert!(matches!(
        client.session(NAME).await,
        Err(Error::InvalidMetadata)
    ));
    assert!(directory.path().join(format!("{NAME}.json")).exists());
}

#[tokio::test]
async fn control_uses_loopback_token_and_preserves_typed_screen_health() {
    let (_directory, listener, client) = tcp_peer(Limits::default()).await;
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_json(&mut stream).await;
        assert_eq!(
            req,
            json!({"op":"capture", "token": TOKEN,
                              "kind":"screen", "styled":true, "join":false, "lines":0})
        );
        let response = serde_json::to_vec(&json!({"ok":true,"text":"屏幕\u{1b}[0m",
            "cursor":[2,3],"alt":false,"cols":80,"rows":24,"lag":10,"dropped":20,"resets":1}))
        .unwrap();
        // Split JSON across multiple writes, including inside UTF-8.
        for chunk in response.chunks(3) {
            stream.write_all(chunk).await.unwrap();
            tokio::task::yield_now().await;
        }
        stream.write_all(b"\n").await.unwrap();
    });
    let response = client
        .request(
            NAME,
            ControlOp::Capture {
                kind: CaptureKind::Screen,
                styled: true,
                join: false,
                lines: 0,
            },
        )
        .await
        .unwrap();
    let ControlReply::Capture(response) = response else {
        panic!("wrong response");
    };
    assert_eq!(response.text, "屏幕\u{1b}[0m");
    assert_eq!(response.cursor, [2, 3]);
    assert_eq!(
        (response.lag, response.dropped, response.resets),
        (Some(10), Some(20), Some(1))
    );
    peer.await.unwrap();
}

#[tokio::test]
async fn control_operations_have_typed_replies_and_info_never_exposes_token() {
    let (_directory, listener, client) = tcp_peer(Limits::default()).await;
    let port = listener.local_addr().unwrap().port();
    let peer = tokio::spawn(async move {
        for expected in [
            "info", "cursor", "paste", "rename", "resize", "send", "keys", "kill",
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json(&mut stream).await;
            assert_eq!(request["op"], expected);
            assert_eq!(request["token"], TOKEN);
            let reply = match expected {
                "info" => json!({"ok":true,"info":record(NAME,port),"exited":false}),
                "cursor" => json!({"ok":true,"x":2,"y":3,"visible":true,"alt":true,
                                   "lag":0,"dropped":0,"resets":0}),
                "paste" => {
                    assert_eq!(request["bracketed"], true);
                    json!({"ok":true,"bracketed":true})
                }
                "rename" => {
                    assert_eq!(request["to"], "renamed");
                    json!({"ok":true,"name":"renamed"})
                }
                "resize" => {
                    assert_eq!(
                        (request["cols"].as_u64(), request["rows"].as_u64()),
                        (Some(80), Some(24))
                    );
                    json!({"ok":true})
                }
                _ => json!({"ok":true}),
            };
            stream
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        }
    });
    let info = client.request(NAME, ControlOp::Info).await.unwrap();
    let debug = format!("{info:?}");
    assert!(!debug.contains(TOKEN));
    assert!(!debug.contains("NEVER_PUBLIC"));
    assert!(matches!(info, ControlReply::Info { exited: false, .. }));
    assert!(matches!(
        client.request(NAME, ControlOp::Cursor).await.unwrap(),
        ControlReply::Cursor(_)
    ));
    assert_eq!(
        client
            .request(
                NAME,
                ControlOp::Paste {
                    text: "hello".into(),
                    bracketed: true
                }
            )
            .await
            .unwrap(),
        ControlReply::Paste { bracketed: true }
    );
    assert_eq!(
        client
            .request(
                NAME,
                ControlOp::Rename {
                    to: "renamed".into()
                }
            )
            .await
            .unwrap(),
        ControlReply::Renamed {
            name: "renamed".into()
        }
    );
    for operation in [
        ControlOp::Resize { size: size() },
        ControlOp::Send {
            text: "hello".into(),
        },
        ControlOp::Keys {
            keys: vec!["Enter".into()],
        },
        ControlOp::Kill { force: false },
    ] {
        assert_eq!(
            client.request(NAME, operation).await.unwrap(),
            ControlReply::Ack
        );
    }
    peer.await.unwrap();
}

#[tokio::test]
async fn auth_rejection_and_invalid_json_never_echo_peer_secrets() {
    let (_directory, listener, client) = tcp_peer(Limits::default()).await;
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        assert_eq!(read_json(&mut stream).await["token"], TOKEN);
        stream
            .write_all(
                format!("{{\"ok\":false,\"error\":\"bad credential {TOKEN}\"}}\n").as_bytes(),
            )
            .await
            .unwrap();
        let (mut stream, _) = listener.accept().await.unwrap();
        read_json(&mut stream).await;
        stream
            .write_all(format!("{TOKEN}:not-json\n").as_bytes())
            .await
            .unwrap();
    });
    let error = client.request(NAME, ControlOp::Info).await.unwrap_err();
    assert!(matches!(error, Error::Rejected));
    assert!(!format!("{error} {error:?}").contains(TOKEN));
    let error = client.request(NAME, ControlOp::Info).await.unwrap_err();
    assert!(matches!(error, Error::InvalidReply));
    assert!(!format!("{error} {error:?}").contains(TOKEN));
    peer.await.unwrap();
}

#[tokio::test]
async fn tcp_requires_a_nonempty_token_and_nonzero_port() {
    let (directory, listener, client) = tcp_peer(Limits::default()).await;
    for (port, token) in [(listener.local_addr().unwrap().port(), ""), (0, TOKEN)] {
        let mut value = record(NAME, port);
        value["token"] = json!(token);
        put_record(directory.path(), NAME, value).await;
        assert!(matches!(
            client.request(NAME, ControlOp::Info).await,
            Err(Error::InvalidEndpoint)
        ));
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn control_eof_timeout_oversized_and_nonobject_responses_fail() {
    let limits = Limits {
        max_line_bytes: 1024,
        operation_timeout: Duration::from_millis(100),
        ..Limits::default()
    };
    let (_directory, listener, client) = tcp_peer(limits).await;
    let peer = tokio::spawn(async move {
        for reply in [
            Some(b"{\"ok\":".to_vec()),
            Some(vec![b'x'; 1025]),
            Some(b"[]\n".to_vec()),
            None,
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_json(&mut stream).await;
            if let Some(reply) = reply {
                stream.write_all(&reply).await.unwrap();
            } else {
                tokio::time::sleep(Duration::from_millis(180)).await;
            }
        }
    });
    assert!(matches!(
        client.request(NAME, ControlOp::Info).await,
        Err(Error::UnexpectedEof)
    ));
    assert!(matches!(
        client.request(NAME, ControlOp::Info).await,
        Err(Error::LineTooLarge)
    ));
    assert!(matches!(
        client.request(NAME, ControlOp::Info).await,
        Err(Error::InvalidReply)
    ));
    assert!(matches!(
        client.request(NAME, ControlOp::Info).await,
        Err(Error::Timeout)
    ));
    peer.await.unwrap();
}

#[tokio::test]
async fn attach_keeps_coalesced_ack_replay_split_frames_and_exit() {
    let (_directory, listener, client) = tcp_peer(Limits::default()).await;
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        assert_eq!(
            read_json(&mut stream).await,
            json!({"op":"attach", "token":TOKEN,
                                                      "cols":80,"rows":24,"replay":true})
        );
        let mut bytes = b"{\"ok\":true,\"cols\":80,\"rows\":24}\n".to_vec();
        bytes.extend(frame(1, &[0xf0, 0x9f]));
        let next = frame(1, &[0x98, 0x80, 0xff]);
        bytes.extend(&next[..2]);
        stream.write_all(&bytes).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        stream.write_all(&next[2..]).await.unwrap();
        stream.write_all(&frame(3, b"{\"code\":7}")).await.unwrap();
    });
    let mut attachment = client.attach(NAME, size(), true).await.unwrap();
    assert_eq!(attachment.size, size());
    assert_eq!(
        attachment.reader.next().await.unwrap(),
        Some(HostEvent::Data(vec![0xf0, 0x9f]))
    );
    assert_eq!(
        attachment.reader.next().await.unwrap(),
        Some(HostEvent::Data(vec![0x98, 0x80, 0xff]))
    );
    assert_eq!(
        attachment.reader.next().await.unwrap(),
        Some(HostEvent::Exit {
            code: 7,
            output_complete: None,
            reason: None
        })
    );
    assert!(attachment.reader.next().await.unwrap().is_none());
    peer.await.unwrap();
}

#[tokio::test]
async fn exit_completeness_is_typed_and_arbitrary_reasons_are_not_exposed() {
    for (payload, expected) in [
        (
            json!({"code":0,"output_complete":true}),
            Some((Some(true), None)),
        ),
        (
            json!({"code":0,"output_complete":false,"reason":"pty_drain_timeout"}),
            Some((
                Some(false),
                Some(ptyhost_client::ExitReason::PtyDrainTimeout),
            )),
        ),
        (
            json!({"code":0,"output_complete":false,"reason":"pty_read_error"}),
            Some((Some(false), Some(ptyhost_client::ExitReason::PtyReadError))),
        ),
        (
            json!({"code":0,"output_complete":false,"reason":TOKEN}),
            Some((Some(false), Some(ptyhost_client::ExitReason::Unknown))),
        ),
        (
            json!({"code":0,"output_complete":true,"reason":"pty_drain_timeout"}),
            None,
        ),
        (json!({"code":0,"output_complete":"false"}), None),
    ] {
        let (_directory, listener, client) = tcp_peer(Limits::default()).await;
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_json(&mut stream).await;
            let mut bytes = b"{\"ok\":true,\"cols\":80,\"rows\":24}\n".to_vec();
            bytes.extend(frame(3, &serde_json::to_vec(&payload).unwrap()));
            stream.write_all(&bytes).await.unwrap();
        });
        let mut attachment = client.attach(NAME, size(), true).await.unwrap();
        let result = attachment.reader.next().await;
        assert!(!format!("{result:?}").contains(TOKEN));
        if let Some((output_complete, reason)) = expected {
            assert_eq!(
                result.unwrap(),
                Some(HostEvent::Exit {
                    code: 0,
                    output_complete,
                    reason
                })
            );
        } else {
            assert!(matches!(result, Err(Error::InvalidFrame)));
        }
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn attach_input_resize_and_shutdown_are_local_frames_not_websocket() {
    let (_directory, listener, client) = tcp_peer(Limits::default()).await;
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        assert_eq!(read_json(&mut stream).await["replay"], false);
        stream
            .write_all(b"{\"ok\":true,\"cols\":80,\"rows\":24}\n")
            .await
            .unwrap();
        assert_eq!(read_frame(&mut stream).await, (1, vec![0, 0xff, 0x1b]));
        let (kind, payload) = read_frame(&mut stream).await;
        assert_eq!(kind, 2);
        assert_eq!(
            serde_json::from_slice::<Value>(&payload).unwrap(),
            json!({"cols":120,"rows":32})
        );
        assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
    });
    let (_, mut writer) = client
        .attach(NAME, size(), false)
        .await
        .unwrap()
        .into_split();
    writer.send_data(&[0, 0xff, 0x1b]).await.unwrap();
    writer
        .resize(TerminalSize::new(120, 32).unwrap())
        .await
        .unwrap();
    writer.shutdown().await.unwrap();
    assert!(matches!(
        writer.send_data(b"later").await,
        Err(Error::Closed)
    ));
    peer.await.unwrap();
}

#[tokio::test]
async fn attach_rejects_large_unknown_and_truncated_frames() {
    let limits = Limits {
        max_frame_bytes: 16,
        ..Limits::default()
    };
    let (_directory, listener, client) = tcp_peer(limits).await;
    let peer = tokio::spawn(async move {
        for bytes in [
            vec![1, 0, 0, 0, 17],
            vec![99, 0, 0, 0, 0],
            vec![1, 0, 0, 0, 3, b'a'],
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_json(&mut stream).await;
            let mut out = b"{\"ok\":true,\"cols\":80,\"rows\":24}\n".to_vec();
            out.extend(bytes);
            stream.write_all(&out).await.unwrap();
        }
    });
    let mut attachment = client.attach(NAME, size(), false).await.unwrap();
    assert!(matches!(
        attachment.reader.next().await,
        Err(Error::FrameTooLarge)
    ));
    assert!(attachment.reader.next().await.unwrap().is_none());
    let mut attachment = client.attach(NAME, size(), false).await.unwrap();
    assert!(matches!(
        attachment.reader.next().await,
        Err(Error::InvalidFrame)
    ));
    let mut attachment = client.attach(NAME, size(), false).await.unwrap();
    assert!(matches!(
        attachment.reader.next().await,
        Err(Error::UnexpectedEof)
    ));
    peer.await.unwrap();
}

#[tokio::test]
async fn attach_read_cancellation_keeps_partial_header_and_idle_has_no_deadline() {
    let limits = Limits {
        partial_frame_timeout: Some(Duration::from_secs(1)),
        operation_timeout: Duration::from_millis(100),
        ..Limits::default()
    };
    let (_directory, listener, client) = tcp_peer(limits).await;
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_json(&mut stream).await;
        stream
            .write_all(b"{\"ok\":true,\"cols\":80,\"rows\":24}\n\x01\x00")
            .await
            .unwrap();
        resume_rx.await.unwrap();
        stream.write_all(&frame(1, b"safe")[2..]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        stream
            .write_all(&frame(1, b"idle is normal"))
            .await
            .unwrap();
    });
    let mut attachment = client.attach(NAME, size(), false).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), attachment.reader.next())
            .await
            .is_err()
    );
    resume_tx.send(()).unwrap();
    assert_eq!(
        attachment.reader.next().await.unwrap(),
        Some(HostEvent::Data(b"safe".to_vec()))
    );
    assert_eq!(
        attachment.reader.next().await.unwrap(),
        Some(HostEvent::Data(b"idle is normal".to_vec()))
    );
    assert!(attachment.reader.next().await.unwrap().is_none());
    peer.await.unwrap();
}

#[tokio::test]
async fn attach_stalled_partial_frame_times_out_and_rejects_large_input_before_write() {
    let limits = Limits {
        max_frame_bytes: 16,
        partial_frame_timeout: Some(Duration::from_millis(40)),
        ..Limits::default()
    };
    let (_directory, listener, client) = tcp_peer(limits).await;
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_json(&mut stream).await;
        stream
            .write_all(b"{\"ok\":true,\"cols\":80,\"rows\":24}\n\x01")
            .await
            .unwrap();
        assert_eq!(read_frame(&mut stream).await, (1, b"valid".to_vec()));
        tokio::time::sleep(Duration::from_millis(100)).await;
    });
    let mut attachment = client.attach(NAME, size(), false).await.unwrap();
    assert!(matches!(
        attachment.writer.send_data(&[0; 17]).await,
        Err(Error::FrameTooLarge)
    ));
    attachment.writer.send_data(b"valid").await.unwrap();
    assert!(matches!(
        attachment.reader.next().await,
        Err(Error::Timeout)
    ));
    peer.await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn unix_uses_explicit_directory_socket_and_does_not_delete_unknown_pid_records() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(format!("{NAME}.sock"));
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let mut value = record(NAME, 0);
    value.as_object_mut().unwrap().remove("port");
    value.as_object_mut().unwrap().remove("token");
    value["sock"] = json!(path);
    put_record(directory.path(), NAME, value).await;
    let client = HostClient::new(directory.path(), Limits::default()).unwrap();
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_json(&mut stream).await;
        assert_eq!(req["op"], "send");
        assert_eq!(req["token"], "");
        stream.write_all(b"{\"ok\":true}\n").await.unwrap();
    });
    assert_eq!(
        client
            .request(
                NAME,
                ControlOp::Send {
                    text: "fake input".into()
                }
            )
            .await
            .unwrap(),
        ControlReply::Ack
    );
    assert_eq!(client.discover().await.unwrap().len(), 1);
    assert!(path.exists());
    assert!(directory.path().join(format!("{NAME}.json")).exists());
    peer.await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_records_and_symlink_sockets_are_not_followed() {
    let directory = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let external = elsewhere.path().join("record.json");
    tokio::fs::write(&external, serde_json::to_vec(&record(NAME, 54321)).unwrap())
        .await
        .unwrap();
    let metadata = directory.path().join(format!("{NAME}.json"));
    std::os::unix::fs::symlink(&external, &metadata).unwrap();
    let client = HostClient::new(directory.path(), Limits::default()).unwrap();
    assert!(client.discover().await.unwrap().is_empty());
    assert!(matches!(
        client.session(NAME).await,
        Err(Error::InvalidMetadata)
    ));
    assert!(
        metadata
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    tokio::fs::remove_file(&metadata).await.unwrap();
    let external_socket = elsewhere.path().join(format!("{NAME}.sock"));
    let _listener = tokio::net::UnixListener::bind(&external_socket).unwrap();
    let local_socket = directory.path().join(format!("{NAME}.sock"));
    std::os::unix::fs::symlink(&external_socket, &local_socket).unwrap();
    let mut value = record(NAME, 0);
    value.as_object_mut().unwrap().remove("port");
    value["sock"] = json!(external_socket);
    put_record(directory.path(), NAME, value).await;
    assert!(matches!(
        client.request(NAME, ControlOp::Info).await,
        Err(Error::InvalidEndpoint)
    ));
}
