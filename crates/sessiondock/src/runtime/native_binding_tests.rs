use super::*;
use crate::sessions::{SessionRoots, SessionStore};
use ptyhost_client::Limits;
use serde_json::json;
use std::{fs, path::Path};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

const INSTANCE: &str = "synthetic-instance-original";
const LAUNCH: &str = "synthetic-launch-original";
const SID: &str = "actual-native-thread";

struct Fixture {
    _native: TempDir,
    hosts: TempDir,
    store: SessionStore,
    uid: String,
}
fn write(path: &Path, records: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        records
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
}
fn header(id: &str, display: &str) -> Value {
    json!({"type":"session_meta","payload":{"id":id,"session_id":display}})
}
impl Fixture {
    fn new() -> Self {
        let native = TempDir::new().unwrap();
        write(
            &native.path().join("file-label.jsonl"),
            &[header(SID, "display-alias")],
        );
        let store = SessionStore::new(SessionRoots {
            codex: Some(native.path().into()),
            ..Default::default()
        });
        let uid = store.list(true).unwrap()["sessions"][0]["uid"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            _native: native,
            hosts: TempDir::new().unwrap(),
            store,
            uid,
        }
    }
    fn binding(&self, sid: &str) -> Value {
        json!({"version":1,"source":"codex","sid":sid,"uid":self.uid,
            "instance_id":INSTANCE,"launch_id":LAUNCH,"method":"operator"})
    }
    fn metadata(&self) -> Value {
        json!({"source":"codex","instance_id":INSTANCE,"launch_id":LAUNCH})
    }
    async fn host(&self, name: &str, binding: Value, metadata: Value) -> JoinHandle<()> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let record = json!({"name":name,"host_pid":10,"pid":11,"created":12,"cols":80,"rows":24,
            "port":listener.local_addr().unwrap().port(),"token":"PRIVATE_SYNTHETIC_TOKEN",
            "meta":metadata,"argv":["PRIVATE_ARGV"]});
        fs::write(
            self.hosts.path().join(format!("{name}.json")),
            record.to_string(),
        )
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
                assert!(bytes.len() < 8192);
            }
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                request,
                json!({"op":"info","token":"PRIVATE_SYNTHETIC_TOKEN"})
            );
            let reply = json!({"ok":true,"info":record,"exited":false,
                "capabilities":{"instance_guard":1,"launch_guard":1,"launch_bind":1},"native_binding":binding});
            stream
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        })
    }
    async fn observe(&self, catalog: &NativeCatalog) -> RuntimeSnapshot {
        let client = HostClient::new(self.hosts.path(), Limits::default()).unwrap();
        ManagedRuntime::new(client, RuntimeLimits::default())
            .unwrap()
            .observe(catalog)
            .await
            .unwrap()
    }
}
fn reason(host: &ManagedHost) -> AssociationReason {
    match host.association {
        NativeAssociation::Unknown { reason } => reason,
        _ => panic!("unexpected native match"),
    }
}

#[tokio::test]
async fn explicit_binding_matches_real_native_id_and_keeps_launch_origin_on_control_target() {
    let fixture = Fixture::new();
    let peer = fixture
        .host("bound", fixture.binding(SID), fixture.metadata())
        .await;
    let result = fixture
        .observe(&fixture.store.native_catalog().unwrap())
        .await;
    peer.await.unwrap();
    let host = &result.hosts[0];
    assert_eq!(host.native_uid(), Some(fixture.uid.as_str()));
    let target = host.bound_target().unwrap();
    assert_eq!(target.sid(), SID);
    assert_eq!(target.instance_id(), INSTANCE);
    assert_eq!(target.origin_launch_id(), Some(LAUNCH));
    let encoded = serde_json::to_string(&result).unwrap();
    assert!(!encoded.contains("PRIVATE_"));
    assert!(!encoded.contains("display-alias"));
    assert_eq!(
        fixture.store.list(false).unwrap()["sessions"][0]["sid"],
        "display-alias"
    );
}

