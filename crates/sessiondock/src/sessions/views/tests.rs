//! `Views` against private temporary roots: on-demand builds, incremental
//! append, rewrite/pin/row invalidation, inherited prefixes through the
//! dependency callback (checked against the frozen-inventory reference
//! resolver), agent selection, LRU bounds and transient scans.
use super::*;
use crate::metadata::TimelinePin;
use crate::sessions::{MessageQuery, SessionError, hash, history};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const NOW: &str = "2026-09-11T00:00:00.000Z";

fn codex_header(sid: &str) -> Value {
    json!({"type":"session_meta", "timestamp":NOW,
        "payload":{"id":sid,"session_id":sid,"timestamp":NOW,"cwd":"/synthetic/project","thread_source":"user"}})
}
fn codex_message(role: &str, text: &str) -> Value {
    json!({"type":"response_item","timestamp":NOW,"payload":{
        "type":"message","role":role,"turn_id":"synthetic-turn",
        "content":[{"type":"input_text","text":text}]}})
}
fn codex_fork(sid: &str, parent: &str, cut: usize) -> Value {
    let mut row = codex_header(sid);
    row["payload"]["forked_from_id"] = json!(parent);
    row["payload"]["history_base"] = json!({"thread_id":parent,"end_byte_offset":cut});
    row
}
fn claude_row(id: &str, parent: Value, role: &str, text: &str) -> Value {
    json!({"type": role, "uuid": id, "parentUuid": parent,
           "sessionId": "synthetic-session", "cwd": "/workspace/demo",
           "timestamp": NOW,
           "message": {"role": role, "content": [{"type": "text", "text": text}]}})
}
fn encoded(records: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for record in records {
        bytes.extend(serde_json::to_vec(record).unwrap());
        bytes.push(b'\n');
    }
    bytes
}
fn write(path: &Path, records: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, encoded(records)).unwrap();
}
fn append(path: &Path, records: &[Value]) {
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(&encoded(records))
        .unwrap();
}
pub(super) fn candidate(source: &'static str, root: &Path, path: &Path) -> Candidate {
    restamp(&Candidate {
        source,
        root: root.to_path_buf(),
        path: path.to_path_buf(),
        data: path.to_path_buf(),
        summary: None,
        stamps: Vec::new(),
    })
    .unwrap()
}
fn row(uid: &str, title: &str) -> Value {
    json!({"uid": uid, "title": title, "source": "codex", "supported": true,
        "cwd": "/synthetic/project", "agent_items": []})
}
fn request(uid: &str, owner: &Candidate, title: &str) -> ViewRequest {
    ViewRequest {
        uid: uid.to_owned(),
        agent: String::new(),
        owner: owner.clone(),
        selected: None,
        pin: None,
        row: row(uid, title),
    }
}
fn texts(snapshot: &ViewSnapshot) -> Vec<String> {
    snapshot.texts().map(|(_, text)| text.to_owned()).collect()
}
fn continuation(batch: &Value) -> MessageQuery {
    MessageQuery {
        start: batch["end"].as_u64().unwrap(),
        head: batch["version"]["head"].as_str().unwrap().to_owned(),
        anchor: batch["anchor"].as_str().unwrap().to_owned(),
        ..Default::default()
    }
}

/// Dependencies over a fixed thread-id map, the way the index answers them.
#[derive(Default)]
pub(super) struct MapDeps {
    threads: BTreeMap<String, Candidate>,
    ambiguous: BTreeSet<String>,
}
impl Dependencies for MapDeps {
    fn thread(&self, source: &str, thread_id: &str) -> Result<Candidate, SessionError> {
        assert_eq!(source, "codex");
        if self.ambiguous.contains(thread_id) {
            return Err(SessionError::new(409, "父线程 ID 在已配置索引中存在歧义"));
        }
        self.threads
            .get(thread_id)
            .cloned()
            .ok_or_else(|| SessionError::new(501, "父线程不在已配置索引中"))
    }
}

struct CodexForkFixture {
    _temp: TempDir,
    root: PathBuf,
    parent: PathBuf,
    child: PathBuf,
    cut: usize,
}
fn codex_fork_fixture() -> CodexForkFixture {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap().join("codex");
    let prefix = [codex_header("parent-sid"), codex_message("user", "kept")];
    let cut = encoded(&prefix).len();
    let parent = root.join("2026/09/11/rollout-parent.jsonl");
    write(
        &parent,
        &[
            prefix[0].clone(),
            prefix[1].clone(),
            codex_message("user", "old tail"),
        ],
    );
    let child = root.join("2026/09/11/rollout-child.jsonl");
    write(
        &child,
        &[
            codex_fork("child-sid", "parent-sid", cut),
            codex_message("user", "new branch"),
        ],
    );
    CodexForkFixture {
        _temp: temp,
        root,
        parent,
        child,
        cut,
    }
}
fn deps_for(fixture: &CodexForkFixture) -> MapDeps {
    let mut deps = MapDeps::default();
    deps.threads.insert(
        "parent-sid".into(),
        candidate("codex", &fixture.root, &fixture.parent),
    );
    deps
}
fn child_request(fixture: &CodexForkFixture) -> ViewRequest {
    let owner = candidate("codex", &fixture.root, &fixture.child);
    request(&uid_for("codex", &fixture.child), &owner, "child")
}

