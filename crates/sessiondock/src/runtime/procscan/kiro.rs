//! External ACP ownership from the live process's own open diagnostic log.
//! These observations never grant process-control authority.
use super::ProcTree;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    io::{BufRead, BufReader},
};

pub(super) fn sessions(tree: &ProcTree, pid: u32) -> BTreeSet<String> {
    let mut sessions = BTreeSet::new();
    let Some(started) = tree.started_at(pid) else {
        return sessions;
    };
    let directory = tree.root().join(pid.to_string()).join("fd");
    for fd in std::fs::read_dir(directory).into_iter().flatten().flatten() {
        let Ok(target) = std::fs::read_link(fd.path()) else {
            continue;
        };
        let target = target.to_string_lossy();
        let target = target.strip_suffix(" (deleted)").unwrap_or(&target);
        if !target.contains("/.kiro/logs/") || !target.ends_with("/kiro.log") {
            continue;
        }
        // Opening the fd reference reads the log, including an unlinked log,
        // with an independent offset. Never read stdin/stdout pipes.
        let Ok(file) = std::fs::File::open(fd.path()) else {
            continue;
        };
        if !file.metadata().is_ok_and(|m| m.is_file()) {
            continue;
        }
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(row) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(at) = row["timestamp"]
                .as_str()
                .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
            else {
                continue;
            };
            if (at.timestamp_millis() as f64 / 1000.0) < started {
                continue;
            }
            let message = row["message"].as_str().unwrap_or_default();
            if let Some(rest) =
                message.strip_prefix("[KiroAgent] ACP session/load complete sessionId=")
                && let Some(sid) = rest
                    .split_ascii_whitespace()
                    .next()
                    .filter(|id| !id.is_empty())
            {
                sessions.insert(sid.to_ascii_lowercase());
            }
            // Turn-context metadata is emitted by this ACP owner, unlike a
            // directory query or a log line quoting arbitrary prompt text.
            if let Some(sid) = row["rootConversationId"]
                .as_str()
                .filter(|id| !id.is_empty())
                && row["rootTurnId"].is_string()
            {
                sessions.insert(sid.to_ascii_lowercase());
            }
        }
    }
    sessions
}
