//! Read-only `/proc` scan for external CLI processes (Python `live.py`).
//!
//! Linux uses `/proc` by default; `SESSIONDOCK_PROC_ROOT` points tests at a
//! synthetic tree (Python `PROC_FS`).
//! The scan reads `cmdline`, `stat`, `environ`, the `cwd` link and the `fd`
//! links of processes in the table, never signals, writes or follows a link
//! outside the tree. The three families leave different traces, so three
//! signals are collected exactly like Python:
//!
//! - Codex keeps the rollout open → `fd` links name the session file;
//! - Claude carries `--session-id`/`--resume`, tool children inherit
//!   `CLAUDE_CODE_SESSION_ID` (only counted when a live CLI ancestor exists);
//! - Grok TUI sessions are listed in an explicit active-sessions file;
//!   headless `grok -p` keeps the session directory's `events.jsonl` open.
//!
//! Rules kept verbatim from Python: a session id on the command line beats an
//! inherited one in the environment, an inherited id never crosses families
//! (Grok only answers to `GROK_SESSION_ID`), an orphan helper without a live
//! CLI ancestor counts for nothing, a bare `claude` is paired with a session
//! by cwd + start time, Codex fork chains hand a shared process to the deepest
//! fork, and `tmux`/host ancestry is what `tmux_uids` means.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use indexmap::IndexMap;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use super::process::ProcClock;

/// Cache seconds after a scan completes (Python `TTL`); the TTL starts when the
/// scan finishes so a slow scan does not immediately expire its own result.
pub const TTL: Duration = Duration::from_secs(3);
/// Python `_ANCESTRY_DEPTH`: spawn-parent walks.
pub const ANCESTRY_DEPTH: usize = 16;
/// Python `_cli_ancestor` / `term_tmux.hosts` walk depth.
const CLI_ANCESTOR_DEPTH: usize = 12;

const KEYWORDS: [&str; 3] = ["claude", "codex", "grok"];
const CLI_NAMES: [&str; 3] = ["claude", "codex", "grok"];
/// Environment identity a CLI sets for its tool children (Python `_ENV_FAMILY`).
const ENV_FAMILY: [(&str, &str); 3] = [
    ("CLAUDE_CODE_SESSION_ID=", "claude"),
    ("CODEX_COMPANION_SESSION_ID=", "codex"),
    ("GROK_SESSION_ID=", "grok"),
];
/// Spawner identity left in a child session's environment (Python `SPAWN_ENV`).
/// `CODEX_THREAD_ID` is a subagent thread's own id, `CODEX_SESSION_ID` the root
/// thread; both are collected so a subagent's children land under the root.
pub const SPAWN_ENV: [(&str, &str); 4] = [
    ("CLAUDE_CODE_SESSION_ID", "claude"),
    ("CODEX_THREAD_ID", "codex"),
    ("CODEX_SESSION_ID", "codex"),
    ("GROK_SESSION_ID", "grok"),
];
/// `SPAWN_ENV` plus `CLAUDE_PID` (Python `SPAWN_ENV_KEYS`).
pub const SPAWN_ENV_KEYS: [&str; 5] = [
    "CLAUDE_CODE_SESSION_ID",
    "CODEX_THREAD_ID",
    "CODEX_SESSION_ID",
    "GROK_SESSION_ID",
    "CLAUDE_PID",
];
const SESSION_FILE_MARKERS: [&str; 3] = ["/.codex/sessions/", "/.claude/projects/", "/.grok/"];

/// Where session files may live: Python's literal home markers plus the
/// configured read roots. On the real machine the roots are exactly those
/// homes, so the root rule only widens synthetic trees (the one deliberate
/// difference from `live.py`, see docs/liveness.md).
#[derive(Clone, Debug, Default)]
pub struct SessionRoots {
    prefixes: Vec<String>,
}

impl SessionRoots {
    /// Roots as spelled by the configuration (already canonical); a session
    /// row's `path` is built from the same spelling.
    pub fn new<'a>(roots: impl IntoIterator<Item = &'a Path>) -> Self {
        Self {
            prefixes: roots
                .into_iter()
                .map(|root| format!("{}/", root.to_string_lossy().trim_end_matches('/')))
                .filter(|prefix| prefix.len() > 1)
                .collect(),
        }
    }

    fn holds(&self, target: &str) -> bool {
        target.ends_with(".jsonl")
            && (SESSION_FILE_MARKERS
                .iter()
                .any(|marker| target.contains(marker))
                || self
                    .prefixes
                    .iter()
                    .any(|prefix| target.starts_with(prefix)))
    }
}