#[test]
fn open_streams_one_file_and_appends_extend_from_the_committed_offset() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap().join("codex");
    let path = root.join("2026/09/11/rollout-a.jsonl");
    write(&path, &[codex_header("a"), codex_message("user", "first")]);
    let owner = candidate("codex", &root, &path);
    let uid = uid_for("codex", &path);
    let mut views = Views::new();
    let deps = MapDeps::default();
    let first = views.open(&request(&uid, &owner, "A"), &deps).unwrap();
    assert_eq!(texts(&first), ["first"]);
    assert_eq!(first.metadata()["title"], "A");
    assert_eq!(views.stats().views, 1);
    assert_eq!(views.stats().files, 1);
    let batch = first.messages(&MessageQuery::default()).unwrap();
    assert_eq!(batch["end"], fs::metadata(&path).unwrap().len());
    // Unchanged file, unchanged row: the very same immutable snapshot.
    let again = views.open(&request(&uid, &owner, "A"), &deps).unwrap();
    assert!(Arc::ptr_eq(&first, &again));
    // A stale request stamp is restamped here, never trusted.
    append(&path, &[codex_message("assistant", "second")]);
    let decoded = views.records().decoded;
    let extended = views.open(&request(&uid, &owner, "A"), &deps).unwrap();
    assert_eq!(texts(&extended), ["first", "second"]);
    assert_eq!(views.records().decoded - decoded, 1);
    assert_eq!(views.records().reused, 2);
    let delta = extended.messages(&continuation(&batch)).unwrap();
    assert_eq!(delta["reset"], false);
    assert_eq!(delta["messages"].as_array().unwrap().len(), 1);
    assert_eq!(views.stats().files, 1);
    assert_eq!(views.stats().views, 1);
}

#[test]
fn rewrite_rebuilds_and_resets_while_row_changes_recompose_without_reparse() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap().join("codex");
    let path = root.join("2026/09/11/rollout-a.jsonl");
    write(&path, &[codex_header("a"), codex_message("user", "first")]);
    let owner = candidate("codex", &root, &path);
    let uid = uid_for("codex", &path);
    let mut views = Views::new();
    let deps = MapDeps::default();
    let first = views.open(&request(&uid, &owner, "A"), &deps).unwrap();
    let batch = first.messages(&MessageQuery::default()).unwrap();
    // Same row, new title: no file access beyond stat, same parsed Arc.
    let renamed = views.open(&request(&uid, &owner, "B"), &deps).unwrap();
    assert!(Arc::ptr_eq(&first.view.parsed, &renamed.view.parsed));
    assert_ne!(first.revision(), renamed.revision());
    assert_eq!(renamed.metadata()["title"], "B");
    assert_eq!(renamed.anchor, first.anchor);
    assert_eq!(views.records().decoded, 2);
    // A rewrite of the committed prefix is a rebuild and a cursor reset.
    write(
        &path,
        &[codex_header("a"), codex_message("user", "changed")],
    );
    let rewritten = views.open(&request(&uid, &owner, "B"), &deps).unwrap();
    assert_eq!(texts(&rewritten), ["changed"]);
    let reset = rewritten.messages(&continuation(&batch)).unwrap();
    assert_eq!(reset["reset"], true);
    assert!(views.records().decoded > 2);
}

