//! OpenCode session mirror.
//!
//! OpenCode 2 keeps every session in one SQLite database (`session_v2` rows
//! and ordered `session_message` rows). The session index reads files, so
//! this mirror projects each session into a SessionDock-owned directory
//! `<root>/<project id>/<session id>/`:
//!
//! - `summary.json`: the session row (and its project worktree), rewritten
//!   atomically whenever it changes;
//! - `messages.jsonl`: one line per settled message row, exactly as stored
//!   (`{"id","type","seq","time_created","time_updated","data"}`), appended
//!   in `seq` order. A streaming assistant row is written once it completes.
//!   When an exported row changes or disappears (revert, compaction) the file
//!   is rewritten atomically, which the index reads as a new file.
//!
//! The database is only ever opened read-only; the mirror never writes to
//! OpenCode's data. See `docs/opencode.md`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};

pub(crate) mod discovery;

/// Supplemental stores have no configured launcher in their original sandbox.
/// Their history remains readable, but the default CLI must not control them.
pub(crate) const ISOLATED_CONTROL_NOTE: &str =
    "独立沙箱中的 OpenCode 会话：历史可读；网页暂不支持在原沙箱中恢复或删除";

pub(crate) fn isolated_store(path: &Path) -> bool {
    fs::read(path.join(SUMMARY_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|summary| summary["database"].is_string())
}

/// Mirror layout version recorded in every `summary.json`.
pub const FORMAT: &str = "sessiondock-opencode-mirror";
pub const SUMMARY_FILE: &str = "summary.json";
pub const MESSAGES_FILE: &str = "messages.jsonl";
/// How often the database is polled for changed sessions.
pub const POLL: Duration = Duration::from_secs(1);
/// Every this many polls every session's message rows are re-checked even
/// when its `time_updated` did not move.
const FULL_CHECK_EVERY: u32 = 60;

/// OpenCode identifiers (`ses_…`, project hashes) as path components.
pub fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        && !id.starts_with('-')
}

/// A new session id in OpenCode's own shape (`ses_` + 12 hex digits of a
/// descending timestamp + 14 base62 characters), so a session SessionDock
/// creates sorts among OpenCode's own sessions.
pub fn new_session_id() -> io::Result<String> {
    const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64);
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
    let counter = u64::from(u16::from_le_bytes([random[14], random[15]]) & 0x0fff);
    let descending = !(millis.wrapping_mul(0x1000).wrapping_add(counter)) & 0xffff_ffff_ffff;
    let tail: String = random[..14]
        .iter()
        .map(|byte| BASE62[usize::from(*byte) % BASE62.len()] as char)
        .collect();
    Ok(format!("ses_{descending:012x}{tail}"))
}

#[derive(Default)]
struct Exported {
    /// `(message id, time_updated)` of every line in `messages.jsonl`.
    rows: Vec<(String, i64)>,
    /// `(count, max seq, max time_updated)` once every row is exported;
    /// `None` while a row is still streaming, so the next poll looks again.
    signature: Option<(i64, i64, i64)>,
    /// The file on disk was reconciled at least once by this process.
    initialized: bool,
}

struct Mirrored {
    dir: PathBuf,
    summary: String,
    session_updated: i64,
    exported: Exported,
}

pub struct Mirror {
    database: PathBuf,
    root: PathBuf,
    connection: Option<Connection>,
    sessions: HashMap<String, Mirrored>,
    polls: u32,
    last_error: Option<String>,
    /// Independently stored sessions proven by native command receipts.
    external: Option<HashMap<String, Value>>,
}

