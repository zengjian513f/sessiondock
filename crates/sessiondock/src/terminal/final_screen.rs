//! Read-only access to the final screens exited ptyhost sessions leave under
//! `<ptyhost dir>/screens/<id>/` (`meta.json` + `snapshot.json`).
//!
//! The host writes one when its session exits, before it reports the exit:
//! a reset grid snapshot that carries the whole retained history
//! (`docs/terminal-grid.md`). This module lists them by host name and serves
//! one as saved. Nothing here takes an
//! ownership lease or talks to a host process. See `docs/terminal-final-screen.md`.
//!
//! Hosts from before final screens wrote segmented recordings under
//! `records/`; those are no longer read and are deleted once their host is gone.

use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

pub const SCREENS_SUBDIR: &str = "screens";
const LEGACY_RECORDS_SUBDIR: &str = "records";

/// One final screen as listed on a terminal row.
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub created_ms: u64,
    pub ended_ms: u64,
    pub exit: Value,
}

/// `<created_ms>-<host_pid>-<name>`: digits, digits, then the host's safe name.
pub fn valid_id(id: &str) -> bool {
    let mut parts = id.splitn(3, '-');
    let (Some(created), Some(pid), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let digits = |s: &str, max: usize| {
        !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit())
    };
    digits(created, 20)
        && digits(pid, 10)
        && !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn meta(dir: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(dir.join("meta.json")).ok()?).ok()
}

/// Directories of `subdir` with a valid id, paired with their `meta.json`.
fn scan(root: &Path, subdir: &str) -> io::Result<Vec<(String, PathBuf, Value)>> {
    let entries = match std::fs::read_dir(root.join(subdir)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(id) = name.to_str() else { continue };
        let path = entry.path();
        if !valid_id(id) || !path.is_dir() {
            continue;
        }
        if let Some(meta) = meta(&path) {
            out.push((id.to_string(), path, meta));
        }
    }
    Ok(out)
}

fn entry_of(id: String, meta: &Value) -> Entry {
    let num = |key: &str| meta.get(key).and_then(Value::as_u64).unwrap_or(0);
    Entry {
        id,
        name: meta
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        created_ms: num("created_ms"),
        ended_ms: num("ended_ms"),
        exit: meta.get("exit").cloned().unwrap_or(Value::Null),
    }
}

/// Every final screen under `root`, newest first. A missing `screens/` is empty.
pub fn list(root: &Path) -> io::Result<Vec<Entry>> {
    let mut out: Vec<Entry> = scan(root, SCREENS_SUBDIR)?
        .into_iter()
        .map(|(id, _, meta)| entry_of(id, &meta))
        .collect();
    out.sort_by(|a, b| {
        b.created_ms
            .cmp(&a.created_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(out)
}

/// Delete host `name`'s final screens and any recordings an older host left
/// (a discarded SSH session takes them with it). Returns how many were removed.
pub fn remove_for_host(root: &Path, name: &str) -> io::Result<usize> {
    let mut removed = 0;
    for subdir in [SCREENS_SUBDIR, LEGACY_RECORDS_SUBDIR] {
        for (_, path, meta) in scan(root, subdir)? {
            if meta.get("name").and_then(Value::as_str) == Some(name) {
                std::fs::remove_dir_all(&path)?;
                removed += 1;
            }
        }
    }
    Ok(removed)
}

fn host_alive(pid: u64) -> bool {
    #[cfg(target_os = "linux")]
    {
        pid > 0 && Path::new("/proc").join(pid.to_string()).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
}

/// Delete the segmented recordings hosts from before final screens wrote,
/// once their host is gone. Returns how many were removed.
pub fn remove_legacy_recordings(root: &Path) -> io::Result<usize> {
    let mut removed = 0;
    for (_, path, meta) in scan(root, LEGACY_RECORDS_SUBDIR)? {
        let pid = meta.get("host_pid").and_then(Value::as_u64).unwrap_or(0);
        if meta.get("ended_ms").is_none() && host_alive(pid) {
            continue;
        }
        std::fs::remove_dir_all(&path)?;
        removed += 1;
    }
    Ok(removed)
}

/// `{exit, ended_ms, cols, rows, snapshot}` for one final screen, where
/// `snapshot` is the reset grid snapshot the host saved, with all of its
/// retained history. `None` when the id is invalid, absent or unreadable.
pub fn render(root: &Path, id: &str) -> Option<Value> {
    if !valid_id(id) {
        return None;
    }
    let dir = root.join(SCREENS_SUBDIR).join(id);
    let meta = meta(&dir)?;
    let snapshot: Value =
        serde_json::from_slice(&std::fs::read(dir.join("snapshot.json")).ok()?).ok()?;
    Some(json!({
        "exit": meta.get("exit").cloned().unwrap_or(Value::Null),
        "ended_ms": meta.get("ended_ms").cloned().unwrap_or(Value::Null),
        "cols": snapshot.get("cols").cloned().unwrap_or(Value::Null),
        "rows": snapshot.get("rows").cloned().unwrap_or(Value::Null),
        "snapshot": snapshot,
    }))
}