#[test]
fn pin_changes_the_logical_view_and_its_anchor() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap().join("claude");
    let path = root.join("project/session.jsonl");
    write(
        &path,
        &[
            claude_row("u1", Value::Null, "user", "one"),
            claude_row("a1", json!("u1"), "assistant", "two"),
            claude_row("u2", json!("a1"), "user", "three"),
        ],
    );
    let owner = candidate("claude", &root, &path);
    let uid = uid_for("claude", &path);
    let mut views = Views::new();
    let deps = MapDeps::default();
    let mut request = request(&uid, &owner, "C");
    let natural = views.open(&request, &deps).unwrap();
    assert_eq!(texts(&natural), ["one", "two", "three"]);
    assert!(natural.metadata().get("timeline_pin").is_none());
    request.pin = Some(TimelinePin {
        tip: "a1".into(),
        stale_end: fs::metadata(&path).unwrap().len(),
        target: Some("u2".into()),
        pinned_at: None,
    });
    let pinned = views.open(&request, &deps).unwrap();
    assert_eq!(texts(&pinned), ["one", "two"]);
    assert_eq!(pinned.metadata()["timeline_pin"]["tip"], "a1");
    assert_eq!(pinned.metadata()["timeline_pin"]["native_rewind"], false);
    assert_ne!(pinned.anchor, natural.anchor);
    assert!(!Arc::ptr_eq(&pinned.view.parsed, &natural.view.parsed));
    // Same file, same pin: cached.
    let again = views.open(&request, &deps).unwrap();
    assert!(Arc::ptr_eq(&pinned, &again));
    request.pin = None;
    let unpinned = views.open(&request, &deps).unwrap();
    assert_eq!(unpinned.anchor, natural.anchor);
}

#[test]
fn fork_child_inherits_the_parent_prefix_exactly_like_the_reference_resolver() {
    let fixture = codex_fork_fixture();
    let deps = deps_for(&fixture);
    let mut views = Views::new();
    let request = child_request(&fixture);
    let view = views.open(&request, &deps).unwrap();
    assert_eq!(texts(&view), ["kept", "new branch"]);
    let events = view.view.all_events();
    assert_eq!(events[0].end, 0);
    assert_eq!(events[1].end as usize, view.view.parsed.committed);
    assert_eq!(view.view.sources.len(), 2);
    assert_eq!(view.view.sources[1].path, fixture.parent);
    // Reference: the frozen-inventory resolver over full parses of both files.
    let mut cache = records::RecordCache::default();
    let inventory: BTreeMap<String, Arc<Parsed>> = [&fixture.parent, &fixture.child]
        .into_iter()
        .map(|path| {
            let candidate = candidate("codex", &fixture.root, path);
            (
                uid_for("codex", path),
                Arc::new(parse_candidate(candidate, None, &mut cache, None).unwrap()),
            )
        })
        .collect();
    let reference = history::resolve(&inventory, &request.uid, "").unwrap();
    assert_eq!(reference.identity, view.view.identity);
    assert_eq!(
        reference
            .events()
            .map(|event| event.message.clone())
            .collect::<Vec<_>>(),
        view.view
            .events()
            .map(|event| event.message.clone())
            .collect::<Vec<_>>()
    );
    let reference = ViewSnapshot::new(Arc::new(reference));
    assert_eq!(reference.anchor, view.anchor);
    assert_eq!(reference.head, view.head);
    let _ = fixture.cut;
}

#[test]
fn parent_tail_append_keeps_child_identity_and_prefix_rewrite_changes_it() {
    let fixture = codex_fork_fixture();
    let deps = deps_for(&fixture);
    let mut views = Views::new();
    let request = child_request(&fixture);
    let first = views.open(&request, &deps).unwrap();
    let batch = first.messages(&MessageQuery::default()).unwrap();
    // Parent tail appends do not touch the fixed prefix: same identity,
    // and the child's own append is an ordinary delta.
    append(
        &fixture.parent,
        &[codex_message("assistant", "later parent")],
    );
    append(&fixture.child, &[codex_message("assistant", "child reply")]);
    let stale_deps = deps; // the index may still carry the old parent stamp
    let second = views.open(&request, &stale_deps).unwrap();
    assert_eq!(second.view.identity, first.view.identity);
    assert_eq!(texts(&second), ["kept", "new branch", "child reply"]);
    let delta = second.messages(&continuation(&batch)).unwrap();
    assert_eq!(delta["reset"], false);
    assert_eq!(delta["messages"][0]["text"], "child reply");
    // Rewriting inside the parent's prefix (same length) changes the
    // inherited digest, the identity and therefore the anchor.
    let bytes = fs::read(&fixture.parent).unwrap();
    let rewritten = String::from_utf8(bytes).unwrap().replace("kept", "KEPT");
    fs::write(&fixture.parent, rewritten).unwrap();
    let third = views.open(&request, &stale_deps).unwrap();
    assert_ne!(third.view.identity, first.view.identity);
    assert_eq!(texts(&third), ["KEPT", "new branch", "child reply"]);
    let reset = third
        .messages(&continuation(
            &second.messages(&MessageQuery::default()).unwrap(),
        ))
        .unwrap();
    assert_eq!(reset["reset"], true);
}

