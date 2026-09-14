//! Python `tests/test_live.py` over a synthetic process tree (`FakeProc`):
//! only the files `_scan` and the ancestry walks read exist. No real CLI, no
//! real `/proc` except the final Linux smoke test, which reads it and nothing
//! else.

use std::{fs, path::Path};

use super::*;

const SID_RUNNING: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
const SID_STOPPED: &str = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";
const SID_GROK: &str = "01a09418-a82e-7370-9303-9c4fcf0b20c3";
const SID_CLAUDE: &str = "c357894e-ac88-4c9e-823e-3b5a9c19b9c0";

/// `cmdline`, `stat`, `environ`, `fd/` links.
pub(crate) struct FakeProc {
    pub root: PathBuf,
}

impl FakeProc {
    pub fn new(root: &Path) -> Self {
        fs::create_dir_all(root).unwrap();
        Self {
            root: root.to_path_buf(),
        }
    }

    pub fn add(
        &self,
        pid: u32,
        comm: &str,
        ppid: u32,
        cmdline: &str,
        env: &[(&str, &str)],
        fds: &[(u32, &str)],
    ) {
        self.add_started(pid, comm, ppid, cmdline, env, fds, 0);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_started(
        &self,
        pid: u32,
        comm: &str,
        ppid: u32,
        cmdline: &str,
        env: &[(&str, &str)],
        fds: &[(u32, &str)],
        start_ticks: u64,
    ) {
        let directory = self.root.join(pid.to_string());
        fs::create_dir_all(directory.join("fd")).unwrap();
        let mut bytes = cmdline.replace(' ', "\0").into_bytes();
        bytes.push(0);
        fs::write(directory.join("cmdline"), bytes).unwrap();
        fs::write(
            directory.join("stat"),
            format!("{pid} ({comm}) S {ppid} {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {start_ticks} 0 0\n"),
        )
        .unwrap();
        let environ: Vec<u8> = env
            .iter()
            .flat_map(|(key, value)| format!("{key}={value}\0").into_bytes())
            .collect();
        fs::write(directory.join("environ"), environ).unwrap();
        for (fd, target) in fds {
            std::os::unix::fs::symlink(target, directory.join("fd").join(fd.to_string())).unwrap();
        }
    }

    pub fn cwd(&self, pid: u32, target: &str) {
        std::os::unix::fs::symlink(target, self.root.join(pid.to_string()).join("cwd")).unwrap();
    }

    pub fn boot(&self, btime: u64) {
        fs::write(
            self.root.join("stat"),
            format!("cpu  1 2 3 4\nbtime {btime}\nprocesses 1\n"),
        )
        .unwrap();
    }

    pub fn scan(&self) -> Scan {
        scan(
            Arc::new(ProcTree::open(self.root.clone())),
            None,
            &SessionRoots::default(),
        )
    }
}

fn pids(set: &BTreeSet<i64>) -> Vec<i64> {
    set.iter().copied().collect()
}

pub(crate) fn session(source: &str, sid: &str, created: &str) -> SessionRow {
    SessionRow {
        uid: format!("{source}:{}", &sid[..8.min(sid.len())]),
        source: source.into(),
        sid: sid.into(),
        path: format!("/tmp/{sid}.jsonl"),
        cwd: Some("/tmp/project".into()),
        created: created.into(),
        forked_from_id: String::new(),
        continued_in: None,
    }
}

#[test]
fn command_names_families_and_session_ids_follow_python() {
    assert_eq!(cli_name("/home/x/.local/bin/claude "), "claude");
    assert_eq!(cli_name(r"C:\tools\Codex.EXE"), "codex");
    assert_eq!(cli_family("/opt/grok-4.6"), Some("grok"));
    assert_eq!(cli_family("/usr/bin/claude-code"), Some("claude"));
    assert_eq!(cli_family("codex-companion"), Some("codex"));
    assert_eq!(cli_family("bash"), None);
    assert!(is_cli("claude --resume x"));
    assert!(is_cli("/bin/codex-exec run"));
    assert!(!is_cli("bash -c codex exec"));
    assert!(!is_cli(""));
    let found = command_sids(&format!(
        "claude --resume {} --session-id={}",
        SID_RUNNING.to_uppercase(),
        SID_STOPPED
    ));
    assert_eq!(
        found,
        [SID_RUNNING, SID_STOPPED]
            .into_iter()
            .map(String::from)
            .collect()
    );
    assert!(command_sids("claude --session-id not-a-uuid").is_empty());
    assert_eq!(
        parse_parent("77 (tmux: server) S 1 77 77 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 5 0 0"),
        Some(Parent {
            name: "tmux: server".into(),
            ppid: 1
        })
    );
    assert_eq!(parse_parent("77 (x"), None);
}

/// A bare `claude` matches only the session created
/// in its cwd within -5 s..30 s of the process start.
#[test]
fn bare_claude_matches_only_nearby_session_in_same_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let tree = Arc::new(ProcTree::open(temp.path().join("proc")));
    let started = 1_786_268_640.0_f64;
    let mut scan = Scan {
        sids: BTreeMap::new(),
        paths: BTreeMap::new(),
        bare_claude: BTreeMap::new(),
        stats: ScanStats::default(),
        completed: Instant::now(),
        tree,
    };
    scan.bare_claude.insert(
        123,
        BareClaude {
            cwd: "/tmp/project".into(),
            started,
        },
    );
    let session = |delta: f64, cwd: &str| {
        let created = chrono::DateTime::from_timestamp_micros(((started + delta) * 1e6) as i64)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        SessionRow {
            uid: "claude:test".into(),
            source: "claude".into(),
            sid: "session-id".into(),
            cwd: Some(cwd.into()),
            created,
            path: "/tmp/session.jsonl".into(),
            forked_from_id: String::new(),
            continued_in: None,
        }
    };
    assert!(scan.is_live(&session(2.0, "/tmp/project")));
    assert_eq!(pids(&scan.pids_of(&session(2.0, "/tmp/project"))), [123]);
    assert!(scan.is_live(&session(-5.0, "/tmp/project")));
    assert!(scan.is_live(&session(30.0, "/tmp/project")));
    assert!(!scan.is_live(&session(60.0, "/tmp/project")));
    assert!(!scan.is_live(&session(-6.0, "/tmp/project")));
    assert!(!scan.is_live(&session(2.0, "/tmp/other")));
    // A naive stamp is read as UTC (rows always carry a zone).
    let naive = session(2.0, "/tmp/project");
    let plain = chrono::DateTime::from_timestamp_micros(((started + 2.0) * 1e6) as i64)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%S%.6f")
        .to_string();
    assert!(scan.is_live(&SessionRow {
        created: plain,
        ..naive
    }));
    assert!(!scan.is_live(&SessionRow {
        created: "not a date".into(),
        ..session(2.0, "/tmp/project")
    }));
}

