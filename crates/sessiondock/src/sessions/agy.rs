//! Read-only Agy catalog and complete transcript projection. The binary
//! conversation databases remain native; this mirror is never a resume copy.
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};

pub const FORMAT: &str = "sessiondock-agy-mirror";
pub const SUMMARY_FILE: &str = "summary.json";
pub const MESSAGES_FILE: &str = "messages.jsonl";
pub const TRANSCRIPT_UNAVAILABLE: &str =
    "Agy 完整记录暂时不可读取；保留上次读取的历史，等待原生文件恢复";

/// Strip only Agy's outer user envelope, preserving the user's own text.
pub(crate) fn user_text(text: &str) -> String {
    text.strip_prefix("<USER_REQUEST>\n")
        .and_then(|body| body.rsplit_once("\n</USER_REQUEST>"))
        .filter(|(_, tail)| tail.is_empty() || tail.starts_with("\n<ADDITIONAL_METADATA>"))
        .map_or_else(|| text.to_owned(), |(body, _)| body.to_owned())
}

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: Option<std::time::SystemTime>,
    identity: String,
}
fn stamp(path: &Path) -> io::Result<Stamp> {
    let metadata = fs::metadata(path)?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = format!("{:?}", metadata.created().ok());
    Ok(Stamp {
        size: metadata.len(),
        modified: metadata.modified().ok(),
        identity,
    })
}

pub struct Mirror {
    home: PathBuf,
    root: PathBuf,
    connection: Option<Connection>,
    identity: Option<String>,
    version: Option<i64>,
    sessions: BTreeMap<String, Value>,
    exported: BTreeMap<String, Option<Stamp>>,
}

impl Mirror {
    pub fn new(home: PathBuf, root: PathBuf) -> Self {
        Self {
            home,
            root,
            connection: None,
            identity: None,
            version: None,
            sessions: BTreeMap::new(),
            exported: BTreeMap::new(),
        }
    }

