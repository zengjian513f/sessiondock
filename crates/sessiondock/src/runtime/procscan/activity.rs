//! Work that outlives a turn, including detached commands carrying a native
//! session identity. This is display evidence only, never PID control or CLI
//! liveness. Reuse the scan's identity/ancestry reads; do not inspect CPU usage.
use super::{ProcTree, SPAWN_ENV, SessionRow, argv0_of, cli_name};
use std::collections::{BTreeSet, HashMap, HashSet};

fn infrastructure(cmd: &str) -> bool {
    let name = cli_name(argv0_of(cmd));
    let args: Vec<_> = cmd.split_ascii_whitespace().collect();
    name == "codex-code-mode-host"
        || (name == "codex" && args.get(1) == Some(&"app-server"))
        || (name == "opencode" && args.get(1) == Some(&"serve"))
        || args.windows(2).any(|pair| pair == ["mcp", "serve"])
        || name.starts_with("mcp-server-")
}

impl ProcTree {
    pub fn working_uids(
        &self,
        sessions: &[SessionRow],
        owners: &indexmap::IndexMap<String, Vec<i64>>,
        live: &BTreeSet<&str>,
    ) -> Vec<String> {
        let by_pid: HashMap<_, _> = owners
            .iter()
            .flat_map(|(uid, pids)| {
                pids.iter()
                    .filter(|pid| **pid > 0)
                    .map(move |pid| (*pid as u32, uid.as_str()))
            })
            .filter(|(pid, _)| !self.cmdline(*pid).is_some_and(|cmd| infrastructure(&cmd)))
            .collect();
        let by_sid: HashMap<_, _> = sessions
            .iter()
            .filter(|row| live.contains(row.uid.as_str()))
            .map(|row| {
                (
                    (row.source.as_str(), row.sid.to_ascii_lowercase()),
                    row.uid.as_str(),
                )
            })
            .collect();
        let mut working = BTreeSet::new();
        for directory in std::fs::read_dir(self.root())
            .into_iter()
            .flatten()
            .flatten()
        {
            let Some(pid) = directory
                .file_name()
                .to_str()
                .and_then(|v| v.parse::<u32>().ok())
            else {
                continue;
            };
            if by_pid.contains_key(&pid) {
                continue; // CLI main processes are alive, not necessarily working.
            }
            let Some(cmd) = self.cmdline(pid).filter(|cmd| !cmd.trim().is_empty()) else {
                continue; // Includes zombies, whose command line is empty.
            };
            if infrastructure(&cmd) {
                continue;
            }
            let mut current = pid;
            let mut seen = HashSet::new();
            let mut owner = None;
            let mut transport = false;
            // Nearest positively owned CLI wins over inherited root identity.
            // A code-mode host is transparent to commands it launches; an MCP
            // server's own descendants belong to its persistent transport.
            while current > 1 && seen.insert(current) {
                if let Some(uid) = by_pid.get(&current) {
                    owner = Some(*uid);
                    break;
                }
                if let Some(cmd) = self.cmdline(current) {
                    if infrastructure(&cmd) && cli_name(argv0_of(&cmd)) != "codex-code-mode-host" {
                        transport = true;
                        break;
                    }
                    if current != pid
                        && self.is_cli_process(current)
                        && cli_name(argv0_of(&cmd)) != "codex-code-mode-host"
                    {
                        break; // Do not cross a CLI with unknown identity.
                    }
                }
                let Some(parent) = self.parent(current) else {
                    break;
                };
                if parent.name.starts_with("tmux") {
                    break;
                }
                current = parent.ppid;
            }
            if transport {
                continue;
            }
            if owner.is_none() && !self.is_cli_process(pid) {
                let env = self.spawn_env(pid);
                // Thread identity precedes the Codex root; an unresolved
                // subagent may still use its known root session.
                owner = SPAWN_ENV.iter().find_map(|(key, source)| {
                    let sid = env.get(*key)?.to_ascii_lowercase();
                    by_sid.get(&(*source, sid)).copied()
                });
            }
            if let Some(uid) = owner.filter(|uid| live.contains(uid)) {
                working.insert(uid.to_owned());
            }
        }
        working.into_iter().collect()
    }
}