/// A bare `claude` in the tree: cwd from the link, start from the boot clock.
#[test]
fn bare_claude_is_collected_with_cwd_and_start_time() {
    let temp = tempfile::tempdir().unwrap();
    let proc = FakeProc::new(&temp.path().join("proc"));
    proc.boot(1_700_000_000);
    proc.add(1, "systemd", 0, "/sbin/init", &[], &[]);
    proc.add_started(
        50,
        "claude",
        1,
        "/home/x/.local/bin/claude",
        &[],
        &[],
        1_250,
    );
    proc.cwd(50, "/tmp/project");
    // A resume is not bare, an unreadable clock is not paired.
    proc.add_started(
        51,
        "claude",
        1,
        &format!("claude --resume {SID_RUNNING}"),
        &[],
        &[],
        1_250,
    );
    proc.cwd(51, "/tmp/project");
    let scan = proc.scan();
    assert_eq!(
        scan.bare_claude,
        BTreeMap::from([(
            50,
            BareClaude {
                cwd: "/tmp/project".into(),
                started: 1_700_000_012.5
            }
        )])
    );
    assert_eq!(scan.sids[SID_RUNNING], BTreeSet::from([51]));
    assert_eq!(scan.started_at(&[50, 51, -50]), Some(1_700_000_012.5));
    assert_eq!(scan.stats.matched, 2);
    let unclocked = FakeProc::new(&temp.path().join("noclock"));
    unclocked.add(50, "claude", 1, "claude", &[], &[]);
    unclocked.cwd(50, "/tmp/project");
    assert!(unclocked.scan().bare_claude.is_empty());
}

