use super::*;
use ptyhost_client::Limits;
use serde_json::json;
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn rows() -> Vec<Value> {
    vec![
        json!({"source":"codex","sid":"complete-native-session-one","uid":"codex:1111111111111111","supported":true}),
        json!({"source":"codex","sid":"complete-native-session-two","uid":"codex:2222222222222222","supported":true}),
    ]
}

fn identity(sid: Option<&str>, uid: Option<&str>) -> Association {
    Association {
        source: Source::Codex,
        sid: sid.map(Into::into),
        uid: uid.map(Into::into),
    }
}

#[test]
fn exact_native_identity_rejects_prefixes_conflicts_duplicates_and_subagents() {
    let catalog = NativeCatalog::from_rows(&rows());
    assert_eq!(
        catalog
            .match_identity(&identity(Some("complete-native-session-one"), None))
            .unwrap()
            .uid,
        "codex:1111111111111111"
    );
    assert_eq!(
        catalog
            .match_identity(&identity(None, Some("codex:1111111111111111")))
            .unwrap()
            .sid,
        "complete-native-session-one"
    );
    for (association, reason) in [
        (
            identity(Some("complete"), None),
            AssociationReason::NativeMissing,
        ),
        (
            identity(
                Some("complete-native-session-one"),
                Some("codex:2222222222222222"),
            ),
            AssociationReason::NativeConflict,
        ),
        (identity(None, None), AssociationReason::MissingMetadata),
    ] {
        assert_eq!(catalog.match_identity(&association).err(), Some(reason));
    }
    let mut duplicate = rows();
    duplicate.push(duplicate[0].clone());
    assert_eq!(
        NativeCatalog::from_rows(&duplicate)
            .match_identity(&identity(Some("complete-native-session-one"), None))
            .err(),
        Some(AssociationReason::NativeAmbiguous)
    );
    for (field, value, reason) in [
        (
            "supported",
            json!(false),
            AssociationReason::UnsupportedNative,
        ),
        ("agent_id", json!("child-id"), AssociationReason::Subagent),
        ("_is_subagent", json!(true), AssociationReason::Subagent),
    ] {
        let mut data = rows();
        data[0][field] = value;
        assert_eq!(
            NativeCatalog::from_rows(&data)
                .match_identity(&identity(Some("complete-native-session-one"), None))
                .err(),
            Some(reason)
        );
    }
}

async fn host(
    directory: &TempDir,
    name: &str,
    meta: Value,
    exited: bool,
) -> tokio::task::JoinHandle<()> {
    host_with_guard(directory, name, meta, exited, false).await
}
async fn host_with_guard(
    directory: &TempDir,
    name: &str,
    meta: Value,
    exited: bool,
    guard: bool,
) -> tokio::task::JoinHandle<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let record = json!({"name":name,"host_pid":123,"pid":124,"created":12,
        "cols":80,"rows":24,"port":listener.local_addr().unwrap().port(),
        "token":"SYNTHETIC_SECRET","meta":meta,"argv":["PRIVATE_ARGUMENT"]});
    tokio::fs::write(
        directory.path().join(format!("{name}.json")),
        serde_json::to_vec(&record).unwrap(),
    )
    .await
    .unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let byte = stream.read_u8().await.unwrap();
            if byte == b'\n' {
                break;
            }
            bytes.push(byte);
        }
        let request: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(request["op"], "info");
        assert_eq!(request["token"], "SYNTHETIC_SECRET");
        let mut reply =
            serde_json::to_vec(&json!({"ok":true,"info":record,"exited":exited,"capabilities":{"instance_guard":if guard {1}else{0}}})).unwrap();
        reply.push(b'\n');
        stream.write_all(&reply).await.unwrap();
    })
}

