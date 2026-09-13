use std::fs;

use serde_json::json;

use super::*;
use crate::hub::registry::parse_networks;

const NID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NID_C: &str = "cccccccccccccccccccccccccccccccc";

/// A registry with A and B enabled and C disabled; nothing is ever contacted.
fn registry() -> (tempfile::TempDir, Registry) {
    let dir = tempfile::tempdir().unwrap();
    let rows: Vec<Value> = [
        (NID_A, "A", None),
        (NID_B, "B", None),
        (NID_C, "C", Some(false)),
    ]
    .iter()
    .map(|(id, name, enabled)| {
        json!({"url": "http://127.0.0.1:1", "token": "t".repeat(32), "id": id, "name": name,
                   "enabled": enabled})
    })
    .collect();
    fs::write(
        dir.path().join("nodes.json"),
        serde_json::to_vec(&rows).unwrap(),
    )
    .unwrap();
    let registry = Registry::open(
        &dir.path().join("nodes.json"),
        parse_networks("127.0.0.0/8").unwrap(),
        &dir.path().join("cache"),
    )
    .unwrap();
    (dir, registry)
}

fn q(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn ids(nodes: &[Node]) -> Vec<&str> {
    nodes.iter().map(|node| node.id.as_str()).collect()
}

fn answer(registry: &Registry, nid: &str, data: Value, failure: Option<Value>) -> Answer {
    Answer {
        node: registry.find(nid).unwrap(),
        data,
        failure,
    }
}

fn failure(nid: &str, name: &str) -> Value {
    json!({"node_id": nid, "name": name, "error": "节点暂时离线", "error_code": "connection_failed",
           "last_seen": null})
}

#[test]
fn selection_follows_the_nodes_filter_and_rejects_unknown_or_disabled_ids() {
    let (_dir, registry) = registry();
    assert_eq!(ids(&selected(&registry, &[]).unwrap()), [NID_A, NID_B]);
    assert_eq!(
        ids(&selected(&registry, &q(&[("nodes", "")])).unwrap()),
        [] as [&str; 0]
    );
    assert_eq!(
        ids(&selected(&registry, &q(&[("nodes", &format!("{NID_B},,{NID_A}"))])).unwrap()),
        [NID_A, NID_B]
    );
    assert_eq!(
        ids(&selected(&registry, &q(&[("nodes", NID_B), ("nodes", NID_A)])).unwrap()),
        [NID_B]
    );
    for bad in [NID_C, "zz", &format!("{NID_A},{}", "d".repeat(32))] {
        assert_eq!(
            selected(&registry, &q(&[("nodes", bad)]))
                .unwrap_err()
                .message(),
            "筛选包含未注册的机器",
            "{bad}"
        );
    }
}

#[test]
fn upstream_drops_hub_keys_and_groups_repeated_keys_like_parse_qs() {
    let query = q(&[
        ("q", "a"),
        ("nodes", NID_A),
        ("limit", "5"),
        ("q", "b"),
        ("sig", "x"),
        ("progress", "1"),
        ("force", ""),
    ]);
    assert_eq!(
        upstream(&query),
        q(&[("q", "a"), ("q", "b"), ("limit", "5"), ("force", "")])
    );
    assert!(progress_requested(&query));
    assert!(!progress_requested(&q(&[("progress", "0")])));
    assert_eq!(first(&query, "q"), Some("a"));
    assert_eq!(first(&query, "missing"), None);
}

#[test]
fn rows_merge_newest_first_with_uid_as_the_tie_breaker() {
    let (_dir, registry) = registry();
    let answers = [
        answer(
            &registry,
            NID_A,
            json!({"sessions": [
            {"uid": "claude:a~1", "updated": "2026-09-01"}, {"uid": "claude:a~2", "updated": "2026-09-03"},
            {"uid": "claude:a~0"}]}),
            None,
        ),
        answer(
            &registry,
            NID_B,
            json!({"sessions": [
            {"uid": "claude:b~1", "updated": "2026-09-01"}, {"uid": "claude:b~9", "updated": "2026-09-03"}]}),
            None,
        ),
    ];
    let uids: Vec<String> = merged_rows(&answers, "sessions")
        .iter()
        .map(|row| row["uid"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        uids,
        [
            "claude:b~9",
            "claude:a~2",
            "claude:b~1",
            "claude:a~1",
            "claude:a~0"
        ]
    );
}

#[test]
fn sums_stay_integers_until_a_float_appears() {
    assert_eq!(
        sum([Some(&json!(1)), None, Some(&json!(2))].into_iter()),
        json!(3)
    );
    assert_eq!(
        sum([Some(&json!(1)), Some(&json!(2.5))].into_iter()),
        json!(3.5)
    );
    assert_eq!(
        sum([Some(&json!("x")), Some(&json!(null))].into_iter()),
        json!(0)
    );
    assert_eq!(sum(std::iter::empty()), json!(0));
}

#[test]
fn signature_tracks_rows_and_node_identity_but_not_health_details() {
    let (_dir, registry) = registry();
    let body = |rows: Value, online: Value| {
        let mut result = Map::new();
        result.insert("errors".into(), json!([]));
        result.insert("partial".into(), json!(false));
        result.insert(
            "nodes".into(),
            json!([{"id": NID_A, "name": "A", "color": "blue", "online": online, "last_seen": 1.0}]),
        );
        result.insert("sessions".into(), rows);
        result
    };
    let base = signature(&body(json!([{"uid": "x"}]), json!(true)));
    assert_eq!(base.len(), 24);
    assert!(base.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(base, signature(&body(json!([{"uid": "x"}]), json!(true))));
    assert_ne!(base, signature(&body(json!([{"uid": "y"}]), json!(true))));
    assert_ne!(base, signature(&body(json!([{"uid": "x"}]), json!(false))));
    // Only id/name/online of a node row are signed.
    let mut other = body(json!([{"uid": "x"}]), json!(true));
    other["nodes"][0]["color"] = json!("rose");
    other["nodes"][0]["last_seen"] = json!(2.0);
    assert_eq!(base, signature(&other));
    // Key order never matters.
    let mut reordered = Map::new();
    reordered.insert("sessions".into(), json!([{"uid": "x"}]));
    reordered.insert("nodes".into(), other["nodes"].clone());
    reordered.insert("partial".into(), json!(false));
    reordered.insert("errors".into(), json!([]));
    assert_eq!(base, signature(&reordered));
    let _ = registry;
}

#[test]
fn sessions_body_signs_and_answers_unchanged_for_a_matching_sig() {
    let (_dir, registry) = registry();
    let answers = [
        answer(
            &registry,
            NID_A,
            json!({"sessions": [{"uid": "claude:a~1", "updated": "1"}], "truncated": true,
                                        "total_pool": 3}),
            None,
        ),
        answer(
            &registry,
            NID_B,
            json!({"sessions": [{"uid": "claude:b~1", "updated": "2", "stale": true}]}),
            Some(failure(NID_B, "B")),
        ),
    ];
    let full = sessions_body(&registry, &answers, "");
    let keys: Vec<&str> = full
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "errors",
            "partial",
            "nodes",
            "sessions",
            "truncated",
            "truncated_nodes",
            "total_pool",
            "sig",
            "built_at"
        ]
    );
    assert_eq!(full["partial"], true);
    assert_eq!(full["errors"], json!([failure(NID_B, "B")]));
    assert_eq!(full["truncated"], true);
    assert_eq!(full["truncated_nodes"], json!([NID_A]));
    assert_eq!(full["total_pool"], 3);
    assert_eq!(full["sessions"][0]["uid"], "claude:b~1");
    assert_eq!(full["nodes"].as_array().unwrap().len(), 2);
    assert!(full["built_at"].as_f64().unwrap() > 1.0e9);
    let sig = full["sig"].as_str().unwrap();
    let same = sessions_body(&registry, &answers, sig);
    assert_eq!(
        same,
        json!({"unchanged": true, "sig": sig, "nodes": full["nodes"], "errors": full["errors"]})
    );
    assert_ne!(
        sessions_body(&registry, &answers, "stale")["unchanged"],
        true
    );
}

#[test]
fn live_body_only_counts_nodes_that_answered() {
    let (_dir, registry) = registry();
    let answers = [
        answer(
            &registry,
            NID_A,
            json!({"uids": ["claude:a~1"], "tmux_uids": ["claude:a~2"],
                                        "started_at": {"claude:a~1": 5}}),
            None,
        ),
        answer(
            &registry,
            NID_B,
            json!({"uids": ["claude:b~1"], "started_at": {"claude:b~1": 6}}),
            Some(failure(NID_B, "B")),
        ),
    ];
    let body = live_body(&registry, &answers);
    assert_eq!(body["uids"], json!(["claude:a~1"]));
    assert_eq!(body["tmux_uids"], json!(["claude:a~2"]));
    assert_eq!(body["started_at"], json!({"claude:a~1": 5}));
    assert_eq!(body["partial"], true);
}

#[test]
fn term_list_body_reports_capabilities_per_machine_and_unions_sources() {
    let (_dir, registry) = registry();
    let answers = [
        answer(
            &registry,
            NID_A,
            json!({"enabled": true, "unavailable_reason": "", "home": "/home/u/a",
            "sources": {"claude": true, "codex": false}, "sessions": [{"name": "a~t"}], "pending": [],
            "backend": "tmux", "backends": [{"name": "tmux"}]}),
            None,
        ),
        answer(
            &registry,
            NID_B,
            json!({"enabled": false, "sources": {}, "sessions": [{"name": "b~t", "stale": true}],
            "pending": [{"name": "b~p", "stale": true}], "backend": "ptyhost", "backends": [{"name": "ptyhost"}]}),
            Some(failure(NID_B, "B")),
        ),
    ];
    let body = term_list_body(&registry, &answers);
    let keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "errors",
            "partial",
            "nodes",
            "enabled",
            "home",
            "sources",
            "sessions",
            "pending",
            "capabilities"
        ]
    );
    assert_eq!(body["enabled"], true);
    assert_eq!(body["home"], "");
    assert_eq!(body["sources"], json!({"claude": true, "codex": false}));
    assert_eq!(
        body["sessions"],
        json!([{"name": "a~t"}, {"name": "b~t", "stale": true}])
    );
    assert_eq!(body["pending"], json!([{"name": "b~p", "stale": true}]));
    assert_eq!(
        body["capabilities"][NID_A],
        json!({"enabled": true, "unavailable_reason": "", "sources": {"claude": true, "codex": false},
               "home": "/home/u/a", "backend": "tmux", "backends": [{"name": "tmux"}]})
    );
    assert_eq!(
        body["capabilities"][NID_B],
        json!({"enabled": false, "unavailable_reason": "节点暂时离线", "sources": {}, "home": "",
               "backend": "ptyhost", "backends": []})
    );
    // `sources.get(source, False) or available`: a later true wins, a later false does not erase.
    let answers = [
        answer(
            &registry,
            NID_A,
            json!({"enabled": false, "unavailable_reason": "no tmux",
            "sources": {"claude": false, "codex": true}}),
            None,
        ),
        answer(
            &registry,
            NID_B,
            json!({"enabled": true, "sources": {"claude": true, "codex": false}}),
            None,
        ),
    ];
    let body = term_list_body(&registry, &answers);
    assert_eq!(body["sources"], json!({"claude": true, "codex": true}));
    assert_eq!(body["enabled"], true);
    assert_eq!(body["capabilities"][NID_A]["unavailable_reason"], "no tmux");
    assert_eq!(body["capabilities"][NID_A]["enabled"], false);
    assert_eq!(body["capabilities"][NID_B]["backends"], json!([]));
    assert_eq!(body["sessions"], json!([]));
}

