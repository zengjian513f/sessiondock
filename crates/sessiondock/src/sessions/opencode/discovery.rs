//! Recover isolated OpenCode stores from native, completed CLI executions.
//! Commands and stdout are evidence, never instructions: nothing is executed.
//! Only a temporary job containing an observed launcher script is traversed;
//! a native SID in command output and birth inside its launch interval must
//! agree with the read-only database before a session enters the mirror.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    time::SystemTime,
};

use regex::Regex;
use serde_json::{Value, json};

use super::{Mirror, read_sessions, sql};
use crate::runtime::procscan::{SessionRow, parse_created};

#[derive(Default)]
struct Evidence {
    stamp: Option<(u64, SystemTime)>,
    launches: Vec<(PathBuf, i64, i64)>,
    observed: BTreeSet<String>,
}

#[derive(Default)]
pub(crate) struct Discovery {
    histories: HashMap<PathBuf, Evidence>,
    mirrors: BTreeMap<PathBuf, Mirror>,
}

fn sid_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\bses_[A-Za-z0-9]+\b").expect("SID regex"))
}

fn script_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"\bpython[0-9.]*\s+(?:-[A-Za-z]+\s+)*['"]?(/[^\s'"<>;]+\.py)"#)
            .expect("launcher script regex")
    })
}

/// The job's immediate temporary-directory child, not the whole temp tree.
fn job_root(script: &Path) -> Option<PathBuf> {
    let temporary = std::env::temp_dir().canonicalize().ok()?;
    let script = script.canonicalize().ok()?;
    let relative = script.strip_prefix(&temporary).ok()?;
    let first = relative.components().next()?;
    let job = temporary.join(first);
    (job.is_dir() && job != temporary).then_some(job)
}

fn evidence(path: &Path, known: &mut Evidence) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    let stamp = (metadata.len(), metadata.modified()?);
    if known.stamp == Some(stamp) {
        return Ok(());
    }
    let mut next = Evidence::default();
    for line in BufReader::new(fs::File::open(path)?).lines() {
        let line = line?;
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if record["type"] != "event_msg" || record["payload"]["type"] != "item_completed" {
            continue;
        }
        let payload = &record["payload"];
        let item = &payload["item"];
        if item["type"] != "CommandExecution" || item["status"] != "completed" {
            continue;
        }
        for output in ["stdout", "aggregated_output"] {
            if let Some(text) = item[output].as_str() {
                next.observed.extend(
                    sid_regex()
                        .find_iter(text)
                        .map(|sid| sid.as_str().to_owned()),
                );
            }
        }
        let (Some(start), Some(end)) = (
            payload["started_at_ms"].as_i64(),
            payload["completed_at_ms"].as_i64(),
        ) else {
            continue;
        };
        let Some(command) = item["command"]
            .as_array()
            .and_then(|args| args.last())
            .and_then(Value::as_str)
        else {
            continue;
        };
        for capture in script_regex().captures_iter(command) {
            let script = Path::new(&capture[1]);
            // A poll/query script is not launch evidence. Inspect text only;
            // the command receipt and DB timestamps remain the authority.
            let Ok(text) = fs::read_to_string(script) else {
                continue;
            };
            if !text.contains("opencode run") {
                continue;
            }
            if let Some(root) = job_root(script) {
                next.launches.push((root, start, end));
            }
        }
    }
    next.stamp = Some(stamp);
    *known = next;
    Ok(())
}

fn databases(root: &Path, found: &mut BTreeSet<PathBuf>) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let candidate = path.join(".local/share/opencode/opencode.db");
        if candidate.is_file() {
            if let Ok(canonical) = candidate.canonicalize()
                && canonical.starts_with(root)
            {
                found.insert(canonical);
            }
            // OpenCode's shell output/cache trees cannot contain another home.
            continue;
        }
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            // Do not follow symlinks into unrelated data or credentials.
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push(entry.path());
            }
        }
    }
}