#[tokio::test]
async fn only_unique_running_guard_capable_instances_offer_a_nonserializable_control_target() {
    let directory = tempfile::tempdir().unwrap();
    let first=host_with_guard(&directory,"one",json!({"source":"codex","sid":"complete-native-session-one","instance_id":"synthetic-instance-0001"}),false,true).await;
    let second=host_with_guard(&directory,"two",json!({"source":"codex","sid":"complete-native-session-two","instance_id":"synthetic-instance-0002"}),false,false).await;
    let snapshot = runtime(&directory)
        .observe(&NativeCatalog::from_rows(&rows()))
        .await
        .unwrap();
    assert_eq!(
        snapshot.hosts[0].bound_target().unwrap().uid(),
        "codex:1111111111111111"
    );
    assert!(snapshot.hosts[1].bound_target().is_none());
    assert!(
        !serde_json::to_string(&snapshot)
            .unwrap()
            .contains("control")
    );
    first.await.unwrap();
    second.await.unwrap();
    let first=host_with_guard(&directory,"one",json!({"source":"codex","sid":"complete-native-session-one","instance_id":"synthetic-instance-0001"}),false,true).await;
    let second=host_with_guard(&directory,"two",json!({"source":"codex","sid":"complete-native-session-one","instance_id":"synthetic-instance-0002"}),false,true).await;
    let snapshot = runtime(&directory)
        .observe(&NativeCatalog::from_rows(&rows()))
        .await
        .unwrap();
    assert!(
        snapshot
            .hosts
            .iter()
            .all(|host| host.bound_target().is_none())
    );
    assert!(snapshot.hosts.iter().all(|host| matches!(
        host.association,
        NativeAssociation::Unknown {
            reason: AssociationReason::DuplicateHost
        }
    )));
    first.await.unwrap();
    second.await.unwrap();
}

/// After a Codex rollback the pane taken over for the parent keeps running
/// the CLI, which now writes the branch's file. The host stays bound to the
/// parent; the branch resolves to it only with process evidence under that
/// host, and never once the process moved on to a deeper branch.
#[tokio::test]
async fn codex_rollback_branch_resolves_to_its_ancestors_host_by_process_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let first=host_with_guard(&directory,"one",json!({"source":"codex","sid":"complete-native-session-one","instance_id":"synthetic-instance-0001"}),false,true).await;
    let snapshot = runtime(&directory)
        .observe(&NativeCatalog::from_rows(&rows()))
        .await
        .unwrap();
    first.await.unwrap();
    let host_pid = snapshot.hosts[0].summary.pid;
    let codex = |uid: &str, sid: &str, parent: &str| procscan::SessionRow {
        uid: format!("codex:{uid}"),
        source: "codex".into(),
        sid: sid.into(),
        path: format!("/home/x/.codex/sessions/{uid}.jsonl"),
        cwd: None,
        created: String::new(),
        forked_from_id: parent.into(),
        continued_in: None,
    };
    let parent = codex("1111111111111111", "complete-native-session-one", "");
    let branch = codex(
        "bbbbbbbbbbbbbbbb",
        "sid-branch",
        "complete-native-session-one",
    );
    let leaf = codex("cccccccccccccccc", "sid-leaf", "sid-branch");
    let stranger = codex("dddddddddddddddd", "sid-stranger", "");
    let temp = tempfile::tempdir().unwrap();
    let proc = procscan::tests::FakeProc::new(&temp.path().join("proc"));
    proc.add(host_pid, "ptyhost", 1, "ptyhost", &[], &[]);
    // The CLI under the host holds the branch file; an outside CLI holds the stranger's.
    proc.add(
        500,
        "codex",
        host_pid,
        "codex resume complete-native-session-one",
        &[],
        &[(3, &branch.path)],
    );
    proc.add(600, "codex", 1, "codex", &[], &[(3, &stranger.path)]);
    let scan = proc.scan();
    let sessions = [parent.clone(), branch.clone(), stranger.clone()];
    let found = snapshot.fork_host(&scan, &sessions, &branch.uid).unwrap();
    assert_eq!(found.summary.name, "one");
    assert_eq!(found.bound_target().unwrap().uid(), parent.uid);
    // The parent binds exactly; a root without ancestors and a foreign process are not forks.
    assert!(snapshot.fork_host(&scan, &sessions, &parent.uid).is_none());
    assert!(
        snapshot
            .fork_host(&scan, &sessions, &stranger.uid)
            .is_none()
    );
    assert!(
        snapshot
            .fork_host(&scan, &sessions, "codex:missing")
            .is_none()
    );
    // Once the process writes a deeper branch the middle branch owns nothing.
    let proc = procscan::tests::FakeProc::new(&temp.path().join("proc2"));
    proc.add(host_pid, "ptyhost", 1, "ptyhost", &[], &[]);
    proc.add(
        500,
        "codex",
        host_pid,
        "codex resume complete-native-session-one",
        &[],
        &[(3, &leaf.path)],
    );
    let scan = proc.scan();
    let sessions = [parent.clone(), branch.clone(), leaf.clone()];
    assert!(snapshot.fork_host(&scan, &sessions, &branch.uid).is_none());
    assert_eq!(
        snapshot
            .fork_host(&scan, &sessions, &leaf.uid)
            .unwrap()
            .summary
            .name,
        "one"
    );
    // A branch whose process left the host is not served by it.
    let proc = procscan::tests::FakeProc::new(&temp.path().join("proc3"));
    proc.add(host_pid, "ptyhost", 1, "ptyhost", &[], &[]);
    proc.add(500, "codex", 1, "codex", &[], &[(3, &leaf.path)]);
    assert!(
        snapshot
            .fork_host(&proc.scan(), &sessions, &leaf.uid)
            .is_none()
    );
}