#[tokio::test]
async fn display_alias_binding_and_display_only_catalog_cannot_prove_native_identity() {
    for display_catalog in [false, true] {
        let fixture = Fixture::new();
        let sid = if display_catalog {
            SID
        } else {
            "display-alias"
        };
        let peer = fixture
            .host("bound", fixture.binding(sid), fixture.metadata())
            .await;
        let catalog = if display_catalog {
            // Even forged public rows containing the correct real SID are not
            // a substitute for parsed provenance in the new binding protocol.
            NativeCatalog::from_rows(&[
                json!({"uid":fixture.uid,"source":"codex","sid":SID,"supported":true}),
            ])
        } else {
            fixture.store.native_catalog().unwrap()
        };
        let result = fixture.observe(&catalog).await;
        peer.await.unwrap();
        assert_eq!(
            reason(&result.hosts[0]),
            if display_catalog {
                AssociationReason::UnverifiedCatalog
            } else {
                AssociationReason::NativeConflict
            }
        );
        assert!(result.hosts[0].bound_target().is_none());
    }
}

#[tokio::test]
async fn duplicate_native_files_and_duplicate_hosts_never_offer_a_control_target() {
    let fixture = Fixture::new();
    write(
        &fixture._native.path().join("duplicate.jsonl"),
        &[header(SID, "different-display")],
    );
    let peer = fixture
        .host("bound", fixture.binding(SID), fixture.metadata())
        .await;
    let result = fixture
        .observe(&fixture.store.native_catalog().unwrap())
        .await;
    peer.await.unwrap();
    assert_eq!(reason(&result.hosts[0]), AssociationReason::NativeAmbiguous);
    assert!(result.hosts[0].bound_target().is_none());
    let fixture = Fixture::new();
    let one = fixture
        .host("one", fixture.binding(SID), fixture.metadata())
        .await;
    let two = fixture
        .host("two", fixture.binding(SID), fixture.metadata())
        .await;
    let result = fixture
        .observe(&fixture.store.native_catalog().unwrap())
        .await;
    one.await.unwrap();
    two.await.unwrap();
    assert_eq!(result.hosts.len(), 2);
    for host in result.hosts {
        assert_eq!(reason(&host), AssociationReason::DuplicateHost);
        assert!(host.bound_target().is_none());
    }
}

#[tokio::test]
async fn missing_unsupported_and_subagent_native_entries_fail_explicit_binding() {
    for case in ["missing", "unsupported", "subagent", "conflict"] {
        let fixture = Fixture::new();
        let mut binding = fixture.binding(SID);
        match case {
            "missing" => binding["uid"] = json!("codex:missing-physical-entry"),
            "unsupported" => write(
                &fixture._native.path().join("file-label.jsonl"),
                &[json!({"type":"session_meta","payload":{"session_id":"display-alias"}})],
            ),
            "subagent" => write(
                &fixture._native.path().join("file-label.jsonl"),
                &[
                    json!({"type":"session_meta","payload":{"id":SID,"thread_source":"subagent","parent_thread_id":"absent-parent"}}),
                ],
            ),
            // The bound file's unique identity (its first
            // session_meta) is not the SID the binding claims.
            _ => write(
                &fixture._native.path().join("file-label.jsonl"),
                &[header("conflicting-native-id", "second")],
            ),
        }
        let peer = fixture.host("bound", binding, fixture.metadata()).await;
        let result = fixture
            .observe(&fixture.store.native_catalog().unwrap())
            .await;
        peer.await.unwrap();
        assert_eq!(
            reason(&result.hosts[0]),
            match case {
                "missing" => AssociationReason::NativeMissing,
                "unsupported" => AssociationReason::UnsupportedNative,
                "subagent" => AssociationReason::Subagent,
                _ => AssociationReason::NativeConflict,
            }
        );
        assert!(result.hosts[0].bound_target().is_none());
    }
}

