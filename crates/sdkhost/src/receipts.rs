//! A bounded-memory index over the canonical append-only delivery receipts.
//! The index is disposable: offline/older clients keep reading receipts.jsonl.
use rusqlite::{Connection, OptionalExtension};
use sdkhost_client::{Receipt, platform::OpenOptionsExt};
use std::{
    io::{self, Write},
    path::Path,
};

pub struct Receipts {
    index: Connection,
    log: std::fs::File,
    failed: bool,
}
impl Receipts {
    pub fn new(dir: &Path) -> io::Result<Self> {
        let path = dir.join("receipts-index.sqlite");
        let index = Connection::open(&path).map_err(io::Error::other)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        // Each host directory starts once. The durable JSONL, never this cache,
        // is recovery evidence. Bound SQLite's page cache rather than receipt IDs.
        index
            .execute_batch(
                "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-128;
            CREATE TABLE receipts(id TEXT PRIMARY KEY,state TEXT NOT NULL);",
            )
            .map_err(io::Error::other)?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(dir.join("receipts.jsonl"))?;
        Ok(Self {
            index,
            log,
            failed: false,
        })
    }
    pub fn get(&self, id: &str) -> io::Result<Option<Receipt>> {
        if self.failed {
            return Err(io::Error::other(
                "receipt index failed; delivery remains uncertain",
            ));
        }
        self.index
            .query_row("SELECT state FROM receipts WHERE id=?1", [id], |r| {
                Ok(Receipt { state: r.get(0)? })
            })
            .optional()
            .map_err(io::Error::other)
    }
    pub fn save(&mut self, id: &str, state: &str) -> io::Result<Receipt> {
        self.get(id)?;
        let receipt = Receipt {
            state: state.into(),
        };
        // A partial log/index failure is fail-closed for the host's lifetime:
        // never let a retry turn missing cache evidence into a second pipe write.
        self.failed = true;
        serde_json::to_writer(
            &mut self.log,
            &serde_json::json!({"id":id,"receipt":receipt}),
        )?;
        self.log.write_all(b"\n")?;
        self.log.sync_data()?;
        self.index.execute("INSERT INTO receipts VALUES (?1,?2) ON CONFLICT(id) DO UPDATE SET state=excluded.state", [id,state]).map_err(io::Error::other)?;
        self.failed = false;
        Ok(receipt)
    }
}