/// A helper left behind by an exited CLI
/// still carries the session id but is not a running instance.
#[test]
fn orphaned_helper_does_not_keep_a_stopped_session_live() {
    let temp = tempfile::tempdir().unwrap();
    let proc = FakeProc::new(temp.path());
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
        &format!("claude --resume {SID_RUNNING}"),
        &[],
        &[],
    );
    proc.add(
        101,
        "bash",
        100,
        "bash /tmp/claude-1000/x/tool.sh",
        &[("CLAUDE_CODE_SESSION_ID", SID_RUNNING)],
        &[],
    );
    proc.add(
        200,
        "bash",
        4856,
        "bash /tmp/claude-1000/y/dispatch.sh",
        &[("CLAUDE_CODE_SESSION_ID", SID_STOPPED)],
        &[],
    );
    proc.add(
        201,
        "sleep",
        200,
        "sleep 15",
        &[("CLAUDE_CODE_SESSION_ID", SID_STOPPED)],
        &[],
    );
    let scan = proc.scan();
    assert_eq!(scan.sids.get(SID_RUNNING), Some(&BTreeSet::from([100])));
    assert!(!scan.sids.contains_key(SID_STOPPED));
    assert!(scan.paths.is_empty());
    let running = session("claude", SID_RUNNING, "2026-09-12T12:00:00+08:00");
    let stopped = session("claude", SID_STOPPED, "2026-09-12T12:00:00+08:00");
    let active = scan.active_processes(&[running.clone(), stopped.clone()]);
    assert_eq!(active.uids, std::slice::from_ref(&running.uid));
    assert_eq!(active.owned[&stopped.uid], Vec::<i64>::new());
    assert_eq!(active.owned[&running.uid], [100]);
    assert!(!scan.is_live(&stopped));
    assert!(scan.pids_of(&stopped).is_empty());
}

/// The command line is the authority: an inherited older id in the
/// environment of a resumed CLI is not a second live session, and an id of
/// another family never crosses over. A Codex companion under Claude still
/// claims `CODEX_COMPANION_SESSION_ID`.
#[test]
fn command_line_sid_beats_inherited_env_and_families_do_not_cross() {
    let temp = tempfile::tempdir().unwrap();
    let proc = FakeProc::new(temp.path());
    proc.add(1, "systemd", 0, "/sbin/init", &[], &[]);
    proc.add(
        100,
        "claude",
        1,
        &format!("claude --resume {SID_RUNNING}"),
        &[("CLAUDE_CODE_SESSION_ID", SID_STOPPED)],
        &[],
    );
    proc.add(
        110,
        "claude",
        1,
        "claude -p hello",
        &[
            ("GROK_SESSION_ID", SID_GROK),
            ("CLAUDE_CODE_SESSION_ID", SID_CLAUDE),
        ],
        &[],
    );
    proc.add(
        120,
        "codex",
        100,
        "codex exec task",
        &[
            ("CODEX_COMPANION_SESSION_ID", SID_STOPPED),
            ("CLAUDE_CODE_SESSION_ID", SID_RUNNING),
        ],
        &[],
    );
    proc.add(
        130,
        "node",
        120,
        "node /x/codex-helper.js",
        &[
            ("CODEX_COMPANION_SESSION_ID", SID_STOPPED),
            ("GROK_SESSION_ID", SID_GROK),
        ],
        &[],
    );
    let scan = proc.scan();
    assert_eq!(scan.sids.get(SID_RUNNING), Some(&BTreeSet::from([100])));
    // 110: a bare claude's own env id counts, the Grok id does not.
    assert_eq!(scan.sids.get(SID_CLAUDE), Some(&BTreeSet::from([110])));
    // 120 owns the companion id; its inherited Claude id is refused (main,
    // families differ). 130's helper env resolves to its CLI ancestor 120;
    // Python only applies the family check to main processes, so the helper's
    // inherited Grok id is attributed to 120 as well (owner is not grok).
    assert_eq!(scan.sids.get(SID_STOPPED), Some(&BTreeSet::from([120])));
    assert_eq!(scan.sids.get(SID_GROK), Some(&BTreeSet::from([120])));
}

