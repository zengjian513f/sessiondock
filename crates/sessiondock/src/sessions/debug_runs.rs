//! Debug-run registry (Python `agenthub/debug_runs.py`): paid monkey/test
//! sessions registered under a run id stay out of the ordinary views, and
//! `?debug_run=<id>` shows exactly that run.
//!
//! The registry is `<SESSIONDOCK_STATE_DIR>/debug-runs.json` in Python's
//! format — `{"version":1,"runs":{<run_id>:{"root":…,"created":…,
//! "sessions":[{source,cwd,sid,uid,name}]}}}` — written by the test tooling
//! (never by this service) and reloaded whenever its `stat` changes, like
//! Python's mtime reload. A missing, unreadable or malformed file is an
//! empty registry: nothing is hidden and every `debug_run` view is empty.
//!
//! Matching is Python's `_match`: a row's `uid`, `sid` or `name` found in a
//! run's session list, or its `cwd` equal to / under a run's root; the run
//! registered first (registry order) wins. `filter_rows` keeps the rows that
//! match no run for the default view, and only the rows of `run_id` for a
//! debug view (an unknown or malformed id yields nothing, as in Python).

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;

pub const DEBUG_RUNS_FILENAME: &str = "debug-runs.json";
/// `?debug_run=<id>` of a raw query string (Python `_debug_run`: the first
/// 64 characters). Valid ids are `[A-Za-z0-9_-]`, so no percent-decoding
/// is needed: an encoded or otherwise malformed id names no run.
pub fn debug_run_of(query: Option<&str>) -> String {
    query
        .unwrap_or("")
        .split('&')
        .find_map(|pair| pair.strip_prefix("debug_run="))
        .map(|value| value.chars().take(64).collect())
        .unwrap_or_default()
}

fn valid_id(run_id: &str) -> bool {
    (1..=64).contains(&run_id.len())
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// `os.path.normpath` for POSIX paths: collapse separators, drop `.`,
/// resolve `..` lexically, keep a leading `/`, no trailing separator.
pub(crate) fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    match (absolute, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_owned(),
        (false, false) => joined,
    }
}

/// Python `_abspath`: normalize an absolute path lexically; a relative one
/// is resolved against the process working directory.
fn abspath(path: &str) -> String {
    if path.starts_with('/') {
        return normpath(path);
    }
    match std::env::current_dir() {
        Ok(cwd) => normpath(&format!("{}/{path}", cwd.to_string_lossy())),
        Err(_) => normpath(path),
    }
}

struct Root {
    root: String,
    prefix: String,
    order: usize,
    run_id: String,
}

/// The registry pre-resolved into lookup tables (Python `_build_index`).
#[derive(Default)]
pub struct RunIndex {
    roots: Vec<Root>,
    tables: HashMap<&'static str, HashMap<String, (usize, String)>>,
    ids: BTreeSet<String>,
}

impl RunIndex {
    fn build(runs: &serde_json::Map<String, Value>) -> Self {
        let mut index = Self::default();
        for key in ["uid", "sid", "name"] {
            index.tables.insert(key, HashMap::new());
        }
        for (order, (run_id, run)) in runs.iter().enumerate() {
            let Some(run) = run.as_object() else {
                continue;
            };
            index.ids.insert(run_id.clone());
            let root = text(&run["root"]);
            // A root that is relative or unnormalized could never equal a
            // normalized absolute cwd, so it is dropped (Python drops it too).
            if !root.is_empty() && root == normpath(root) && root.starts_with('/') {
                let prefix = if root.ends_with('/') {
                    root.to_owned()
                } else {
                    format!("{root}/")
                };
                index.roots.push(Root {
                    root: root.to_owned(),
                    prefix,
                    order,
                    run_id: run_id.clone(),
                });
            }
            for item in run["sessions"].as_array().into_iter().flatten() {
                if !item.is_object() {
                    continue;
                }
                for key in ["uid", "sid", "name"] {
                    let value = text(&item[key]);
                    if value.is_empty() {
                        continue;
                    }
                    index
                        .tables
                        .get_mut(key)
                        .expect("tables seeded")
                        .entry(value.to_owned())
                        .or_insert((order, run_id.clone()));
                }
            }
        }
        index
    }