impl Mirror {
    pub fn new(database: PathBuf, root: PathBuf) -> Self {
        Self {
            database,
            root,
            connection: None,
            sessions: HashMap::new(),
            polls: 0,
            last_error: None,
            external: None,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Run [`Mirror::sync`] every [`POLL`] until `stop` is cancelled.
    pub fn spawn(mut self, stop: tokio_util::sync::CancellationToken) -> io::Result<()> {
        std::thread::Builder::new()
            .name("sessiondock-opencode-mirror".into())
            .spawn(move || {
                while !stop.is_cancelled() {
                    match self.sync() {
                        Ok(()) => self.last_error = None,
                        Err(error) => {
                            let text = error.to_string();
                            if self.last_error.as_deref() != Some(&text) {
                                eprintln!("opencode mirror: {text}");
                                self.last_error = Some(text);
                            }
                            self.connection = None;
                        }
                    }
                    std::thread::sleep(POLL);
                }
            })
            .map(drop)
    }

    fn connect(&mut self) -> io::Result<Option<&Connection>> {
        if self.connection.is_none() {
            if !self.database.is_file() {
                return Ok(None);
            }
            let connection = Connection::open_with_flags(
                &self.database,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(sql)?;
            connection
                .busy_timeout(Duration::from_millis(1500))
                .map_err(sql)?;
            self.connection = Some(connection);
        }
        Ok(self.connection.as_ref())
    }

    /// One pass: mirror new and changed sessions, remove deleted ones.
    pub fn sync(&mut self) -> io::Result<()> {
        fs::create_dir_all(&self.root)?;
        self.polls = self.polls.wrapping_add(1);
        let full = self.polls % FULL_CHECK_EVERY == 1;
        let Some(connection) = self.connect()? else {
            // No database yet (OpenCode never ran): nothing to mirror.
            return self.retain(&HashSet::new(), true);
        };
        let has_v2: bool = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'session_v2'",
                [],
                |_| Ok(true),
            )
            .optional()
            .map_err(sql)?
            .unwrap_or(false);
        if !has_v2 {
            return self.retain(&HashSet::new(), true);
        }
        let projects = read_projects(connection)?;
        let rows = read_sessions(connection)?;
        let mut seen = HashSet::new();
        let mut work = Vec::new();
        for row in rows {
            let id = row.id.clone();
            if self
                .external
                .as_ref()
                .is_some_and(|sessions| !sessions.contains_key(&id))
            {
                continue;
            }
            if !safe_id(&id) || !safe_id(&row.project) {
                continue;
            }
            seen.insert(id.clone());
            let project = if self.external.is_some() {
                format!("discovered-{}", row.project)
            } else {
                row.project.clone()
            };
            let dir = self.root.join(project).join(&id);
            let summary = if let Some(external) = &self.external {
                let mut value: Value =
                    serde_json::from_str(&summary_json(&row, projects.get(&row.project)))?;
                value["database"] = json!(self.database);
                value["launch_parent"] = external[&id].clone();
                format!("{value}\n")
            } else {
                summary_json(&row, projects.get(&row.project))
            };
            let changed = match self.sessions.get(&id) {
                Some(known) => {
                    known.dir != dir
                        || known.summary != summary
                        || known.session_updated != row.updated
                        || known.exported.signature.is_none()
                        || full
                }
                None => true,
            };
            if changed {
                work.push((id, dir, summary, row.updated));
            }
        }
        for (id, dir, summary, updated) in work {
            self.mirror_session(&id, dir, summary, updated)?;
        }
        self.retain(&seen, full)
    }

    fn mirror_session(
        &mut self,
        id: &str,
        dir: PathBuf,
        summary: String,
        updated: i64,
    ) -> io::Result<()> {
        let mut known = self.sessions.remove(id).unwrap_or_else(|| Mirrored {
            dir: dir.clone(),
            summary: String::new(),
            session_updated: 0,
            exported: Exported::default(),
        });
        if known.dir != dir {
            // Moved to another project: the old directory is a stale copy.
            remove_session_dir(&self.root, &known.dir);
            known.dir = dir.clone();
            known.summary.clear();
            known.exported = Exported::default();
        }
        fs::create_dir_all(&dir)?;
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| io::Error::other("database closed"))?;
        sync_messages(connection, id, &dir, &mut known.exported)?;
        if known.summary != summary {
            write_if_changed(&dir.join(SUMMARY_FILE), summary.as_bytes())?;
            known.summary = summary;
        }
        known.session_updated = updated;
        self.sessions.insert(id.to_owned(), known);
        Ok(())
    }

    /// Forget and remove mirrored sessions that no longer exist; `sweep`
    /// also removes directories left by an earlier process.
    fn retain(&mut self, live: &HashSet<String>, sweep: bool) -> io::Result<()> {
        let gone: Vec<String> = self
            .sessions
            .keys()
            .filter(|id| !live.contains(*id))
            .cloned()
            .collect();
        for id in gone {
            if let Some(known) = self.sessions.remove(&id) {
                remove_session_dir(&self.root, &known.dir);
            }
        }
        // Supplemental mirrors share the index root, but never sweep another
        // database's directories. The default mirror reserves their namespace.
        if !sweep || self.external.is_some() {
            return Ok(());
        }
        let Ok(projects) = fs::read_dir(&self.root) else {
            return Ok(());
        };
        for project in projects.flatten() {
            if project
                .file_name()
                .to_string_lossy()
                .starts_with("discovered-")
            {
                continue;
            }
            let path = project.path();
            let Ok(entries) = fs::read_dir(&path) else {
                continue;
            };
            let mut empty = true;
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if live.contains(&name) || name.starts_with('.') {
                    empty = false;
                } else {
                    remove_session_dir(&self.root, &entry.path());
                }
            }
            if empty {
                let _ = fs::remove_dir(&path);
            }
        }
        Ok(())
    }
}

