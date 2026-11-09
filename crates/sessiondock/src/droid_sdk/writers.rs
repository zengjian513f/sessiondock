//! Cross-platform Droid writer evidence. Native process identity, explicit
//! resume arguments and timestamped TUI logs remain separate from mere cwd.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(serde::Deserialize)]
struct Candidate {
    pid: u32,
    #[serde(default)]
    ppid: u32,
    command: String,
    started: f64,
    cwd: Option<PathBuf>,
}

impl Bridge {
    /// Follow only native callingSessionId evidence. A flattened UI owner can
    /// omit intermediate parents, whose runtime may still own the child task.
    pub(super) fn ancestor_sids(&self, session: &Session) -> Result<BTreeSet<String>, String> {
        use std::io::BufRead;
        let mut pending = vec![session.path.clone()];
        let mut ancestors = BTreeSet::new();
        let mut seen = BTreeSet::from([session.sid.clone()]);
        while let Some(path) = pending.pop() {
            let file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "droid_sdk: native parent read failed ({})",
                        error.kind()
                    ));
                }
            };
            let line = std::io::BufReader::new(file)
                .lines()
                .next()
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            let Ok(header) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(parent) = header["callingSessionId"]
                .as_str()
                .filter(|sid| !sid.is_empty())
            else {
                continue;
            };
            ancestors.insert(parent.to_ascii_lowercase());
            if !seen.insert(parent.to_owned()) {
                continue;
            }
            // Missing/cross-machine parents still provide an explicit SID to
            // check for writers. Ambiguous files contribute every known edge;
            // neither absence, ambiguity nor a cycle invents a live process.
            let name = format!("{parent}.jsonl");
            let legacy = self.root.join(&name);
            if legacy.is_file() {
                pending.push(legacy);
            }
            for entry in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.path().is_dir() {
                    let candidate = entry.path().join(&name);
                    if candidate.is_file() {
                        pending.push(candidate);
                    }
                }
            }
        }
        Ok(ancestors)
    }

    pub(super) async fn portable_conflict(&self, session: &Arc<Session>) -> Result<(), String> {
        let ancestors = self.ancestor_sids(session)?;
        let fixture = self.proc_root.join("droid-processes.json");
        let candidates: Vec<Candidate> =
            if self.proc_root != Path::new("/proc") && fixture.is_file() {
                serde_json::from_slice(&tokio::fs::read(fixture).await.map_err(|e| e.to_string())?)
                    .map_err(|e| format!("droid_sdk: process snapshot unreadable: {e}"))?
            } else {
                candidates().await?
            };
        let own: BTreeSet<_> = lock(&session.live).pid.into_iter().collect();
        let known: BTreeMap<_, _> = lock(&self.state)
            .sessions
            .values()
            .filter_map(|s| {
                lock(&s.live)
                    .pid
                    .map(|pid| (pid, s.sid.to_ascii_lowercase()))
            })
            .collect();
        let parents: BTreeMap<_, _> = candidates.iter().map(|p| (p.pid, p.ppid)).collect();
        let ours = |pid: u32| {
            let mut pid = pid;
            let mut seen = BTreeSet::new();
            while seen.insert(pid) {
                if own.contains(&pid) {
                    return true;
                }
                let Some(parent) = parents.get(&pid) else {
                    break;
                };
                pid = *parent;
            }
            false
        };
        let log = self
            .native_home()?
            .join(".factory/logs/droid-log-single.log");
        let rows = tokio::fs::read_to_string(log).await.unwrap_or_default();
        let mut latest: BTreeMap<u32, (f64, String)> = BTreeMap::new();
        for line in rows.lines() {
            let Some((prefix, context)) = line.split_once(" | Context: ") else {
                continue;
            };
            let Some(time) = prefix
                .strip_prefix('[')
                .and_then(|s| s.split_once(']'))
                .and_then(|(s, _)| crate::runtime::procscan::parse_created(s))
            else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(context) else {
                continue;
            };
            let Some(pid) = value["process.pid"]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
            else {
                continue;
            };
            let Some(process) = candidates.iter().find(|p| p.pid == pid) else {
                continue;
            };
            if time < process.started {
                continue;
            }
            let tags = &value["tags"];
            if tags["clientMode"] != "tui" && tags["subcommand"] != "tui" {
                continue;
            }
            let Some(sid) = tags["sessionId"].as_str().filter(|s| !s.is_empty()) else {
                continue;
            };
            if value["sessionId"].as_str().is_some_and(|v| v != sid) {
                continue;
            }
            if latest
                .get(&pid)
                .is_none_or(|(previous, _)| time >= *previous)
            {
                latest.insert(pid, (time, sid.to_ascii_lowercase()));
            }
        }
        let resume = regex::Regex::new(
            r#"(?:^|\s)(?:(?:--resume|-r|--session-id|-s)(?:=|\s+)|resume\s+)["']?([^\s"']+)"#,
        )
        .expect("static regex");
        let uuid =
            regex::Regex::new(r"(?i)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
                .expect("static uuid regex");
        for process in candidates {
            if process.command.trim_end().ends_with(" --help")
                || process.command.trim_end().ends_with(" --version")
                || process.command.contains(" update --check")
            {
                continue;
            }
            if ours(process.pid) {
                continue;
            }
            let explicit = resume
                .captures(&process.command)
                .and_then(|c| uuid.find(&c[1]).map(|id| id.as_str().to_ascii_lowercase()));
            let sid = latest
                .get(&process.pid)
                .map(|(_, sid)| sid.as_str())
                .or(explicit.as_deref())
                .or_else(|| known.get(&process.pid).map(String::as_str));
            if let Some(sid) = sid {
                if sid == session.sid.to_ascii_lowercase() || ancestors.contains(sid) {
                    return Err("droid_sdk_session_busy: an independent Droid TUI owns this SID; stop it explicitly before SDK resume (shared_tui:false).".into());
                }
                continue;
            }
            // An unrelated cwd is affirmative evidence, unlike absent Windows
            // cwd. Only unresolved Droid candidates constrain existing resume.
            let cwd_matches = process.cwd.as_ref().is_none_or(|cwd| {
                crate::runtime::procscan::resolve_path(&cwd.to_string_lossy())
                    == crate::runtime::procscan::resolve_path(&session.cwd.to_string_lossy())
            });
            if session.path.is_file() && cwd_matches {
                return Err("droid_sdk_session_busy: a Droid process has unresolved session identity in this cwd (or its cwd is unavailable); existing-session resume remains unavailable until that writer is resolved or stopped. No takeover was attempted.".into());
            }
        }
        Ok(())
    }
}

