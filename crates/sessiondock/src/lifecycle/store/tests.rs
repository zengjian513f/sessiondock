use super::*;
use crate::lifecycle::model::{BindingMethod, Source};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

struct Fixture {
    _temp: tempfile::TempDir,
    ledger: std::path::PathBuf,
    cwd: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let ledger = temp.path().join("ledger");
        let cwd = temp.path().join("work");
        fs::create_dir(&ledger).unwrap();
        fs::create_dir(&cwd).unwrap();
        fs::set_permissions(&ledger, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _temp: temp,
            ledger,
            cwd,
        }
    }
    fn store(&self) -> LifecycleStore {
        LifecycleStore::initialize(&self.ledger).unwrap()
    }
    fn spec(&self) -> LaunchSpec {
        LaunchSpec::new(Source::Claude, "claude-v1".into(), &self.cwd).unwrap()
    }
    fn bytes(&self) -> Vec<u8> {
        fs::read(self.ledger.join(LEDGER_FILENAME)).unwrap()
    }
}

/// Downgrade a current-schema row to what an older ledger held: the schema-5
/// fields on the record and, when a binding object exists, on the binding.
fn strip_schema5(row: &mut serde_json::Value) {
    let fields = row.as_object_mut().unwrap();
    for key in ["created_at", "finished_at", "discarded"] {
        fields.remove(key);
    }
    if let Some(binding) = fields
        .get_mut("binding")
        .and_then(serde_json::Value::as_object_mut)
    {
        for key in ["method", "evidence", "bound_at"] {
            binding.remove(key);
        }
    }
}

#[test]
fn intent_and_start_are_durable_before_authority_and_replay_does_not_authorize() {
    let f = Fixture::new();
    let mut store = f.store();
    let spec = f.spec();
    let created = store.create("request-one", &spec).unwrap();
    let id = created.record.record_id().to_owned();
    let disk = json::decode(&f.bytes()).unwrap();
    assert!(disk.records[&id] == created.record);
    assert_eq!(created.record.state(), State::Prepared);
    assert!(model::nonce(created.record.launch_id()));
    assert!(model::nonce(created.record.instance_id()));
    assert!(created.record.host_name().starts_with("sessiondock-"));
    let before = f.bytes();
    let replay = store.create("request-one", &spec).unwrap();
    assert!(replay.record == created.record);
    assert!(replay.prepared.is_none());
    assert_eq!(f.bytes(), before);
    let start = store.begin_start(created.prepared.unwrap()).unwrap();
    assert_eq!(
        json::decode(&f.bytes()).unwrap().records[&id].state(),
        State::Starting
    );
    assert_eq!(start.record().state(), State::Starting);
    let running = store.mark_running(start).unwrap();
    assert_eq!(running.state(), State::Running);
    let exited = store.mark_exited(&id).unwrap();
    assert_eq!(exited.state(), State::Exited);
    let before = f.bytes();
    assert!(store.mark_exited(&id).unwrap() == exited);
    assert!(
        store
            .create("request-one", &spec)
            .unwrap()
            .prepared
            .is_none()
    );
    assert_eq!(f.bytes(), before);
}

#[test]
fn all_spec_fields_bind_idempotency_and_errors_do_not_echo_private_spec() {
    let f = Fixture::new();
    let mut store = f.store();
    let spec = f.spec();
    store.create("request-one", &spec).unwrap();
    let baseline = f.bytes();
    let other = f._temp.path().join("private-other");
    fs::create_dir(&other).unwrap();
    for changed in [
        LaunchSpec::new(Source::Codex, "claude-v1".into(), &f.cwd).unwrap(),
        LaunchSpec::new(Source::Claude, "adapter-other".into(), &f.cwd).unwrap(),
        LaunchSpec::new(Source::Claude, "claude-v1".into(), &other).unwrap(),
    ] {
        let Err(error) = store.create("request-one", &changed) else {
            panic!("conflict accepted")
        };
        assert_eq!(error, Error::Conflict);
        let text = format!("{error} {error:?}");
        assert!(!text.contains(f.cwd.to_str().unwrap()));
        assert!(!text.contains("adapter-other"));
    }
    assert_eq!(f.bytes(), baseline);
    for (source, id) in [
        (Source::Claude, "request-claude"),
        (Source::Codex, "request-codex"),
        (Source::Grok, "request-grok"),
    ] {
        let spec = LaunchSpec::new(source, "approved-v1".into(), &f.cwd).unwrap();
        assert_eq!(
            store.create(id, &spec).unwrap().record.spec().source(),
            source
        );
    }
}

#[test]
fn starting_restart_is_uncertain_and_old_authorities_cannot_finish_it() {
    let f = Fixture::new();
    let mut store = f.store();
    let created = store.create("request-one", &f.spec()).unwrap();
    let id = created.record.record_id().to_owned();
    let start = store.begin_start(created.prepared.unwrap()).unwrap();
    let old_revision = start.record().revision();
    drop(store);
    let mut reopened = LifecycleStore::open(&f.ledger).unwrap();
    let recovered = reopened.get(&id).unwrap();
    assert_eq!(recovered.state(), State::Uncertain);
    assert!(recovered.revision() > old_revision);
    assert_eq!(
        json::decode(&f.bytes()).unwrap().records[&id].state(),
        State::Uncertain
    );
    assert!(matches!(
        reopened.mark_running(start),
        Err(Error::StaleAuthority)
    ));
    let replay = reopened.create("request-one", &f.spec()).unwrap();
    assert!(replay.prepared.is_none());
    assert_eq!(replay.record.state(), State::Uncertain);
    assert!(matches!(reopened.mark_exited(&id), Err(Error::WrongState)));
}