/// A process holding the whole fork chain
/// belongs to the deepest fork only; a separate ancestor process stays.
#[test]
fn shared_process_belongs_only_to_deepest_fork_and_ancestors_keep_their_own() {
    let temp = tempfile::tempdir().unwrap();
    let tree = Arc::new(ProcTree::open(temp.path().join("proc")));
    let codex = |uid: &str, sid: &str, parent: &str| SessionRow {
        uid: format!("codex:{uid}"),
        source: "codex".into(),
        sid: sid.into(),
        path: format!("/tmp/{uid}.jsonl"),
        cwd: None,
        created: String::new(),
        forked_from_id: parent.into(),
        continued_in: None,
    };
    let parent = codex("parent", "sid-parent", "");
    let child = codex("child", "sid-child", "sid-parent");
    let leaf = codex("leaf", "sid-leaf", "sid-child");
    let mut scan = Scan {
        sids: BTreeMap::new(),
        paths: [&parent, &child, &leaf]
            .into_iter()
            .map(|row| (row.path.clone(), BTreeSet::from([123])))
            .collect(),
        bare_claude: BTreeMap::new(),
        stats: ScanStats::default(),
        completed: Instant::now(),
        tree: tree.clone(),
    };
    let active = scan.active_processes(&[parent.clone(), child.clone(), leaf.clone()]);
    assert_eq!(active.uids, std::slice::from_ref(&leaf.uid));
    assert_eq!(active.owned[&parent.uid], Vec::<i64>::new());
    assert_eq!(active.owned[&child.uid], Vec::<i64>::new());
    assert_eq!(active.owned[&leaf.uid], [123]);

    scan.paths = BTreeMap::from([
        (parent.path.clone(), BTreeSet::from([111, 222])),
        (child.path.clone(), BTreeSet::from([222])),
    ]);
    let active = scan.active_processes(&[parent.clone(), child.clone()]);
    assert_eq!(active.uids, [parent.uid.clone(), child.uid.clone()]);
    assert_eq!(active.owned[&parent.uid], [111]);
    assert_eq!(active.owned[&child.uid], [222]);
    // Unrelated sessions sharing a helper keep it both.
    let other = SessionRow {
        source: "claude".into(),
        ..codex("other", "sid-other", "")
    };
    scan.paths = BTreeMap::from([
        (parent.path.clone(), BTreeSet::from([-9])),
        (other.path.clone(), BTreeSet::from([-9])),
    ]);
    let active = scan.active_processes(&[parent.clone(), other.clone()]);
    assert_eq!(active.uids, [parent.uid.clone(), other.uid.clone()]);
    assert_eq!(active.owned[&other.uid], [-9]);
}