#[test]
fn trash_body_concatenates_items_and_sums_sizes() {
    let (_dir, registry) = registry();
    let answers = [
        answer(
            &registry,
            NID_A,
            json!({"items": [{"id": "a~x"}], "size": 10}),
            None,
        ),
        answer(&registry, NID_B, json!({}), Some(failure(NID_B, "B"))),
    ];
    let body = trash_body(&registry, &answers);
    assert_eq!(body["items"], json!([{"id": "a~x"}]));
    assert_eq!(body["size"], 10);
    assert_eq!(body["dir"], TRASH_DIR);
    assert_eq!(body["partial"], true);
}

#[test]
fn progress_lines_sum_known_totals_and_flag_unknown_ones() {
    let (_dir, registry) = registry();
    let mut scans: IndexMap<String, Scan> = IndexMap::new();
    scans.insert(
        NID_A.into(),
        Scan {
            node: registry.find(NID_A).unwrap(),
            done: json!(1),
            total: Value::Null,
            state: "preparing",
        },
    );
    scans.insert(
        NID_B.into(),
        Scan {
            node: registry.find(NID_B).unwrap(),
            done: json!(2),
            total: json!(0),
            state: "offline",
        },
    );
    let event: Value = serde_json::from_str(&progress_line(&scans)).unwrap();
    assert_eq!(
        event,
        json!({"type": "progress", "done": 3, "total": 0, "total_known": false, "nodes": [
            {"id": NID_A, "name": "A", "done": 1, "total": null, "state": "preparing"},
            {"id": NID_B, "name": "B", "done": 2, "total": 0, "state": "offline"}]})
    );
    scans[NID_A].total = json!(10);
    scans[NID_A].state = "scanning";
    let event: Value = serde_json::from_str(&progress_line(&scans)).unwrap();
    assert_eq!(
        (event["total"].clone(), event["total_known"].clone()),
        (json!(10), json!(true))
    );
    assert!(progress_line(&scans).ends_with('\n'));
}