#[test]
fn missing_ambiguous_cyclic_and_out_of_range_parents_fail_closed() {
    let fixture = codex_fork_fixture();
    let mut views = Views::new();
    let child = child_request(&fixture);
    let error = views.open(&child, &MapDeps::default()).unwrap_err();
    assert_eq!(error.status, 501);
    let mut deps = deps_for(&fixture);
    deps.ambiguous.insert("parent-sid".into());
    assert_eq!(views.open(&child, &deps).unwrap_err().status, 409);
    // A parent that declares the child as its own parent is a cycle.
    let mut deps = deps_for(&fixture);
    write(
        &fixture.parent,
        &[
            codex_fork("parent-sid", "child-sid", 10),
            codex_message("user", "kept"),
        ],
    );
    deps.threads.insert(
        "child-sid".into(),
        candidate("codex", &fixture.root, &fixture.child),
    );
    deps.threads.insert(
        "parent-sid".into(),
        candidate("codex", &fixture.root, &fixture.parent),
    );
    let error = views.open(&child, &deps).unwrap_err();
    assert_eq!(error.status, 501, "{}", error.message);
    // A cut past the parent or off a line boundary is unsupported, not a read.
    let child_over = fixture.root.join("2026/09/11/rollout-over.jsonl");
    write(
        &child_over,
        &[
            codex_fork("over-sid", "parent-sid", 1 << 30),
            codex_message("user", "x"),
        ],
    );
    let over = request(
        &uid_for("codex", &child_over),
        &candidate("codex", &fixture.root, &child_over),
        "over",
    );
    assert_eq!(views.open(&over, &deps).unwrap_err().status, 501);
    let child_mid = fixture.root.join("2026/09/11/rollout-mid.jsonl");
    write(
        &child_mid,
        &[
            codex_fork("mid-sid", "parent-sid", 7),
            codex_message("user", "x"),
        ],
    );
    let mid = request(
        &uid_for("codex", &child_mid),
        &candidate("codex", &fixture.root, &child_mid),
        "mid",
    );
    assert_eq!(views.open(&mid, &deps).unwrap_err().status, 501);
    assert_eq!(views.stats().views, 0);
}

/// Old-style fork (2026-07/08): own meta (`history_base` null,
/// `forked_from_id` = parent), the ancestors' metas copied after it, then the
/// copied history and the own records. The file is read alone
/// (`_history_segments` → `[]`) and every meta after the first is ignored.
#[test]
fn legacy_fork_with_copied_metas_opens_alone_without_a_parent() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap().join("codex");
    let child = root.join("2026/07/01/rollout-legacy.jsonl");
    let mut own = codex_header("child-sid");
    own["payload"]["forked_from_id"] = json!("parent-sid");
    own["payload"]["history_base"] = Value::Null;
    let mut parent = codex_header("parent-sid");
    parent["payload"]["forked_from_id"] = json!("grandparent-sid");
    parent["payload"]["history_base"] = Value::Null;
    write(
        &child,
        &[
            own,
            parent,
            codex_header("grandparent-sid"),
            codex_header("root-sid"),
            codex_message("user", "copied question"),
            codex_message("assistant", "copied answer"),
            codex_message("user", "own question"),
        ],
    );
    let owner = candidate("codex", &root, &child);
    let legacy = request(&uid_for("codex", &child), &owner, "legacy");
    let mut views = Views::new();
    // No parent is indexed: nothing is inherited, so none is needed.
    let view = views.open(&legacy, &MapDeps::default()).unwrap();
    assert_eq!(
        texts(&view),
        ["copied question", "copied answer", "own question"]
    );
    assert!(view.view.inherited.is_empty());
    assert_eq!(view.view.sources.len(), 1);
    assert!(view.view.parsed.unsupported.is_none());
    assert_eq!(view.view.parsed.meta["sid"], "child-sid");
    assert_eq!(view.view.parsed.native_id.as_deref().unwrap(), "child-sid");
    assert_eq!(
        view.view.parsed.meta["migration_warnings"],
        json!(["跳过重复的Codex session_meta ×3"])
    );
    assert_eq!(
        view.view.identity,
        history::native_identity(&view.view.parsed, ""),
        "self-contained: no inherited digest enters the identity"
    );
    let batch = view.messages(&MessageQuery::default()).unwrap();
    assert_eq!(batch["message_total"], 3);
    assert!(
        batch["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message["role"] != "status")
    );
    // The frozen-inventory reference resolver agrees.
    let mut cache = records::RecordCache::default();
    let inventory: BTreeMap<String, Arc<Parsed>> = [(
        legacy.uid.clone(),
        Arc::new(parse_candidate(owner, None, &mut cache, None).unwrap()),
    )]
    .into_iter()
    .collect();
    let reference = history::resolve(&inventory, &legacy.uid, "").unwrap();
    assert_eq!(reference.identity, view.view.identity);
    assert_eq!(
        reference
            .events()
            .map(|event| event.message.clone())
            .collect::<Vec<_>>(),
        view.view
            .events()
            .map(|event| event.message.clone())
            .collect::<Vec<_>>()
    );
}