/// `grok -p` under Claude keeps the session
/// directory's `events.jsonl` open and inherits the parent's Claude id, which
/// is not its identity.
#[test]
fn events_jsonl_fd_marks_headless_grok_live_and_inherited_claude_id_is_not_grok_identity() {
    let temp = tempfile::tempdir().unwrap();
    let grok_dir = temp
        .path()
        .join("home/.grok/sessions/%2Ftmp%2Fproject")
        .join(SID_GROK);
    fs::create_dir_all(&grok_dir).unwrap();
    let events = grok_dir.join("events.jsonl");
    fs::write(&events, "{}\n").unwrap();
    fs::write(grok_dir.join("chat_history.jsonl"), "{}\n").unwrap();
    let proc = FakeProc::new(&temp.path().join("proc"));
    proc.add(1, "systemd", 0, "/sbin/init", &[], &[]);
    proc.add(
        100,
        "claude",
        1,
        &format!("claude --resume {SID_CLAUDE}"),
        &[],
        &[],
    );
    proc.add(
        200,
        "grok",
        100,
        "/home/zj/.local/bin/grok -p do the task --cwd /tmp/project",
        &[("CLAUDE_CODE_SESSION_ID", SID_CLAUDE)],
        &[(36, events.to_str().unwrap())],
    );
    let scan = proc.scan();
    let grok = SessionRow {
        uid: "grok:headless".into(),
        source: "grok".into(),
        sid: SID_GROK.into(),
        cwd: Some("/tmp/project".into()),
        path: grok_dir.to_string_lossy().into_owned(),
        created: "2026-09-12T13:30:39+08:00".into(),
        forked_from_id: String::new(),
        continued_in: None,
    };
    let claude = SessionRow {
        uid: "claude:parent".into(),
        source: "claude".into(),
        sid: SID_CLAUDE.into(),
        cwd: Some("/tmp/project".into()),
        path: "/tmp/parent.jsonl".into(),
        created: "2026-09-12T12:00:00+08:00".into(),
        forked_from_id: String::new(),
        continued_in: None,
    };
    assert_eq!(
        scan.paths.get(events.to_str().unwrap()),
        Some(&BTreeSet::from([200]))
    );
    assert!(!scan.sids.contains_key(SID_GROK));
    assert_eq!(scan.sids.get(SID_CLAUDE), Some(&BTreeSet::from([100])));
    assert!(scan.is_live(&grok));
    assert_eq!(pids(&scan.pids_of(&grok)), [200]);
    let active = scan.active_processes(&[grok.clone(), claude.clone()]);
    assert_eq!(active.uids, [grok.uid.clone(), claude.uid.clone()]);
    assert_eq!(active.owned[&grok.uid], [200]);
    assert_eq!(active.owned[&claude.uid], [100]);
    // A helper (not the CLI) holding the file is a negative pid.
    let helper = FakeProc::new(&temp.path().join("helper"));
    helper.add(1, "systemd", 0, "/sbin/init", &[], &[]);
    helper.add(
        300,
        "node",
        1,
        "node /x/grok-worker.js",
        &[],
        &[(3, events.to_str().unwrap())],
    );
    helper.add(
        301,
        "node",
        1,
        "node /x/unrelated.js",
        &[],
        &[(3, "/tmp/other.jsonl")],
    );
    let scan = helper.scan();
    assert_eq!(
        scan.paths.get(events.to_str().unwrap()),
        Some(&BTreeSet::from([-300]))
    );
    assert_eq!(scan.paths.len(), 1);
    assert_eq!(pids(&scan.pids_of(&grok)), [-300]);
    assert_eq!(scan.started_at(&[-300]), None);
}

