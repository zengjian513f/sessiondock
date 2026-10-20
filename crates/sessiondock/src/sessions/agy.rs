//! Read-only Agy catalog and complete transcript projection. The binary
//! conversation databases remain native; this mirror is never a resume copy.
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};

pub const FORMAT: &str = "sessiondock-agy-mirror";
pub const SUMMARY_FILE: &str = "summary.json";
pub const MESSAGES_FILE: &str = "messages.jsonl";
/// Fixed buffer for comparing an existing mirror file with the new snapshot.
/// This is not a file-size limit; comparison reads through EOF.
const COMPARE_CHUNK: usize = 8 * 1024;
pub fn transcript_warning(summary: &Value, has_history: bool) -> Option<&'static str> {
    if !has_history && summary["session"]["step_count"].as_u64() == Some(0) {
        return None;
    }
    (summary["transcript_missing"] == true).then_some(if has_history {
        "未找到 Agy 的完整正文文件；保留上次读取的历史。正文文件可读取后会自动更新。"
    } else {
        "仅找到 Agy 的会话索引，未找到完整正文文件，也没有已缓存的历史，因此暂时无法显示消息。请在 Agy 中确认该会话是否仍可打开；正文文件可读取后会自动显示。"
    })
}

/// Strip only Agy's outer user envelope, preserving the user's own text.
pub(crate) fn user_text(text: &str) -> String {
    user_envelope(text).map_or_else(|| text.to_owned(), |(body, _)| body.to_owned())
}

/// The native body and its trailing injections, never tags inside user text.
pub(super) fn user_envelope(text: &str) -> Option<(&str, &str)> {
    text.strip_prefix("<USER_REQUEST>\n")
        .and_then(|body| body.rsplit_once("\n</USER_REQUEST>"))
        .filter(|(_, tail)| {
            tail.is_empty()
                || tail.starts_with("\n<ADDITIONAL_METADATA>")
                || tail.starts_with("\n<USER_SETTINGS_CHANGE>")
        })
}

/// The CLI's model notice outside the user's request, never quoted/tool text.
fn user_model(text: &str) -> Option<String> {
    let (_, tail) = user_envelope(text)?;
    super::providers::envelopes::fields(tail)?
        .into_iter()
        .filter(|(tag, _)| *tag == "USER_SETTINGS_CHANGE")
        .filter_map(|(_, body)| {
            let change = body
                .trim()
                .strip_prefix("The user changed setting `Model Selection` from ")?;
            let (_, next) = change.split_once(" to ")?;
            let (model, _) = next.split_once(". No need to comment on this change")?;
            (!model.is_empty() && model != "None").then(|| model.to_owned())
        })
        .next_back()
}

pub(crate) fn record_model(record: &Value) -> Option<String> {
    (record["type"] == "USER_INPUT" && record["status"] == "DONE")
        .then(|| user_model(record["content"].as_str()?))
        .flatten()
}

/// Latest complete native model notice; a partial line is not yet evidence.
pub(crate) fn transcript_model(bytes: &[u8]) -> Option<String> {
    bytes
        .split_inclusive(|byte| *byte == b'\n')
        .rev()
        .filter(|line| line.ends_with(b"\n"))
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .find_map(|record| record_model(&record))
}

/// Agy's exact system injection envelope; ordinary user/code tags are never
/// passed here. Unrecognized or incomplete layouts stay verbatim.
pub(crate) fn system_message(text: &str) -> (String, Value) {
    const INTRO: &str = "The following is a <SYSTEM_MESSAGE> not actually sent by the user. It is provided by the system as important information to pay attention to.\n\n";
    let wrapped = text.strip_prefix(INTRO).unwrap_or(text);
    let body = super::providers::envelopes::whole(wrapped, "SYSTEM_MESSAGE")
        .map(str::trim)
        .unwrap_or(text);
    let header = body
        .strip_prefix("[Message] timestamp=")
        .and_then(|rest| rest.split_once(" sender="))
        .and_then(|(timestamp, rest)| {
            rest.split_once(" priority=")
                .map(|(sender, rest)| (timestamp, sender, rest))
        })
        .and_then(|(timestamp, sender, rest)| {
            rest.split_once(" content=")
                .map(|(priority, content)| (timestamp, sender, priority, content))
        });
    match header {
        Some((timestamp, sender, priority, content)) => (
            content.to_owned(),
            json!({
                "native_type":"SYSTEM_MESSAGE", "counted":false, "system_timestamp":timestamp,
                "system_sender":sender, "system_priority":priority,
            }),
        ),
        None => (
            body.to_owned(),
            json!({"native_type":"SYSTEM_MESSAGE", "counted":false}),
        ),
    }
}