fn runtime(directory: &TempDir) -> ManagedRuntime {
    ManagedRuntime::new(
        HostClient::new(directory.path(), Limits::default()).unwrap(),
        RuntimeLimits::default(),
    )
    .unwrap()
}

#[tokio::test]
async fn only_info_confirms_running_or_exited_and_global_view_stays_partial() {
    let directory = tempfile::tempdir().unwrap();
    let running = host(&directory, "running", json!({"source":"codex","sid":"complete-native-session-one","instance_id":"synthetic-instance-0001","private":"NO_PUBLIC_META"}), false).await;
    let exited = host(
        &directory,
        "exited",
        json!({"source":"codex","uid":"codex:2222222222222222"}),
        true,
    )
    .await;
    let snapshot = runtime(&directory)
        .observe(&NativeCatalog::from_rows(&rows()))
        .await
        .unwrap();
    assert!(snapshot.known);
    assert!(snapshot.partial);
    let stopped = &snapshot.hosts[0];
    assert_eq!(stopped.liveness, Liveness::Exited);
    assert!(stopped.known);
    assert!(stopped.identity_unverified);
    let running_host = &snapshot.hosts[1];
    assert_eq!(running_host.liveness, Liveness::Running);
    assert!(!running_host.identity_unverified);
    assert_eq!(running_host.native_uid(), Some("codex:1111111111111111"));
    let encoded = serde_json::to_string(&snapshot).unwrap();
    for secret in [
        "SYNTHETIC_SECRET",
        "PRIVATE_ARGUMENT",
        "NO_PUBLIC_META",
        "\"token\"",
        "\"port\"",
    ] {
        assert!(!encoded.contains(secret));
    }
    running.await.unwrap();
    exited.await.unwrap();
}

#[tokio::test]
async fn empty_meta_and_legacy_eight_character_host_names_never_guess() {
    let directory = tempfile::tempdir().unwrap();
    let peer = host(&directory, "sessiondock-codex-complete", json!({}), false).await;
    let snapshot = runtime(&directory)
        .observe(&NativeCatalog::from_rows(&rows()))
        .await
        .unwrap();
    assert_eq!(snapshot.hosts[0].liveness, Liveness::Running);
    assert_eq!(
        snapshot.hosts[0].association,
        NativeAssociation::Unknown {
            reason: AssociationReason::MissingMetadata
        }
    );
    peer.await.unwrap();
}

#[tokio::test]
async fn duplicate_hosts_for_one_native_session_are_both_unassociated() {
    let directory = tempfile::tempdir().unwrap();
    let meta = json!({"source":"codex","sid":"complete-native-session-one"});
    let one = host(&directory, "one", meta.clone(), false).await;
    let two = host(&directory, "two", meta, true).await;
    let snapshot = runtime(&directory)
        .observe(&NativeCatalog::from_rows(&rows()))
        .await
        .unwrap();
    assert!(snapshot.hosts.iter().all(|host| host.association
        == NativeAssociation::Unknown {
            reason: AssociationReason::DuplicateHost
        }));
    one.await.unwrap();
    two.await.unwrap();
}