#[test]
fn prepared_restart_and_foreign_handles_cannot_reissue_or_use_old_authority() {
    let f = Fixture::new();
    let mut store = f.store();
    let created = store.create("request-one", &f.spec()).unwrap();
    let id = created.record.record_id().to_owned();
    drop(store);
    let baseline = f.bytes();
    let mut reopened = LifecycleStore::open(&f.ledger).unwrap();
    assert_eq!(f.bytes(), baseline);
    assert!(matches!(
        reopened.begin_start(created.prepared.unwrap()),
        Err(Error::StaleAuthority)
    ));
    assert!(
        reopened
            .create("request-one", &f.spec())
            .unwrap()
            .prepared
            .is_none()
    );
    assert_eq!(
        reopened.cancel_prepared(&id).unwrap().failure(),
        Some(Failure::PreparationCancelled)
    );
    let other = Fixture::new();
    let mut foreign = other.store();
    let prepared = reopened.create("request-two", &f.spec()).unwrap();
    assert!(matches!(
        foreign.begin_start(prepared.prepared.unwrap()),
        Err(Error::StaleAuthority)
    ));
    let prepared = reopened.create("request-three", &f.spec()).unwrap();
    let start = reopened.begin_start(prepared.prepared.unwrap()).unwrap();
    assert!(matches!(
        foreign.mark_running(start),
        Err(Error::StaleAuthority)
    ));
}

#[test]
fn failed_uncertain_and_cancelled_records_remain_non_reauthorizing_tombstones() {
    let f = Fixture::new();
    let mut store = f.store();
    let spec = f.spec();
    let one = store.create("request-one", &spec).unwrap();
    let id = one.record.record_id().to_owned();
    store.cancel_prepared(&id).unwrap();
    assert!(matches!(
        store.begin_start(one.prepared.unwrap()),
        Err(Error::StaleAuthority)
    ));
    let two = store.create("request-two", &spec).unwrap();
    let start = store.begin_start(two.prepared.unwrap()).unwrap();
    assert_eq!(
        store
            .mark_failed(start, Failure::LaunchRejected)
            .unwrap()
            .state(),
        State::Failed
    );
    let three = store.create("request-three", &spec).unwrap();
    let start = store.begin_start(three.prepared.unwrap()).unwrap();
    assert_eq!(
        store.mark_uncertain(start).unwrap().state(),
        State::Uncertain
    );
    for id in ["request-one", "request-two", "request-three"] {
        assert!(store.create(id, &spec).unwrap().prepared.is_none());
    }
    assert_eq!(store.list(0, 128).unwrap().len(), 3);
    assert!(store.list(0, 0).unwrap().is_empty());
    assert!(store.list(3, 1).unwrap().is_empty());
    assert_eq!(store.list(0, 129).unwrap().len(), 3);
}

