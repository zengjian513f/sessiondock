//! Incremental durable attribution state; old JSON is imported once and retained.
use im::{OrdMap, ordmap::DiffItem};
use process_links::{
    connections::{ConnectionRecord, Evidence, RecordKey, Records, StoredRecord},
    engine::{SaveSnapshot, Saved},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, BufReader},
    path::Path,
    sync::Arc,
    time::Duration,
};

pub struct Store {
    db: Connection,
    previous: Evidence,
    scope: Option<String>,
    meta: Value,
    reconcile: bool,
}
fn error(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
// Abort an in-flight lock wait promptly on shutdown. The final batch sets
// its own short wait budget so the normal systemd stop grace can flush it.
pub fn wait_for_database(attempt: i32, limit: i32) -> bool {
    use std::sync::atomic::Ordering;
    if attempt >= limit || crate::server::STOP.load(Ordering::Relaxed) {
        return false;
    }
    std::thread::sleep(Duration::from_millis(50));
    !crate::server::STOP.load(Ordering::Relaxed)
}

fn metadata(s: &Saved, final_records: Option<&Records>) -> Value {
    let mut data = json!({"node_id":s.node_id,"boot_id":s.boot_id,"catalog":s.catalog,
        "links":s.links,"bindings":s.bindings,"forked_from":s.forked_from});
    if let Some(records) = final_records {
        // An orderly rollback can read the latest last-live times using the
        // old SQLite layout. Only the final flush materializes this index.
        let last_seen: Vec<_> = records
            .iter()
            .filter_map(|(key, row)| row.last_seen.map(|at| (key, at)))
            .collect();
        if !last_seen.is_empty() {
            data["last_seen"] = json!(last_seen);
        }
    }
    data
}

// The maps share untouched tree nodes, so the diff skips static history.
fn changes<T: Clone + PartialEq, K: Clone + Ord + Serialize>(
    tx: &Transaction<'_>,
    scope: &str,
    section: &str,
    old: &OrdMap<K, T>,
    new: &OrdMap<K, T>,
    data: impl Fn(&K, &T) -> serde_json::Result<String>,
) -> io::Result<()> {
    let mut upsert=tx.prepare_cached("INSERT INTO evidence VALUES(?1,?2,?3,?4) ON CONFLICT(scope,section,key) DO UPDATE SET data=excluded.data").map_err(error)?;
    let mut delete = tx
        .prepare_cached("DELETE FROM evidence WHERE scope=?1 AND section=?2 AND key=?3")
        .map_err(error)?;
    for item in old.diff(new) {
        match item {
            DiffItem::Add(key, value)
            | DiffItem::Update {
                new: (key, value), ..
            } => {
                upsert
                    .execute(params![
                        scope,
                        section,
                        serde_json::to_string(key).map_err(error)?,
                        data(key, value).map_err(error)?
                    ])
                    .map_err(error)?;
            }
            DiffItem::Remove(key, _) => {
                delete
                    .execute(params![
                        scope,
                        section,
                        serde_json::to_string(key).map_err(error)?
                    ])
                    .map_err(error)?;
            }
        }
    }
    Ok(())
}
fn stored_record(key: &RecordKey, data: &str) -> io::Result<StoredRecord> {
    #[derive(Deserialize)]
    struct LastSeen {
        last_seen: Option<f64>,
    }
    let mut record: ConnectionRecord = serde_json::from_str(data).map_err(error)?;
    // The key retains the exact opened-at bits across JSON float parsing.
    record.opened_at = f64::from_bits(key.2);
    let last_seen = if record.closed_at.is_none() {
        serde_json::from_str::<LastSeen>(data)
            .map_err(error)?
            .last_seen
    } else {
        None
    };
    Ok(StoredRecord { record, last_seen })
}

fn sqlite_text<'a>(row: &'a rusqlite::Row<'_>, column: usize) -> io::Result<&'a str> {
    row.get_ref(column).map_err(error)?.as_str().map_err(error)
}

fn restore_records(
    db: &Connection,
    scope: &str,
    connections: &mut Evidence,
    snapshot: &process_links::linux::Snapshot,
    cutoff: f64,
    legacy_seen: &BTreeMap<RecordKey, f64>,
) -> io::Result<()> {
    #[derive(Deserialize)]
    struct Lifetime {
        process: process_links::Process,
        closed_at: Option<f64>,
    }
    let live = |p: &process_links::Process| {
        snapshot
            .entries
            .get(&p.pid)
            .is_some_and(|e| e.process == *p)
    };
    let mut st = db
        .prepare("SELECT section,key,data FROM evidence WHERE scope=?1")
        .map_err(error)?;
    let mut rows = st.query([scope]).map_err(error)?;
    let mut unfinished = BTreeMap::<process_links::Connection, Vec<f64>>::new();
    while let Some(row) = rows.next().map_err(error)? {
        let data = sqlite_text(row, 2)?;
        match sqlite_text(row, 0)? {
            "records" => {
                // Skip the session and launch chain of expired rows during startup.
                let life: Lifetime = serde_json::from_str(data).map_err(error)?;
                let is_live = live(&life.process);
                if !is_live && life.closed_at.is_some_and(|end| end < cutoff) {
                    continue;
                }
                let key: RecordKey = serde_json::from_str(sqlite_text(row, 1)?).map_err(error)?;
                let record = stored_record(&key, data)?;
                if record.record.closed_at.is_none() {
                    let opened = record.record.opened_at;
                    let last_live = record
                        .last_seen
                        .or(legacy_seen.get(&key).copied())
                        .unwrap_or(opened)
                        .max(opened);
                    if !is_live && last_live < cutoff {
                        continue;
                    }
                    unfinished.entry(key.1.clone()).or_default().push(opened);
                }
                connections.records.insert(key, Arc::new(record));
            }
            "parents" => {
                let (key, value) = serde_json::from_str(data).map_err(error)?;
                connections.parents.insert(key, value);
            }
            "origins" => {
                let (key, value) = serde_json::from_str(data).map_err(error)?;
                connections.origins.insert(key, value);
            }
            "shared" => {
                connections
                    .shared
                    .insert(serde_json::from_str(data).map_err(error)?, ());
            }
            _ => return Err(error("unknown evidence section")),
        }
    }
    drop(rows);
    if !unfinished.is_empty() {
        // A retained unfinished span needs its first strictly later connect.
        // The keys contain all the inputs for this search, so loading the
        // discarded rows' JSON bodies a second time is unnecessary.
        let mut st = db
            .prepare("SELECT key FROM evidence WHERE scope=?1 AND section='records'")
            .map_err(error)?;
        let mut rows = st.query([scope]).map_err(error)?;
        let mut witnesses = BTreeMap::<(process_links::Connection, u64), RecordKey>::new();
        while let Some(row) = rows.next().map_err(error)? {
            let key: RecordKey = serde_json::from_str(sqlite_text(row, 0)?).map_err(error)?;
            if let Some(starts) = unfinished.get(&key.1) {
                let at = f64::from_bits(key.2);
                for start in starts.iter().filter(|start| at > **start) {
                    let slot = witnesses
                        .entry((key.1.clone(), start.to_bits()))
                        .or_insert_with(|| key.clone());
                    let old_at = f64::from_bits(slot.2);
                    if at < old_at || at == old_at && key < *slot {
                        *slot = key.clone();
                    }
                }
            }
        }
        drop(rows);
        let mut read = db
            .prepare("SELECT data FROM evidence WHERE scope=?1 AND section='records' AND key=?2")
            .map_err(error)?;
        for key in witnesses.into_values() {
            if !connections.records.contains_key(&key) {
                let text: String = read
                    .query_row(
                        params![scope, serde_json::to_string(&key).map_err(error)?],
                        |r| r.get(0),
                    )
                    .map_err(error)?;
                let row = stored_record(&key, &text)?;
                connections.records.insert(key, Arc::new(row));
            }
        }
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
    ) -> io::Result<(Self, Option<SaveSnapshot>)> {
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
        let mut store = Self {
            db,
            previous: Evidence::default(),
            scope: None,
            meta: Value::Null,
            reconcile: true,
        };
        let saved = if let Some(raw) = raw {
            let saved: Saved = serde_json::from_str(&raw).map_err(error)?;
            let meta: Value = serde_json::from_str(&raw).map_err(error)?;
            let last_seen: BTreeMap<RecordKey, f64> = serde_json::from_value::<Vec<_>>(
                meta.get("last_seen").cloned().unwrap_or_else(|| json!([])),
            )
            .map_err(error)?
            .into_iter()
            .collect();
            let mut connections = Evidence::default();
            restore_records(
                &store.db,
                &scope,
                &mut connections,
                snapshot,
                at - retention - 2.0,
                &last_seen,
            )?;
            store.previous = connections.clone();
            store.scope = Some(scope);
            store.meta = meta;
            // Older databases held last-live timestamps in the metadata array.
            // Move each retained unfinished timestamp into its own row in the
            // same transaction that replaces the old metadata.
            for (key, at) in last_seen {
                if let Some(row) = connections.records.get(&key)
                    && row.record.closed_at.is_none()
                    && row.last_seen.is_none()
                {
                    Arc::make_mut(connections.records.get_mut(&key).unwrap()).last_seen = Some(at);
                }
            }
            Some(SaveSnapshot {
                state: saved,
                connections,
            })
        } else if store
            .db
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
                    let mut saved: Saved =
                        serde_json::from_reader(BufReader::new(file)).map_err(error)?;
                    let connections = std::mem::take(&mut saved.connections).into();
                    Some(SaveSnapshot {
                        state: saved,
                        connections,
                    })
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            }
        };
        store
            .db
            .busy_handler(Some(|attempt| wait_for_database(attempt, 600)))
            .map_err(error)?;
        Ok((store, saved))
    }
    pub fn prepare_final_save(&self) -> io::Result<()> {
        self.db.busy_timeout(Duration::from_secs(2)).map_err(error)
    }

    fn reconcile(&mut self, scope: &str) -> io::Result<()> {
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS retained(section TEXT,key TEXT,PRIMARY KEY(section,key)) WITHOUT ROWID; DELETE FROM retained;").map_err(error)?;
        {
            let tx = self.db.transaction().map_err(error)?;
            let mut insert = tx
                .prepare("INSERT OR IGNORE INTO retained VALUES(?1,?2)")
                .map_err(error)?;
            for key in self.previous.records.keys() {
                insert
                    .execute(params![
                        "records",
                        serde_json::to_string(key).map_err(error)?
                    ])
                    .map_err(error)?;
            }
            for key in self.previous.parents.keys() {
                insert
                    .execute(params![
                        "parents",
                        serde_json::to_string(key).map_err(error)?
                    ])
                    .map_err(error)?;
            }
            for key in self.previous.origins.keys() {
                insert
                    .execute(params![
                        "origins",
                        serde_json::to_string(key).map_err(error)?
                    ])
                    .map_err(error)?;
            }
            for key in self.previous.shared.keys() {
                insert
                    .execute(params![
                        "shared",
                        serde_json::to_string(key).map_err(error)?
                    ])
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
    pub fn save(&mut self, saved: SaveSnapshot, final_save: bool) -> io::Result<()> {
        let scope =
            serde_json::to_string(&(&saved.state.node_id, &saved.state.boot_id)).map_err(error)?;
        if self.reconcile {
            self.reconcile(&scope)?;
            self.reconcile = false;
        }
        let same_scope = self.scope.as_ref() == Some(&scope);
        let empty = Evidence::default();
        let old = if same_scope { &self.previous } else { &empty };
        let meta = metadata(
            &saved.state,
            final_save.then_some(&saved.connections.records),
        );
        let tx = self.db.transaction().map_err(error)?;
        if !same_scope || self.meta != meta {
            tx.execute("INSERT INTO snapshots VALUES(?1,?2) ON CONFLICT(scope) DO UPDATE SET data=excluded.data",params![scope,meta.to_string()]).map_err(error)?;
        }
        let new = saved.connections;
        changes(
            &tx,
            &scope,
            "records",
            &old.records,
            &new.records,
            |_, row| serde_json::to_string(row),
        )?;
        changes(
            &tx,
            &scope,
            "parents",
            &old.parents,
            &new.parents,
            |key, value| serde_json::to_string(&(key, value)),
        )?;
        changes(
            &tx,
            &scope,
            "origins",
            &old.origins,
            &new.origins,
            |key, value| serde_json::to_string(&(key, value)),
        )?;
        changes(&tx, &scope, "shared", &old.shared, &new.shared, |key, _| {
            serde_json::to_string(key)
        })?;
        if !same_scope {
            // Publish the new scope and retire the old metadata atomically
            // so a crash cannot cause the JSON backup to be replayed.
            tx.execute("DELETE FROM evidence WHERE scope<>?1", [&scope])
                .map_err(error)?;
        }
        // A previous boot snapshot may still exist after startup reconciliation.
        tx.execute("DELETE FROM snapshots WHERE scope<>?1", [&scope])
            .map_err(error)?;
        tx.commit().map_err(error)?;
        self.previous = new;
        self.scope = Some(scope);
        self.meta = meta;
        Ok(())
    }
}
