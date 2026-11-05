//! Incremental durable attribution state; old JSON is imported once and retained.
use process_links::engine::Saved;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, BufReader},
    path::Path,
    time::Duration,
};

pub struct Store {
    db: Connection,
    previous: Saved,
    meta: Value,
}
fn error(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
fn metadata(s: &Saved) -> Value {
    json!({"node_id":s.node_id,"boot_id":s.boot_id,"catalog":s.catalog,
        "links":s.links,"bindings":s.bindings,"forked_from":s.forked_from})
}
// Compare typed records, serializing only changed records. No whole-history JSON rewrite.
fn changes<T: PartialEq + Serialize, K: Ord + Serialize>(
    tx: &Transaction<'_>,
    scope: &str,
    section: &str,
    old: &[T],
    new: &[T],
    key: impl Fn(&T) -> K,
) -> io::Result<()> {
    let old: BTreeMap<K, &T> = old.iter().map(|v| (key(v), v)).collect();
    let mut upsert=tx.prepare_cached("INSERT INTO evidence VALUES(?1,?2,?3,?4) ON CONFLICT(scope,section,key) DO UPDATE SET data=excluded.data").map_err(error)?;
    for v in new {
        let k = key(v);
        if old.get(&k).is_some_and(|before| *before == v) {
            continue;
        }
        upsert
            .execute(params![
                scope,
                section,
                serde_json::to_string(&k).map_err(error)?,
                serde_json::to_string(v).map_err(error)?
            ])
            .map_err(error)?;
    }
    // Retain evidence removed from the live working set as historical records.
    Ok(())
}
impl Store {
    pub fn open(path: &Path, node: &str, boot: &str) -> io::Result<(Self, Option<Saved>)> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let database = path.with_extension("sqlite3");
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&database)?;
        fs::set_permissions(&database, fs::Permissions::from_mode(0o600))?;
        let db = Connection::open(database).map_err(error)?;
        db.busy_timeout(Duration::from_secs(30)).map_err(error)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS snapshots(scope TEXT PRIMARY KEY,data TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS evidence(scope TEXT,section TEXT,key TEXT,data TEXT NOT NULL,PRIMARY KEY(scope,section,key)) WITHOUT ROWID;").map_err(error)?;
        let scope = serde_json::to_string(&(node, boot)).map_err(error)?;
        let raw: Option<String> = db
            .query_row("SELECT data FROM snapshots WHERE scope=?1", [&scope], |r| {
                r.get(0)
            })
            .optional()
            .map_err(error)?;
        let restored = raw.is_some();
        let saved = if let Some(raw) = raw {
            let mut value: Value = serde_json::from_str(&raw).map_err(error)?;
            let mut connections = json!({"records":[],"parents":[],"origins":[],"shared":[]});
            let mut st = db
                .prepare("SELECT section,data FROM evidence WHERE scope=?1")
                .map_err(error)?;
            for row in st
                .query_map([&scope], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(error)?
            {
                let (section, data) = row.map_err(error)?;
                connections[&section]
                    .as_array_mut()
                    .ok_or_else(|| error("unknown evidence section"))?
                    .push(serde_json::from_str(&data).map_err(error)?);
            }
            value["connections"] = connections;
            Some(serde_json::from_value::<Saved>(value).map_err(error)?)
        } else if db
            .query_row("SELECT EXISTS(SELECT 1 FROM snapshots)", [], |r| {
                r.get::<_, bool>(0)
            })
            .map_err(error)?
        {
            // A new boot starts fresh live attribution. Never replay an older
            // JSON backup over evidence already migrated and updated in SQLite.
            None
        } else {
            match fs::File::open(path) {
                Ok(file) => {
                    Some(serde_json::from_reader::<_, Saved>(BufReader::new(file)).map_err(error)?)
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            }
        };
        let mut store = Self {
            db,
            previous: Saved::default(),
            meta: Value::Null,
        };
        // Import the complete old state, even when it belongs to an older boot.
        if let Some(s) = &saved {
            if restored {
                store.meta = metadata(s);
                store.previous = s.clone();
            } else {
                store.save(s.clone())?;
            }
        }
        Ok((store, saved))
    }
    pub fn save(&mut self, saved: Saved) -> io::Result<()> {
        let scope = serde_json::to_string(&(&saved.node_id, &saved.boot_id)).map_err(error)?;
        let empty = Saved::default();
        let old =
            if saved.node_id == self.previous.node_id && saved.boot_id == self.previous.boot_id {
                &self.previous
            } else {
                &empty
            };
        let meta = metadata(&saved);
        let tx = self.db.transaction().map_err(error)?;
        if self.meta != meta {
            tx.execute("INSERT INTO snapshots VALUES(?1,?2) ON CONFLICT(scope) DO UPDATE SET data=excluded.data",params![scope,meta.to_string()]).map_err(error)?;
        }
        changes(
            &tx,
            &scope,
            "records",
            &old.connections.records,
            &saved.connections.records,
            |v| {
                (
                    v.process.clone(),
                    v.connection.clone(),
                    v.opened_at.to_bits(),
                )
            },
        )?;
        changes(
            &tx,
            &scope,
            "parents",
            &old.connections.parents,
            &saved.connections.parents,
            |v| v.0.clone(),
        )?;
        changes(
            &tx,
            &scope,
            "origins",
            &old.connections.origins,
            &saved.connections.origins,
            |v| v.0.clone(),
        )?;
        changes(
            &tx,
            &scope,
            "shared",
            &old.connections.shared,
            &saved.connections.shared,
            |v| v.clone(),
        )?;
        tx.commit().map_err(error)?;
        self.previous = saved;
        self.meta = meta;
        Ok(())
    }
}