#[cfg(any(target_os = "macos", windows))]
async fn output(program: &str, args: &[&str]) -> Result<String, String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), command.output())
        .await
        .map_err(|_| "droid_sdk: native writer enumeration timed out".to_owned())?
        .map_err(|e| format!("droid_sdk: native writer enumeration failed: {e}"))?;
    if !result.status.success() {
        return Err("droid_sdk: native writer enumeration failed".into());
    }
    Ok(String::from_utf8_lossy(&result.stdout).into_owned())
}

#[cfg(target_os = "macos")]
async fn candidates() -> Result<Vec<Candidate>, String> {
    let listing = output("/bin/ps", &["-axo", "pid=,ppid=,comm="]).await?;
    let clock = crate::runtime::process::load_clock()
        .await
        .ok_or("droid_sdk: process clock unavailable")?;
    let mut rows = Vec::new();
    for line in listing.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(name)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if Path::new(name).file_name().and_then(|s| s.to_str()) != Some("droid") {
            continue;
        }
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        let identity = match crate::runtime::process::observe(pid, true).await {
            Ok(identity) => identity,
            Err(crate::runtime::process::IdentityFailure::NotOwned | crate::runtime::process::IdentityFailure::NotVisible) => continue,
            Err(_) => return Err("droid_sdk: Droid process identity is unreadable; resume cannot confirm writer ownership".into()),
        };
        let command = output("/bin/ps", &["-p", &pid.to_string(), "-o", "command="]).await?;
        let cwd = output(
            "/usr/sbin/lsof",
            &["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"],
        )
        .await
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|line| line.strip_prefix('n').map(PathBuf::from))
        });
        if crate::runtime::process::observe(pid, true).await.ok() != Some(identity) {
            continue;
        }
        rows.push(Candidate {
            pid,
            ppid: ppid.parse().unwrap_or(0),
            command,
            started: clock.unix_seconds(identity.start_time),
            cwd,
        });
    }
    Ok(rows)
}

#[cfg(windows)]
async fn candidates() -> Result<Vec<Candidate>, String> {
    let listing=output("powershell.exe", &["-NoProfile","-NonInteractive","-Command", "$ErrorActionPreference='Stop'; @(Get-CimInstance Win32_Process -Filter \"Name='droid.exe'\" | Select-Object ProcessId,ParentProcessId,CommandLine,@{Name='Started';Expression={([DateTimeOffset]$_.CreationDate).ToUnixTimeMilliseconds()/1000.0}}) | ConvertTo-Json -Compress"]).await?;
    let value: Value = if listing.trim().is_empty() {
        json!([])
    } else {
        serde_json::from_str(listing.trim_start_matches('\u{feff}'))
            .map_err(|_| "droid_sdk: malformed native process enumeration")?
    };
    let values = if let Some(rows) = value.as_array() {
        rows.clone()
    } else {
        vec![value]
    };
    let clock = crate::runtime::process::load_clock()
        .await
        .ok_or("droid_sdk: process clock unavailable")?;
    let mut rows = Vec::new();
    for value in values {
        let Some(pid) = value["ProcessId"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
        else {
            continue;
        };
        let identity = match crate::runtime::process::observe(pid, true).await {
            Ok(identity) => identity,
            Err(crate::runtime::process::IdentityFailure::NotOwned | crate::runtime::process::IdentityFailure::NotVisible) => continue,
            Err(_) => return Err("droid_sdk: Droid process identity is unreadable; resume cannot confirm writer ownership".into()),
        };
        let started = clock.unix_seconds(identity.start_time);
        if value["Started"]
            .as_f64()
            .is_none_or(|observed| (observed - started).abs() > 0.02)
        {
            continue;
        }
        rows.push(Candidate {
            pid,
            ppid: value["ParentProcessId"].as_u64().unwrap_or(0) as u32,
            command: value["CommandLine"].as_str().unwrap_or_default().into(),
            started: clock.unix_seconds(identity.start_time),
            cwd: None,
        });
    }
    Ok(rows)
}

#[cfg(not(any(target_os = "macos", windows)))]
async fn candidates() -> Result<Vec<Candidate>, String> {
    Err("droid_sdk: native process enumeration unavailable on this platform".into())
}