/// A rewind past the parent's own fork point: `history_base.thread_id` names
/// the physical file holding the cut, `forked_from_id` the logical parent.
/// The prefix is read from `history_base.thread_id`; the logical parent
/// is never opened. Cut validation is unchanged.
#[test]
fn history_base_thread_id_wins_over_forked_from_id_for_the_prefix() {
    let fixture = codex_fork_fixture();
    // R = the fixture parent (holds the fork point); Q = another indexed
    // thread that must not be read.
    let logical = fixture.root.join("2026/09/11/rollout-logical.jsonl");
    write(
        &logical,
        &[
            codex_fork("logical-sid", "parent-sid", fixture.cut),
            codex_message("user", "logical parent tail"),
        ],
    );
    let mut deps = deps_for(&fixture);
    deps.threads.insert(
        "logical-sid".into(),
        candidate("codex", &fixture.root, &logical),
    );
    let rewind = fixture.root.join("2026/09/11/rollout-rewind.jsonl");
    let mut own = codex_fork("rewind-sid", "logical-sid", fixture.cut);
    own["payload"]["history_base"] =
        json!({"thread_id": "parent-sid", "end_byte_offset": fixture.cut});
    write(&rewind, &[own, codex_message("user", "rewound branch")]);
    let owner = candidate("codex", &fixture.root, &rewind);
    let rewind_request = request(&uid_for("codex", &rewind), &owner, "rewind");
    let mut views = Views::new();
    let view = views.open(&rewind_request, &deps).unwrap();
    assert_eq!(texts(&view), ["kept", "rewound branch"]);
    assert_eq!(view.view.sources.len(), 2);
    assert_eq!(view.view.sources[1].path, fixture.parent);
    assert!(
        view.view
            .sources
            .iter()
            .all(|source| source.path != logical)
    );
    assert_eq!(view.view.parsed.meta["forked_from_id"], "logical-sid");
    // Q vanishing changes nothing for a cold open; R missing is the usual 501.
    fs::remove_file(&logical).unwrap();
    let mut fresh = Views::new();
    let again = fresh.open(&rewind_request, &deps).unwrap();
    assert_eq!(again.view.identity, view.view.identity);
    let mut without_physical = MapDeps::default();
    without_physical.threads.insert(
        "logical-sid".into(),
        candidate("codex", &fixture.root, &fixture.child),
    );
    let mut fresh = Views::new();
    assert_eq!(
        fresh
            .open(&rewind_request, &without_physical)
            .unwrap_err()
            .status,
        501
    );
    // Cut validation is unchanged: off a line boundary in R is unsupported.
    let mid = fixture.root.join("2026/09/11/rollout-rewind-mid.jsonl");
    let mut own = codex_fork("mid-sid", "logical-sid", 7);
    own["payload"]["history_base"] = json!({"thread_id": "parent-sid", "end_byte_offset": 7});
    write(&mid, &[own, codex_message("user", "x")]);
    let mid_request = request(
        &uid_for("codex", &mid),
        &candidate("codex", &fixture.root, &mid),
        "mid",
    );
    assert_eq!(fresh.open(&mid_request, &deps).unwrap_err().status, 501);
}