#[tokio::test]
async fn dead_endpoint_is_unknown_and_record_is_not_removed() {
    let directory = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let path = directory.path().join("unreachable.json");
    tokio::fs::write(&path, serde_json::to_vec(&json!({"name":"unreachable","host_pid":4294967295u32,"pid":4294967295u32,"cols":80,"rows":24,"port":port,"token":"TEST_SECRET","meta":{"source":"codex","sid":"complete-native-session-one"}})).unwrap()).await.unwrap();
    let snapshot = runtime(&directory)
        .observe(&NativeCatalog::from_rows(&rows()))
        .await
        .unwrap();
    assert_eq!(snapshot.hosts[0].liveness, Liveness::Unknown);
    assert!(!snapshot.hosts[0].known);
    let expected = if cfg!(windows) {
        ProbeFailure::Timeout
    } else {
        ProbeFailure::Unreachable
    };
    assert_eq!(snapshot.hosts[0].probe_error, Some(expected));
    assert!(path.exists());
}

#[tokio::test]
async fn total_deadline_bounds_silent_hosts_without_an_inventory_quota() {
    let directory = tempfile::tempdir().unwrap();
    let mut peers = Vec::new();
    for index in 0..3 {
        let name = format!("silent{index}");
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        tokio::fs::write(directory.path().join(format!("{name}.json")), serde_json::to_vec(&json!({"name":name,"host_pid":1,"pid":2,"cols":80,"rows":24,"port":listener.local_addr().unwrap().port(),"token":"TEST"})).unwrap()).await.unwrap();
        peers.push(listener);
    }
    let mut service = runtime(&directory);
    service.limits.parallel_probes = 1;
    service.limits.snapshot_timeout = Duration::from_millis(60);
    let begin = Instant::now();
    let snapshot = service.observe(&NativeCatalog::default()).await.unwrap();
    assert!(begin.elapsed() < Duration::from_secs(1));
    assert!(
        snapshot
            .hosts
            .iter()
            .all(|host| host.probe_error == Some(ProbeFailure::Timeout))
    );
    assert_eq!(
        service
            .observe(&NativeCatalog::default())
            .await
            .unwrap()
            .hosts
            .len(),
        3
    );
}

// ---------------------------------------------------------------------------
// Process identity, three-state sessions, memory and the shared cache.

/// A loopback fake host answering every Info request until aborted. Its
/// record names real or synthetic PIDs; nothing here is a real host process.
async fn serving_host(
    directory: &TempDir,
    name: &str,
    meta: Value,
    exited: bool,
    host_pid: u32,
    pid: u32,
    created: u64,
) -> tokio::task::JoinHandle<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let record = json!({"name":name,"host_pid":host_pid,"pid":pid,"created":created,
        "cols":80,"rows":24,"port":listener.local_addr().unwrap().port(),
        "token":"SYNTHETIC_SECRET","meta":meta,"argv":["PRIVATE_ARGUMENT"]});
    tokio::fs::write(
        directory.path().join(format!("{name}.json")),
        serde_json::to_vec(&record).unwrap(),
    )
    .await
    .unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let record = record.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                loop {
                    let Ok(byte) = stream.read_u8().await else {
                        return;
                    };
                    if byte == b'\n' {
                        break;
                    }
                    bytes.push(byte);
                }
                let mut reply = serde_json::to_vec(&json!({"ok":true,"info":record,"exited":exited,"capabilities":{"instance_guard":1}})).unwrap();
                reply.push(b'\n');
                let _ = stream.write_all(&reply).await;
            });
        }
    })
}