#[test]
fn bulk_uids_group_by_machine_in_first_appearance_order() {
    let a1 = format!("claude:{NID_A}~one");
    let b1 = format!("codex:{NID_B}~two");
    let a2 = format!("claude:{NID_A}~three");
    let groups = group_uids(&json!({"uids": [a1, b1, a2]}), "没有选中任何会话").unwrap();
    assert_eq!(
        groups.into_iter().collect::<Vec<_>>(),
        [
            (
                NID_A.to_string(),
                vec!["claude:one".to_string(), "claude:three".to_string()]
            ),
            (NID_B.to_string(), vec!["codex:two".to_string()]),
        ]
    );
    assert_eq!(
        group_uids(&json!({}), "没有选中任何会话")
            .unwrap_err()
            .message(),
        "没有选中任何会话"
    );
    assert_eq!(
        group_uids(&json!({"uids": []}), "没有选中任何父会话")
            .unwrap_err()
            .message(),
        "没有选中任何父会话"
    );
    assert_eq!(
        group_uids(&json!({"uids": ["claude:local"]}), "x")
            .unwrap_err()
            .message(),
        "missing or invalid machine reference"
    );
    assert_eq!(
        group_uids(&json!({"uids": ["nosource"]}), "x")
            .unwrap_err()
            .message(),
        "missing session source"
    );
}
