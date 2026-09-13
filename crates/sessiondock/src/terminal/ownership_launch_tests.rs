use super::*;
use ptyhost_client::{
    Association, AssociationState, HostObservation, LaunchIdentity, LaunchState, SessionSummary,
    Source,
};

const INSTANCE: &str = "synthetic-instance-original";
const LAUNCH: &str = "synthetic-launch-original";
const UID: &str = "codex:synthetic-native-uid";

fn observation(launch: &str, instance: &str) -> HostObservation {
    HostObservation {
        summary: SessionSummary {
            name: "terminal".into(),
            created: 1,
            attached: false,
            pid: 1,
            host_pid: 2,
            cwd: String::new(),
            cmd: String::new(),
            cols: 80,
            rows: 24,
            owned: true,
            server: "ptyhost",
            backend: "ptyhost",
        },
        association: AssociationState::Declared(Association {
            source: Source::Codex,
            sid: Some("synthetic-native-sid".into()),
            uid: Some(UID.into()),
        }),
        instance_id: Some(instance.into()),
        exited: false,
        instance_guard_v1: true,
        launch_guard_v1: true,
        native_binding: Default::default(),
        launch: LaunchState::Declared(LaunchIdentity {
            source: Source::Codex,
            launch_id: launch.into(),
        }),
    }
}

fn target(launch: &str, instance: &str) -> Arc<LaunchTarget> {
    Arc::new(
        LaunchTarget::from_observation(
            &observation(launch, instance),
            Source::Codex,
            launch,
            instance,
        )
        .unwrap(),
    )
}

fn native() -> Arc<BoundTarget> {
    Arc::new(
        BoundTarget::from_observation(
            &observation(LAUNCH, INSTANCE),
            Source::Codex,
            "synthetic-native-sid",
            UID,
        )
        .unwrap(),
    )
}