    /// No run registered anything: nothing is hidden.
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty() && self.tables.values().all(HashMap::is_empty)
    }

    /// Python `get(run_id) is not None`: a well-formed, registered id.
    pub fn known(&self, run_id: &str) -> bool {
        valid_id(run_id) && self.ids.contains(run_id)
    }

    /// Python `_match`: the run this row belongs to, if any.
    pub fn run_for<'a>(&'a self, row: &Value) -> Option<&'a str> {
        let mut best: Option<&(usize, String)> = None;
        for key in ["uid", "sid", "name"] {
            let value = text(&row[key]);
            if value.is_empty() {
                continue;
            }
            if let Some(hit) = self.tables[key].get(value)
                && best.is_none_or(|current| hit.0 < current.0)
            {
                best = Some(hit);
            }
        }
        let cwd = text(&row["cwd"]);
        if !cwd.is_empty() && !self.roots.is_empty() {
            let path = abspath(cwd);
            for root in &self.roots {
                if best.is_some_and(|current| root.order >= current.0) {
                    break; // roots keep run order; none can win now
                }
                if path == root.root || path.starts_with(&root.prefix) {
                    return Some(&root.run_id);
                }
            }
        }
        best.map(|hit| hit.1.as_str())
    }

    /// Whether `row` stays in the view `run_id` names (`""` = the ordinary
    /// view, which excludes every registered run).
    pub fn keeps(&self, row: &Value, run_id: &str) -> bool {
        if run_id.is_empty() {
            self.is_empty() || self.run_for(row).is_none()
        } else {
            self.known(run_id) && !self.is_empty() && self.run_for(row) == Some(run_id)
        }
    }

    /// Python `filter_rows`.
    pub fn filter_rows(&self, rows: Vec<Value>, run_id: &str) -> Vec<Value> {
        if run_id.is_empty() && self.is_empty() {
            return rows;
        }
        rows.into_iter()
            .filter(|row| self.keeps(row, run_id))
            .collect()
    }

    /// Retain the rows of the view in `document["sessions"]`; the uids of
    /// the dropped rows are returned so a caller can hide their other
    /// evidence (a managed instance of a hidden session, say).
    pub fn split_document(&self, document: &mut Value, run_id: &str) -> BTreeSet<String> {
        let mut hidden = BTreeSet::new();
        if run_id.is_empty() && self.is_empty() {
            return hidden;
        }
        if let Some(rows) = document.get_mut("sessions").and_then(Value::as_array_mut) {
            rows.retain(|row| {
                let keep = self.keeps(row, run_id);
                if !keep && let Some(uid) = row["uid"].as_str() {
                    hidden.insert(uid.to_owned());
                }
                keep
            });
        }
        hidden
    }
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}

