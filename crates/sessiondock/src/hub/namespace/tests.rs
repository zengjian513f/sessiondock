use serde_json::json;

use super::*;

const NID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn node() -> Node {
    Node {
        url: "http://127.0.0.1:1".into(),
        token: "t".repeat(32),
        id: NID.into(),
        name: "A".into(),
        color: None,
        enabled: None,
        renderer: None,
    }
}

#[test]
fn qualify_and_split_round_trip_uids_terminal_names_and_epochs() {
    let scoped = qualify(NID, "codex:abc", true).unwrap();
    assert_eq!(scoped, format!("codex:{NID}~abc"));
    assert_eq!(
        split(&scoped, true).unwrap(),
        (NID.to_string(), "codex:abc".to_string())
    );
    // A uid tail may itself contain ':' and '~'; only the first of each splits.
    let odd = qualify(NID, "claude:a:b~c", true).unwrap();
    assert_eq!(odd, format!("claude:{NID}~a:b~c"));
    assert_eq!(split(&odd, true).unwrap().1, "claude:a:b~c");
    let name = qualify(NID, "same-terminal", false).unwrap();
    assert_eq!(name, format!("{NID}~same-terminal"));
    assert_eq!(
        split(&name, false).unwrap(),
        (NID.to_string(), "same-terminal".to_string())
    );
    assert_eq!(qualify(NID, "", true).unwrap(), "");
    assert_eq!(qualify(NID, "", false).unwrap(), "");
}

#[test]
fn malformed_references_carry_the_python_messages() {
    assert_eq!(
        qualify(NID, "no-source", true).unwrap_err().message(),
        "invalid session reference"
    );
    assert_eq!(
        split("no-source", true).unwrap_err().message(),
        "missing session source"
    );
    for value in [
        "claude:abc",
        &format!("claude:{}~abc", "z".repeat(32)),
        &format!("claude:{NID}~"),
        &format!("claude:{}~abc", &NID[..31]),
    ] {
        assert_eq!(
            split(value, true).unwrap_err().message(),
            "missing or invalid machine reference",
            "{value}"
        );
    }
    assert_eq!(
        split("plain", false).unwrap_err().message(),
        "missing or invalid machine reference"
    );
}

#[test]
fn file_metadata_does_not_rewrite_reference_names() {
    let payload = json!({
        "resolved": {"uid": "/project/uid", "epoch": "/project/epoch"},
        "targets": [{"ref": "uid", "path": "/project/uid", "kind": "file"}],
        "node_id": NID,
    });
    assert_eq!(
        public_payload(payload.clone(), &node(), "/api/session/resolve-files"),
        payload
    );
}

#[test]
fn payload_boundaries_keep_text_input_and_native_ids() {
    let data = json!({
        "meta": {"uid": "codex:abc", "cwd": "/same"},
        "messages": [{
            "id": "native-message", "text": "/api/media/do-not-rewrite",
            "input": {"uid": "arbitrary user input"}, "media": [{"src": "/api/media/token"}],
        }],
    });
    let out = public_payload(data.clone(), &node(), "/api/watch");
    assert_eq!(out["meta"]["node_id"], NID);
    assert_eq!(out["meta"]["node_name"], "A");
    assert_eq!(out["meta"]["uid"], format!("codex:{NID}~abc"));
    assert_eq!(out["messages"][0]["input"], data["messages"][0]["input"]);
    assert_eq!(out["messages"][0]["id"], "native-message");
    assert_eq!(out["messages"][0]["text"], "/api/media/do-not-rewrite");
    assert_eq!(
        out["messages"][0]["media"][0]["src"],
        format!("/api/nodes/{NID}/api/media/token")
    );
}

#[test]
fn continued_in_is_qualified_like_uid() {
    let data =
        json!({"sessions": [{"uid": "claude:old", "continued_in": "claude:new", "title": "t"}]});
    let out = public_payload(data, &node(), "/api/sessions");
    assert_eq!(out["sessions"][0]["uid"], format!("claude:{NID}~old"));
    assert_eq!(
        out["sessions"][0]["continued_in"],
        format!("claude:{NID}~new")
    );
    assert_eq!(out["sessions"][0]["node_name"], "A");
}

#[test]
fn live_keys_are_always_present_and_scoped() {
    let out = public_payload(
        json!({"uids": ["claude:a"], "started_at": {"claude:a": 1.5}}),
        &node(),
        "/api/live",
    );
    assert_eq!(
        out,
        json!({
            "uids": [format!("claude:{NID}~a")], "tmux_uids": [],
            "started_at": {format!("claude:{NID}~a"): 1.5},
        })
    );
}

