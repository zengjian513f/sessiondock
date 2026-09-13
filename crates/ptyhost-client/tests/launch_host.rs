//! Opt-in typed interoperability. Never discovers a binary or native CLI home.
#![cfg(unix)]

use std::{
    io::Write,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use ptyhost_client::{
    AssociationState, AttachReader, ControlOp, ControlReply, Error, HostClient, HostEvent,
    HostObservation, LaunchState, LaunchTarget, Limits, Source, TerminalSize,
};
use serde_json::{Value, json};
use tokio::time::{sleep, timeout};

const NAME: &str = "typed-launch";
const INSTANCE: &str = "synthetic-instance-original";
const LAUNCH: &str = "synthetic-launch-original";
const WRONG_LAUNCH: &str = "synthetic-launch-forged-record";

struct Host {
    child: Child,
    directory: PathBuf,
    metadata: Value,
}

impl Host {
    fn spawn(binary: &Path, directory: &Path, instance: &str, launch: &str) -> Self {
        let metadata = json!({"source":"codex","instance_id":instance,"launch_id":launch});
        let child = Command::new(binary)
            .current_dir(directory)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", "xterm-256color")
            .arg("--dir")
            .arg(directory)
            .arg("run")
            .arg("--cwd")
            .arg(directory)
            .args(["--name", NAME, "--meta", &metadata.to_string(),
                "--cols", "80", "--rows", "24", "--", "/bin/sh", "-c",
                "stty -echo; printf 'TYPED_READY\\n'; while IFS= read -r line; do if [ \"$line\" = bye ]; then printf 'TYPED_DONE\\n'; exit 0; fi; printf '<%s>\\n' \"$line\"; done"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start explicitly selected host with isolated free shell");
        Self {
            child,
            directory: directory.to_owned(),
            metadata,
        }
    }

    async fn observe(&mut self, client: &HostClient) -> HostObservation {
        timeout(Duration::from_secs(5), async {
            loop {
                assert!(
                    self.child.try_wait().unwrap().is_none(),
                    "synthetic host exited early"
                );
                if let Ok(observation) = client.probe(NAME).await {
                    return observation;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("host becomes ready")
    }

    async fn wait(&mut self) {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    assert!(status.success());
                    return;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("synthetic host exits");
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(None)) {
            return;
        }
        // Failure cleanup still pins the exact synthetic launch. Never kill by
        // a discovered name or read an unrelated record. Reap our own child.
        if let Ok(mut stream) = UnixStream::connect(self.directory.join(format!("{NAME}.sock"))) {
            let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
            let request = json!({"op":"launch_guard_v1",
                "expected_instance_id":self.metadata["instance_id"],"expected_source":"codex",
                "expected_launch_id":self.metadata["launch_id"],"request":{"op":"kill","force":true}});
            let _ = writeln!(stream, "{request}");
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

async fn read_until(reader: &mut AttachReader, marker: &[u8]) {
    timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        loop {
            match reader.next().await.unwrap() {
                Some(HostEvent::Data(data)) => bytes.extend(data),
                other => panic!("expected shell output, got {other:?}"),
            }
            assert!(bytes.len() <= 64 * 1024);
            if bytes.windows(marker.len()).any(|window| window == marker) {
                return;
            }
        }
    })
    .await
    .expect("expected shell output arrives");
}

#[tokio::test]
#[ignore = "requires SESSIONDOCK_TEST_PTYHOST_BINARY as explicit absolute built host path; runs only a free /bin/sh"]
async fn typed_pending_launch_guard_attach_exit_and_replacement() {
    let binary = PathBuf::from(
        std::env::var_os("SESSIONDOCK_TEST_PTYHOST_BINARY")
            .expect("set explicit SESSIONDOCK_TEST_PTYHOST_BINARY"),
    );
    assert!(
        binary.is_absolute() && binary.is_file(),
        "host path must be an explicit absolute existing file"
    );
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let client = HostClient::new(directory.path(), Limits::default()).unwrap();
    let mut host = Host::spawn(&binary, directory.path(), INSTANCE, LAUNCH);
    let observation = host.observe(&client).await;
    assert_eq!(observation.association, AssociationState::Invalid);
    let target =
        LaunchTarget::from_observation(&observation, Source::Codex, LAUNCH, INSTANCE).unwrap();
    assert!(matches!(
        client
            .request_launch(&target, ControlOp::Info)
            .await
            .unwrap(),
        ControlReply::Info { exited: false, .. }
    ));

    // A forged disk record can pass the client's local preflight, but cannot
    // change the receiving host's immutable launch identity. Both dangerous
    // control and attach must be rejected there, without killing the child.
    let record_path = directory.path().join(format!("{NAME}.json"));
    let original = std::fs::read(&record_path).unwrap();
    let mut forged_record: Value = serde_json::from_slice(&original).unwrap();
    forged_record["meta"]["launch_id"] = json!(WRONG_LAUNCH);
    std::fs::write(&record_path, serde_json::to_vec(&forged_record).unwrap()).unwrap();
    let mut forged_observation = observation.clone();
    let LaunchState::Declared(identity) = &mut forged_observation.launch else {
        panic!()
    };
    identity.launch_id = WRONG_LAUNCH.to_owned();
    let forged =
        LaunchTarget::from_observation(&forged_observation, Source::Codex, WRONG_LAUNCH, INSTANCE)
            .unwrap();
    let rejected_kill = client
        .request_launch(&forged, ControlOp::Kill { force: true })
        .await;
    let rejected_attach = client
        .attach_launch(&forged, TerminalSize::new(99, 30).unwrap(), true)
        .await;
    std::fs::write(&record_path, original).unwrap();
    assert!(matches!(rejected_kill, Err(Error::Rejected)));
    assert!(matches!(rejected_attach, Err(Error::Rejected)));
    let after_rejection = client.probe(NAME).await.unwrap();
    assert!(!after_rejection.exited);
    assert_eq!(after_rejection.summary.cols, 80);
    assert_eq!(after_rejection.summary.rows, 24);

    let (mut reader, mut writer) = client
        .attach_launch(&target, TerminalSize::new(80, 24).unwrap(), true)
        .await
        .unwrap()
        .into_split();
    read_until(&mut reader, b"TYPED_READY").await;
    writer.send_data(b"typed-byte-input\n").await.unwrap();
    read_until(&mut reader, b"<typed-byte-input>").await;
    writer.send_data(b"bye\n").await.unwrap();
    let tail = timeout(Duration::from_secs(5), async {
        let mut tail = Vec::new();
        loop {
            match reader.next().await.unwrap() {
                Some(HostEvent::Data(data)) => {
                    tail.extend(data);
                    assert!(tail.len() <= 64 * 1024);
                }
                Some(HostEvent::Exit {
                    code: 0,
                    output_complete: Some(true),
                    reason: None,
                }) => return tail,
                other => panic!("expected complete successful exit, got {other:?}"),
            }
        }
    })
    .await
    .unwrap();
    assert!(
        tail.windows(b"TYPED_DONE".len())
            .any(|part| part == b"TYPED_DONE")
    );
    drop((reader, writer));
    host.wait().await;

    let mut replacement = Host::spawn(
        &binary,
        directory.path(),
        "synthetic-instance-replacement",
        "synthetic-launch-replacement",
    );
    let replacement_observation = replacement.observe(&client).await;
    assert!(matches!(
        client
            .request_launch(&target, ControlOp::Kill { force: true })
            .await,
        Err(Error::IdentityChanged)
    ));
    assert!(matches!(
        client
            .attach_launch(&target, TerminalSize::new(80, 24).unwrap(), true)
            .await,
        Err(Error::IdentityChanged)
    ));
    let replacement_target = LaunchTarget::from_observation(
        &replacement_observation,
        Source::Codex,
        "synthetic-launch-replacement",
        "synthetic-instance-replacement",
    )
    .unwrap();
    assert!(matches!(
        client
            .request_launch(&replacement_target, ControlOp::Info)
            .await
            .unwrap(),
        ControlReply::Info { exited: false, .. }
    ));
    client
        .request_launch(
            &replacement_target,
            ControlOp::Send {
                text: "bye\n".into(),
            },
        )
        .await
        .unwrap();
    replacement.wait().await;
}