/// Parse a registry document (Python `_read`): anything but a well-formed
/// `{"runs": {…}}` is the empty registry.
pub(crate) fn parse(bytes: &[u8]) -> RunIndex {
    let Ok(raw) = serde_json::from_slice::<Value>(bytes) else {
        return RunIndex::default();
    };
    match raw["runs"].as_object() {
        Some(runs) => RunIndex::build(runs),
        None => RunIndex::default(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    size: u64,
    mtime_ns: u128,
    ino: u64,
}

struct Cached {
    stamp: Option<Stamp>,
    index: Arc<RunIndex>,
}

/// The registry file, reloaded by stamp on every use (one `stat` when
/// unchanged). The service never writes it.
pub struct DebugRuns {
    path: PathBuf,
    cache: Mutex<Cached>,
}

impl DebugRuns {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            cache: Mutex::new(Cached {
                stamp: None,
                index: Arc::new(RunIndex::default()),
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn stamp(&self) -> Option<Stamp> {
        let meta = fs::symlink_metadata(&self.path).ok()?;
        if !meta.is_file() {
            return None;
        }
        #[cfg(unix)]
        let ino = {
            use std::os::unix::fs::MetadataExt;
            meta.ino()
        };
        #[cfg(not(unix))]
        let ino = 0;
        Some(Stamp {
            size: meta.len(),
            mtime_ns: meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_nanos()),
            ino,
        })
    }

    /// The current registry index; the same `Arc` while the file is unchanged.
    pub fn current(&self) -> Arc<RunIndex> {
        let stamp = self.stamp();
        let Ok(mut cache) = self.cache.lock() else {
            return Arc::new(RunIndex::default());
        };
        if cache.stamp == stamp && (stamp.is_some() || cache.index.is_empty()) {
            return cache.index.clone();
        }
        let index = match stamp {
            Some(_) => fs::read(&self.path)
                .map(|bytes| parse(&bytes))
                .unwrap_or_default(),
            _ => RunIndex::default(),
        };
        cache.stamp = stamp;
        cache.index = Arc::new(index);
        cache.index.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn registry() -> RunIndex {
        parse(
            json!({"version": 1, "runs": {
                "run-a": {"root": "/tmp/run-a", "created": 1.0, "sessions": [
                    {"source": "claude", "cwd": "/tmp/run-a/claude/s1", "sid": "sid-a1", "uid": "claude:a1", "name": "agenthub-monkey-claude-a1"},
                    {"source": "codex", "cwd": "/tmp/run-a/codex/s1", "sid": "", "uid": "", "name": "agenthub-monkey-codex-a2"}
                ]},
                "run-b": {"root": "/tmp/run-b", "sessions": [
                    {"source": "claude", "cwd": "/elsewhere", "sid": "sid-b1", "uid": "", "name": ""}
                ]},
                "bad": "not an object",
                "relative": {"root": "relative/root", "sessions": []},
                "trailing": {"root": "/tmp/trail/", "sessions": []}
            }})
            .to_string()
            .as_bytes(),
        )
    }

    #[test]
    fn normpath_matches_python() {
        assert_eq!(normpath("/tmp//a/./b/../c/"), "/tmp/a/c");
        assert_eq!(normpath("/"), "/");
        assert_eq!(normpath("/.."), "/");
        assert_eq!(normpath("a/../../b"), "../b");
        assert_eq!(normpath(""), ".");
        assert_eq!(normpath("./"), ".");
    }

    #[test]
    fn matches_by_uid_sid_name_and_cwd_root() {
        let index = registry();
        assert!(!index.is_empty());
        assert!(index.known("run-a") && index.known("run-b"));
        // Python `get`: a run that is not an object is `None`, so unknown.
        assert!(!index.known("bad") && !index.known("missing") && !index.known("bad id!"));
        assert_eq!(index.run_for(&json!({"uid": "claude:a1"})), Some("run-a"));
        assert_eq!(index.run_for(&json!({"sid": "sid-b1"})), Some("run-b"));
        assert_eq!(
            index.run_for(&json!({"name": "agenthub-monkey-codex-a2"})),
            Some("run-a")
        );
        assert_eq!(
            index.run_for(&json!({"uid": "x", "cwd": "/tmp/run-b/anything/deeper"})),
            Some("run-b")
        );
        assert_eq!(index.run_for(&json!({"cwd": "/tmp/run-b"})), Some("run-b"));
        assert_eq!(index.run_for(&json!({"cwd": "/tmp/run-bx"})), None);
        assert_eq!(
            index.run_for(&json!({"cwd": "/tmp/../tmp/run-a/x"})),
            Some("run-a")
        );
        assert_eq!(
            index.run_for(&json!({"uid": "claude:other", "cwd": "/home/x"})),
            None
        );
        // Earliest run wins: sid-b1 (run-b) inside run-a's root → run-a.
        assert_eq!(
            index.run_for(&json!({"sid": "sid-b1", "cwd": "/tmp/run-a/q"})),
            Some("run-a")
        );
        assert_eq!(index.run_for(&json!({"cwd": "relative/root/x"})), None);
        // Python drops a root that is not already normalized (`_build_index`).
        assert_eq!(index.run_for(&json!({"cwd": "/tmp/trail/x"})), None);
    }

    #[test]
    fn filter_rows_follows_python() {
        let index = registry();
        let rows = vec![
            json!({"uid": "claude:a1", "cwd": "/tmp/run-a/claude/s1"}),
            json!({"uid": "claude:z", "cwd": "/home/z"}),
            json!({"uid": "codex:b", "sid": "sid-b1", "cwd": "/home/b"}),
        ];
        let default = index.filter_rows(rows.clone(), "");
        assert_eq!(default.len(), 1);
        assert_eq!(default[0]["uid"], "claude:z");
        let run_a = index.filter_rows(rows.clone(), "run-a");
        assert_eq!(run_a.len(), 1);
        assert_eq!(run_a[0]["uid"], "claude:a1");
        assert!(index.filter_rows(rows.clone(), "missing").is_empty());
        assert!(index.filter_rows(rows.clone(), "bad id!").is_empty());
        let empty = RunIndex::default();
        assert_eq!(empty.filter_rows(rows.clone(), "").len(), 3);
        assert!(empty.filter_rows(rows, "run-a").is_empty());
    }

    #[test]
    fn split_document_returns_hidden_uids() {
        let index = registry();
        let mut document = json!({"sessions": [
            {"uid": "claude:a1", "cwd": "/tmp/run-a/claude/s1"},
            {"uid": "claude:z", "cwd": "/home/z"}
        ]});
        let hidden = index.split_document(&mut document, "");
        assert_eq!(hidden, BTreeSet::from(["claude:a1".to_owned()]));
        assert_eq!(document["sessions"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn malformed_registry_is_empty() {
        assert!(parse(b"not json").is_empty());
        assert!(parse(b"[]").is_empty());
        assert!(parse(br#"{"version":1,"runs":[]}"#).is_empty());
        assert!(parse(br#"{"version":1,"runs":{}}"#).is_empty());
    }

    #[test]
    fn reloads_when_the_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DEBUG_RUNS_FILENAME);
        let runs = DebugRuns::new(path.clone());
        assert!(runs.current().is_empty());
        fs::write(
            &path,
            json!({"version": 1, "runs": {"r": {"root": "/tmp/r", "sessions": []}}}).to_string(),
        )
        .unwrap();
        let first = runs.current();
        assert!(!first.is_empty() && first.known("r"));
        assert!(Arc::ptr_eq(&first, &runs.current()));
        fs::remove_file(&path).unwrap();
        assert!(runs.current().is_empty());
    }
}