/// Python `_cli_name`: basename without directories or `.exe`, lowercase.
pub fn cli_name(argv0: &str) -> String {
    let trimmed = argv0.trim();
    let head = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let head = head
        .rsplit('\\')
        .next()
        .unwrap_or(head)
        .to_ascii_lowercase();
    head.strip_suffix(".exe")
        .map_or(head.clone(), str::to_owned)
}

/// Python `_cli_family`: which CLI family a command belongs to.
pub fn cli_family(argv0: &str) -> Option<&'static str> {
    let head = cli_name(argv0);
    if head == "grok" || head.starts_with("grok-") {
        Some("grok")
    } else if head == "claude" || head.starts_with("claude-") {
        Some("claude")
    } else if head == "codex" || head.starts_with("codex-") {
        Some("codex")
    } else {
        None
    }
}

/// Python `_is_cli`: the CLI main process itself, not a shell it started.
pub fn is_cli(cmd: &str) -> bool {
    let head = cli_name(argv0_of(cmd));
    CLI_NAMES.contains(&head.as_str()) || head.starts_with("codex-") || head.starts_with("claude-")
}

fn argv0_of(cmd: &str) -> &str {
    cmd.trim().split(' ').next().unwrap_or("")
}

fn cli_comm(name: &str) -> bool {
    CLI_NAMES.contains(&name) || name.starts_with("codex-") || name.starts_with("claude-")
}

fn session_regex() -> &'static Regex {
    static REGEX: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    REGEX.get_or_init(|| {
        const UUID: &str =
            "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}";
        Regex::new(&format!("--session-id[= ]({UUID})|--resume[= ]({UUID})")).expect("static regex")
    })
}

/// Python `_CMD_SID`: `--session-id`/`--resume` UUIDs, lowercased.
pub fn command_sids(cmd: &str) -> BTreeSet<String> {
    session_regex()
        .captures_iter(cmd)
        .filter_map(|capture| capture.get(1).or_else(|| capture.get(2)))
        .map(|found| found.as_str().to_ascii_lowercase())
        .collect()
}

/// Python `Path(...).resolve()` without the strict requirement: an existing
/// path is canonicalized, a missing one is kept as spelled.
fn resolve_path(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|resolved| resolved.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_owned())
}

/// `(comm, ppid)` from `/proc/<pid>/stat` (Python `_process_parent`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parent {
    pub name: String,
    pub ppid: u32,
}

#[derive(Default)]
struct Memo {
    parents: HashMap<u32, Option<Parent>>,
    spawn_env: HashMap<u32, Arc<BTreeMap<String, String>>>,
    cmdlines: HashMap<u32, Option<Arc<str>>>,
    starts: HashMap<u32, Option<f64>>,
}

