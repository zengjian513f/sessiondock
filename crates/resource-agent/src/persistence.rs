//! Incremental durable attribution state; old JSON is imported once and retained.
use process_links::engine::Saved;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufReader},
    path::Path,
    time::Duration,
};

pub struct Store {
    db: Connection,
    previous: Saved,
    meta: Value,
    reconcile: bool,
}
fn error(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
fn metadata(s: &Saved) -> Value {
    json!({"node_id":s.node_id,"boot_id":s.boot_id,"catalog":s.catalog,
        "links":s.links,"bindings":s.bindings,"forked_from":s.forked_from,
        "last_seen":s.connections.last_seen})
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
    let current: BTreeSet<_> = new.iter().map(key).collect();
    let mut delete = tx
        .prepare_cached("DELETE FROM evidence WHERE scope=?1 AND section=?2 AND key=?3")
        .map_err(error)?;
    for k in old.keys().filter(|k| !current.contains(*k)) {
        delete
            .execute(params![
                scope,
                section,
                serde_json::to_string(k).map_err(error)?
            ])
            .map_err(error)?;
    }
    Ok(())
}
impl Store {
    pub fn open(
        path: &Path,
        node: &str,
        boot: &str,
        snapshot: &process_links::linux::Snapshot,
        at: f64,
        retention: f64,
    ) -> io::Result<(Self, Option<Saved>)> {
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
            let mut saved: Saved = serde_json::from_str(&raw).map_err(error)?;
            let meta: Value = serde_json::from_str(&raw).map_err(error)?;
            saved.connections.last_seen =
                serde_json::from_value(meta.get("last_seen").cloned().unwrap_or_else(|| json!([])))
                    .map_err(error)?;
            let live = |p: &process_links::Process| {
                snapshot
                    .entries
                    .get(&p.pid)
                    .is_some_and(|e| e.process == *p)
            };
            let cutoff = at - retention - 2.0;
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
                match section.as_str() {
                    "records" => {
                        let record: process_links::connections::ConnectionRecord =
                            serde_json::from_str(&data).map_err(error)?;
                        if live(&record.process) || record.closed_at.is_none_or(|end| end >= cutoff)
                        {
                            saved.connections.records.push(record);
                        }
                    }
                    "parents" => saved
                        .connections
                        .parents
                        .push(serde_json::from_str(&data).map_err(error)?),
                    "origins" => saved
                        .connections
                        .origins
                        .push(serde_json::from_str(&data).map_err(error)?),
                    "shared" => saved
                        .connections
                        .shared
                        .push(serde_json::from_str(&data).map_err(error)?),
                    _ => return Err(error("unknown evidence section")),
                }
            }
            // Streaming recovery avoids materializing expired history as a JSON
            // tree. Retain the first newer connect for each unfinished span so
            // pruning cannot remove its superseded_at boundary.
            let mut unfinished: BTreeMap<process_links::Connection, Vec<f64>> = BTreeMap::new();
            for r in &saved.connections.records {
                if r.closed_at.is_none() {
                    unfinished
                        .entry(r.connection.clone())
                        .or_default()
                        .push(r.opened_at);
                }
            }
            if !unfinished.is_empty() {
                let mut witnesses = BTreeMap::new();
                for row in st
                    .query_map([&scope], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })
                    .map_err(error)?
                {
                    let (section, data) = row.map_err(error)?;
                    if section != "records" {
                        continue;
                    }
                    let record: process_links::connections::ConnectionRecord =
                        serde_json::from_str(&data).map_err(error)?;
                    if let Some(starts) = unfinished.get(&record.connection) {
                        for start in starts.iter().filter(|start| record.opened_at > **start) {
                            let key = (record.connection.clone(), start.to_bits());
                            let old = witnesses.entry(key).or_insert_with(|| record.clone());
                            if record.opened_at < old.opened_at {
                                *old = record.clone();
                            }
                        }
                    }
                }
                let existing: BTreeSet<_> = saved
                    .connections
                    .records
                    .iter()
                    .map(|r| {
                        (
                            r.process.clone(),
                            r.connection.clone(),
                            r.opened_at.to_bits(),
                        )
                    })
                    .collect();
                let mut added = existing;
                for record in witnesses.into_values() {
                    if added.insert((
                        record.process.clone(),
                        record.connection.clone(),
                        record.opened_at.to_bits(),
                    )) {
                        saved.connections.records.push(record);
                    }
                }
            }
            Some(saved)
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
            reconcile: true,
        };
        // Legacy state is imported once; the first bounded save reconciles disk.
        if let Some(s) = &saved
            && restored
        {
            store.meta = metadata(s);
            store.previous = s.clone();
        }
        Ok((store, saved))
    }
    fn reconcile(&mut self, saved: &Saved, scope: &str) -> io::Result<()> {
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS retained(section TEXT,key TEXT,PRIMARY KEY(section,key)) WITHOUT ROWID; DELETE FROM retained;").map_err(error)?;
        {
            let tx = self.db.transaction().map_err(error)?;
            let mut insert = tx
                .prepare("INSERT OR IGNORE INTO retained VALUES(?1,?2)")
                .map_err(error)?;
            for r in &saved.connections.records {
                insert
                    .execute(params![
                        "records",
                        serde_json::to_string(&(
                            r.process.clone(),
                            r.connection.clone(),
                            r.opened_at.to_bits()
                        ))
                        .map_err(error)?
                    ])
                    .map_err(error)?;
            }
            for (p, _) in &saved.connections.parents {
                insert
                    .execute(params!["parents", serde_json::to_string(p).map_err(error)?])
                    .map_err(error)?;
            }
            for (p, _) in &saved.connections.origins {
                insert
                    .execute(params!["origins", serde_json::to_string(p).map_err(error)?])
                    .map_err(error)?;
            }
            for p in &saved.connections.shared {
                insert
                    .execute(params!["shared", serde_json::to_string(p).map_err(error)?])
                    .map_err(error)?;
            }
            drop(insert);
            tx.commit().map_err(error)?;
        }
        // Runs on the persistence worker, after serving has started. Batches
        // bound each transaction; the engine/publication lock is never held.
        loop {
            let removed = self.db.execute("DELETE FROM evidence WHERE (scope,section,key) IN (SELECT scope,section,key FROM evidence e WHERE scope<>?1 OR NOT EXISTS(SELECT 1 FROM retained r WHERE r.section=e.section AND r.key=e.key) LIMIT 2048)",[scope]).map_err(error)?;
            if removed == 0 {
                break;
            }
            std::thread::yield_now();
        }
        Ok(())
    }
    pub fn save(&mut self, saved: Saved) -> io::Result<()> {
        let scope = serde_json::to_string(&(&saved.node_id, &saved.boot_id)).map_err(error)?;
        if self.reconcile {
            self.reconcile(&saved, &scope)?;
            self.reconcile = false;
        }
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
        // Replace the boot metadata in the same transaction: a crash must never
        // leave an empty snapshot table that could re-import a legacy backup.
        tx.execute("DELETE FROM snapshots WHERE scope<>?1", [&scope])
            .map_err(error)?;
        tx.commit().map_err(error)?;
        self.previous = saved;
        self.meta = meta;
        Ok(())
    }
}