#[cfg(unix)]
type FileIdentity = (u64, u64);
#[cfg(not(unix))]
type FileIdentity = String;

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: Option<std::time::SystemTime>,
    identity: FileIdentity,
}
fn stamp(path: &Path) -> io::Result<Stamp> {
    let metadata = fs::metadata(path)?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = format!("{:?}", metadata.created().ok());
    Ok(Stamp {
        size: metadata.len(),
        modified: metadata.modified().ok(),
        identity,
    })
}

/// A successful native deletion remains authoritative when a stale native
/// summary reappears. These receipts never change native storage.
pub(crate) fn record_deleted(root: &Path, sid: &str) -> io::Result<()> {
    let directory = root.join("deleted");
    private_dir(&directory)?;
    publish(&directory.join(sid), sid.as_bytes(), false)
}

fn deleted_receipt(home: &Path, root: &Path, sid: &str) -> io::Result<bool> {
    let receipt = root.join("deleted").join(sid);
    match fs::read(&receipt) {
        Ok(bytes) if bytes == sid.as_bytes() => (),
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    }
    // A genuinely restored native session takes precedence over an old receipt.
    if home
        .join("conversations")
        .join(format!("{sid}.db"))
        .try_exists()?
        || home.join("brain").join(sid).try_exists()?
    {
        fs::remove_file(receipt)?;
        return Ok(false);
    }
    Ok(true)
}

pub struct Mirror {
    home: PathBuf,
    root: PathBuf,
    connection: Option<Connection>,
    identity: Option<FileIdentity>,
    version: Option<i64>,
    sessions: BTreeMap<String, Value>,
    exported: BTreeMap<String, (Value, Option<Stamp>)>,
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
                                crate::log::warn(
                                    "agy.mirror_failed",
                                    serde_json::json!({"error": message}),
                                );
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
        let fresh = if changed {
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
            Some(found)
        } else {
            None
        };
        let sessions = fresh.as_ref().unwrap_or(&self.sessions);
        let mut known = BTreeSet::new();
        for (sid, session) in sessions {
            if !deleted_receipt(&self.home, &self.root, sid)? {
                Self::export(&self.home, &self.root, &mut self.exported, sid, session)?;
                known.insert(sid.clone());
            }
        }
        if changed || known.len() != sessions.len() {
            // Only remove private projections marked as ours, including stale entries after restart.
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
            if let Some(fresh) = fresh {
                self.sessions = fresh;
            }
            self.version = Some(version);
        }
        Ok(())
    }

    fn export(
        home: &Path,
        root: &Path,
        exported: &mut BTreeMap<String, (Value, Option<Stamp>)>,
        sid: &str,
        session: &Value,
    ) -> io::Result<()> {
        let directory = root.join("cli").join(sid);
        let native = home
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
        let cached = exported.get(sid);
        if cached.is_some_and(|(last_session, last_stamp)| {
            last_session == session && *last_stamp == current
        }) && messages.is_file()
            && summary.is_file()
        {
            return Ok(());
        }
        let mut value = session.clone();
        let transcript_missing = current.is_none();
        let previous_bytes = fs::read(&summary).ok();
        let previous = previous_bytes
            .as_deref()
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
        // Agy init writes Go's zero time until the first input. It is not
        // conversation activity. Keep a stable native-file time in the private
        // projection until the catalog supplies a real activity timestamp.
        if zero_time(&value["time_updated"]) {
            if let Some(updated) = previous
                .as_ref()
                .map(|v| &v["session"]["time_updated"])
                .filter(|v| v.is_string() && !zero_time(v))
            {
                value["time_updated"] = updated.clone();
            } else if let Ok(modified) =
                fs::metadata(home.join("conversations").join(format!("{sid}.db")))
                    .and_then(|meta| meta.modified())
            {
                value["time_updated"] = chrono::DateTime::<chrono::Utc>::from(modified)
                    .to_rfc3339()
                    .into();
            }
        }
        value["time_created"] = previous
            .as_ref()
            .map(|v| v["session"]["time_created"].clone())
            .unwrap_or(Value::Null);
        value["model"] = previous
            .as_ref()
            .map(|v| v["session"]["model"].clone())
            .unwrap_or(Value::Null);
        let transcript_changed = cached.is_none_or(|(_, last_stamp)| *last_stamp != current);
        if (current.is_some() && transcript_changed) || !messages.is_file() {
            let bytes = if current.is_some() {
                fs::read(&native)?
            } else {
                Vec::new()
            };
            // Never acknowledge a concurrently replaced or appended transcript as this version.
            if current.is_some() && stamp(&native).ok() != current {
                return Ok(());
            }
            // created_at comes only from the first complete line. Empty, invalid
            // and later lines do not supply it. The exported prefix stops at the
            // last LF, so a trailing partial line stays out of the mirror.
            let mut complete = 0;
            if let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
                let line = &bytes[..=end];
                let row = serde_json::from_slice::<Value>(line).ok();
                if let Some(created) = row.as_ref().and_then(|v| v.get("created_at")) {
                    value["time_created"] = created.clone();
                }
                complete = end + 1;
                if let Some(last) = bytes[complete..].iter().rposition(|byte| *byte == b'\n') {
                    complete += last + 1;
                }
            }
            value["model"] = transcript_model(&bytes[..complete])
                .map(|name| json!({"id":name}))
                .unwrap_or(Value::Null);
            publish(&messages, &bytes[..complete], true)?;
        }
        if value["time_created"].is_null() || zero_time(&value["time_created"]) {
            value["time_created"] = value["time_updated"].clone();
        }
        let bytes = serde_json::to_vec(&json!({"format":FORMAT,"session":value,
            "transcript_missing":transcript_missing,
            "native_database":home.join("conversations").join(format!("{sid}.db")),
            "native_transcript":native}))
        .map_err(io::Error::other)?;
        if previous_bytes.as_deref() != Some(bytes.as_slice()) || !summary.is_file() {
            write_atomic(&summary, &bytes)?;
        }
        exported.insert(sid.to_owned(), (session.clone(), current));
        Ok(())
    }
}

