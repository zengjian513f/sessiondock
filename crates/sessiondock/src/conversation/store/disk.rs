//! Durable per-record updates. The legacy JSON is imported once, never rewritten.
use super::{Document, Failure, Result, io};
use rusqlite::{Connection, Transaction, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::BTreeMap, fs, path::Path, time::Duration};

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(
        503,
        "conversation_storage",
        format!("会话保存失败：{error}"),
    )
}
fn load<T: DeserializeOwned>(db: &Connection, section: &str) -> Result<BTreeMap<String, T>> {
    let mut statement = db
        .prepare("SELECT key,data FROM records WHERE section=?1")
        .map_err(failure)?;
    let mut values = BTreeMap::new();
    for row in statement
        .query_map([section], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(failure)?
    {
        let (key, data) = row.map_err(failure)?;
        values.insert(key, serde_json::from_str(&data).map_err(failure)?);
    }
    Ok(values)
}
pub(super) fn open(directory: &Path) -> Result<(Connection, Document)> {
    let path = directory.join("conversation-ledger.sqlite3");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(io)?;
    }
    drop(file);
    let mut db = Connection::open(&path).map_err(failure)?;
    db.busy_timeout(Duration::from_secs(5)).map_err(failure)?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
      CREATE TABLE IF NOT EXISTS records(section TEXT NOT NULL,key TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(section,key)) WITHOUT ROWID;
      CREATE TABLE IF NOT EXISTS migration(version INTEGER PRIMARY KEY);").map_err(failure)?;
    let migrated: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM migration WHERE version=1)",
            [],
            |r| r.get(0),
        )
        .map_err(failure)?;
    let state = if migrated {
        Document {
            drafts: load(&db, "drafts")?,
            requests: load(&db, "requests")?,
            uploads: load(&db, "uploads")?,
            legacy: load(&db, "legacy")?,
            aliases: load(&db, "aliases")?,
            reports: load(&db, "reports")?,
            queued: load(&db, "queued")?,
            merged_echoes: load(&db, "merged_echoes")?,
        }
    } else {
        let legacy = directory.join("conversation-ledger.json");
        let state = match fs::read(legacy) {
            Ok(bytes) => serde_json::from_slice::<Document>(&bytes).map_err(|_| {
                Failure::new(503, "conversation_storage", "会话记录无法读取，保留原文件")
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Document::default(),
            Err(e) => return Err(io(e)),
        };
        let tx = db.transaction().map_err(failure)?;
        records(&tx, &Document::default(), &state)?;
        tx.execute("INSERT INTO migration VALUES(1)", [])
            .map_err(failure)?;
        tx.commit().map_err(failure)?;
        state
    };
    Ok((db, state))
}
fn changes<T: PartialEq + Serialize>(
    tx: &Transaction<'_>,
    section: &str,
    before: &BTreeMap<String, T>,
    after: &BTreeMap<String, T>,
) -> Result<()> {
    let mut put=tx.prepare_cached("INSERT INTO records VALUES(?1,?2,?3) ON CONFLICT(section,key) DO UPDATE SET data=excluded.data").map_err(failure)?;
    for (key, value) in after {
        if before.get(key) == Some(value) {
            continue;
        }
        put.execute(params![
            section,
            key,
            serde_json::to_string(value).map_err(failure)?
        ])
        .map_err(failure)?;
    }
    let mut delete = tx
        .prepare_cached("DELETE FROM records WHERE section=?1 AND key=?2")
        .map_err(failure)?;
    for key in before.keys().filter(|key| !after.contains_key(*key)) {
        delete.execute(params![section, key]).map_err(failure)?;
    }
    Ok(())
}
fn records(tx: &Transaction<'_>, before: &Document, after: &Document) -> Result<()> {
    changes(tx, "drafts", &before.drafts, &after.drafts)?;
    changes(tx, "requests", &before.requests, &after.requests)?;
    changes(tx, "uploads", &before.uploads, &after.uploads)?;
    changes(tx, "legacy", &before.legacy, &after.legacy)?;
    changes(tx, "aliases", &before.aliases, &after.aliases)?;
    changes(tx, "reports", &before.reports, &after.reports)?;
    changes(tx, "queued", &before.queued, &after.queued)?;
    changes(
        tx,
        "merged_echoes",
        &before.merged_echoes,
        &after.merged_echoes,
    )
}
pub(super) fn persist(db: &mut Connection, before: &Document, after: &Document) -> Result<()> {
    let tx = db.transaction().map_err(failure)?;
    records(&tx, before, after)?;
    tx.commit().map_err(failure)?;
    Ok(())
}