fn sql(error: rusqlite::Error) -> io::Error {
    io::Error::other(format!("OpenCode database: {error}"))
}

struct SessionRow {
    id: String,
    project: String,
    updated: i64,
    fields: Value,
}

fn read_projects(connection: &Connection) -> io::Result<HashMap<String, String>> {
    let mut statement = connection
        .prepare("SELECT id, worktree FROM project")
        .map_err(sql)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}

/// Columns copied into `summary.json`; JSON-valued columns are parsed.
const SESSION_COLUMNS: &[(&str, bool)] = &[
    ("id", false),
    ("project_id", false),
    ("parent_id", false),
    ("fork_session_id", false),
    ("fork_boundary", true),
    ("slug", false),
    ("directory", false),
    ("path", false),
    ("title", false),
    ("version", false),
    ("agent", false),
    ("model", true),
    ("revert", true),
    ("metadata", true),
    ("summary_additions", false),
    ("summary_deletions", false),
    ("summary_files", false),
    ("cost", false),
    ("tokens_input", false),
    ("tokens_output", false),
    ("tokens_reasoning", false),
    ("tokens_cache_read", false),
    ("tokens_cache_write", false),
    ("time_created", false),
    ("time_updated", false),
    ("time_idle", false),
    ("time_archived", false),
    ("time_compacting", false),
    ("idle_outcome", false),
];