fn db_error(error: rusqlite::Error) -> io::Error {
    io::Error::other(error)
}
fn zero_time(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|time| time.starts_with("0001-01-01T00:00:00"))
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
struct Existing {
    /// Matched byte count. This is the whole previous file only when `prefix` is set.
    length: usize,
    /// Every byte of the previous file occurs at the start of `bytes`.
    prefix: bool,
}

/// Read `path` through a fixed buffer and compare it with `bytes` one chunk at a time.
/// The previous file is not retained. A read continues to EOF so a late IO error
/// still fails the publish. A missing file is not handled here.
fn compare_existing(path: &Path, bytes: &[u8]) -> io::Result<Existing> {
    let mut file = fs::File::open(path)?;
    let mut buffer = [0u8; COMPARE_CHUNK];
    let mut length = 0usize;
    let mut prefix = true;
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if !prefix {
            continue;
        }
        if bytes[length..].starts_with(&buffer[..read]) {
            length += read;
        } else {
            prefix = false;
        }
    }
    Ok(Existing { length, prefix })
}

fn publish(path: &Path, bytes: &[u8], append: bool) -> io::Result<()> {
    // Identical bytes keep the inode and mtime. Append mode writes only the
    // suffix when the old file is a byte-for-byte prefix. Same-length edits,
    // rollbacks and a missing file replace the path atomically.
    let existing = match compare_existing(path, bytes) {
        Ok(existing) => existing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Existing {
            length: 0,
            prefix: true,
        },
        Err(error) => return Err(error),
    };
    if existing.prefix && existing.length == bytes.len() && path.is_file() {
        return Ok(());
    }
    if append && path.is_file() && existing.prefix {
        fs::OpenOptions::new()
            .append(true)
            .open(path)?
            .write_all(&bytes[existing.length..])?;
        return Ok(());
    }
    write_atomic(path, bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
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
