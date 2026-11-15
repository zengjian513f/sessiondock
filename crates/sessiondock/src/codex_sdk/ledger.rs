//! Incremental private receipts. Completed identities remain on disk for
//! idempotency; changing one input never rewrites other sessions' snapshots.
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{io, path::Path, sync::Mutex};

pub(super) struct Ledger(Mutex<Connection>);
impl Ledger {
    pub fn open(root: Option<&Path>, name: &str) -> io::Result<Self> {
        if let Some(root) = root {
            crate::sdk_common::private_dir(root)?;
        }
        let db = match root {
            Some(root) => Connection::open(root.join(format!("{name}.sqlite"))),
            None => Connection::open_in_memory(),
        }
        .map_err(io::Error::other)?;
        #[cfg(unix)]
        if let Some(root) = root {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                root.join(format!("{name}.sqlite")),
                std::fs::Permissions::from_mode(0o600),
            )?;
        }
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA fullfsync=ON; PRAGMA checkpoint_fullfsync=ON;
            CREATE TABLE IF NOT EXISTS records(scope TEXT NOT NULL,id TEXT NOT NULL,active INTEGER NOT NULL,row TEXT NOT NULL,PRIMARY KEY(scope,id));
            CREATE INDEX IF NOT EXISTS active_records ON records(active);
            CREATE INDEX IF NOT EXISTS native_items ON records(scope,json_extract(row,'$.native_item_id'));")
            .map_err(io::Error::other)?;
        Ok(Self(Mutex::new(db)))
    }
    pub fn save<T: Serialize>(
        &self,
        scope: &str,
        id: &str,
        active: bool,
        row: &T,
    ) -> io::Result<()> {
        self.0.lock().unwrap().execute("INSERT INTO records VALUES (?1,?2,?3,?4) ON CONFLICT(scope,id) DO UPDATE SET active=excluded.active,row=excluded.row",
            params![scope,id,active,serde_json::to_string(row)?]).map_err(io::Error::other)?;
        Ok(())
    }
    pub fn import<T: Serialize>(
        &self,
        rows: impl IntoIterator<Item = (String, String, bool, T)>,
    ) -> io::Result<()> {
        let mut db = self.0.lock().unwrap();
        let tx = db.transaction().map_err(io::Error::other)?;
        for (scope, id, active, row) in rows {
            tx.execute(
                "INSERT OR IGNORE INTO records VALUES (?1,?2,?3,?4)",
                params![scope, id, active, serde_json::to_string(&row)?],
            )
            .map_err(io::Error::other)?;
        }
        tx.commit().map_err(io::Error::other)
    }
    pub fn load<T: DeserializeOwned>(&self, active_only: bool) -> io::Result<Vec<(String, T)>> {
        let db = self.0.lock().unwrap();
        let mut query = db
            .prepare("SELECT scope,row FROM records WHERE NOT ?1 OR active=1 ORDER BY rowid")
            .map_err(io::Error::other)?;
        query
            .query_map([active_only], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(io::Error::other)?
            .map(|r| {
                let (scope, row) = r.map_err(io::Error::other)?;
                Ok((scope, serde_json::from_str(&row)?))
            })
            .collect()
    }
    pub fn get<T: DeserializeOwned>(&self, scope: &str, id: &str) -> io::Result<Option<T>> {
        let row: Option<String> = self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT row FROM records WHERE scope=?1 AND id=?2",
                [scope, id],
                |r| r.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        row.map(|r| serde_json::from_str(&r).map_err(io::Error::other))
            .transpose()
    }
    pub fn consumed(&self, scope: &str, id: &str) -> io::Result<bool> {
        self.0.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM records WHERE scope=?1 AND json_extract(row,'$.native_item_id')=?2)", [scope,id], |r|r.get(0)).map_err(io::Error::other)
    }
}