fn ip() -> IpAddr {
    "192.0.2.1".parse().unwrap()
}
fn claim(registry: &Registry, target: Arc<LaunchTarget>, page: &str, force: bool) -> String {
    registry
        .claim_launch(target, page, ip(), force)
        .unwrap()
        .into_api_json()["token"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn launch_native_raw_claim_and_bind_kinds_never_interchange_even_with_force() {
    let registry = Registry::new(4).unwrap();
    let target = target(LAUNCH, INSTANCE);
    let token = claim(&registry, target.clone(), "page", false);
    assert_eq!(
        registry.bind("terminal", "page", &token).err(),
        Some(OwnershipError::BindingMismatch)
    );
    assert_eq!(
        registry
            .bind_bound("terminal", "page", &token, UID, INSTANCE)
            .err(),
        Some(OwnershipError::BindingMismatch)
    );
    for (launch, instance) in [
        ("synthetic-launch-wrong", INSTANCE),
        (LAUNCH, "synthetic-instance-wrong"),
        ("synthetic-launch", INSTANCE),
    ] {
        assert_eq!(
            registry
                .bind_launch("terminal", "page", &token, launch, instance)
                .err(),
            Some(OwnershipError::BindingMismatch)
        );
    }
    for force in [false, true] {
        assert_eq!(
            registry.claim("terminal", "page", ip(), force).err(),
            Some(OwnershipError::BindingMismatch)
        );
        assert_eq!(
            registry.claim_bound(native(), "page", ip(), force).err(),
            Some(OwnershipError::BindingMismatch)
        );
    }
    let bound = registry
        .bind_launch("terminal", "page", &token, LAUNCH, INSTANCE)
        .unwrap();
    assert!(
        matches!(bound.lease_target(),LeaseTarget::Launch(actual) if actual.launch_id()==LAUNCH)
    );
    assert!(registry.release(&bound).unwrap());
    for native_kind in [false, true] {
        let response = if native_kind {
            registry.claim_bound(native(), "page", ip(), false)
        } else {
            registry.claim("terminal", "page", ip(), false)
        }
        .unwrap();
        let token = response.into_api_json()["token"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            registry
                .claim_launch(target.clone(), "page", ip(), true)
                .err(),
            Some(OwnershipError::BindingMismatch)
        );
        assert_eq!(
            registry
                .bind_launch("terminal", "page", &token, LAUNCH, INSTANCE)
                .err(),
            Some(OwnershipError::BindingMismatch)
        );
        assert!(
            registry
                .release_reservation("terminal", "page", &token)
                .unwrap()
        );
    }
}

#[test]
fn launch_force_reconnect_revokes_and_old_cleanup_cannot_release_replacement() {
    let registry = Registry::new(4).unwrap();
    let target = target(LAUNCH, INSTANCE);
    let token = claim(&registry, target.clone(), "old", false);
    let old = registry
        .bind_launch("terminal", "old", &token, LAUNCH, INSTANCE)
        .unwrap();
    assert_eq!(
        registry
            .claim_launch(target.clone(), "new", ip(), false)
            .unwrap()
            .status(),
        409
    );
    let new_token = claim(&registry, target, "new", true);
    assert!(old.revocations().borrow().as_ref().unwrap().notify);
    assert!(!registry.is_current(&old).unwrap());
    assert!(!registry.release(&old).unwrap());
    let new = registry
        .bind_launch("terminal", "new", &new_token, LAUNCH, INSTANCE)
        .unwrap();
    assert!(registry.is_current(&new).unwrap());
}

#[test]
fn retire_is_exact_idempotent_revokes_bound_and_blocks_reservation_reclaim() {
    let registry = Registry::new(4).unwrap();
    let original = target(LAUNCH, INSTANCE);
    let token = claim(&registry, original.clone(), "page", false);
    let bound = registry
        .bind_launch("terminal", "page", &token, LAUNCH, INSTANCE)
        .unwrap();
    registry.retire_launch(&original).unwrap();
    registry.retire_launch(&original).unwrap();
    assert_eq!(
        bound.revocations().borrow().as_ref().unwrap().reason,
        RevocationReason::LaunchRetired
    );
    assert!(!registry.is_current(&bound).unwrap());
    assert_eq!(
        registry
            .claim_launch(original.clone(), "page", ip(), true)
            .err(),
        Some(OwnershipError::LaunchRetired)
    );
    assert_eq!(
        registry
            .bind_launch("terminal", "page", &token, LAUNCH, INSTANCE)
            .err(),
        Some(OwnershipError::NotOwner)
    );
    let replacement = target(LAUNCH, "synthetic-instance-replacement");
    let token = claim(&registry, replacement.clone(), "new", false);
    registry.retire_launch(&original).unwrap();
    let new = registry
        .bind_launch("terminal", "new", &token, LAUNCH, replacement.instance_id())
        .unwrap();
    assert!(registry.is_current(&new).unwrap());
    assert!(!registry.release(&bound).unwrap());
    registry.release(&new).unwrap();
    let unbound = target("synthetic-launch-unbound", INSTANCE);
    let token = claim(&registry, unbound.clone(), "page", false);
    registry.retire_launch(&unbound).unwrap();
    assert_eq!(
        registry
            .bind_launch("terminal", "page", &token, unbound.launch_id(), INSTANCE)
            .err(),
        Some(OwnershipError::NotOwner)
    );
}

#[test]
fn retirement_capacity_never_evicts_and_failure_does_not_partially_revoke() {
    let registry = Registry::new(4).unwrap();
    for index in 0..MAX_RETIRED_LAUNCHES {
        registry
            .retire_launch(&target(&format!("synthetic-launch-{index:04}"), INSTANCE))
            .unwrap();
    }
    let original = target(LAUNCH, INSTANCE);
    let token = claim(&registry, original.clone(), "page", false);
    let bound = registry
        .bind_launch("terminal", "page", &token, LAUNCH, INSTANCE)
        .unwrap();
    assert_eq!(
        registry.retire_launch(&original),
        Err(OwnershipError::RetirementCapacity)
    );
    assert!(registry.is_current(&bound).unwrap());
    let first = target("synthetic-launch-0000", INSTANCE);
    registry.retire_launch(&first).unwrap();
    assert_eq!(
        registry.check_launch(&first),
        Err(OwnershipError::LaunchRetired)
    );
    assert_eq!(
        registry.state.lock().unwrap().retired.len(),
        MAX_RETIRED_LAUNCHES
    );
}

#[test]
fn retired_launch_never_matches_another_source_name_nonce_or_native_lease() {
    let registry = Registry::new(4).unwrap();
    let original = target(LAUNCH, INSTANCE);
    registry.retire_launch(&original).unwrap();
    for dimension in ["source", "name", "launch", "instance"] {
        let mut observed = observation(LAUNCH, INSTANCE);
        let mut source = Source::Codex;
        let mut launch = LAUNCH;
        let mut instance = INSTANCE;
        match dimension {
            "source" => {
                source = Source::Claude;
                observed.launch = LaunchState::Declared(LaunchIdentity {
                    source,
                    launch_id: LAUNCH.into(),
                });
            }
            "name" => observed.summary.name = "another-terminal".into(),
            "launch" => {
                launch = "synthetic-launch-another";
                observed.launch = LaunchState::Declared(LaunchIdentity {
                    source,
                    launch_id: launch.into(),
                });
            }
            _ => {
                instance = "synthetic-instance-another";
                observed.instance_id = Some(instance.into());
            }
        }
        let other =
            Arc::new(LaunchTarget::from_observation(&observed, source, launch, instance).unwrap());
        let token = claim(&registry, other.clone(), "page", false);
        registry.retire_launch(&original).unwrap();
        let bound = registry
            .bind_launch(other.name(), "page", &token, launch, instance)
            .unwrap();
        assert!(registry.is_current(&bound).unwrap());
        registry.release(&bound).unwrap();
    }
    let token = registry
        .claim_bound(native(), "native", ip(), false)
        .unwrap()
        .into_api_json()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let bound = registry
        .bind_bound("terminal", "native", &token, UID, INSTANCE)
        .unwrap();
    registry.retire_launch(&original).unwrap();
    assert!(registry.is_current(&bound).unwrap());
}