/// Read-only, memoized access to one process tree. A failed read is an absent
/// value because a process may vanish mid-walk.
pub struct ProcTree {
    root: PathBuf,
    /// Boot clock of this tree (`<root>/stat btime`, ticks from this process's
    /// own auxv like Python's `sysconf`); absent when the tree has no `stat`.
    clock: Option<ProcClock>,
    memo: Mutex<Memo>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl ProcTree {
    pub fn open(root: PathBuf) -> Self {
        let boot_time = std::fs::read_to_string(root.join("stat"))
            .ok()
            .and_then(|stat| ProcClock::parse_boot_time(&stat));
        let ticks_per_second = {
            #[cfg(target_os = "linux")]
            {
                std::fs::read("/proc/self/auxv")
                    .map(|bytes| ProcClock::parse_auxv(&bytes))
                    .unwrap_or(ProcClock::DEFAULT_TICKS)
            }
            #[cfg(not(target_os = "linux"))]
            {
                ProcClock::DEFAULT_TICKS
            }
        };
        Self {
            root,
            clock: boot_time.map(|boot_time| ProcClock {
                boot_time,
                ticks_per_second,
            }),
            memo: Mutex::default(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn pid_dir(&self, pid: u32) -> PathBuf {
        self.root.join(pid.to_string())
    }

    /// Full command line with NULs as spaces (Python `_process_cmdline`).
    pub fn cmdline(&self, pid: u32) -> Option<Arc<str>> {
        if let Some(known) = lock(&self.memo).cmdlines.get(&pid) {
            return known.clone();
        }
        let value = std::fs::read(self.pid_dir(pid).join("cmdline"))
            .ok()
            .map(|bytes| Arc::from(cmdline_text(&bytes)));
        lock(&self.memo).cmdlines.insert(pid, value.clone());
        value
    }

    /// `(comm, ppid)`; `None` when the stat line is unreadable or malformed.
    pub fn parent(&self, pid: u32) -> Option<Parent> {
        if let Some(known) = lock(&self.memo).parents.get(&pid) {
            return known.clone();
        }
        let value = std::fs::read_to_string(self.pid_dir(pid).join("stat"))
            .ok()
            .and_then(|stat| parse_parent(&stat));
        lock(&self.memo).parents.insert(pid, value.clone());
        value
    }

    /// Unix seconds the process started (Python `_process_started_at`), the
    /// raw `btime + ticks / CLK_TCK` value without rounding.
    pub fn started_at(&self, pid: u32) -> Option<f64> {
        if let Some(known) = lock(&self.memo).starts.get(&pid) {
            return *known;
        }
        let value = self.clock.and_then(|clock| {
            let stat = std::fs::read_to_string(self.pid_dir(pid).join("stat")).ok()?;
            let ticks = parse_start_ticks(&stat)?;
            Some(clock.boot_time as f64 + ticks as f64 / clock.ticks_per_second as f64)
        });
        lock(&self.memo).starts.insert(pid, value);
        value
    }

    /// The `SPAWN_ENV_KEYS` present in the process environment with non-empty
    /// values (Python `_process_spawn_env`).
    pub fn spawn_env(&self, pid: u32) -> Arc<BTreeMap<String, String>> {
        if let Some(known) = lock(&self.memo).spawn_env.get(&pid) {
            return known.clone();
        }
        let mut found = BTreeMap::new();
        if let Ok(bytes) = std::fs::read(self.pid_dir(pid).join("environ")) {
            for entry in String::from_utf8_lossy(&bytes).split('\0') {
                if let Some((key, value)) = entry.split_once('=')
                    && SPAWN_ENV_KEYS.contains(&key)
                    && !value.trim().is_empty()
                {
                    found.insert(key.to_owned(), value.trim().to_owned());
                }
            }
        }
        let value = Arc::new(found);
        lock(&self.memo).spawn_env.insert(pid, value.clone());
        value
    }

    /// Python `_cli_ancestor`: the CLI main process a helper belongs to, by
    /// `comm` up the tree; `None` when the chain ends without one.
    pub fn cli_ancestor(&self, pid: u32) -> Option<u32> {
        let mut current = pid;
        for _ in 0..CLI_ANCESTOR_DEPTH {
            let parent = self.parent(current)?;
            if cli_comm(&parent.name) {
                return Some(current);
            }
            if parent.ppid <= 1 {
                return None;
            }
            current = parent.ppid;
        }
        None
    }

    /// Python `live.is_cli_process`: this pid is a session's CLI main process
    /// (`claude`/`codex`/`grok` itself). The pane-ownership walks stop at one
    /// that is neither their start nor their target: a `grok -p` or `codex
    /// exec` a pane's Claude spawned sits in that pane's process tree, but
    /// the console is the parent CLI's, not the grandchild session's.
    pub fn is_cli_process(&self, pid: u32) -> bool {
        self.cmdline(pid).is_some_and(|cmd| is_cli(&cmd))
    }

    /// Python `term_tmux.hosts`: some process runs under a tmux server with no
    /// other CLI main process between them.
    pub fn in_tmux(&self, pids: &[i64]) -> bool {
        pids.iter().any(|pid| {
            let mut current = pid.unsigned_abs() as u32;
            for step in 0..CLI_ANCESTOR_DEPTH {
                let Some(parent) = self.parent(current) else {
                    break;
                };
                if parent.name.starts_with("tmux") {
                    return true;
                }
                if step > 0 && self.is_cli_process(current) {
                    break;
                }
                if parent.ppid <= 1 {
                    break;
                }
                current = parent.ppid;
            }
            false
        })
    }

    /// Python `term_host.hosts` / `term.process_belongs_to`: some CLI main
    /// process is, or descends from, a managed host's session root with no
    /// other CLI main process between them (`procs.ancestor_matches` /
    /// `descendant_of` with the `is_cli_process` barrier).
    pub fn hosted(&self, pids: &[i64], roots: &BTreeSet<u32>) -> bool {
        if roots.is_empty() {
            return false;
        }
        pids.iter().filter(|pid| **pid > 0).any(|pid| {
            let mut current = pid.unsigned_abs() as u32;
            for step in 0..ANCESTRY_DEPTH {
                if roots.contains(&current) {
                    return true;
                }
                if step > 0 && self.is_cli_process(current) {
                    break;
                }
                match self.parent(current) {
                    Some(parent) if parent.ppid > 1 => current = parent.ppid,
                    _ => break,
                }
            }
            false
        })
    }
}

fn cmdline_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace('\0', " ")
}

/// `pid (comm) state ppid …` → `(comm, ppid)`; comm may hold spaces/parens.
pub fn parse_parent(stat: &str) -> Option<Parent> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    if close < open {
        return None;
    }
    let ppid = stat[close + 1..]
        .split_ascii_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(Parent {
        name: stat[open + 1..close].to_owned(),
        ppid,
    })
}

/// Field 22 (start ticks) located after the last `)`.
fn parse_start_ticks(stat: &str) -> Option<u64> {
    let close = stat.rfind(')')?;
    stat[close + 1..]
        .split_ascii_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

/// A bare `claude` (no session id anywhere): paired later by cwd + start time.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BareClaude {
    pub cwd: String,
    pub started: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ScanStats {
    pub processes: usize,
    pub matched: usize,
    pub elapsed_ms: u64,
}

/// One completed scan (Python `_cache`). Positive pids are CLI main processes,
/// negative ones related helpers.
pub struct Scan {
    /// Lowercase session id → owning pids.
    pub sids: BTreeMap<String, BTreeSet<i64>>,
    /// Open session file → holders (`pid` main, `-pid` helper).
    pub paths: BTreeMap<String, BTreeSet<i64>>,
    pub bare_claude: BTreeMap<u32, BareClaude>,
    pub stats: ScanStats,
    pub completed: Instant,
    pub tree: Arc<ProcTree>,
}

/// The list-row fields the scan pairs processes with (Python `session` dict).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub uid: String,
    pub source: String,
    pub sid: String,
    pub path: String,
    pub cwd: Option<String>,
    pub created: String,
    pub forked_from_id: String,
    /// Claude row whose JSONL this session continued into (the row's
    /// `continued_in` uid); the continued session inherits this row's pane.
    pub continued_in: Option<String>,
}

impl SessionRow {
    pub fn from_value(row: &Value) -> Option<Self> {
        let text = |key: &str| row[key].as_str().unwrap_or("").to_owned();
        let uid = row["uid"].as_str()?;
        if uid.is_empty() {
            return None;
        }
        Some(Self {
            uid: uid.to_owned(),
            source: text("source"),
            sid: text("sid"),
            path: text("path"),
            cwd: row["cwd"]
                .as_str()
                .filter(|cwd| !cwd.is_empty())
                .map(str::to_owned),
            created: text("created"),
            forked_from_id: text("forked_from_id"),
            continued_in: row["continued_in"]
                .as_str()
                .filter(|target| !target.is_empty())
                .map(str::to_owned),
        })
    }

    pub fn from_list(document: &Value) -> Vec<Self> {
        document["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Self::from_value)
            .collect()
    }
}

/// Python `_scan` over one process tree.
pub fn scan(tree: Arc<ProcTree>, grok_active: Option<&Path>, roots: &SessionRoots) -> Scan {
    let started = Instant::now();
    let mut sids: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    let mut paths: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    let mut bare_claude = BTreeMap::new();
    let mut stats = ScanStats::default();
    let entries: Vec<u32> = std::fs::read_dir(tree.root())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .collect();
    stats.processes = entries.len();
    for pid in entries {
        let Some(cmd) = tree.cmdline(pid) else {
            continue;
        };
        let lower = cmd.to_ascii_lowercase();
        if !KEYWORDS.iter().any(|keyword| lower.contains(keyword)) {
            continue;
        }
        stats.matched += 1;
        let main = is_cli(&cmd);
        // The id in a resume command is this process's authoritative identity:
        // an inherited `CLAUDE_CODE_SESSION_ID` from an older session must not
        // mark both sessions live.
        let cmd_sids = command_sids(&cmd);
        for sid in &cmd_sids {
            sids.entry(sid.clone()).or_default().insert(i64::from(pid));
        }
        let argv0 = argv0_of(&cmd);
        let head = argv0.rsplit('/').next().unwrap_or(argv0);
        if main && head == "claude" && cmd_sids.is_empty() {
            let directory = tree.root().join(pid.to_string());
            if let Ok(target) = std::fs::read_link(directory.join("cwd"))
                && let Some(started) = tree.started_at(pid)
            {
                bare_claude.insert(
                    pid,
                    BareClaude {
                        cwd: resolve_path(&target.to_string_lossy()),
                        started,
                    },
                );
            }
        }
        let directory = tree.root().join(pid.to_string());
        if let Ok(bytes) = std::fs::read(directory.join("environ")) {
            for entry in String::from_utf8_lossy(&bytes).split('\0') {
                let Some((prefix, family)) = ENV_FAMILY
                    .iter()
                    .find(|(prefix, _)| entry.starts_with(prefix))
                else {
                    continue;
                };
                let env_sid = entry[prefix.len()..].trim().to_ascii_lowercase();
                // An inherited id only counts when a live CLI owns it: orphan
                // helpers left by setsid/nohup after the CLI exited, and a
                // `grok -p` carrying its parent Claude's id, count for nothing.
                let Some(owner) = env_owner(&tree, pid, main, argv0, &cmd_sids, family, &env_sid)
                else {
                    continue;
                };
                sids.entry(env_sid).or_default().insert(i64::from(owner));
            }
        }
        let Ok(fds) = std::fs::read_dir(directory.join("fd")) else {
            continue;
        };
        for entry in fds.flatten() {
            let Ok(target) = std::fs::read_link(entry.path()) else {
                continue;
            };
            let target = target.to_string_lossy();
            if roots.holds(&target) {
                let holder = if main {
                    i64::from(pid)
                } else {
                    -i64::from(pid)
                };
                paths.entry(target.into_owned()).or_default().insert(holder);
            }
        }
    }
    if let Some(file) = grok_active {
        note_grok_sessions(&mut sids, file);
    }
    stats.elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Scan {
        sids,
        paths,
        bare_claude,
        stats,
        completed: Instant::now(),
        tree,
    }
}

/// Python `_env_owner`: whether an environment session id is this CLI's own.
fn env_owner(
    tree: &ProcTree,
    pid: u32,
    main: bool,
    argv0: &str,
    cmd_sids: &BTreeSet<String>,
    env_family: &str,
    env_sid: &str,
) -> Option<u32> {
    if main && !cmd_sids.is_empty() && !cmd_sids.contains(env_sid) {
        return None;
    }
    let owner = if main { pid } else { tree.cli_ancestor(pid)? };
    let proc_family = if main {
        cli_family(argv0)
    } else {
        tree.cmdline(owner)
            .and_then(|cmd| cli_family(argv0_of(&cmd)))
    };
    if proc_family == Some("grok") && env_family != "grok" {
        return None;
    }
    if main && proc_family.is_some_and(|family| family != env_family) {
        return None;
    }
    Some(owner)
}

/// Python `_note_grok_sessions`: Grok's own active list names sessions without
/// exposing a pid; a stale entry stays exactly as Python would show it.
fn note_grok_sessions(sids: &mut BTreeMap<String, BTreeSet<i64>>, file: &Path) {
    let Some(data) = std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    else {
        return;
    };
    let entries = match &data {
        Value::Array(entries) => entries.clone(),
        Value::Object(_) => data["sessions"].as_array().cloned().unwrap_or_default(),
        _ => Vec::new(),
    };
    for entry in entries {
        let sid = match &entry {
            Value::Object(_) => entry["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .or_else(|| entry["session_id"].as_str())
                .map(str::to_owned),
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        };
        if let Some(sid) = sid.filter(|sid| !sid.is_empty()) {
            sids.entry(sid.to_ascii_lowercase()).or_default();
        }
    }
}

/// Python `active_processes` output: live uids in list order and the pids each
/// one owns after Codex fork-chain folding.
pub struct ActiveProcesses {
    pub uids: Vec<String>,
    pub owned: IndexMap<String, Vec<i64>>,
}

impl Scan {
    /// Python `_session_path_pids`: holders of the session file and of any
    /// JSONL under a Grok session directory (`events.jsonl` for `grok -p`).
    fn session_path_pids(&self, session: &SessionRow) -> BTreeSet<i64> {
        let mut found = BTreeSet::new();
        if session.path.is_empty() {
            return found;
        }
        if let Some(pids) = self.paths.get(&session.path) {
            found.extend(pids);
        }
        let prefix = format!("{}/", session.path.trim_end_matches('/'));
        for (held, pids) in &self.paths {
            if held.starts_with(&prefix) {
                found.extend(pids);
            }
        }
        found
    }

    /// Python `_bare_claude_pids`: a bare `claude` in the session's cwd that
    /// started within 5 s before to 30 s after the session was created.
    fn bare_claude_pids(&self, session: &SessionRow) -> BTreeSet<i64> {
        let mut found = BTreeSet::new();
        if session.source != "claude" || self.bare_claude.is_empty() {
            return found;
        }
        let Some(cwd) = session.cwd.as_deref() else {
            return found;
        };
        let Some(created) = parse_created(&session.created) else {
            return found;
        };
        let cwd = resolve_path(&expand_user(cwd));
        for (pid, bare) in &self.bare_claude {
            let delta = created - bare.started;
            if bare.cwd == cwd && (-5.0..=30.0).contains(&delta) {
                found.insert(i64::from(*pid));
            }
        }
        found
    }

    /// Python `pids_of`: positive pids are CLI main processes, negative related helpers.
    pub fn pids_of(&self, session: &SessionRow) -> BTreeSet<i64> {
        let mut found = BTreeSet::new();
        let sid = session.sid.to_ascii_lowercase();
        if !sid.is_empty()
            && let Some(pids) = self.sids.get(&sid)
        {
            found.extend(pids);
        }
        found.extend(self.session_path_pids(session));
        found.extend(self.bare_claude_pids(session));
        found
    }

    /// Python `is_live`: a listed Grok session id counts even without a pid.
    pub fn is_live(&self, session: &SessionRow) -> bool {
        let sid = session.sid.to_ascii_lowercase();
        (!sid.is_empty() && self.sids.contains_key(&sid))
            || !self.session_path_pids(session).is_empty()
            || !self.bare_claude_pids(session).is_empty()
    }

    /// Python `started_at`: the earliest CLI main process start among `pids`.
    pub fn started_at(&self, pids: &[i64]) -> Option<f64> {
        pids.iter()
            .filter(|pid| **pid > 0)
            .map(|pid| pid.unsigned_abs() as u32)
            .filter(|pid| self.tree.cmdline(*pid).is_some_and(|cmd| is_cli(&cmd)))
            .filter_map(|pid| self.tree.started_at(pid))
            .reduce(f64::min)
    }

    /// Python `active_processes`: a process held by a Codex fork and its
    /// ancestors belongs to the deepest fork only; unrelated sessions sharing
    /// a helper signal keep it.
    pub fn active_processes(&self, sessions: &[SessionRow]) -> ActiveProcesses {
        let rows: IndexMap<&str, &SessionRow> = sessions
            .iter()
            .map(|session| (session.uid.as_str(), session))
            .collect();
        let raw: IndexMap<&str, BTreeSet<i64>> = sessions
            .iter()
            .map(|session| (session.uid.as_str(), self.pids_of(session)))
            .collect();
        let by_sid: HashMap<&str, &SessionRow> = sessions
            .iter()
            .filter(|session| session.source == "codex" && !session.sid.is_empty())
            .map(|session| (session.sid.as_str(), session))
            .collect();
        let ancestors: HashMap<&str, BTreeSet<String>> = rows
            .iter()
            .map(|(uid, row)| (*uid, codex_ancestor_sids(row, &by_sid)))
            .collect();
        let mut candidates: IndexMap<i64, IndexMap<&str, ()>> = IndexMap::new();
        for (uid, pids) in &raw {
            for pid in pids {
                candidates.entry(*pid).or_default().insert(*uid, ());
            }
        }
        let mut owned: IndexMap<String, Vec<i64>> = rows
            .keys()
            .map(|uid| ((*uid).to_owned(), Vec::new()))
            .collect();
        for (pid, uids) in &candidates {
            for uid in uids.keys() {
                let superseded = uids.keys().any(|other| {
                    uid != other
                        && ancestors
                            .get(other)
                            .is_some_and(|chain| chain.contains(rows[uid].sid.as_str()))
                });
                if !superseded && let Some(pids) = owned.get_mut(*uid) {
                    pids.push(*pid);
                }
            }
        }
        for pids in owned.values_mut() {
            pids.sort_unstable();
            pids.dedup();
        }
        let uids = sessions
            .iter()
            .filter(|session| {
                let uid = session.uid.as_str();
                owned.get(uid).is_some_and(|pids| !pids.is_empty())
                    || (raw.get(uid).is_some_and(BTreeSet::is_empty) && self.is_live(session))
            })
            .map(|session| session.uid.clone())
            .collect();
        ActiveProcesses { uids, owned }
    }
}

/// Python `_codex_ancestor_sids`: the fork chain a Codex rollback branch can
/// confirm within the current list.
pub(crate) fn codex_ancestor_sids(
    session: &SessionRow,
    by_sid: &HashMap<&str, &SessionRow>,
) -> BTreeSet<String> {
    let mut ancestors = BTreeSet::new();
    if session.source != "codex" {
        return ancestors;
    }
    let mut parent = session.forked_from_id.clone();
    while !parent.is_empty() && !ancestors.contains(&parent) {
        ancestors.insert(parent.clone());
        let Some(row) = by_sid.get(parent.as_str()) else {
            break;
        };
        parent = row.forked_from_id.clone();
    }
    ancestors
}

/// Python `Path.expanduser()` for the `~`/`~/` forms rows can carry.
fn expand_user(path: &str) -> String {
    if (path == "~" || path.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME")
    {
        return format!("{}{}", home.to_string_lossy(), &path[1..]);
    }
    path.to_owned()
}

/// Python `datetime.fromisoformat(created.replace("Z", "+00:00")).timestamp()`.
/// Index rows always carry a zone (`…Z`, like Python `list_sessions`); a naive
/// stamp, which Python would read as local time, is read as UTC here.
fn parse_created(created: &str) -> Option<f64> {
    let text = created.replace('Z', "+00:00");
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&text) {
        return Some(parsed.timestamp_micros() as f64 / 1e6);
    }
    let naive = chrono::NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M:%S"))
        .ok()?;
    Some(naive.and_utc().timestamp_micros() as f64 / 1e6)
}

#[derive(Debug, PartialEq, Eq)]
pub enum ScanError {
    /// No `/proc` process table on this target; nothing is inferred.
    UnsupportedPlatform,
    /// The scan thread failed.
    Failed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum KillError {
    UnsupportedPlatform,
    UnsafeProcessRoot,
}

/// PIDs that accepted TERM, followed by the subset still present after the
/// TERM/KILL window. PID start ticks are rechecked before every signal so a
/// recycled PID is never touched.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct KillOutcome {
    pub killed: Vec<u32>,
    pub remaining: Vec<u32>,
}

fn process_stamp(root: &Path, pid: u32) -> Option<(char, u64)> {
    let stat = std::fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    let close = stat.rfind(')')?;
    let mut fields = stat[close + 1..].split_ascii_whitespace();
    let state = fields.next()?.chars().next()?;
    let start = stat[close + 1..]
        .split_ascii_whitespace()
        .nth(19)?
        .parse()
        .ok()?;
    Some((state, start))
}

#[cfg(target_os = "linux")]
fn signal_if_same(root: &Path, pid: u32, start: u64, signal: i32) -> bool {
    if !matches!(process_stamp(root, pid), Some((state, current)) if state != 'Z' && current == start)
    {
        return false;
    }
    // SAFETY: `pid` is a positive process ID proven by the frozen proc scan;
    // start ticks were re-read immediately above to prevent PID-reuse kills.
    unsafe { libc::kill(pid as i32, signal) == 0 }
}

fn still_same(root: &Path, pid: u32, start: u64) -> bool {
    matches!(process_stamp(root, pid), Some((state, current)) if state != 'Z' && current == start)
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedPlatform => "process table scan is unsupported on this platform",
            Self::Failed => "process table scan failed",
        })
    }
}

/// The shared scan for display: fresh or reused within the TTL.
pub struct ScanSnapshot {
    pub scan: Arc<Scan>,
    pub cached: bool,
    pub age: Duration,
}

/// TTL-cached, single-flight scanner (Python `snapshot`): concurrent callers
/// wait for one scan and reuse it; `force` bypasses the TTL but still serializes.
pub struct ProcScanner {
    root: PathBuf,
    grok_active: Option<PathBuf>,
    roots: SessionRoots,
    ttl: Duration,
    cached: Mutex<Option<Arc<Scan>>>,
    refresh: tokio::sync::Mutex<()>,
}

impl ProcScanner {
    pub fn new(root: PathBuf, grok_active: Option<PathBuf>, roots: SessionRoots) -> Self {
        Self::with_ttl(root, grok_active, roots, TTL)
    }

