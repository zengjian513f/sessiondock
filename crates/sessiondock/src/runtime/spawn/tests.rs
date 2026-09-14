//! The SpawnParentTests cases over the synthetic tree, plus the write-once
//! recording through a private metadata directory.

use std::{fs, sync::Arc};

use indexmap::IndexMap;

use super::*;
use crate::runtime::procscan::{
    ProcScanner, ProcTree, SessionRoots,
    tests::{FakeProc, session},
};

const SID_CLAUDE: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
const SID_CODEX: &str = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";
const SID_GROK: &str = "cccccccc-3333-4333-8333-cccccccccccc";
const SID_CHILD: &str = "dddddddd-4444-4444-8444-dddddddddddd";
const SID_WEB: &str = "eeeeeeee-5555-4555-8555-eeeeeeeeeeee";
const SID_TMUX: &str = "ffffffff-6666-4666-8666-ffffffffffff";
const SID_SUBAGENT_CHILD: &str = "77777777-7777-4777-8777-777777777777";

/// The fixture: a user's Claude whose Bash tool ran `codex exec`, which
/// sent a Grok through ptyhost (adopted by systemd, CLI env stripped, host env
/// kept), a Claude-spawned child Claude, a Codex-subagent-spawned Claude, a
/// web-created session (no identity in the host env) and a tmux session whose
/// server was first started from the Claude session.
fn fixture(root: &std::path::Path) -> FakeProc {
    let proc = FakeProc::new(root);
    proc.add(1, "systemd", 0, "/sbin/init", &[], &[]);
    proc.add(
        4856,
        "systemd",
        1,
        "/usr/lib/systemd/systemd --user",
        &[],
        &[],
    );
    proc.add(
        100,
        "claude",
        4856,
        &format!("claude --session-id {SID_CLAUDE}"),
        &[],
        &[],
    );
    proc.add(
        101,
        "bash",
        100,
        "bash -c codex exec do-something",
        &[
            ("CLAUDE_CODE_SESSION_ID", SID_CLAUDE),
            ("CLAUDE_PID", "100"),
        ],
        &[],
    );
    proc.add(
        102,
        "codex",
        101,
        "codex exec do-something",
        &[
            ("CLAUDE_CODE_SESSION_ID", SID_CLAUDE),
            ("CLAUDE_PID", "100"),
        ],
        &[],
    );
    proc.add(
        200,
        "ptyhost",
        4856,
        "ptyhost run --name sessiondock-grok-cccccccc -- grok",
        &[
            ("CLAUDE_CODE_SESSION_ID", SID_CLAUDE),
            ("CLAUDE_PID", "100"),
            ("CODEX_THREAD_ID", SID_CODEX),
        ],
        &[],
    );
    proc.add(
        201,
        "grok",
        200,
        &format!("grok --session-id {SID_GROK}"),
        &[("CLAUDE_PID", "100")],
        &[],
    );
    proc.add(
        300,
        "claude",
        101,
        &format!("claude -p --session-id {SID_CHILD}"),
        &[
            ("CLAUDE_CODE_SESSION_ID", SID_CLAUDE),
            ("CLAUDE_PID", "100"),
        ],
        &[],
    );
    proc.add(
        310,
        "claude",
        102,
        &format!("claude -p --session-id {SID_SUBAGENT_CHILD}"),
        &[
            ("CODEX_THREAD_ID", "99999999-9999-4999-8999-999999999999"),
            ("CODEX_SESSION_ID", SID_CODEX),
        ],
        &[],
    );
    proc.add(
        400,
        "ptyhost",
        4856,
        "ptyhost run --name sessiondock-claude-eeeeeeee -- claude",
        &[],
        &[],
    );
    proc.add(
        401,
        "claude",
        400,
        &format!("claude --session-id {SID_WEB}"),
        &[],
        &[],
    );
    proc.add(
        500,
        "tmux: server",
        1,
        "tmux -L sessiondock",
        &[
            ("CLAUDE_CODE_SESSION_ID", SID_CLAUDE),
            ("CLAUDE_PID", "100"),
        ],
        &[],
    );
    proc.add(
        501,
        "claude",
        500,
        &format!("claude --session-id {SID_TMUX}"),
        &[],
        &[],
    );
    proc
}

fn sessions() -> Vec<SessionRow> {
    vec![
        session("claude", SID_CLAUDE, "2026-09-12T10:00:00+08:00"),
        session("codex", SID_CODEX, "2026-09-12T10:30:00+08:00"),
        session("grok", SID_GROK, "2026-09-12T11:00:00+08:00"),
        session("claude", SID_CHILD, "2026-09-12T11:10:00+08:00"),
        session("claude", SID_WEB, "2026-09-12T11:20:00+08:00"),
        session("claude", SID_TMUX, "2026-09-12T11:30:00+08:00"),
        session("claude", SID_SUBAGENT_CHILD, "2026-09-12T11:40:00+08:00"),
    ]
}

fn owned(sessions: &[SessionRow], pids: &[i64]) -> IndexMap<String, Vec<i64>> {
    sessions
        .iter()
        .zip(pids)
        .map(|(session, pid)| (session.uid.clone(), vec![*pid]))
        .collect()
}

fn parent(source: &str, sid: &str) -> SpawnedBy {
    SpawnedBy {
        source: source.into(),
        sid: sid.into(),
    }
}