#[test]
fn agent_view_takes_its_row_item_and_the_owner_proves_native_scope() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap().join("claude");
    let main = root.join("project/synthetic-session.jsonl");
    write(
        &main,
        &[
            claude_row("u1", Value::Null, "user", "main question"),
            claude_row("a1", json!("u1"), "assistant", "main answer"),
        ],
    );
    let sidecar = root.join("project/synthetic-session/subagents/agent-worker.jsonl");
    write(
        &sidecar,
        &[
            claude_row("s1", Value::Null, "user", "agent task"),
            claude_row("s2", json!("s1"), "assistant", "agent result"),
        ],
    );
    let owner = candidate("claude", &root, &main);
    let uid = uid_for("claude", &main);
    let mut agent = ViewRequest {
        uid: uid.clone(),
        agent: "worker".into(),
        owner: owner.clone(),
        selected: Some(candidate("claude", &root, &sidecar)),
        pin: None,
        row: json!({"uid": uid, "title": "Main", "source": "claude", "supported": true,
            "cwd": "/workspace/demo", "agents": 1,
            "agent_items": [{"id": "worker", "title": "Worker", "type": "subagent",
                "path": sidecar.to_string_lossy(), "supported": true, "migration_warnings": []}]}),
    };
    let mut views = Views::new();
    let deps = MapDeps::default();
    let view = views.open(&agent, &deps).unwrap();
    assert_eq!(texts(&view), ["agent task", "agent result"]);
    let meta = view.metadata();
    assert_eq!(meta["agent_id"], "worker");
    assert_eq!(meta["title"], "Worker");
    assert_eq!(meta["parent_title"], "Main");
    assert_eq!(meta["uid"], uid);
    assert!(meta.get("cursor").is_none());
    let scope = view.native_scope().unwrap();
    assert_eq!(scope.session_id, "synthetic-session");
    assert_eq!(scope.agent_id.as_deref(), Some("worker"));
    assert_eq!(scope.uid, uid);
    // Both files are retained: the owner view shares the owner parse.
    assert_eq!(views.stats().files, 2);
    let owner_view = views.open(&request(&uid, &owner, "Main"), &deps).unwrap();
    assert_eq!(texts(&owner_view), ["main question", "main answer"]);
    assert_eq!(views.stats().files, 2);
    assert_eq!(views.stats().views, 2);
    // An id absent from the row's agent menu is not selectable.
    agent.agent = "ghost".into();
    assert_eq!(views.open(&agent, &deps).unwrap_err().status, 404);
    // Selecting without an agent file, or an agent without a selection, is
    // a caller error, never a path guess.
    agent.agent = String::new();
    assert_eq!(views.open(&agent, &deps).unwrap_err().status, 404);
}

#[test]
fn lru_is_bounded_by_count_and_evict_drops_a_session() {
    #[allow(non_snake_case)]
    let VIEW_LIMIT = view_limit();
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap().join("codex");
    let mut views = Views::new();
    let deps = MapDeps::default();
    let mut requests = Vec::new();
    for index in 0..(VIEW_LIMIT + 3) {
        let path = root.join(format!("2026/09/11/rollout-{index:03}.jsonl"));
        write(
            &path,
            &[
                codex_header(&format!("sid-{index}")),
                codex_message("user", &format!("text {index}")),
            ],
        );
        let owner = candidate("codex", &root, &path);
        let request = request(&uid_for("codex", &path), &owner, "t");
        views.open(&request, &deps).unwrap();
        requests.push(request);
    }
    let stats = views.stats();
    assert_eq!(stats.files, VIEW_LIMIT);
    assert_eq!(stats.views, VIEW_LIMIT);
    assert!(stats.bytes > 0);
    // The oldest opened sessions were evicted, the newest are cached.
    assert!(views.cached(&requests[0].uid, "").is_none());
    assert!(views.cached(&requests[VIEW_LIMIT + 2].uid, "").is_some());
    views.evict(&requests[VIEW_LIMIT + 2].uid);
    assert!(views.cached(&requests[VIEW_LIMIT + 2].uid, "").is_none());
    assert_eq!(views.stats().files, VIEW_LIMIT - 1);
    views.clear();
    assert_eq!(views.stats(), ViewStats::default());
}

#[test]
fn transient_open_retains_nothing_and_cached_current_borrows_only_fresh_views() {
    let fixture = codex_fork_fixture();
    let deps = deps_for(&fixture);
    let request = child_request(&fixture);
    let transient = open_transient(&request, &deps, None).unwrap();
    assert_eq!(texts(&transient), ["kept", "new branch"]);
    let mut views = Views::new();
    assert!(views.cached_current(&request, &deps).unwrap().is_none());
    assert_eq!(views.stats(), ViewStats::default());
    let opened = views.open(&request, &deps).unwrap();
    assert_eq!(opened.view.identity, transient.view.identity);
    let borrowed = views.cached_current(&request, &deps).unwrap().unwrap();
    assert!(Arc::ptr_eq(&borrowed, &opened));
    append(&fixture.child, &[codex_message("assistant", "more")]);
    assert!(views.cached_current(&request, &deps).unwrap().is_none());
    assert_eq!(
        views.stats().views,
        1,
        "a stale view is not dropped by a probe"
    );
}

#[test]
fn search_texts_skip_status_and_keep_timeline_order() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap().join("claude");
    let path = root.join("project/session.jsonl");
    write(
        &path,
        &[
            claude_row("u1", Value::Null, "user", "question"),
            claude_row("a1", json!("u1"), "assistant", "answer"),
        ],
    );
    let owner = candidate("claude", &root, &path);
    let mut views = Views::new();
    let view = views
        .open(
            &request(&uid_for("claude", &path), &owner, "S"),
            &MapDeps::default(),
        )
        .unwrap();
    let batch = view.messages(&MessageQuery::default()).unwrap();
    assert_eq!(batch["activity"]["state"], "working");
    assert_eq!(
        view.texts().collect::<Vec<_>>(),
        [("user", "question"), ("assistant", "answer")]
    );
    assert_eq!(crate::search::body(&view), "question\nanswer");
}