#[test]
fn malformed_json_is_preserved_and_unknown_fields_are_ignored() {
    let f = Fixture::new();
    let mut store = f.store();
    store.create("request-one", &f.spec()).unwrap();
    drop(store);
    let original = f.bytes();
    let value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    let id = value["records"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let mut unknown = value.clone();
    unknown["records"][&id]["spec"]["shell"] = serde_json::json!("private-command");
    let mut missing = value.clone();
    missing["records"][&id]
        .as_object_mut()
        .unwrap()
        .remove("failure");
    let mut wrong_schema = value.clone();
    wrong_schema["schema"] = serde_json::json!(99);
    let mut wrong_id = value.clone();
    wrong_id["records"][&id]["record_id"] = serde_json::json!("not-record-id");
    let mut bad_state = value.clone();
    bad_state["records"][&id]["state"] = serde_json::json!("failed");
    for bytes in [
        b"broken".to_vec(),
        serde_json::to_vec(&wrong_schema).unwrap(),
        serde_json::to_vec(&wrong_id).unwrap(),
        serde_json::to_vec(&bad_state).unwrap(),
    ] {
        fs::write(f.ledger.join(LEDGER_FILENAME), &bytes).unwrap();
        assert!(LifecycleStore::open(&f.ledger).is_err());
        assert_eq!(f.bytes(), bytes);
    }
    for bytes in [
        serde_json::to_vec(&unknown).unwrap(),
        serde_json::to_vec(&missing).unwrap(),
        String::from_utf8(original.clone())
            .unwrap()
            .replacen("\"schema\":5", "\"schema\":5,\"schema\":5", 1)
            .into_bytes(),
    ] {
        fs::write(f.ledger.join(LEDGER_FILENAME), &bytes).unwrap();
        drop(LifecycleStore::open(&f.ledger).unwrap());
        assert_eq!(f.bytes(), bytes);
    }
    fs::write(f.ledger.join(LEDGER_FILENAME), &original).unwrap();
    drop(LifecycleStore::open(&f.ledger).unwrap());
    fs::remove_file(f.ledger.join(LEDGER_FILENAME)).unwrap();
    assert!(matches!(
        LifecycleStore::open(&f.ledger),
        Err(Error::MissingLedger)
    ));
    let mut initialized = LifecycleStore::initialize(&f.ledger).unwrap();
    assert!(initialized.list(0, 1).unwrap().is_empty());
    assert!(f.ledger.join(LEDGER_FILENAME).exists());
}

#[test]
fn creation_receipts_survive_former_capacity_limits() {
    let f = Fixture::new();
    let mut store = f.store();
    let spec = f.spec();
    for index in 0..129 {
        store.create(&format!("request-{index:04}"), &spec).unwrap();
    }
    store.create("request-overflow", &spec).unwrap();
    assert_eq!(store.list(0, usize::MAX).unwrap().len(), 130);
    let before = f.bytes();
    assert!(
        store
            .create("request-0000", &spec)
            .unwrap()
            .prepared
            .is_none()
    );
    assert_eq!(f.bytes(), before);
    for bad in [
        "short".to_owned(),
        "x".repeat(129),
        "request/../../private".into(),
    ] {
        let Err(error) = store.create(&bad, &spec) else {
            panic!("bad ID")
        };
        assert_eq!(error, Error::InvalidRequest);
        assert!(!error.to_string().contains(&bad));
    }
    for adapter in [
        "x".repeat(65),
        "sh -c private".into(),
        "adapter;private".into(),
    ] {
        assert!(LaunchSpec::new(Source::Claude, adapter, &f.cwd).is_ok());
    }
    assert!(LaunchSpec::new(Source::Claude, "allowed".into(), Path::new("relative")).is_err());
    let resolved =
        LaunchSpec::new(Source::Claude, "allowed".into(), &f.cwd.join("../work")).unwrap();
    assert_eq!(resolved.cwd(), f.cwd.canonicalize().unwrap());
    assert!(
        LaunchSpec::new(
            Source::Claude,
            "allowed".into(),
            Path::new(&format!("/{}", "x".repeat(4096)))
        )
        .is_err()
    );
    drop(store);
    // A valid ledger larger than the old byte cap still reopens and preserves
    // idempotency. Whitespace keeps this test independent of receipt field sizes.
    let mut oversized = vec![b' '; 1024 * 1024 + 1];
    oversized.extend_from_slice(&before);
    fs::write(f.ledger.join(LEDGER_FILENAME), &oversized).unwrap();
    let mut reopened = LifecycleStore::open(&f.ledger).unwrap();
    assert_eq!(reopened.list(0, usize::MAX).unwrap().len(), 130);
    assert!(
        reopened
            .create("request-0000", &spec)
            .unwrap()
            .prepared
            .is_none()
    );
    assert_eq!(f.bytes(), oversized);
}

#[test]
fn abandoned_module_temp_and_partial_initialization_are_preserved() {
    let f = Fixture::new();
    let store = f.store();
    drop(store);
    let temp = f.ledger.join(format!(".lifecycle-tmp-{}", "a".repeat(32)));
    fs::write(&temp, b"old evidence").unwrap();
    fs::set_permissions(&temp, fs::Permissions::from_mode(0o600)).unwrap();
    drop(LifecycleStore::open(&f.ledger).unwrap());
    assert_eq!(fs::read(&temp).unwrap(), b"old evidence");
    assert!(LifecycleStore::initialize(&f.ledger).is_err());
    fs::remove_file(f.ledger.join(LEDGER_FILENAME)).unwrap();
    assert!(LifecycleStore::initialize(&f.ledger).is_ok());
    assert!(temp.exists());
}

#[test]
fn changed_cwd_and_deserialized_spec_cannot_bypass_fresh_creation_checks() {
    let f = Fixture::new();
    let mut store = f.store();
    let spec = f.spec();
    let created = store.create("request-one", &spec).unwrap();
    let moved = f._temp.path().join("moved-cwd");
    fs::rename(&f.cwd, &moved).unwrap();
    symlink(&moved, &f.cwd).unwrap();
    assert!(matches!(
        store.begin_start(created.prepared.unwrap()),
        Err(Error::InvalidSpec)
    ));
    assert!(matches!(
        store.create("request-two", &spec),
        Err(Error::InvalidSpec)
    ));
    // Durable duplicate reads still work after the original cwd disappears.
    assert!(
        store
            .create("request-one", &spec)
            .unwrap()
            .prepared
            .is_none()
    );
    let mut raw = serde_json::to_value(&spec).unwrap();
    raw["cwd"] = serde_json::json!(f._temp.path().join("missing"));
    let forged: LaunchSpec = serde_json::from_value(raw).unwrap();
    assert!(matches!(
        store.create("request-forged", &forged),
        Err(Error::InvalidSpec)
    ));
    let record = store.get(created.record.record_id()).unwrap();
    assert_eq!(record.state(), State::Prepared);
}

#[test]
fn cancellation_is_durable_non_reauthorizing_and_stale_evidence_is_rejected() {
    let f = Fixture::new();
    let mut store = f.store();
    let created = store.create("request-cancel", &f.spec()).unwrap();
    let start = store.begin_start(created.prepared.unwrap()).unwrap();
    let running = store.mark_running(start).unwrap();
    let stale = ObservationEvidence::new(&running, Observation::Exited);
    let cancellation = store
        .request_cancel(running.record_id(), running.instance_id())
        .unwrap();
    assert_eq!(cancellation.record.state(), State::CancelRequested);
    assert!(json::decode(&f.bytes()).unwrap().records[running.record_id()].cancel_requested());
    assert!(matches!(store.observe(stale), Err(Error::StaleAuthority)));
    assert!(
        store
            .request_cancel(running.record_id(), running.instance_id())
            .unwrap()
            .authority
            .is_none()
    );
    drop(store);
    let mut store = LifecycleStore::open(&f.ledger).unwrap();
    let recovered = store.get(running.record_id()).unwrap();
    assert_eq!(recovered.state(), State::Uncertain);
    assert!(recovered.cancel_requested());
    assert!(matches!(
        store.finish_cancel(cancellation.authority.unwrap(), Observation::Exited),
        Err(Error::StaleAuthority)
    ));
    assert!(
        store
            .request_cancel(running.record_id(), running.instance_id())
            .unwrap()
            .authority
            .is_none()
    );
    let observed = store
        .observe(ObservationEvidence::new(&recovered, Observation::Running))
        .unwrap();
    assert_eq!(observed.state(), State::Uncertain);
    let exited = store
        .observe(ObservationEvidence::new(&observed, Observation::Exited))
        .unwrap();
    assert_eq!(exited.state(), State::Exited);
    assert!(exited.cancel_requested());
}

#[test]
fn schema_one_migration_preserves_intents_and_rejects_mixed_cancel_fields() {
    for populated in [false, true] {
        let f = Fixture::new();
        let mut store = f.store();
        let record = populated.then(|| store.create("request-original", &f.spec()).unwrap().record);
        drop(store);
        let mut raw: serde_json::Value = serde_json::from_slice(&f.bytes()).unwrap();
        raw["schema"] = serde_json::json!(1);
        for row in raw["records"].as_object_mut().unwrap().values_mut() {
            strip_schema5(row);
            row.as_object_mut().unwrap().remove("cancel_requested");
            row.as_object_mut().unwrap().remove("binding");
            row.as_object_mut().unwrap().remove("session_id");
            row["spec"].as_object_mut().unwrap().remove("launch");
        }
        fs::write(
            f.ledger.join(LEDGER_FILENAME),
            serde_json::to_vec(&raw).unwrap(),
        )
        .unwrap();
        let mut reopened = LifecycleStore::open(&f.ledger).unwrap();
        assert_eq!(json::decode(&f.bytes()).unwrap().schema, super::SCHEMA);
        if let Some(record) = record {
            let migrated = reopened.get(record.record_id()).unwrap();
            assert_eq!(migrated.request_id(), record.request_id());
            assert_eq!(migrated.launch_id(), record.launch_id());
            assert_eq!(migrated.instance_id(), record.instance_id());
            assert!(migrated.spec() == record.spec());
            assert!(!migrated.cancel_requested());
            assert!(
                reopened
                    .create(record.request_id(), record.spec())
                    .unwrap()
                    .prepared
                    .is_none()
            );
        }
        drop(reopened);
        drop(LifecycleStore::open(&f.ledger).unwrap());
        if populated {
            raw["records"]
                .as_object_mut()
                .unwrap()
                .values_mut()
                .next()
                .unwrap()["cancel_requested"] = serde_json::json!(false);
            let bytes = serde_json::to_vec(&raw).unwrap();
            fs::write(f.ledger.join(LEDGER_FILENAME), &bytes).unwrap();
            assert!(matches!(
                LifecycleStore::open(&f.ledger),
                Err(Error::Invalid)
            ));
            assert_eq!(f.bytes(), bytes);
        }
    }
}

fn binding_fixture() -> (Fixture, LifecycleStore, Record, BindingSpec) {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let created = store.create("request-binding", &fixture.spec()).unwrap();
    let start = store.begin_start(created.prepared.unwrap()).unwrap();
    let running = store.mark_running(start).unwrap();
    let spec = BindingSpec::new(
        Source::Claude,
        "full.native.session".into(),
        "claude:0123456789abcdef".into(),
    )
    .unwrap();
    (fixture, store, running, spec)
}

#[test]
fn binding_intent_is_durable_exact_and_explicit_same_value_retries_do_not_reinterpret() {
    let (f, mut store, running, spec) = binding_fixture();
    let authority = store.begin_binding(&running, &spec).unwrap();
    let committed = &json::decode(&f.bytes()).unwrap().records[running.record_id()];
    assert_eq!(committed.binding().unwrap().state(), BindingState::Intent);
    assert!(committed.binding().unwrap().spec() == &spec);
    let uncertain = store
        .finish_binding(authority, BindingObservation::Unavailable)
        .unwrap();
    let retry = store.begin_binding(&uncertain, &spec).unwrap();
    assert!(retry.record().revision() > uncertain.revision());
    let confirmed = store
        .finish_binding(retry, BindingObservation::Confirmed(spec.clone()))
        .unwrap();
    assert_eq!(
        confirmed.binding().unwrap().state(),
        BindingState::Confirmed
    );
    let other = BindingSpec::new(
        Source::Claude,
        "different.session".into(),
        spec.uid().into(),
    )
    .unwrap();
    let before = f.bytes();
    assert!(matches!(
        store.begin_binding(&confirmed, &other),
        Err(Error::Conflict)
    ));
    assert_eq!(f.bytes(), before);
    let unchanged = store.create("request-binding", &f.spec()).unwrap();
    assert!(unchanged.prepared.is_none());
    assert!(unchanged.record.binding().unwrap().spec() == &spec);
}

#[test]
fn binding_recovery_preserves_confirmation_without_reissuing_authority() {
    let (f, mut store, running, spec) = binding_fixture();
    let authority = store.begin_binding(&running, &spec).unwrap();
    drop(store);
    let mut reopened = LifecycleStore::open(&f.ledger).unwrap();
    let uncertain = reopened.get(running.record_id()).unwrap();
    assert_eq!(
        uncertain.binding().unwrap().state(),
        BindingState::Uncertain
    );
    assert!(matches!(
        reopened.finish_binding(authority, BindingObservation::Confirmed(spec.clone())),
        Err(Error::StaleAuthority)
    ));
    let confirmed = reopened
        .observe_binding(BindingEvidence::new(
            &uncertain,
            BindingObservation::Confirmed(spec),
        ))
        .unwrap();
    assert_eq!(
        confirmed.binding().unwrap().state(),
        BindingState::Confirmed
    );
    drop(reopened);
    let mut reopened = LifecycleStore::open(&f.ledger).unwrap();
    assert_eq!(
        reopened
            .get(running.record_id())
            .unwrap()
            .binding()
            .unwrap()
            .state(),
        BindingState::Confirmed
    );
}

#[test]
fn process_confirmation_erased_by_schema_five_recovery_is_restored() {
    let (f, mut store, running, spec) = binding_fixture();
    let authority = store
        .begin_binding_with(
            &running,
            &spec,
            BindingMethod::Process,
            Some("verified process evidence".into()),
        )
        .unwrap();
    let confirmed = store
        .finish_binding(authority, BindingObservation::Confirmed(spec))
        .unwrap();
    drop(store);
    let mut raw: serde_json::Value = serde_json::from_slice(&f.bytes()).unwrap();
    raw["records"][confirmed.record_id()]["state"] = serde_json::json!("uncertain");
    raw["records"][confirmed.record_id()]["binding"]["state"] = serde_json::json!("uncertain");
    fs::write(
        f.ledger.join(LEDGER_FILENAME),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();

    let mut reopened = LifecycleStore::open(&f.ledger).unwrap();
    let recovered = reopened.get(confirmed.record_id()).unwrap();
    assert_eq!(recovered.state(), State::Uncertain);
    assert_eq!(
        recovered.binding().unwrap().state(),
        BindingState::Confirmed
    );
}

#[test]
fn binding_observation_conflicts_keep_original_intent_and_cancel_rejects_stale_evidence() {
    let (_f, mut store, running, spec) = binding_fixture();
    let authority = store.begin_binding(&running, &spec).unwrap();
    let confirmed = store
        .finish_binding(authority, BindingObservation::Confirmed(spec.clone()))
        .unwrap();
    let other = BindingSpec::new(
        Source::Claude,
        "different.session".into(),
        spec.uid().into(),
    )
    .unwrap();
    assert!(matches!(
        store.observe_binding(BindingEvidence::new(
            &confirmed,
            BindingObservation::Confirmed(other)
        )),
        Err(Error::Conflict)
    ));
    let uncertain = store.get(running.record_id()).unwrap();
    assert_eq!(
        uncertain.binding().unwrap().state(),
        BindingState::Uncertain
    );
    assert!(uncertain.binding().unwrap().spec() == &spec);
    let stale = BindingEvidence::new(&uncertain, BindingObservation::Confirmed(spec.clone()));
    let cancelled = store
        .request_cancel(running.record_id(), running.instance_id())
        .unwrap();
    assert!(matches!(
        store.observe_binding(stale),
        Err(Error::StaleAuthority)
    ));
    assert!(matches!(
        store.begin_binding(&cancelled.record, &spec),
        Err(Error::WrongState)
    ));
}

#[test]
fn schema_two_migration_preserves_cancellation_and_rejects_injected_binding() {
    let (f, mut store, running, _) = binding_fixture();
    store
        .request_cancel(running.record_id(), running.instance_id())
        .unwrap();
    drop(store);
    let mut raw: serde_json::Value = serde_json::from_slice(&f.bytes()).unwrap();
    raw["schema"] = serde_json::json!(2);
    strip_schema5(&mut raw["records"][running.record_id()]);
    raw["records"][running.record_id()]
        .as_object_mut()
        .unwrap()
        .remove("binding");
    raw["records"][running.record_id()]
        .as_object_mut()
        .unwrap()
        .remove("session_id");
    raw["records"][running.record_id()]["spec"]
        .as_object_mut()
        .unwrap()
        .remove("launch");
    fs::write(
        f.ledger.join(LEDGER_FILENAME),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    let mut reopened = LifecycleStore::open(&f.ledger).unwrap();
    let recovered = reopened.get(running.record_id()).unwrap();
    assert!(recovered.cancel_requested());
    assert_eq!(recovered.state(), State::Uncertain);
    assert!(recovered.binding().is_none());
    drop(reopened);
    raw["records"][running.record_id()]["binding"] = serde_json::Value::Null;
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(f.ledger.join(LEDGER_FILENAME), &bytes).unwrap();
    assert!(matches!(
        LifecycleStore::open(&f.ledger),
        Err(Error::Invalid)
    ));
    assert_eq!(f.bytes(), bytes);
}

#[test]
fn binding_spec_keeps_source_identity_and_accepts_extra_persisted_fields() {
    let (f, mut store, running, spec) = binding_fixture();
    // Grok binds like the others (summary.json `info.id` is its
    // native scope); a Grok uid still needs the `grok:` prefix.
    assert!(
        BindingSpec::new(
            Source::Grok,
            "session".into(),
            "grok:0123456789abcdef".into()
        )
        .is_ok()
    );
    for (source, sid, uid) in [
        (
            Source::Grok,
            "session".into(),
            "codex:0123456789abcdef".into(),
        ),
        (
            Source::Claude,
            spec.sid().into(),
            "codex:0123456789abcdef".into(),
        ),
    ] {
        assert!(BindingSpec::new(source, sid, uid).is_err());
    }
    assert!(BindingSpec::new(Source::Claude, "x".repeat(257), spec.uid().into()).is_ok());
    assert!(BindingSpec::new(Source::Claude, "secret\ncommand".into(), spec.uid().into()).is_ok());
    let authority = store.begin_binding(&running, &spec).unwrap();
    store
        .finish_binding(authority, BindingObservation::Confirmed(spec))
        .unwrap();
    drop(store);
    let original: serde_json::Value = serde_json::from_slice(&f.bytes()).unwrap();
    for field in ["state", "spec"] {
        let mut raw = original.clone();
        raw["records"][running.record_id()]["binding"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        fs::write(
            f.ledger.join(LEDGER_FILENAME),
            serde_json::to_vec(&raw).unwrap(),
        )
        .unwrap();
        assert!(LifecycleStore::open(&f.ledger).is_err());
    }
    let mut raw = original;
    raw["records"][running.record_id()]["binding"]["spec"]["PRIVATE_EXTRA"] =
        serde_json::json!("secret");
    fs::write(
        f.ledger.join(LEDGER_FILENAME),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    assert!(LifecycleStore::open(&f.ledger).is_ok());
}

#[test]
fn schema_three_migration_keeps_binding_and_rejects_injected_launch_fields() {
    let (f, mut store, running, spec) = binding_fixture();
    let authority = store.begin_binding(&running, &spec).unwrap();
    store
        .finish_binding(authority, BindingObservation::Confirmed(spec.clone()))
        .unwrap();
    drop(store);
    let mut raw: serde_json::Value = serde_json::from_slice(&f.bytes()).unwrap();
    raw["schema"] = serde_json::json!(3);
    strip_schema5(&mut raw["records"][running.record_id()]);
    let row = raw["records"][running.record_id()].as_object_mut().unwrap();
    row.remove("session_id");
    row["spec"].as_object_mut().unwrap().remove("launch");
    let legacy = serde_json::to_vec(&raw).unwrap();
    fs::write(f.ledger.join(LEDGER_FILENAME), &legacy).unwrap();
    let mut reopened = LifecycleStore::open(&f.ledger).unwrap();
    assert_eq!(json::decode(&f.bytes()).unwrap().schema, super::SCHEMA);
    let migrated = reopened.get(running.record_id()).unwrap();
    assert!(migrated.spec().launch() == &Launch::Fixed);
    assert!(migrated.session_id().is_none() && migrated.declared_sid().is_none());
    assert!(migrated.binding().unwrap().spec() == &spec);
    assert_eq!(migrated.binding().unwrap().state(), BindingState::Uncertain);
    assert_eq!(
        migrated.binding().unwrap().method(),
        BindingMethod::Operator
    );
    assert!(migrated.binding().unwrap().evidence().is_none());
    assert!(migrated.created_at().is_none() && migrated.finished_at().is_none());
    assert!(!migrated.discarded());
    drop(reopened);
    // A schema-3 file must not already carry schema-4 fields.
    for (path, value) in [
        (vec!["session_id"], serde_json::Value::Null),
        (vec!["spec", "launch"], serde_json::json!({"kind":"fixed"})),
    ] {
        let mut injected = raw.clone();
        let mut target = &mut injected["records"][running.record_id()];
        for key in &path[..path.len() - 1] {
            target = &mut target[*key];
        }
        target[path[path.len() - 1]] = value;
        let bytes = serde_json::to_vec(&injected).unwrap();
        fs::write(f.ledger.join(LEDGER_FILENAME), &bytes).unwrap();
        assert!(matches!(
            LifecycleStore::open(&f.ledger),
            Err(Error::Invalid)
        ));
        assert_eq!(f.bytes(), bytes);
    }
}

/// Schema 4 → 5: every record gains `created_at`/`finished_at`/`discarded`,
/// a binding gains `method: operator`/`evidence`/`bound_at`; a schema-4 file
/// already carrying any of them fails closed. Terminal states record their
/// time; discard is a tombstone of finished/cancelled receipts only.
#[test]
fn schema_four_migration_adds_times_and_binding_method_and_discard_is_bounded() {
    let (f, mut store, running, spec) = binding_fixture();
    let authority = store.begin_binding(&running, &spec).unwrap();
    let confirmed = store
        .finish_binding(authority, BindingObservation::Confirmed(spec.clone()))
        .unwrap();
    assert_eq!(
        confirmed.binding().unwrap().method(),
        BindingMethod::Operator
    );
    assert!(confirmed.binding().unwrap().bound_at().is_some());
    assert!(confirmed.created_at().is_some() && confirmed.finished_at().is_none());
    // A live receipt cannot be discarded; an exited one can, idempotently.
    assert!(matches!(
        store.discard(confirmed.record_id()),
        Err(Error::WrongState)
    ));
    let exited = store
        .observe(ObservationEvidence::new(&confirmed, Observation::Exited))
        .unwrap();
    assert_eq!(exited.state(), State::Exited);
    assert!(exited.finished_at().is_some());
    assert!(exited.discardable() && !exited.discarded());
    let discarded = store.discard(exited.record_id()).unwrap();
    assert!(discarded.discarded());
    assert_eq!(
        store.discard(exited.record_id()).unwrap().revision(),
        discarded.revision()
    );
    drop(store);
    let current: serde_json::Value = serde_json::from_slice(&f.bytes()).unwrap();
    let mut raw = current.clone();
    raw["schema"] = serde_json::json!(4);
    strip_schema5(&mut raw["records"][running.record_id()]);
    let legacy = serde_json::to_vec(&raw).unwrap();
    fs::write(f.ledger.join(LEDGER_FILENAME), &legacy).unwrap();
    let mut reopened = LifecycleStore::open(&f.ledger).unwrap();
    assert_eq!(json::decode(&f.bytes()).unwrap().schema, super::SCHEMA);
    let migrated = reopened.get(running.record_id()).unwrap();
    assert_eq!(migrated.state(), State::Exited);
    assert!(migrated.created_at().is_none() && migrated.finished_at().is_none());
    assert!(!migrated.discarded());
    assert_eq!(
        migrated.binding().unwrap().method(),
        BindingMethod::Operator
    );
    assert!(migrated.binding().unwrap().evidence().is_none());
    assert!(migrated.binding().unwrap().bound_at().is_none());
    drop(reopened);
    for (path, value) in [
        (vec!["discarded"], serde_json::json!(false)),
        (vec!["finished_at"], serde_json::Value::Null),
        (vec!["created_at"], serde_json::Value::Null),
        (vec!["binding", "method"], serde_json::json!("operator")),
    ] {
        let mut injected = raw.clone();
        let mut target = &mut injected["records"][running.record_id()];
        for key in &path[..path.len() - 1] {
            target = &mut target[*key];
        }
        target[path[path.len() - 1]] = value;
        let bytes = serde_json::to_vec(&injected).unwrap();
        fs::write(f.ledger.join(LEDGER_FILENAME), &bytes).unwrap();
        assert!(matches!(
            LifecycleStore::open(&f.ledger),
            Err(Error::Invalid)
        ));
        assert_eq!(f.bytes(), bytes);
    }
    // A current file whose tombstone contradicts the state fails closed too.
    let mut bad = current;
    bad["records"][running.record_id()]["state"] = serde_json::json!("running");
    bad["records"][running.record_id()]["finished_at"] = serde_json::Value::Null;
    let bytes = serde_json::to_vec(&bad).unwrap();
    fs::write(f.ledger.join(LEDGER_FILENAME), &bytes).unwrap();
    assert!(LifecycleStore::open(&f.ledger).is_err());
}

#[test]
fn assigned_session_ids_are_minted_once_and_declared_launches_refuse_operator_binding() {
    let f = Fixture::new();
    let mut store = f.store();
    let assigned = LaunchSpec::profile_new(Source::Claude, "claude-cli-v1".into(), &f.cwd).unwrap();
    assert!(assigned.launch() == &Launch::NewAssigned);
    let created = store.create("request-assigned", &assigned).unwrap();
    let sid = created.record.session_id().unwrap().to_owned();
    assert!(crate::lifecycle::model::native_sid(&sid), "{sid}");
    assert_eq!(created.record.declared_sid(), Some(sid.as_str()));
    assert!(created.record.declared_uid().is_none());
    // Replay repeats the same command-line identity instead of minting another.
    let replay = store.create("request-assigned", &assigned).unwrap();
    assert!(replay.prepared.is_none());
    assert_eq!(replay.record.session_id(), Some(sid.as_str()));
    drop(store);
    let mut reopened = LifecycleStore::open(&f.ledger).unwrap();
    assert_eq!(
        reopened
            .get(created.record.record_id())
            .unwrap()
            .session_id(),
        Some(sid.as_str())
    );
    // New Codex sessions do not receive an upfront SID; Claude never stays pending.
    let pending = LaunchSpec::profile_new(Source::Codex, "codex-cli-v1".into(), &f.cwd).unwrap();
    assert!(pending.launch() == &Launch::NewPending);
    assert!(
        reopened
            .create("request-pending", &pending)
            .unwrap()
            .record
            .session_id()
            .is_none()
    );
    assert!(
        LaunchSpec::with_launch(
            Source::Claude,
            "claude-cli-v1".into(),
            &f.cwd,
            Launch::NewPending
        )
        .is_err()
    );
    assert!(
        LaunchSpec::with_launch(
            Source::Codex,
            "codex-cli-v1".into(),
            &f.cwd,
            Launch::NewAssigned
        )
        .is_err()
    );
    // Resume identities must be full lowercase UUIDs and a matching-source UID.
    let sid = "0123abcd-4567-4ef0-8123-456789abcdef";
    for (source, sid, uid) in [
        (Source::Codex, sid, "codex:0123456789abcdef"),
        (Source::Claude, sid, "claude:0123456789abcdef"),
    ] {
        assert!(
            LaunchSpec::resume(source, "cli-v1".into(), &f.cwd, sid.into(), uid.into()).is_ok()
        );
    }
    for (source, sid, uid) in [
        (
            Source::Codex,
            "0123abcd-4567-4ef0-8123-456789abcdef",
            "claude:0123456789abcdef",
        ),
        (Source::Codex, "", "codex:"),
    ] {
        assert!(
            LaunchSpec::resume(source, "cli-v1".into(), &f.cwd, sid.into(), uid.into()).is_err()
        );
    }
    let resume = LaunchSpec::resume(
        Source::Claude,
        "claude-cli-v1".into(),
        &f.cwd,
        sid.into(),
        "claude:0123456789abcdef".into(),
    )
    .unwrap();
    let created = reopened.create("request-resume", &resume).unwrap();
    let started = reopened.begin_start(created.prepared.unwrap()).unwrap();
    let running = reopened.mark_running(started).unwrap();
    assert_eq!(running.declared_sid(), Some(sid));
    assert_eq!(running.declared_uid(), Some("claude:0123456789abcdef"));
    let binding =
        BindingSpec::new(Source::Claude, sid.into(), "claude:0123456789abcdef".into()).unwrap();
    assert!(matches!(
        reopened.begin_binding(&running, &binding),
        Err(Error::InvalidSpec)
    ));
    // Persisted declared launches with an injected binding fail closed.
    drop(reopened);
    let mut raw: serde_json::Value = serde_json::from_slice(&f.bytes()).unwrap();
    raw["records"][running.record_id()]["binding"] = serde_json::json!({"spec":{"source":"claude","sid":sid,"uid":"claude:0123456789abcdef"},"state":"uncertain"});
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(f.ledger.join(LEDGER_FILENAME), &bytes).unwrap();
    assert!(matches!(
        LifecycleStore::open(&f.ledger),
        Err(Error::Invalid)
    ));
}

#[test]
fn refresh_many_reloads_the_ledger_once_where_single_observations_reload_per_call() {
    let f = Fixture::new();
    let mut store = f.store();
    let spec = f.spec();
    let mut running = Vec::new();
    for index in 0..5 {
        let created = store.create(&format!("request-batch-{index}"), &spec).unwrap();
        let start = store.begin_start(created.prepared.unwrap()).unwrap();
        running.push(store.mark_running(start).unwrap());
    }
    let before = store.disk_reads();
    for record in &running {
        store
            .observe(ObservationEvidence::new(record, Observation::Running))
            .unwrap();
    }
    assert_eq!(store.disk_reads(), before + running.len(), "observe reloads per call");

    let items = running
        .iter()
        .enumerate()
        .map(|(slot, record)| {
            (
                slot,
                ObservationEvidence::new(record, Observation::Running),
                BindingObservation::Unavailable,
            )
        })
        .collect();
    let before = store.disk_reads();
    let refreshed = store.refresh_many(items).unwrap();
    assert_eq!(store.disk_reads(), before + 1, "one batch, one reload");
    assert_eq!(refreshed.len(), running.len());
    assert!(refreshed.iter().all(|(_, r)| r.state() == State::Running));
    assert_eq!(
        refreshed.iter().map(|(slot, _)| *slot).collect::<Vec<_>>(),
        (0..running.len()).collect::<Vec<_>>()
    );

    // A batch still applies each observation: one host reports an exit.
    let items = vec![(
        7,
        ObservationEvidence::new(&running[2], Observation::Exited),
        BindingObservation::Unavailable,
    )];
    let refreshed = store.refresh_many(items).unwrap();
    assert_eq!(refreshed[0].0, 7);
    assert_eq!(refreshed[0].1.state(), State::Exited);
    assert_eq!(
        store.get(running[2].record_id()).unwrap().state(),
        State::Exited
    );
}