#[test]
fn ancestry_and_inherited_identity_name_the_nearest_spawner() {
    let temp = tempfile::tempdir().unwrap();
    let proc = fixture(temp.path());
    let scan = proc.scan();
    let sessions = sessions();
    let owned = owned(&sessions, &[100, 102, 201, 300, 401, 501, 310]);
    let found = spawn_parents(&scan, &sessions, &owned);
    let uid = |index: usize| sessions[index].uid.clone();
    assert_eq!(
        found,
        BTreeMap::from([
            (uid(1), parent("claude", SID_CLAUDE)),
            // The host env carries grandfather Claude and father Codex; the father is younger.
            (uid(2), parent("codex", SID_CODEX)),
            (uid(3), parent("claude", SID_CLAUDE)),
            // Codex main process 102 and the older Claude 100 are both ancestors; Codex is younger.
            (uid(6), parent("codex", SID_CODEX)),
        ])
    );
    // The tmux server and the web host are boundaries: no spawner for those two.
    assert!(!found.contains_key(&uid(4)) && !found.contains_key(&uid(5)));
    // A uid without a row or without positive pids yields nothing.
    let mut extra = owned.clone();
    extra.insert("claude:ghost".into(), vec![300]);
    extra.insert(uid(3), vec![-300]);
    let again = spawn_parents(&scan, &sessions, &extra);
    assert!(!again.contains_key("claude:ghost") && !again.contains_key(&uid(3)));
}

/// Sixteen levels (the process itself plus fifteen ancestors) is the limit;
/// one more shell in between hides the spawner.
#[test]
fn ancestry_depth_is_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let build = |name: &str, shells: u32| {
        let proc = FakeProc::new(&temp.path().join(name));
        proc.add(1, "systemd", 0, "/sbin/init", &[], &[]);
        proc.add(
            100,
            "claude",
            1,
            &format!("claude --session-id {SID_CLAUDE}"),
            &[],
            &[],
        );
        let mut parent_pid = 100;
        for level in 0..shells {
            proc.add(200 + level, "sh", parent_pid, "sh", &[], &[]);
            parent_pid = 200 + level;
        }
        proc.add(
            300,
            "claude",
            parent_pid,
            &format!("claude -p --session-id {SID_CHILD}"),
            &[],
            &[],
        );
        proc.scan()
    };
    let sessions = vec![
        session("claude", SID_CLAUDE, "2026-09-12T10:00:00+08:00"),
        session("claude", SID_CHILD, "2026-09-12T11:00:00+08:00"),
    ];
    let owned = owned(&sessions, &[100, 300]);
    assert_eq!(
        spawn_parents(&build("reachable", 14), &sessions, &owned),
        BTreeMap::from([(sessions[1].uid.clone(), parent("claude", SID_CLAUDE))])
    );
    assert!(spawn_parents(&build("unreachable", 15), &sessions, &owned).is_empty());
}

fn private_dir(path: &std::path::Path) {
    fs::create_dir_all(path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

/// Memoised per scan and skip recorded, plus the
/// metadata write-once rule through the store.
#[test]
fn results_are_memoised_per_scan_and_recorded_once() {
    let temp = tempfile::tempdir().unwrap();
    let proc = fixture(&temp.path().join("proc"));
    let state = temp.path().join("state");
    private_dir(&state);
    let metadata = Arc::new(MetadataStore::open(&state).unwrap());
    let scanner = Arc::new(ProcScanner::new(
        proc.root.clone(),
        None,
        SessionRoots::default(),
    ));
    let watcher = SpawnWatcher::new(scanner, metadata.clone());
    let scan = Arc::new(proc.scan());
    let sessions = vec![
        session("claude", SID_CLAUDE, "2026-09-12T10:00:00+08:00"),
        session("codex", SID_CODEX, "2026-09-12T10:30:00+08:00"),
    ];
    let owned = owned(&sessions, &[100, 102]);
    assert_eq!(watcher.record(&scan, &sessions, &owned).unwrap(), 1);
    let snapshot = metadata.snapshot().unwrap();
    assert_eq!(
        snapshot.spawned_by(&sessions[1].uid),
        Some(&parent("claude", SID_CLAUDE))
    );
    assert_eq!(
        snapshot.spawned_uids(),
        BTreeSet::from([sessions[1].uid.clone()])
    );
    assert_eq!(snapshot.revision(), 1);
    // Already recorded: nothing written, revision unchanged.
    assert_eq!(watcher.record(&scan, &sessions, &owned).unwrap(), 0);
    assert_eq!(metadata.snapshot().unwrap().revision(), 1);
    // Memoised per scan: the tree can vanish and the same scan still answers;
    // a fresh scan of the empty tree finds nothing.
    fs::remove_dir_all(&proc.root).unwrap();
    assert_eq!(
        watcher.parents(&scan, &sessions, &owned),
        BTreeMap::from([(sessions[1].uid.clone(), parent("claude", SID_CLAUDE))])
    );
    let empty = Arc::new(crate::runtime::procscan::scan(
        Arc::new(ProcTree::open(proc.root.clone())),
        None,
        &SessionRoots::default(),
    ));
    assert!(watcher.parents(&empty, &sessions, &owned).is_empty());
    assert_eq!(watcher.record(&empty, &sessions, &owned).unwrap(), 0);
    // A later, different clue never rewrites the first relation.
    let rewritten = vec![(sessions[1].uid.clone(), parent("grok", SID_GROK))];
    assert_eq!(metadata.record_spawn_parents(&rewritten).unwrap(), 0);
    assert_eq!(
        metadata.snapshot().unwrap().spawned_by(&sessions[1].uid),
        Some(&parent("claude", SID_CLAUDE))
    );
}