#[test]
fn unsupported_files_and_oversized_files_fail_only_their_own_session() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap().join("codex");
    let bad = root.join("2026/09/11/rollout-bad.jsonl");
    // A shape the reference adapter cannot read either (scalar `content`);
    // a corrupt line is only a note.
    let mut scalar = codex_message("user", "fine");
    scalar["payload"]["content"] = json!(42);
    write(&bad, &[codex_header("b"), scalar]);
    let good = root.join("2026/09/11/rollout-good.jsonl");
    write(&good, &[codex_header("g"), codex_message("user", "fine")]);
    let mut views = Views::new();
    let deps = MapDeps::default();
    let error = views
        .open(
            &request(
                &uid_for("codex", &bad),
                &candidate("codex", &root, &bad),
                "bad",
            ),
            &deps,
        )
        .unwrap_err();
    assert_eq!(error.status, 501);
    assert!(error.message.contains("无效"));
    let view = views
        .open(
            &request(
                &uid_for("codex", &good),
                &candidate("codex", &root, &good),
                "good",
            ),
            &deps,
        )
        .unwrap();
    assert_eq!(texts(&view), ["fine"]);
    let _ = hash(b"");
}

#[test]
fn serialized_history_above_one_gib_is_counted_without_a_read_quota() {
    // Reuse one message to cross the former accumulated limit without making
    // a GiB-sized fixture or retaining a second full history in the test.
    let message = json!({"role":"user","text":"x".repeat(1024 * 1024)});
    let event = Event {
        end: 1,
        message,
        media: Vec::new(),
    };
    let one = serde_json::to_vec(&event.message).unwrap().len();
    let encoded = EncodedEvents::build(std::iter::repeat_n(&event, 1025), false, None).unwrap();
    assert_eq!(encoded.total_len(), one * 1025);
    assert_eq!(accounted_bytes(&encoded, std::iter::empty()), one * 1025);
    assert!(one * 1025 > 1024 * 1024 * 1024);
}

/// Phase timings of one native file (read-only), for docs/performance.md:
/// `SESSIONDOCK_BENCH_FILE=/path [SESSIONDOCK_BENCH_SOURCE=codex] cargo test -p sessiondock
/// --release --lib sessions::views::tests::benchmark_parse_phases -- --ignored --nocapture`
#[test]
#[ignore]
fn benchmark_parse_phases() {
    use std::io::Read;
    use std::time::Instant;
    let Ok(path) = std::env::var("SESSIONDOCK_BENCH_FILE") else {
        return;
    };
    let path = PathBuf::from(path);
    let root = path.parent().unwrap().to_path_buf();
    let source: &'static str = match std::env::var("SESSIONDOCK_BENCH_SOURCE").as_deref() {
        Ok("claude") => "claude",
        Ok("grok") => "grok",
        _ => "codex",
    };
    let candidate = candidate(source, &root, &path);
    let size = candidate.data_stamp().unwrap().size;
    eprintln!("file {} size={} MB", path.display(), size >> 20);
    let t = Instant::now();
    {
        let mut file = fs::File::open(&path).unwrap();
        let mut buffer = vec![0u8; 1 << 20];
        let mut total = 0u64;
        loop {
            let count = file.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            total += count as u64;
        }
        eprintln!("read_only {:?} {} MB", t.elapsed(), total >> 20);
    }
    let t = Instant::now();
    let mut reader = native_input::CheckedNative::open(
        &candidate.root,
        &candidate.data,
        candidate.data_stamp().unwrap(),
    )
    .unwrap();
    let index = native_input::RawIndex::scan(&mut reader).unwrap();
    reader.finish().unwrap();
    eprintln!(
        "raw_index(hash only) {:?} committed={}",
        t.elapsed(),
        index.committed()
    );
    let t = Instant::now();
    let mut decoder = records::Decoder::cold();
    let index = read_native_input(&candidate, &mut decoder, None).unwrap();
    let batch = decoder.finish(index.committed() as usize);
    eprintln!(
        "scan_native_records(index+decode) {:?} records={} invalid={} error={:?}",
        t.elapsed(),
        batch.records.len(),
        batch.invalid,
        batch.error
    );
    drop(batch);
    drop(index);
    let t = Instant::now();
    let mut cache = records::RecordCache::default();
    let parsed = parse_candidate(candidate.clone(), None, &mut cache, None).unwrap();
    eprintln!(
        "parse_candidate(total) {:?} events={} unsupported={:?}",
        t.elapsed(),
        parsed.events.len(),
        parsed.unsupported
    );
    let t = Instant::now();
    let encoded = parsed.encoded_bytes();
    eprintln!("encoded_bytes {:?} {} MB", t.elapsed(), encoded >> 20);
    let t = Instant::now();
    let digest = projection_digest(&parsed.events, parsed.committed);
    eprintln!("projection_digest {:?} {}", t.elapsed(), &digest[..8]);
    let media = parsed
        .events
        .iter()
        .map(|event| {
            event
                .media
                .iter()
                .map(|image| image.resident_len())
                .sum::<usize>()
        })
        .sum::<usize>();
    eprintln!("resident media {} MB", media >> 20);
    let t = Instant::now();
    let again = parse_candidate(candidate.clone(), Some(&parsed), &mut cache, None).unwrap();
    eprintln!(
        "parse_candidate(again, same stamp) {:?} events={}",
        t.elapsed(),
        again.events.len()
    );
}

