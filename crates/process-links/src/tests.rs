use super::*;

fn report(node: &str) -> Report {
    Report {
        version: 1,
        node_id: node.into(),
        boot_id: format!("boot-{node}"),
        supported: true,
        sampled_at: 500.0,
        outgoing: vec![],
        incoming: vec![],
        bindings: vec![],
        collector: None,
    }
}

#[test]
fn unique_connection_preserves_launcher_and_rejects_reuse_or_ambiguity() {
    let connection = Connection::parse("10.0.0.1 45000 10.0.0.2 50022").unwrap();
    let mut source = report("a");
    source.outgoing.push(Outgoing {
        launch_chain: vec![],
        process: Process {
            pid: 42,
            start: 123,
        },
        started_at: 200.0,
        connection: connection.clone(),
        session: Session {
            node_id: "a".into(),
            source: "codex".into(),
            sid: "parent".into(),
            title: None,
            created: Some(100.0),
        },
    });
    let mut target = report("b");
    target.incoming.push(Incoming {
        process: Process {
            pid: 84,
            start: 456,
        },
        started_at: 201.0,
        connection: connection.clone(),
    });
    let linked = correlate(&[source.clone(), target.clone()]);
    assert_eq!(
        linked["b"].links[0].launcher,
        ProcessKey {
            node_id: "a".into(),
            boot_id: "boot-a".into(),
            process: Process {
                pid: 42,
                start: 123
            }
        }
    );
    assert_eq!(linked["b"].links[0].process.pid, 84);
    target.incoming[0].started_at = 190.0;
    assert!(
        correlate(&[source.clone(), target.clone()])["b"]
            .links
            .is_empty()
    );
    target.incoming[0].started_at = 201.0;
    source.outgoing.push(source.outgoing[0].clone());
    assert!(correlate(&[source, target])["b"].links.is_empty());
    assert_eq!(
        connection,
        Connection::parse("::ffff:10.0.0.1 45000 ::ffff:10.0.0.2 50022").unwrap()
    );
    assert!(Connection::parse("10.0.0.1 not-a-port 10.0.0.2 22").is_none());
}

#[test]
fn multihop_keeps_causality_after_original_launcher_disappears() {
    let session = Session {
        node_id: "a".into(),
        source: "codex".into(),
        sid: "parent".into(),
        title: None,
        created: Some(100.0),
    };
    let original = Launch {
        process: ProcessKey {
            node_id: "a".into(),
            boot_id: "boot-a".into(),
            process: Process {
                pid: 42,
                start: 123,
            },
        },
        session: session.clone(),
    };
    let mut source = report("b");
    let connection = Connection::parse("10.0.0.2 45000 10.0.0.3 50022").unwrap();
    source.outgoing.push(Outgoing {
        process: Process {
            pid: 50,
            start: 300,
        },
        started_at: 300.0,
        connection: connection.clone(),
        session: Session {
            node_id: "b".into(),
            sid: "child".into(),
            ..session
        },
        launch_chain: vec![original.clone()],
    });
    let mut target = report("c");
    target.incoming.push(Incoming {
        process: Process {
            pid: 70,
            start: 301,
        },
        started_at: 301.0,
        connection,
    });
    let linked = correlate(&[source, target]);
    let chain = &linked["c"].links[0].launch_chain;
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].session.sid, "child");
    assert_eq!(chain[1].process, original.process);
    assert_eq!(chain[1].session.sid, "parent");
}

#[test]
fn event_fork_preserves_orphan_attribution_without_application() {
    use crate::{
        agent::{Catalog, CollectorStatus, Owner},
        engine::Engine,
        linux::{Entry, Snapshot},
    };
    let parent = Process {
        pid: 10,
        start: 100,
    };
    let child = Process {
        pid: 11,
        start: 101,
    };
    let session = Session {
        node_id: "a".into(),
        source: "codex".into(),
        sid: "native".into(),
        title: None,
        created: Some(1.0),
    };
    let entry = |process: Process, parent| Entry {
        process,
        parent,
        started_at: 1.0,
        connection: None,
        identities: vec![],
        sockets: vec![],
        multiplexed: false,
        shared_parent: false,
    };
    let mut snapshot = Snapshot {
        boot_id: "boot".into(),
        entries: BTreeMap::from([(10, entry(parent.clone(), 1))]),
    };
    let mut engine = Engine::new("a".into(), "boot".into(), None);
    assert!(engine.catalog(Catalog {
        node_id: "a".into(),
        boot_id: "boot".into(),
        sessions: vec![session.clone()],
        owners: vec![Owner {
            process: parent.clone(),
            session
        }]
    }));
    engine.update(&snapshot, 2.0, CollectorStatus::default());
    engine.fork(&parent, child.clone(), 3.0);
    engine.exit(&parent);
    snapshot.entries = BTreeMap::from([(11, entry(child.clone(), 1))]);
    let report = engine.update(&snapshot, 4.0, CollectorStatus::default());
    assert_eq!(report.bindings[0].session.sid, "native");
    let mut recovered = Engine::new("a".into(), "boot".into(), Some(engine.saved()));
    assert_eq!(
        recovered
            .update(&snapshot, 5.0, CollectorStatus::default())
            .bindings
            .len(),
        1
    );
    snapshot.entries = BTreeMap::from([(
        11,
        entry(
            Process {
                pid: 11,
                start: 102,
            },
            1,
        ),
    )]);
    assert!(
        recovered
            .update(&snapshot, 6.0, CollectorStatus::default())
            .bindings
            .is_empty()
    );
}