    pub fn with_ttl(
        root: PathBuf,
        grok_active: Option<PathBuf>,
        roots: SessionRoots,
        ttl: Duration,
    ) -> Self {
        Self {
            root,
            grok_active,
            roots,
            ttl,
            cached: Mutex::new(None),
            refresh: tokio::sync::Mutex::new(()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn supported() -> bool {
        cfg!(target_os = "linux")
    }

    fn fresh(&self) -> Option<ScanSnapshot> {
        let cached = lock(&self.cached);
        let scan = cached.as_ref()?;
        let age = scan.completed.elapsed();
        (age <= self.ttl).then(|| ScanSnapshot {
            scan: scan.clone(),
            cached: true,
            age,
        })
    }

    /// The last completed scan whatever its age; `None` before the first one.
    pub fn last(&self) -> Option<Arc<Scan>> {
        lock(&self.cached).clone()
    }

    /// One synchronous scan of the tree (tests and the blocking worker).
    pub fn scan_blocking(&self) -> Scan {
        let tree = Arc::new(ProcTree::open(self.root.clone()));
        scan(tree, self.grok_active.as_deref(), &self.roots)
    }

    pub async fn snapshot(&self, force: bool) -> Result<ScanSnapshot, ScanError> {
        if !Self::supported() {
            return Err(ScanError::UnsupportedPlatform);
        }
        if !force && let Some(hit) = self.fresh() {
            return Ok(hit);
        }
        let _refresh = self.refresh.lock().await;
        if !force && let Some(hit) = self.fresh() {
            return Ok(hit);
        }
        let root = self.root.clone();
        let grok_active = self.grok_active.clone();
        let roots = self.roots.clone();
        let scan = tokio::task::spawn_blocking(move || {
            let tree = Arc::new(ProcTree::open(root));
            scan(tree, grok_active.as_deref(), &roots)
        })
        .await
        .map_err(|_| ScanError::Failed)?;
        let scan = Arc::new(scan);
        *lock(&self.cached) = Some(scan.clone());
        Ok(ScanSnapshot {
            scan,
            cached: false,
            age: Duration::ZERO,
        })
    }

    /// Python `term.kill_pids`: TERM the positively owned CLI main processes,
    /// wait up to six seconds, then KILL survivors. This is available only for
    /// the real Linux `/proc`; synthetic proc trees remain read-only fixtures.
    pub async fn kill_pids(&self, pids: &[i64]) -> Result<KillOutcome, KillError> {
        if !cfg!(target_os = "linux") {
            return Err(KillError::UnsupportedPlatform);
        }
        if self.root != Path::new("/proc") {
            return Err(KillError::UnsafeProcessRoot);
        }
        let mut targets: Vec<(u32, u64)> = pids
            .iter()
            .filter(|pid| **pid > 0)
            .filter_map(|pid| {
                let pid = *pid as u32;
                let (state, start) = process_stamp(&self.root, pid)?;
                (state != 'Z').then_some((pid, start))
            })
            .collect();
        targets.sort_unstable();
        targets.dedup();
        #[cfg(target_os = "linux")]
        let killed: Vec<(u32, u64)> = targets
            .into_iter()
            .filter(|(pid, start)| signal_if_same(&self.root, *pid, *start, libc::SIGTERM))
            .collect();
        #[cfg(not(target_os = "linux"))]
        let killed: Vec<(u32, u64)> = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        while tokio::time::Instant::now() < deadline
            && killed
                .iter()
                .any(|(pid, start)| still_same(&self.root, *pid, *start))
        {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        #[cfg(target_os = "linux")]
        for (pid, start) in &killed {
            signal_if_same(&self.root, *pid, *start, libc::SIGKILL);
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(KillOutcome {
            killed: killed.iter().map(|(pid, _)| *pid).collect(),
            remaining: killed
                .iter()
                .filter(|(pid, start)| still_same(&self.root, *pid, *start))
                .map(|(pid, _)| *pid)
                .collect(),
        })
    }
}

#[cfg(all(test, unix))]
pub(crate) mod tests;