/// The explicit active file names sessions with
/// no pid; both the list and the `{sessions: []}` shapes are read and a stale
/// entry is kept as-is.
#[test]
fn explicit_grok_active_file_marks_sessions_live_without_a_pid() {
    let temp = tempfile::tempdir().unwrap();
    let proc = FakeProc::new(&temp.path().join("proc"));
    proc.add(1, "systemd", 0, "/sbin/init", &[], &[]);
    let tree = || Arc::new(ProcTree::open(proc.root.clone()));
    let list = temp.path().join("active.json");
    fs::write(
        &list,
        format!(
            r#"[{{"session_id": "{}", "pid": 1}}, {{"id": "ABC"}}, "plain", {{"pid": 3}}]"#,
            SID_GROK.to_uppercase()
        ),
    )
    .unwrap();
    let scan = scan(tree(), Some(&list), &SessionRoots::default());
    assert_eq!(
        scan.sids.keys().cloned().collect::<Vec<_>>(),
        [SID_GROK.to_owned(), "abc".into(), "plain".into()]
    );
    assert!(scan.sids[SID_GROK].is_empty());
    let grok = SessionRow {
        path: String::new(),
        ..session("grok", SID_GROK, "2026-09-12T13:30:39+08:00")
    };
    assert!(scan.is_live(&grok));
    assert!(scan.pids_of(&grok).is_empty());
    let active = scan.active_processes(std::slice::from_ref(&grok));
    assert_eq!(active.uids, std::slice::from_ref(&grok.uid));
    assert_eq!(active.owned[&grok.uid], Vec::<i64>::new());
    fs::write(&list, r#"{"sessions": [{"session_id": "x-1"}]}"#).unwrap();
    assert!(scan_with(&proc, Some(&list)).sids.contains_key("x-1"));
    fs::write(&list, "not json").unwrap();
    assert!(scan_with(&proc, Some(&list)).sids.is_empty());
    assert!(
        scan_with(&proc, Some(&temp.path().join("missing.json")))
            .sids
            .is_empty()
    );
}

fn scan_with(proc: &FakeProc, grok_active: Option<&Path>) -> Scan {
    scan(
        Arc::new(ProcTree::open(proc.root.clone())),
        grok_active,
        &SessionRoots::default(),
    )
}

/// Python `term_tmux.hosts` / `term_host.hosts`: `tmux_uids` means a tmux
/// server or a managed host's session root is an ancestor.
#[test]
fn tmux_and_host_ancestry_are_bounded_walks() {
    let temp = tempfile::tempdir().unwrap();
    let proc = FakeProc::new(temp.path());
    proc.add(1, "systemd", 0, "/sbin/init", &[], &[]);
    proc.add(500, "tmux: server", 1, "tmux -L sessiondock", &[], &[]);
    proc.add(501, "bash", 500, "bash", &[], &[]);
    proc.add(502, "claude", 501, "claude", &[], &[]);
    proc.add(600, "ptyhost", 1, "ptyhost run", &[], &[]);
    proc.add(601, "sh", 600, "sh -c claude", &[], &[]);
    proc.add(602, "claude", 601, "claude", &[], &[]);
    proc.add(700, "claude", 1, "claude", &[], &[]);
    let tree = ProcTree::open(proc.root.clone());
    assert!(tree.in_tmux(&[502]));
    assert!(tree.in_tmux(&[-502]));
    assert!(!tree.in_tmux(&[602, 700]));
    assert!(!tree.in_tmux(&[999]));
    assert!(tree.hosted(&[602], &BTreeSet::from([601])));
    assert!(tree.hosted(&[601], &BTreeSet::from([601])));
    assert!(!tree.hosted(&[-602], &BTreeSet::from([601])));
    assert!(!tree.hosted(&[700], &BTreeSet::from([601])));
    assert!(!tree.hosted(&[602], &BTreeSet::new()));
    assert_eq!(tree.cli_ancestor(601), None);
    assert_eq!(tree.cli_ancestor(602), Some(602));
    assert_eq!(tree.cli_ancestor(501), None);
    // Twelve levels of shells: the CLI at the top is out of reach.
    let deep = FakeProc::new(&temp.path().join("deep"));
    deep.add(1, "systemd", 0, "/sbin/init", &[], &[]);
    deep.add(10, "claude", 1, "claude", &[], &[]);
    let mut parent = 10;
    for level in 0..13u32 {
        deep.add(20 + level, "sh", parent, "sh", &[], &[]);
        parent = 20 + level;
    }
    let tree = ProcTree::open(deep.root.clone());
    assert_eq!(tree.cli_ancestor(20 + 10), Some(10));
    assert_eq!(tree.cli_ancestor(20 + 12), None);
}

/// Python 16cc89c `tests/test_host.py` PaneOwnershipTests: a pane belongs to
/// its own CLI only. Tree: tmux 5 → root sh 10 → claude 11 → bash 12 →
/// grok 13 → codebase-memory 14; claude's tool shell 12 is claude's, the
/// `grok -p` 13 and its child are not this pane's session — but from grok's
/// own point of view its child still belongs to it.
#[test]
fn cli_main_process_between_a_pid_and_its_pane_root_is_a_barrier() {
    let temp = tempfile::tempdir().unwrap();
    let proc = FakeProc::new(temp.path());
    proc.add(1, "systemd", 0, "/sbin/init", &[], &[]);
    proc.add(
        4856,
        "systemd",
        1,
        "/usr/lib/systemd/systemd --user",
        &[],
        &[],
    );
    proc.add(5, "tmux: server", 4856, "tmux -L sessiondock", &[], &[]);
    proc.add(10, "sh", 5, "sh -c claude", &[], &[]);
    proc.add(
        11,
        "claude",
        10,
        "/home/x/.local/bin/claude --resume x",
        &[],
        &[],
    );
    proc.add(12, "bash", 11, "bash /tmp/tool.sh", &[], &[]);
    proc.add(13, "grok", 12, "grok -p do the task", &[], &[]);
    proc.add(14, "node", 13, "node codebase-memory", &[], &[]);
    let tree = ProcTree::open(proc.root.clone());
    assert!(tree.is_cli_process(11) && tree.is_cli_process(13));
    assert!(!tree.is_cli_process(10) && !tree.is_cli_process(12) && !tree.is_cli_process(999));
    let pane = BTreeSet::from([10]);
    // test_own_cli_and_its_helpers_belong_to_the_pane
    assert!(tree.hosted(&[11], &pane));
    assert!(tree.hosted(&[10], &pane));
    assert!(!tree.hosted(&[-12], &pane));
    assert!(tree.hosted(&[11, -12], &pane));
    // test_spawned_cli_under_the_pane_is_not_hosted_by_it
    assert!(!tree.hosted(&[13], &pane));
    assert!(!tree.hosted(&[14], &pane));
    assert!(!tree.hosted(&[13, 14], &pane));
    assert!(tree.hosted(&[14], &BTreeSet::from([13])));
    // term_tmux.hosts: the same barrier on the way to the tmux server; the
    // start of the walk is never a barrier, so the CLI itself still counts.
    assert!(tree.in_tmux(&[11]));
    assert!(tree.in_tmux(&[10]));
    assert!(!tree.in_tmux(&[13]));
    assert!(!tree.in_tmux(&[14]));
    assert!(!tree.in_tmux(&[-14]));
    assert!(tree.in_tmux(&[13, 11]));
}

/// The TTL runs from completion, waiters share one
/// scan, `force` bypasses the TTL. `snapshot` answers `unsupported_platform`
/// off Linux before it looks at any tree, synthetic ones included.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn snapshot_ttl_single_flight_and_force() {
    let temp = tempfile::tempdir().unwrap();
    let proc = FakeProc::new(temp.path());
    proc.add(1, "systemd", 0, "/sbin/init", &[], &[]);
    proc.add(
        100,
        "claude",
        1,
        &format!("claude --resume {SID_RUNNING}"),
        &[],
        &[],
    );
    let scanner = Arc::new(ProcScanner::with_ttl(
        proc.root.clone(),
        None,
        SessionRoots::default(),
        Duration::from_millis(400),
    ));
    assert!(scanner.last().is_none());
    let first = scanner.snapshot(false).await.unwrap();
    assert!(!first.cached);
    assert_eq!(first.scan.sids[SID_RUNNING], BTreeSet::from([100]));
    let again = scanner.snapshot(false).await.unwrap();
    assert!(again.cached && Arc::ptr_eq(&again.scan, &first.scan));
    let forced = scanner.snapshot(true).await.unwrap();
    assert!(!forced.cached && !Arc::ptr_eq(&forced.scan, &first.scan));
    tokio::time::sleep(Duration::from_millis(450)).await;
    let expired = scanner.snapshot(false).await.unwrap();
    assert!(!expired.cached && !Arc::ptr_eq(&expired.scan, &forced.scan));
    assert!(Arc::ptr_eq(&scanner.last().unwrap(), &expired.scan));
    tokio::time::sleep(Duration::from_millis(450)).await;
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let scanner = scanner.clone();
        tasks.push(tokio::spawn(async move {
            scanner.snapshot(false).await.unwrap().scan
        }));
    }
    let mut shared = Vec::new();
    for task in tasks {
        shared.push(task.await.unwrap());
    }
    assert!(shared.iter().all(|scan| Arc::ptr_eq(scan, &shared[0])));
    assert!(!Arc::ptr_eq(&shared[0], &expired.scan));
}