#[test]
fn launcher_identity_inherited_by_child_cli_does_not_outrank_it() {
    use crate::{
        agent::{Catalog, CollectorStatus, Owner},
        engine::Engine,
        linux::{Entry, Snapshot},
    };
    let session = |sid: &str| Session {
        node_id: "a".into(),
        source: "codex".into(),
        sid: sid.into(),
        title: None,
        created: Some(1.0),
    };
    let launched = |sid: &str| vec![("codex".to_string(), sid.to_string())];
    let entry = |pid, parent, identities| Entry {
        process: Process {
            pid,
            start: pid.into(),
        },
        parent,
        started_at: 1.0,
        connection: None,
        identities,
        sockets: vec![],
        multiplexed: false,
        shared_parent: false,
    };
    // A `codex exec` child (20) and its helper (21) inherit the launcher's
    // thread ID; a tool command (22) carries the child's own thread ID.
    let snapshot = Snapshot {
        boot_id: "boot".into(),
        entries: BTreeMap::from([
            (10, entry(10, 1, vec![])),
            (20, entry(20, 10, launched("parent"))),
            (21, entry(21, 20, launched("parent"))),
            (22, entry(22, 21, launched("child"))),
            (30, entry(30, 1, launched("parent"))),
        ]),
    };
    let mut engine = Engine::new("a".into(), "boot".into(), None);
    assert!(engine.catalog(Catalog {
        node_id: "a".into(),
        boot_id: "boot".into(),
        sessions: vec![session("parent"), session("child")],
        owners: vec![
            Owner {
                process: Process { pid: 10, start: 10 },
                session: session("parent")
            },
            Owner {
                process: Process { pid: 20, start: 20 },
                session: session("child")
            },
        ],
    }));
    let report = engine.update(&snapshot, 2.0, CollectorStatus::default());
    let owner = |pid| {
        report
            .bindings
            .iter()
            .find(|b| b.process.pid == pid)
            .map(|b| b.session.sid.as_str())
    };
    assert_eq!(owner(21), Some("child"));
    assert_eq!(owner(22), Some("child"));
    // Detached work without an owning ancestor still uses its identity.
    assert_eq!(owner(30), Some("parent"));
}

#[test]
fn fork_origin_names_spawner_after_launcher_exits() {
    use crate::{
        agent::{Catalog, CollectorStatus, Owner},
        engine::Engine,
        linux::{Entry, Snapshot},
    };
    let session = |sid: &str| Session {
        node_id: "a".into(),
        source: "codex".into(),
        sid: sid.into(),
        title: None,
        created: Some(1.0),
    };
    let entry = |pid: u32, parent| Entry {
        process: Process {
            pid,
            start: pid.into(),
        },
        parent,
        started_at: 1.0,
        connection: None,
        identities: vec![],
        sockets: vec![],
        multiplexed: false,
        shared_parent: false,
    };
    let launcher = Process { pid: 10, start: 10 };
    let child = Process { pid: 20, start: 20 };
    let catalog = |owners: Vec<Owner>| Catalog {
        node_id: "a".into(),
        boot_id: "boot".into(),
        sessions: vec![session("parent"), session("child")],
        owners,
    };
    let mut engine = Engine::new("a".into(), "boot".into(), None);
    engine.catalog(catalog(vec![Owner {
        process: launcher.clone(),
        session: session("parent"),
    }]));
    let mut snapshot = Snapshot {
        boot_id: "boot".into(),
        entries: BTreeMap::from([(10, entry(10, 1))]),
    };
    engine.update(&snapshot, 2.0, CollectorStatus::default());
    engine.fork(&launcher, child.clone(), 3.0);
    engine.exit(&launcher);
    // The dispatcher exited before the child CLI was cataloged.
    snapshot.entries = BTreeMap::from([(20, entry(20, 1))]);
    engine.catalog(catalog(vec![Owner {
        process: child.clone(),
        session: session("child"),
    }]));
    let report = engine.update(&snapshot, 4.0, CollectorStatus::default());
    let owned = &report.bindings[0];
    assert_eq!(owned.session.sid, "child");
    assert_eq!(
        owned.spawner.as_ref().map(|s| s.sid.as_str()),
        Some("parent")
    );
    let recovered = Engine::new("a".into(), "boot".into(), Some(engine.saved()));
    assert_eq!(recovered.forked_from.len(), 1);
}