impl Discovery {
    pub(crate) fn sync(&mut self, root: &Path, sessions: &[SessionRow]) -> io::Result<()> {
        let mut jobs: BTreeMap<PathBuf, Vec<(&SessionRow, i64, i64, &BTreeSet<String>)>> =
            BTreeMap::new();
        for parent in sessions.iter().filter(|row| row.source == "codex") {
            let path = PathBuf::from(&parent.path);
            let known = self.histories.entry(path.clone()).or_default();
            // A vanished/unreadable rollout does not affect other launchers.
            if let Err(error) = evidence(&path, known) {
                eprintln!("OpenCode launch evidence: {error}");
                continue;
            }
        }
        for parent in sessions.iter().filter(|row| row.source == "codex") {
            let Some(known) = self.histories.get(Path::new(&parent.path)) else {
                continue;
            };
            for (job, start, end) in &known.launches {
                jobs.entry(job.clone())
                    .or_default()
                    .push((parent, *start, *end, &known.observed));
            }
        }
        let mut selected: BTreeMap<PathBuf, HashMap<String, Value>> = BTreeMap::new();
        for (job, launches) in jobs {
            let mut stores = BTreeSet::new();
            databases(&job, &mut stores);
            for database in stores {
                let connection = match rusqlite::Connection::open_with_flags(
                    &database,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
                ) {
                    Ok(connection) => connection,
                    Err(error) => {
                        eprintln!("OpenCode isolated store: {}", sql(error));
                        continue;
                    }
                };
                connection
                    .busy_timeout(std::time::Duration::from_millis(1500))
                    .map_err(sql)?;
                let rows = match read_sessions(&connection) {
                    Ok(rows) => rows,
                    Err(error) => {
                        eprintln!("OpenCode isolated store: {error}");
                        continue;
                    }
                };
                for row in &rows {
                    if row.fields["parent_id"]
                        .as_str()
                        .is_some_and(|id| !id.is_empty())
                    {
                        continue;
                    }
                    let Some(created) = row.fields["time_created"].as_i64() else {
                        continue;
                    };
                    let parents: BTreeMap<_, _> = launches
                        .iter()
                        .filter(|(parent, start, end, seen)| {
                            *start <= created
                                && created <= *end
                                && seen.contains(&row.id)
                                && parse_created(&parent.created)
                                    .is_some_and(|born| born * 1000.0 <= created as f64)
                        })
                        .map(|(parent, _, _, _)| {
                            ((parent.source.clone(), parent.sid.clone()), *parent)
                        })
                        .collect();
                    if let Some(((source, sid), _)) =
                        parents.iter().next().filter(|_| parents.len() == 1)
                    {
                        selected
                            .entry(database.clone())
                            .or_default()
                            .insert(row.id.clone(), json!({"source": source, "sid": sid}));
                    }
                }
                // Keep native OpenCode subagents visible with their own identity.
                loop {
                    let mut changed = false;
                    for row in &rows {
                        let Some(parent) = row.fields["parent_id"].as_str() else {
                            continue;
                        };
                        let ids = selected.entry(database.clone()).or_default();
                        if ids.contains_key(parent) && !ids.contains_key(&row.id) {
                            ids.insert(row.id.clone(), Value::Null);
                            changed = true;
                        }
                    }
                    if !changed {
                        break;
                    }
                }
            }
        }
        // Never pick one store arbitrarily when native identities collide.
        let mut counts = HashMap::<String, usize>::new();
        for ids in selected.values() {
            for id in ids.keys() {
                *counts.entry(id.clone()).or_default() += 1;
            }
        }
        for (database, mut ids) in selected {
            ids.retain(|id, _| counts[id] == 1);
            let mirror = self
                .mirrors
                .entry(database.clone())
                .or_insert_with(|| Mirror::new(database, root.to_path_buf()));
            mirror.external = Some(ids);
            mirror.sync()?;
        }
        Ok(())
    }
}