#[cfg(target_os = "linux")]
#[test]
fn real_proc_scan_is_bounded_and_fast() {
    let scanner = ProcScanner::new("/proc".into(), None, SessionRoots::default());
    let started = Instant::now();
    let scan = scanner.scan_blocking();
    let elapsed = started.elapsed();
    assert!(scan.stats.processes > 0);
    assert!(scan.tree.parent(std::process::id()).is_some());
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    eprintln!(
        "real /proc: {} processes, {} matched, {} ms",
        scan.stats.processes, scan.stats.matched, scan.stats.elapsed_ms
    );
}

/// The one deliberate widening of Python's fd rule: a `*.jsonl` under a
/// configured read root counts like one under the literal home markers.
#[test]
fn open_jsonl_under_a_configured_root_counts_like_a_home_marker() {
    let temp = tempfile::tempdir().unwrap();
    let codex_root = temp.path().join("roots/codex");
    let inside = codex_root.join("2026/09/12/rollout-x.jsonl");
    let sibling = temp.path().join("roots/codex-other/rollout-y.jsonl");
    let outside = temp.path().join("elsewhere/z.jsonl");
    let not_jsonl = codex_root.join("2026/session_index.json");
    let marker = "/home/x/.claude/projects/p/s.jsonl";
    let proc = FakeProc::new(&temp.path().join("proc"));
    proc.add(1, "systemd", 0, "/sbin/init", &[], &[]);
    proc.add(
        100,
        "codex",
        1,
        "codex resume",
        &[],
        &[
            (3, inside.to_str().unwrap()),
            (4, sibling.to_str().unwrap()),
            (5, outside.to_str().unwrap()),
            (6, not_jsonl.to_str().unwrap()),
            (7, marker),
        ],
    );
    let roots = SessionRoots::new([codex_root.as_path(), Path::new("/")]);
    let scan = scan(Arc::new(ProcTree::open(proc.root.clone())), None, &roots);
    assert_eq!(
        scan.paths.keys().cloned().collect::<Vec<_>>(),
        [marker.to_owned(), inside.to_string_lossy().into_owned()]
    );
    assert_eq!(scan.paths[marker], BTreeSet::from([100]));
    // Without configured roots only the markers count (Python's rule).
    let plain = proc.scan();
    assert_eq!(plain.paths.keys().collect::<Vec<_>>(), [marker]);
}