    pub fn spawn(mut self, stop: tokio_util::sync::CancellationToken) -> io::Result<()> {
        std::thread::Builder::new()
            .name("sessiondock-agy-mirror".into())
            .spawn(move || {
                let mut last_error = None;
                while !stop.is_cancelled() {
                    match self.sync() {
                        Ok(()) => last_error = None,
                        Err(error) => {
                            let message = error.to_string();
                            if last_error.as_ref() != Some(&message) {
                                eprintln!("agy mirror: {message}");
                            }
                            last_error = Some(message);
                            self.connection = None;
                            self.version = None;
                        }
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            })
            .map(drop)
    }

    pub fn sync(&mut self) -> io::Result<()> {
        let database = self.home.join("conversation_summaries.db");
        let current = match stamp(&database) {
            Ok(current) => current,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.connection = None;
                self.version = None;
                self.identity = None;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if self.identity.as_ref() != Some(&current.identity) || self.connection.is_none() {
            self.connection = Some(
                Connection::open_with_flags(
                    &database,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )
                .map_err(db_error)?,
            );
            self.connection
                .as_ref()
                .unwrap()
                .busy_timeout(Duration::from_secs(1))
                .map_err(db_error)?;
            self.identity = Some(current.identity);
            self.version = None;
            self.exported.clear();
        }
        let connection = self.connection.as_mut().unwrap();
        // Sample before the snapshot, acknowledge only after every projection succeeds.
        let version: i64 = connection
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .map_err(db_error)?;
        let changed = self.version != Some(version);
        let sessions = if changed {
            let transaction = connection.transaction().map_err(db_error)?;
            let mut statement = transaction.prepare("SELECT conversation_id, title, last_modified_time, workspace_uris, parent_conversation_id, status, step_count, agent_name FROM conversation_summaries").map_err(db_error)?;
            let rows = statement
                .query_map([], |row| {
                    let id: String = row.get(0)?;
                    let workspaces: String = row.get(3)?;
                    let workspaces: Value =
                        serde_json::from_str(&workspaces).unwrap_or(Value::Null);
                    let cwd = workspaces
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .find_map(file_uri)
                        .unwrap_or_default();
                    let updated: String = row.get(2)?;
                    Ok((
                        id.clone(),
                        json!({"id":id, "title":row.get::<_,String>(1)?,
                    "directory":cwd, "time_updated": timestamp(&updated),
                    "parent_id":row.get::<_,String>(4)?, "status":row.get::<_,String>(5)?,
                    "step_count":row.get::<_,i64>(6)?, "agent":row.get::<_,String>(7)?}),
                    ))
                })
                .map_err(db_error)?;
            let mut found = BTreeMap::new();
            for row in rows {
                let (id, value) = row.map_err(db_error)?;
                // Catalog IDs become one path component, never a native path supplied by a row.
                let mut components = Path::new(&id).components();
                if matches!(components.next(), Some(Component::Normal(_)))
                    && components.next().is_none()
                {
                    found.insert(id, value);
                }
            }
            drop(statement);
            transaction.commit().map_err(db_error)?;
            found
        } else {
            self.sessions.clone()
        };
        for (sid, session) in &sessions {
            self.export(sid, session)?;
        }
        if changed {
            // Only remove private projections marked as ours, including stale entries after restart.
            let known: BTreeSet<_> = sessions.keys().cloned().collect();
            let folder = self.root.join("cli");
            if folder.is_dir() {
                for entry in fs::read_dir(&folder)? {
                    let entry = entry?;
                    if !entry.file_type()?.is_dir() {
                        continue;
                    }
                    let sid = entry.file_name().to_string_lossy().into_owned();
                    if !known.contains(&sid) {
                        let marker = fs::read(entry.path().join(SUMMARY_FILE))
                            .ok()
                            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
                        if marker
                            .as_ref()
                            .is_some_and(|value| value["format"] == FORMAT)
                        {
                            fs::remove_dir_all(entry.path())?;
                            self.exported.remove(&sid);
                        }
                    }
                }
            }
            self.sessions = sessions;
            self.version = Some(version);
        }
        Ok(())
    }

    fn export(&mut self, sid: &str, session: &Value) -> io::Result<()> {
        let directory = self.root.join("cli").join(sid);
        let native = self
            .home
            .join("brain")
            .join(sid)
            .join(".system_generated/logs/transcript_full.jsonl");
        let current = match stamp(&native) {
            Ok(stamp) => Some(stamp),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        private_dir(&directory)?;
        let messages = directory.join(MESSAGES_FILE);
        let summary = directory.join(SUMMARY_FILE);
        let mut value = session.clone();
        let transcript_missing = current.is_none();
        let previous = fs::read(&summary)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        value["time_created"] = previous
            .as_ref()
            .map(|v| v["session"]["time_created"].clone())
            .unwrap_or(Value::Null);
        if (current.is_some() && self.exported.get(sid) != Some(&current)) || !messages.is_file() {
            let bytes = if current.is_some() {
                fs::read(&native)?
            } else {
                Vec::new()
            };
            // Never acknowledge a concurrently replaced or appended transcript as this version.
            if current.is_some() && stamp(&native).ok() != current {
                return Ok(());
            }
            let mut complete = 0;
            for line in bytes.split_inclusive(|byte| *byte == b'\n') {
                if !line.ends_with(b"\n") {
                    break;
                }
                let row = serde_json::from_slice::<Value>(line).ok();
                if complete == 0
                    && let Some(created) = row.as_ref().and_then(|v| v.get("created_at"))
                {
                    value["time_created"] = created.clone();
                }
                complete += line.len();
            }
            publish(&messages, &bytes[..complete], true)?;
            self.exported.insert(sid.to_owned(), current);
        }
        if value["time_created"].is_null() {
            value["time_created"] = value["time_updated"].clone();
        }
        let bytes = serde_json::to_vec(&json!({"format":FORMAT,"session":value,
            "transcript_missing":transcript_missing,
            "native_database":self.home.join("conversations").join(format!("{sid}.db")),
            "native_transcript":native}))
        .map_err(io::Error::other)?;
        publish(&summary, &bytes, false)
    }
}

fn db_error(error: rusqlite::Error) -> io::Error {
    io::Error::other(error)
}
fn timestamp(text: &str) -> String {
    chrono::DateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f%:z")
        .map(|time| time.to_rfc3339())
        .unwrap_or_else(|_| text.to_owned())
}
fn private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}
fn publish(path: &Path, bytes: &[u8], append: bool) -> io::Result<()> {
    let previous = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    if previous == bytes && path.is_file() {
        return Ok(());
    }
    if append && path.is_file() && bytes.starts_with(&previous) {
        fs::OpenOptions::new()
            .append(true)
            .open(path)?
            .write_all(&bytes[previous.len()..])?;
        return Ok(());
    }
    let temporary = path.with_extension("tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    drop(file);
    fs::rename(&temporary, path)
}

pub(crate) fn file_uri(uri: &str) -> Option<String> {
    let path = uri
        .strip_prefix("file://localhost/")
        .map(|path| format!("/{path}"))
        .or_else(|| uri.strip_prefix("file:///").map(|path| format!("/{path}")))?;
    let path = super::index::summary::unquote(&path);
    #[cfg(windows)]
    let path = if path.as_bytes().get(2) == Some(&b':') {
        path[1..].to_owned()
    } else {
        path
    };
    Some(path)
}