#[cfg(target_os = "linux")]
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(target_os = "linux")]
struct Sleeper(std::process::Child);
#[cfg(target_os = "linux")]
impl Sleeper {
    fn spawn() -> Self {
        Self(
            std::process::Command::new("sleep")
                .arg("60")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
    fn pid(&self) -> u32 {
        self.0.id()
    }
    fn end(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[cfg(target_os = "linux")]
impl Drop for Sleeper {
    fn drop(&mut self) {
        self.end();
    }
}

const UID_ONE: &str = "codex:1111111111111111";
const UID_TWO: &str = "codex:2222222222222222";

fn meta_one(instance: &str) -> Value {
    json!({"source":"codex","sid":"complete-native-session-one","instance_id":instance})
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn running_needs_verified_identity_and_a_vanished_identity_proves_that_instance_ended() {
    let directory = tempfile::tempdir().unwrap();
    let service = runtime(&directory);
    let catalog = NativeCatalog::from_rows(&rows());
    let mut child = Sleeper::spawn();
    let peer = serving_host(
        &directory,
        "one",
        meta_one("synthetic-instance-0001"),
        false,
        std::process::id(),
        child.pid(),
        now_secs(),
    )
    .await;
    let snapshot = service.observe(&catalog).await.unwrap();
    assert!(snapshot.known && snapshot.partial);
    assert_eq!(snapshot.process_identity, "linux_proc");
    let host = &snapshot.hosts[0];
    let ProcessEvidence::Verified {
        child: identity,
        host: host_identity,
    } = &host.process
    else {
        panic!("{:?}", host.process)
    };
    assert_eq!(identity.pid, child.pid());
    assert_eq!(host_identity.pid, std::process::id());
    assert!(host.started_at.is_some_and(|at| at > 1.0e9));
    let session = &snapshot.sessions[UID_ONE];
    assert_eq!(session.state, RunState::Running);
    assert_eq!(session.evidence, Some(RunEvidence::HostInfo));
    assert_eq!(session.pid, Some(child.pid()));
    assert_eq!(session.started_at, host.started_at);
    assert_eq!(snapshot.running_uids(), vec![UID_ONE]);
    assert!(!snapshot.sessions.contains_key(UID_TWO));
    assert_eq!(snapshot.unlisted.state, RunState::Unknown);
    assert_eq!(snapshot.unlisted.reason, UnknownReason::NoInstance);

    // Same record, same host reply, but the identified child is gone: the
    // host claim alone no longer makes the session running.
    child.end();
    let snapshot = service.observe(&catalog).await.unwrap();
    assert_eq!(
        snapshot.hosts[0].process,
        ProcessEvidence::Unverifiable {
            reason: IdentityFailure::NotVisible
        }
    );
    assert_eq!(snapshot.hosts[0].liveness, Liveness::Running);
    let session = &snapshot.sessions[UID_ONE];
    assert_eq!(session.state, RunState::Unknown);
    assert_eq!(session.reason, Some(UnknownReason::IdentityUnverifiable));
    assert_eq!(session.identity, Some(IdentityFailure::NotVisible));
    assert!(snapshot.running_uids().is_empty());

    // The record disappears (host cleanup). The remembered identity is gone
    // from the process table, so that exact instance is known to have ended.
    peer.abort();
    tokio::fs::remove_file(directory.path().join("one.json"))
        .await
        .unwrap();
    for _ in 0..2 {
        let snapshot = service.observe(&catalog).await.unwrap();
        assert!(snapshot.hosts.is_empty());
        let session = &snapshot.sessions[UID_ONE];
        assert_eq!(session.state, RunState::Exited);
        assert_eq!(session.evidence, Some(RunEvidence::IdentityGone));
        assert_eq!(session.host.as_deref(), Some("one"));
        assert_eq!(
            session.instance_id.as_deref(),
            Some("synthetic-instance-0001")
        );
        assert_eq!(session.pid, Some(child.pid()));
    }

    // A replacement instance for the same session supersedes the ended one.
    let replacement = Sleeper::spawn();
    let peer = serving_host(
        &directory,
        "one",
        meta_one("synthetic-instance-0002"),
        false,
        std::process::id(),
        replacement.pid(),
        now_secs(),
    )
    .await;
    let snapshot = service.observe(&catalog).await.unwrap();
    let session = &snapshot.sessions[UID_ONE];
    assert_eq!(session.state, RunState::Running);
    assert_eq!(
        session.instance_id.as_deref(),
        Some("synthetic-instance-0002")
    );
    assert_eq!(lock(&service.memory).entries.len(), 1);
    peer.abort();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_record_that_predates_its_pid_is_a_reused_pid_not_a_running_session() {
    let directory = tempfile::tempdir().unwrap();
    let service = runtime(&directory);
    let child = Sleeper::spawn();
    // The record claims it was created long before this child started.
    let peer = serving_host(
        &directory,
        "stale",
        meta_one("synthetic-instance-0001"),
        false,
        std::process::id(),
        child.pid(),
        now_secs() - 3600,
    )
    .await;
    let snapshot = service
        .observe(&NativeCatalog::from_rows(&rows()))
        .await
        .unwrap();
    assert_eq!(
        snapshot.hosts[0].process,
        ProcessEvidence::Unverifiable {
            reason: IdentityFailure::StartedAfterRecord
        }
    );
    assert_eq!(snapshot.sessions[UID_ONE].state, RunState::Unknown);
    assert!(lock(&service.memory).entries.is_empty());
    peer.abort();
}

#[tokio::test]
async fn synthetic_pids_never_become_running_sessions_and_info_exit_is_exited() {
    let directory = tempfile::tempdir().unwrap();
    let service = runtime(&directory);
    let running = serving_host(
        &directory,
        "one",
        meta_one("synthetic-instance-0001"),
        false,
        u32::MAX,
        u32::MAX - 1,
        12,
    )
    .await;
    let exited = serving_host(
        &directory,
        "two",
        json!({"source":"codex","uid":UID_TWO}),
        true,
        u32::MAX,
        u32::MAX - 1,
        12,
    )
    .await;
    let snapshot = service
        .observe(&NativeCatalog::from_rows(&rows()))
        .await
        .unwrap();
    assert_eq!(snapshot.hosts[0].liveness, Liveness::Running);
    assert!(matches!(
        snapshot.hosts[0].process,
        ProcessEvidence::Unverifiable { .. }
    ));
    assert!(snapshot.hosts[0].started_at.is_none());
    let one = &snapshot.sessions[UID_ONE];
    assert_eq!(one.state, RunState::Unknown);
    assert!(matches!(
        one.reason,
        Some(UnknownReason::IdentityUnverifiable | UnknownReason::PlatformUnsupported)
    ));
    assert_eq!(snapshot.hosts[1].process, ProcessEvidence::Reaped);
    let two = &snapshot.sessions[UID_TWO];
    assert_eq!(two.state, RunState::Exited);
    assert_eq!(two.evidence, Some(RunEvidence::HostExit));
    assert_eq!(two.host.as_deref(), Some("two"));
    assert!(snapshot.running_uids().is_empty());
    let encoded = serde_json::to_string(&snapshot).unwrap();
    assert!(encoded.contains("\"unlisted\":{\"state\":\"unknown\",\"reason\":\"no_instance\"}"));
    assert!(!encoded.contains("SYNTHETIC_SECRET") && !encoded.contains("PRIVATE_ARGUMENT"));
    running.abort();
    exited.abort();
}

#[tokio::test]
async fn duplicate_and_unreachable_hosts_leave_sessions_unknown_with_typed_reasons() {
    let directory = tempfile::tempdir().unwrap();
    let service = runtime(&directory);
    let one = serving_host(
        &directory,
        "one",
        meta_one("synthetic-instance-0001"),
        false,
        u32::MAX,
        u32::MAX - 1,
        12,
    )
    .await;
    let two = serving_host(
        &directory,
        "two",
        meta_one("synthetic-instance-0002"),
        false,
        u32::MAX,
        u32::MAX - 2,
        12,
    )
    .await;
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    tokio::fs::write(directory.path().join("dead.json"), serde_json::to_vec(&json!({"name":"dead","host_pid":4294967295u32,"pid":4294967295u32,"created":12,"cols":80,"rows":24,"port":port,"token":"TEST_SECRET","meta":{"source":"codex","uid":UID_TWO,"instance_id":"synthetic-instance-0003"}})).unwrap()).await.unwrap();
    let snapshot = service
        .observe(&NativeCatalog::from_rows(&rows()))
        .await
        .unwrap();
    let duplicate = &snapshot.sessions[UID_ONE];
    assert_eq!(duplicate.state, RunState::Unknown);
    assert_eq!(duplicate.reason, Some(UnknownReason::DuplicateHost));
    let dead = &snapshot.hosts[0];
    assert_eq!(dead.summary.name, "dead");
    assert_eq!(dead.process, ProcessEvidence::Unchecked);
    assert_eq!(
        dead.declared,
        Some(DeclaredIdentity {
            source: Source::Codex,
            sid: None,
            uid: Some(UID_TWO.into()),
            instance_id: Some("synthetic-instance-0003".into()),
        })
    );
    assert_eq!(dead.native_uid(), None);
    let unreachable = &snapshot.sessions[UID_TWO];
    assert_eq!(unreachable.state, RunState::Unknown);
    assert_eq!(unreachable.reason, Some(UnknownReason::HostUnreachable));
    let expected = if cfg!(windows) {
        ProbeFailure::Timeout
    } else {
        ProbeFailure::Unreachable
    };
    assert_eq!(unreachable.probe_error, Some(expected));
    assert_eq!(
        unreachable.instance_id.as_deref(),
        Some("synthetic-instance-0003")
    );
    assert!(
        !serde_json::to_string(&snapshot)
            .unwrap()
            .contains("TEST_SECRET")
    );
    one.abort();
    two.abort();
}

#[tokio::test]
async fn exit_receipts_apply_only_without_a_different_current_instance() {
    let directory = tempfile::tempdir().unwrap();
    let service = runtime(&directory);
    let catalog = NativeCatalog::from_rows(&rows());
    let receipt = ExitReceipt {
        uid: UID_ONE.into(),
        host: "bound".into(),
        instance_id: "synthetic-instance-0009".into(),
    };
    let snapshot = service
        .observe_with(&catalog, std::slice::from_ref(&receipt))
        .await
        .unwrap();
    let session = &snapshot.sessions[UID_ONE];
    assert_eq!(session.state, RunState::Exited);
    assert_eq!(session.evidence, Some(RunEvidence::ExitReceipt));
    assert_eq!(
        session.instance_id.as_deref(),
        Some("synthetic-instance-0009")
    );
    assert_eq!(session.host.as_deref(), Some("bound"));
    // A newer reachable instance outranks the old receipt even while its
    // identity is unverifiable; the session is unknown, not exited.
    let peer = serving_host(
        &directory,
        "newer",
        meta_one("synthetic-instance-0010"),
        false,
        u32::MAX,
        u32::MAX - 1,
        12,
    )
    .await;
    let snapshot = service
        .observe_with(&catalog, std::slice::from_ref(&receipt))
        .await
        .unwrap();
    assert_eq!(snapshot.sessions[UID_ONE].state, RunState::Unknown);
    peer.abort();
    // An unreachable record of exactly the receipt's instance is stale: the
    // durable receipt wins. A different unreachable instance stays unknown.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    tokio::fs::remove_file(directory.path().join("newer.json"))
        .await
        .unwrap();
    for (instance, expected) in [
        ("synthetic-instance-0009", RunState::Exited),
        ("synthetic-instance-0011", RunState::Unknown),
    ] {
        tokio::fs::write(directory.path().join("bound.json"), serde_json::to_vec(&json!({"name":"bound","host_pid":4294967295u32,"pid":4294967295u32,"created":12,"cols":80,"rows":24,"port":port,"token":"TEST_SECRET","meta":{"source":"codex","sid":"complete-native-session-one","instance_id":instance}})).unwrap()).await.unwrap();
        let snapshot = service
            .observe_with(&catalog, std::slice::from_ref(&receipt))
            .await
            .unwrap();
        assert_eq!(snapshot.sessions[UID_ONE].state, expected, "{instance}");
    }
}

fn candidate(rank: u8, key: Option<&str>, state: RunState, instance: Option<&str>) -> Candidate {
    Candidate {
        rank,
        key: key.map(|name| InstanceKey {
            name: name.into(),
            host_pid: 1,
            pid: 2,
            created: 3,
            instance_id: instance.map(Into::into),
        }),
        state: SessionRunState {
            state,
            evidence: None,
            reason: None,
            host: key.map(Into::into),
            instance_id: instance.map(Into::into),
            pid: None,
            started_at: None,
            probe_error: None,
            identity: None,
        },
    }
}

#[test]
fn precedence_prefers_current_instances_over_memories_and_receipts() {
    let fold = |candidates: Vec<Candidate>| {
        fold_sessions(
            candidates
                .into_iter()
                .map(|c| (UID_ONE.to_owned(), c))
                .collect(),
        )
        .remove(UID_ONE)
        .unwrap()
    };
    // Verified running beats everything.
    let chosen = fold(vec![
        candidate(6, None, RunState::Exited, Some("i1")),
        candidate(0, Some("a"), RunState::Running, Some("i2")),
        candidate(3, Some("b"), RunState::Exited, Some("i0")),
    ]);
    assert_eq!(
        (chosen.state, chosen.instance_id.as_deref()),
        (RunState::Running, Some("i2"))
    );
    // A gone memory yields to an unreachable record of a different instance...
    let chosen = fold(vec![
        candidate(3, Some("a"), RunState::Exited, Some("i1")),
        candidate(4, Some("a"), RunState::Unknown, Some("i2")),
    ]);
    assert_eq!(
        (chosen.state, chosen.instance_id.as_deref()),
        (RunState::Unknown, Some("i2"))
    );
    // ...but not to the same instance's stale record.
    let chosen = fold(vec![
        candidate(3, Some("a"), RunState::Exited, Some("i1")),
        candidate(4, Some("a"), RunState::Unknown, Some("i1")),
    ]);
    assert_eq!(chosen.state, RunState::Exited);
    // A receipt for the unreachable record's exact instance outranks it.
    let chosen = fold(vec![
        candidate(4, Some("a"), RunState::Unknown, Some("i1")),
        candidate(6, None, RunState::Exited, Some("i1")),
    ]);
    assert_eq!(chosen.state, RunState::Exited);
    let chosen = fold(vec![
        candidate(4, Some("a"), RunState::Unknown, Some("i1")),
        candidate(6, None, RunState::Exited, Some("i2")),
    ]);
    assert_eq!(chosen.state, RunState::Unknown);
    // A live process without a record contradicts a receipt: unknown.
    let chosen = fold(vec![
        candidate(5, Some("a"), RunState::Unknown, Some("i1")),
        candidate(6, None, RunState::Exited, Some("i1")),
    ]);
    assert_eq!(chosen.state, RunState::Unknown);
}

#[tokio::test]
async fn shared_observation_is_single_flight_cached_and_force_bypasses_ttl() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let directory = tempfile::tempdir().unwrap();
    let peer = serving_host(
        &directory,
        "one",
        meta_one("synthetic-instance-0001"),
        false,
        u32::MAX,
        u32::MAX - 1,
        12,
    )
    .await;
    let mut service = runtime(&directory);
    service.limits.cache_ttl = Duration::from_millis(400);
    let service = Arc::new(service);
    let prepared = Arc::new(AtomicUsize::new(0));
    let prepare = |gate: Option<Arc<tokio::sync::Notify>>| {
        let prepared = prepared.clone();
        async move || {
            prepared.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = gate {
                gate.notified().await;
            }
            Ok::<_, ()>(((), NativeCatalog::from_rows(&rows()), Vec::new()))
        }
    };
    assert!(service.cached().is_none());
    let first = service.observe_shared(false, prepare(None)).await.unwrap();
    assert!(!first.cached && first.age.is_zero());
    assert_eq!(prepared.load(Ordering::SeqCst), 1);
    let second = service.observe_shared(false, prepare(None)).await.unwrap();
    assert!(second.cached);
    assert!(Arc::ptr_eq(&first.snapshot, &second.snapshot));
    assert_eq!(prepared.load(Ordering::SeqCst), 1);
    let forced = service.observe_shared(true, prepare(None)).await.unwrap();
    assert!(!forced.cached && !Arc::ptr_eq(&first.snapshot, &forced.snapshot));
    assert_eq!(prepared.load(Ordering::SeqCst), 2);
    // Concurrent refreshes: the second waiter reuses the first refresh.
    tokio::time::sleep(Duration::from_millis(450)).await;
    assert!(service.cached().is_none());
    let gate = Arc::new(tokio::sync::Notify::new());
    let leader = {
        let service = service.clone();
        let prepare = prepare(Some(gate.clone()));
        tokio::spawn(async move { service.observe_shared(false, prepare).await.unwrap() })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(prepared.load(Ordering::SeqCst), 3);
    let follower = {
        let service = service.clone();
        let prepare = prepare(None);
        tokio::spawn(async move { service.observe_shared(false, prepare).await.unwrap() })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(prepared.load(Ordering::SeqCst), 3);
    gate.notify_one();
    let (leader, follower) = (leader.await.unwrap(), follower.await.unwrap());
    assert!(!leader.cached && follower.cached);
    assert!(Arc::ptr_eq(&leader.snapshot, &follower.snapshot));
    assert_eq!(prepared.load(Ordering::SeqCst), 3);
    // A failed preparation releases the single flight and caches nothing new.
    tokio::time::sleep(Duration::from_millis(450)).await;
    let failed = service
        .observe_shared(false, async || {
            Err::<((), NativeCatalog, Vec<ExitReceipt>), &str>("busy")
        })
        .await;
    assert!(matches!(failed, Err(SharedError::Prepare("busy"))));
    assert!(service.cached().is_none());
    peer.abort();
}
