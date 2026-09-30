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
