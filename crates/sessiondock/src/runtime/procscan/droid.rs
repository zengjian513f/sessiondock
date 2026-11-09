//! Droid closes its transcript between writes. Its native log still records
//! the current TUI session with an explicit process PID and timestamp.
use super::{BareClaude, ProcTree, parse_created};
use serde_json::Value;
use std::{collections::BTreeMap, io::BufRead, path::PathBuf};

pub(super) fn identities(
    tree: &ProcTree,
    candidates: &BTreeMap<u32, BareClaude>,
) -> BTreeMap<u32, String> {
    let mut logs: BTreeMap<PathBuf, BTreeMap<u32, f64>> = BTreeMap::new();
    for (pid, process) in candidates {
        let Ok(env) = std::fs::read(tree.root().join(pid.to_string()).join("environ")) else {
            continue;
        };
        let env = String::from_utf8_lossy(&env);
        let value = |key: &str| {
            env.split('\0')
                .find_map(|entry| entry.strip_prefix(key))
                .filter(|s| !s.is_empty())
        };
        let Some(home) = value("FACTORY_HOME_OVERRIDE=").or_else(|| value("HOME=")) else {
            continue;
        };
        logs.entry(PathBuf::from(home).join(".factory/logs/droid-log-single.log"))
            .or_default()
            .insert(*pid, process.started);
    }
    let mut latest: BTreeMap<u32, (f64, String)> = BTreeMap::new();
    for (path, processes) in logs {
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        if !file.metadata().is_ok_and(|m| m.is_file()) {
            continue;
        }
        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
            if !line.contains("\"sessionId\"") {
                continue;
            }
            let Some((prefix, context)) = line.split_once(" | Context: ") else {
                continue;
            };
            let Some(time) = prefix
                .strip_prefix('[')
                .and_then(|s| s.split_once(']'))
                .and_then(|(s, _)| parse_created(s))
            else {
                continue;
            };
            let Ok(record) = serde_json::from_str::<Value>(context) else {
                continue;
            };
            let Some(pid) = record["process.pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
            else {
                continue;
            };
            let Some(started) = processes.get(&pid) else {
                continue;
            };
            if time < *started {
                continue;
            }
            let tags = &record["tags"];
            if tags["clientMode"] != "tui" && tags["subcommand"] != "tui" {
                continue;
            }
            let Some(sid) = tags["sessionId"].as_str().filter(|s| !s.is_empty()) else {
                continue;
            };
            if record["sessionId"].as_str().is_some_and(|id| id != sid) {
                continue;
            }
            if latest
                .get(&pid)
                .is_none_or(|(previous, _)| time >= *previous)
            {
                latest.insert(pid, (time, sid.to_ascii_lowercase()));
            }
        }
    }
    latest
        .into_iter()
        .map(|(pid, (_, sid))| (pid, sid))
        .collect()
}