#[test]
fn forks_of_one_parent_share_a_single_inherited_prefix_parse() {
    let fixture = codex_fork_fixture();
    let mut deps = deps_for(&fixture);
    let sibling = fixture.root.join("2026/09/11/rollout-sibling.jsonl");
    write(
        &sibling,
        &[
            codex_fork("sibling-sid", "parent-sid", fixture.cut),
            codex_message("user", "other branch"),
        ],
    );
    let mut views = Views::new();
    let first = views.open(&child_request(&fixture), &deps).unwrap();
    let owner = candidate("codex", &fixture.root, &sibling);
    let sibling_request = request(&uid_for("codex", &sibling), &owner, "sibling");
    let second = views.open(&sibling_request, &deps).unwrap();
    assert_eq!(texts(&first), ["kept", "new branch"]);
    assert_eq!(texts(&second), ["kept", "other branch"]);
    // One parent prefix, one parse: both views hold the same projected events.
    assert!(Arc::ptr_eq(&first.view.inherited, &second.view.inherited));
    let prefixes = views.prefixes();
    assert_eq!(prefixes.lock().unwrap().len(), 1);
    // A transient (search) projection borrows the same prefix.
    let transient = open_transient(&sibling_request, &deps, Some(&prefixes)).unwrap();
    assert!(Arc::ptr_eq(
        &transient.view.inherited,
        &second.view.inherited
    ));
    assert_eq!(transient.anchor, second.anchor);
    // The parent growing is a new stamp: the prefix is read again (same
    // content, new projection), never served from the stale entry.
    append(&fixture.parent, &[codex_message("user", "parent tail")]);
    deps.threads.insert(
        "parent-sid".into(),
        candidate("codex", &fixture.root, &fixture.parent),
    );
    let third = open_transient(&sibling_request, &deps, Some(&prefixes)).unwrap();
    assert!(!Arc::ptr_eq(&third.view.inherited, &second.view.inherited));
    assert_eq!(texts(&third), ["kept", "other branch"]);
    assert_eq!(third.anchor, second.anchor);
    assert_eq!(prefixes.lock().unwrap().len(), 1);
    // The child's own append does not touch the parent prefix.
    append(&sibling, &[codex_message("user", "more")]);
    let owner = candidate("codex", &fixture.root, &sibling);
    let grown = request(&uid_for("codex", &sibling), &owner, "sibling");
    let fourth = views.open(&grown, &deps).unwrap();
    assert!(Arc::ptr_eq(&fourth.view.inherited, &third.view.inherited));
    assert_eq!(texts(&fourth), ["kept", "other branch", "more"]);
}

/// Pure decoder baseline for `benchmark_parse_phases`: every complete line of
/// the file through `serde_json` alone (what the record path cannot beat).
#[test]
#[ignore]
fn benchmark_serde_baseline() {
    use std::time::Instant;
    let Ok(path) = std::env::var("SESSIONDOCK_BENCH_FILE") else {
        return;
    };
    let bytes = fs::read(&path).unwrap();
    let t = Instant::now();
    let mut lines = 0usize;
    let mut weight = 0usize;
    let mut values = Vec::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_slice(line).unwrap();
        lines += 1;
        weight += records::value_weight_for_bench(&value);
        values.push(value);
    }
    eprintln!(
        "serde_json all lines {:?} lines={} weight={} MB",
        t.elapsed(),
        lines,
        weight >> 20
    );
    let t = Instant::now();
    drop(values);
    eprintln!("drop values {:?}", t.elapsed());
}