fn read_sessions(connection: &Connection) -> io::Result<Vec<SessionRow>> {
    let available: HashSet<String> = connection
        .prepare("SELECT name FROM pragma_table_info('session_v2')")
        .map_err(sql)?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(sql)?
        .collect::<Result<_, _>>()
        .map_err(sql)?;
    let columns: Vec<(&str, bool)> = SESSION_COLUMNS
        .iter()
        .copied()
        .filter(|(name, _)| available.contains(*name))
        .collect();
    for required in ["id", "project_id", "time_updated"] {
        if !columns.iter().any(|(name, _)| *name == required) {
            return Err(io::Error::other(format!(
                "OpenCode session_v2 has no {required} column"
            )));
        }
    }
    let list = columns
        .iter()
        .map(|(name, _)| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = connection
        .prepare(&format!("SELECT {list} FROM session_v2"))
        .map_err(sql)?;
    let rows = statement
        .query_map([], |row| {
            let mut fields = serde_json::Map::new();
            for (index, (name, json_column)) in columns.iter().enumerate() {
                let value = match row.get_ref(index)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(number) => json!(number),
                    rusqlite::types::ValueRef::Real(number) => json!(number),
                    rusqlite::types::ValueRef::Text(text) => {
                        let text = String::from_utf8_lossy(text).into_owned();
                        if *json_column {
                            serde_json::from_str(&text).unwrap_or(Value::String(text))
                        } else {
                            Value::String(text)
                        }
                    }
                    rusqlite::types::ValueRef::Blob(_) => Value::Null,
                };
                fields.insert((*name).to_owned(), value);
            }
            Ok(fields)
        })
        .map_err(sql)?;
    let mut sessions = Vec::new();
    for fields in rows {
        let fields = fields.map_err(sql)?;
        let (Some(id), Some(project), Some(updated)) = (
            fields.get("id").and_then(Value::as_str).map(str::to_owned),
            fields
                .get("project_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            fields.get("time_updated").and_then(Value::as_i64),
        ) else {
            continue;
        };
        sessions.push(SessionRow {
            id,
            project,
            updated,
            fields: Value::Object(fields),
        });
    }
    Ok(sessions)
}

fn summary_json(row: &SessionRow, worktree: Option<&String>) -> String {
    let value = json!({
        "format": FORMAT,
        "version": 1,
        "session": row.fields,
        "project": {"id": row.project, "worktree": worktree},
    });
    let mut text = serde_json::to_string(&value).unwrap_or_default();
    text.push('\n');
    text
}

struct MessageMeta {
    id: String,
    kind: String,
    seq: i64,
    created: i64,
    updated: i64,
}

fn sync_messages(
    connection: &Connection,
    session: &str,
    dir: &Path,
    exported: &mut Exported,
) -> io::Result<()> {
    let signature: (i64, i64, i64) = connection
        .query_row(
            "SELECT count(*), coalesce(max(seq), -1), coalesce(max(time_updated), -1) \
             FROM session_message WHERE session_id = ?1",
            [session],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(sql)?;
    let path = dir.join(MESSAGES_FILE);
    if exported.signature == Some(signature) && path.is_file() {
        return Ok(());
    }
    let mut statement = connection
        .prepare(
            "SELECT id, type, seq, time_created, time_updated FROM session_message \
             WHERE session_id = ?1 ORDER BY seq, id",
        )
        .map_err(sql)?;
    let metas: Vec<MessageMeta> = statement
        .query_map([session], |row| {
            Ok(MessageMeta {
                id: row.get(0)?,
                kind: row.get(1)?,
                seq: row.get(2)?,
                created: row.get(3)?,
                updated: row.get(4)?,
            })
        })
        .map_err(sql)?
        .collect::<Result<_, _>>()
        .map_err(sql)?;
    // A first pass over an existing file (a restart) or an exported row that
    // changed or vanished: rebuild the whole file and keep it only if it
    // differs from what is on disk.
    let prefix_intact = exported.rows.len() <= metas.len()
        && exported
            .rows
            .iter()
            .zip(&metas)
            .all(|((id, updated), meta)| *id == meta.id && *updated == meta.updated);
    let rebuild = !exported.initialized || !prefix_intact || !path.is_file();
    let start = if rebuild { 0 } else { exported.rows.len() };
    let mut data_statement = connection
        .prepare("SELECT data FROM session_message WHERE id = ?1")
        .map_err(sql)?;
    let mut lines = Vec::new();
    let mut rows = Vec::new();
    for (index, meta) in metas.iter().enumerate().skip(start) {
        let data: String = data_statement
            .query_row([&meta.id], |row| row.get(0))
            .map_err(sql)?;
        let parsed: Value = match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(_) => break,
        };
        if !settled(&meta.kind, &parsed, &metas[index + 1..]) {
            break;
        }
        let mut line = json!({
            "id": meta.id,
            "type": meta.kind,
            "seq": meta.seq,
            "time_created": meta.created,
            "time_updated": meta.updated,
            "data": parsed,
        });
        if meta.kind == "user" {
            line["content"] = user_content(&mut line["data"]);
        }
        let mut text = serde_json::to_string(&line).map_err(io::Error::other)?;
        text.push('\n');
        lines.push(text);
        rows.push((meta.id.clone(), meta.updated));
    }
    if rebuild {
        write_if_changed(&path, lines.concat().as_bytes())?;
        exported.rows = rows;
    } else if !lines.is_empty() {
        let mut file = fs::OpenOptions::new().append(true).open(&path)?;
        file.write_all(lines.concat().as_bytes())?;
        exported.rows.extend(rows);
    }
    exported.initialized = true;
    // Keep polling while a row is still streaming.
    exported.signature = (exported.rows.len() == metas.len()).then_some(signature);
    Ok(())
}

/// The user's words and attachments as one content array in the shape the
/// session pipeline already renders: the text, then each inline image as a
/// base64 image block. The image bytes move out of `data.files`, which keeps
/// only the attachment's name and type.
fn user_content(data: &mut Value) -> Value {
    let mut content = Vec::new();
    if let Some(text) = data["text"].as_str()
        && !text.is_empty()
    {
        content.push(json!({"type": "text", "text": text}));
    }
    if let Some(files) = data["files"].as_array_mut() {
        for file in files {
            let mime = file["mime"].as_str().unwrap_or("").to_owned();
            let inline = file["source"]["type"] == "inline" || file["source"].is_null();
            if inline
                && mime.starts_with("image/")
                && let Some(bytes) = file.get_mut("data").map(Value::take)
                && bytes.is_string()
            {
                content.push(json!({"type": "image", "source": {
                    "type": "base64", "media_type": mime, "data": bytes}}));
                if let Some(object) = file.as_object_mut() {
                    object.remove("data");
                }
            } else if let Some(name) = file["name"].as_str().filter(|name| !name.is_empty()) {
                content.push(json!({"type": "text", "text": format!("[附件] {name}")}));
            }
        }
    }
    Value::Array(content)
}

/// A row is final once written, except an assistant turn that is still
/// streaming: it has no `time.completed` and nothing after it yet.
fn settled(kind: &str, data: &Value, later: &[MessageMeta]) -> bool {
    kind != "assistant"
        || data["time"]["completed"].is_number()
        || data["finish"].is_string()
        || later.iter().any(|meta| meta.kind != "assistant")
}

/// Write through a temporary file and rename, unless the bytes are already
/// on disk (a restart re-derives identical files without touching stamps).
fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    let temporary = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temporary, path)
}

fn remove_session_dir(root: &Path, dir: &Path) {
    if dir.starts_with(root) && dir != root {
        let _ = fs::remove_dir_all(dir);
        if let Some(parent) = dir.parent()
            && parent != root
        {
            let _ = fs::remove_dir(parent);
        }
    }
}