#[test]
fn terminal_names_and_trash_ids_are_scoped_only_where_python_scopes_them() {
    let out = public_payload(
        json!({"sessions": [{"name": "term", "uid": "claude:x", "current_uid": "codex:y"}], "pending": [{"name": ""}]}),
        &node(),
        "/api/term/list",
    );
    assert_eq!(out["sessions"][0]["name"], format!("{NID}~term"));
    assert_eq!(out["sessions"][0]["uid"], format!("claude:{NID}~x"));
    assert_eq!(out["sessions"][0]["current_uid"], format!("codex:{NID}~y"));
    assert_eq!(out["pending"][0]["name"], "");
    assert_eq!(out["pending"][0]["node_id"], NID);
    let out = public_payload(
        json!({"items": [{"id": "claude/x", "name": "n"}]}),
        &node(),
        "/api/trash",
    );
    assert_eq!(out["items"][0]["id"], format!("{NID}~claude/x"));
    assert_eq!(out["items"][0]["name"], "n");
    // The same rows under another path are only stamped, never scoped.
    let out = public_payload(
        json!({"items": [{"id": "claude/x"}]}),
        &node(),
        "/api/other",
    );
    assert_eq!(out["items"][0], json!({"id": "claude/x"}));
    let out = public_payload(
        json!({"name": "term", "session": {"uid": "claude:x", "name": "keep"}}),
        &node(),
        "/api/term/create",
    );
    assert_eq!(out["name"], format!("{NID}~term"));
    assert_eq!(out["session"]["name"], "keep");
    assert_eq!(out["session"]["node_id"], NID);
    let out = public_payload(
        json!({"worker": {"name": "w"}, "ok": true}),
        &node(),
        "/api/bug-report",
    );
    assert_eq!(out["worker"]["name"], format!("{NID}~w"));
    assert!(out.get("node_id").is_none());
}

#[test]
fn epoch_and_src_rewrite_everywhere_but_not_inside_opaque_subtrees() {
    let out = public_payload(
        json!({
            "outbox_version": {"epoch": "e1", "revision": 2},
            "data": {"epoch": "raw", "src": "/api/media/x", "uid": "claude:y"},
            "nested": [{"content": [{"uid": "claude:z"}], "from_uid": "claude:p", "to_uid": ""}],
            "src": "/api/other/x", "epoch": "",
        }),
        &node(),
        "/api/session/outbox",
    );
    assert_eq!(out["outbox_version"]["epoch"], format!("{NID}~e1"));
    assert_eq!(
        out["data"],
        json!({"epoch": "raw", "src": "/api/media/x", "uid": "claude:y"})
    );
    assert_eq!(out["nested"][0]["content"], json!([{"uid": "claude:z"}]));
    assert_eq!(out["nested"][0]["from_uid"], format!("claude:{NID}~p"));
    assert_eq!(out["nested"][0]["to_uid"], "");
    assert_eq!(out["src"], "/api/other/x");
    assert_eq!(out["epoch"], "");
}

#[test]
fn strict_rewrite_rejects_unscopable_references_and_lenient_keeps_them() {
    let data = json!({"sessions": [{"uid": "no-source"}]});
    assert_eq!(
        try_public_payload(data.clone(), &node(), "/api/sessions")
            .unwrap_err()
            .message(),
        "invalid session reference"
    );
    let out = public_payload(data, &node(), "/api/sessions");
    assert_eq!(out["sessions"][0]["uid"], "no-source");
    assert_eq!(out["sessions"][0]["node_id"], NID);
    assert_eq!(
        try_public_payload(json!({"uids": ["bad"]}), &node(), "/api/live")
            .unwrap_err()
            .message(),
        "invalid session reference"
    );
    assert_eq!(
        public_payload(json!({"started_at": {"bad": 1}}), &node(), "/api/live")["started_at"],
        json!({"bad": 1})
    );
}

#[test]
fn scalars_and_arrays_at_the_top_level_pass_through() {
    assert_eq!(
        public_payload(json!(null), &node(), "/api/sessions"),
        json!(null)
    );
    assert_eq!(
        public_payload(json!("uid"), &node(), "/api/trash"),
        json!("uid")
    );
    assert_eq!(
        public_payload(json!([{"uid": "claude:a"}]), &node(), "/api/sessions"),
        json!([{"uid": format!("claude:{NID}~a")}])
    );
}