#[tokio::test]
async fn malformed_binding_or_wrong_launch_instance_never_falls_back_to_metadata() {
    for field in ["launch_id", "instance_id", "method", "uid", "legacy_sid"] {
        let fixture = Fixture::new();
        let mut binding = fixture.binding(SID);
        let mut metadata = fixture.metadata();
        if field == "legacy_sid" {
            metadata["sid"] = json!(SID);
            metadata["uid"] = json!(fixture.uid);
        } else {
            binding[field] = json!("synthetic-forged-value");
        }
        let peer = fixture.host("bound", binding, metadata).await;
        let result = fixture
            .observe(&fixture.store.native_catalog().unwrap())
            .await;
        peer.await.unwrap();
        assert_eq!(
            reason(&result.hosts[0]),
            AssociationReason::InvalidMetadata,
            "field {field}"
        );
        assert!(result.hosts[0].bound_target().is_none());
    }
}

#[tokio::test]
async fn new_host_without_explicit_binding_preserves_legacy_native_metadata_semantics() {
    let fixture = Fixture::new();
    for catalog in [
        fixture.store.native_catalog().unwrap(),
        fixture.store.search_snapshot().unwrap().native_catalog(),
    ] {
        let metadata = json!({"source":"codex","instance_id":INSTANCE,"sid":"display-alias","uid":fixture.uid});
        let peer = fixture.host("legacy", Value::Null, metadata).await;
        let result = fixture.observe(&catalog).await;
        peer.await.unwrap();
        let target = result.hosts[0].bound_target().unwrap();
        assert_eq!(target.sid(), "display-alias");
        assert_eq!(target.origin_launch_id(), None);
        assert_eq!(
            catalog.verified_scope(&fixture.uid).unwrap().session_id,
            SID
        );
    }
}

/// A Grok main session has a verified scope (`summary.json`
/// `info.id`), so both a legacy declared metadata host and an explicit
/// binding resolve to it.
#[tokio::test]
async fn legacy_grok_metadata_remains_usable_and_explicit_binding_now_resolves() {
    let mut fixture = Fixture::new();
    let root = fixture._native.path().join("grok");
    let session = root.join("project/session");
    fs::create_dir_all(&session).unwrap();
    fs::write(
        session.join("summary.json"),
        json!({"info":{"id":"grok-display-id"}}).to_string(),
    )
    .unwrap();
    fixture.store = SessionStore::new(SessionRoots {
        grok: Some(root),
        ..Default::default()
    });
    fixture.uid = fixture.store.list(true).unwrap()["sessions"][0]["uid"]
        .as_str()
        .unwrap()
        .into();
    for explicit in [false, true] {
        let catalog = fixture.store.search_snapshot().unwrap().native_catalog();
        let scope = catalog.verified_scope(&fixture.uid).unwrap();
        assert_eq!(scope.source, "grok");
        assert_eq!(scope.session_id, "grok-display-id");
        assert_eq!(scope.uid, fixture.uid);
        let (binding, metadata) = if explicit {
            let mut binding = fixture.binding("grok-display-id");
            binding["source"] = json!("grok");
            (
                binding,
                json!({"source":"grok","instance_id":INSTANCE,"launch_id":LAUNCH}),
            )
        } else {
            (
                Value::Null,
                json!({"source":"grok","instance_id":INSTANCE,"sid":"grok-display-id","uid":fixture.uid}),
            )
        };
        let peer = fixture.host("grok", binding, metadata).await;
        let result = fixture.observe(&catalog).await;
        peer.await.unwrap();
        let target = result.hosts[0].bound_target().unwrap();
        assert_eq!(target.sid(), "grok-display-id");
        assert_eq!(target.uid(), fixture.uid);
        if explicit {
            assert_eq!(target.origin_launch_id(), Some(LAUNCH));
        } else {
            assert_eq!(target.origin_launch_id(), None);
        }
    }
}
