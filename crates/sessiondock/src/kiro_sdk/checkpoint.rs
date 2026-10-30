//! Private bridge evidence. Native session files are never used as a journal.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Rpc {
    pub key: String,
    pub id: Option<u64>,
    pub write_id: String,
    pub frame: Value,
    pub delivery: Option<String>,
    pub response: Option<Value>,
    pub turn: Option<u64>,
    pub request_key: Option<String>,
}

pub(super) fn path(host_dir: &Path, key: &str) -> PathBuf {
    host_dir
        .parent()
        .unwrap_or(host_dir)
        .join("sdk-adapters")
        .join("kiro")
        .join(format!("{key}.json"))
}

fn database_path(path: &Path) -> PathBuf {
    path.with_extension("sqlite")
}

pub(super) fn exists(path: &Path) -> bool {
    path.exists() || database_path(path).exists()
}

fn database(path: &Path) -> Result<rusqlite::Connection, String> {
    let db_path = database_path(path);
    std::fs::create_dir_all(path.parent().ok_or("checkpoint parent missing")?)
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path.parent().unwrap(),
            std::fs::Permissions::from_mode(0o700),
        )
        .map_err(|e| e.to_string())?;
    }
    let db = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE IF NOT EXISTS checkpoint (id INTEGER PRIMARY KEY, state TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS items (owner TEXT NOT NULL, ordinal INTEGER NOT NULL, value TEXT NOT NULL, PRIMARY KEY(owner,ordinal));").map_err(|e|e.to_string())?;
    Ok(db)
}

pub(super) fn load(path: &Path) -> Result<super::live::LiveState, String> {
    use rusqlite::OptionalExtension;
    let db = database(path)?;
    let value: Option<String> = db
        .query_row("SELECT state FROM checkpoint WHERE id=1", [], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    let mut state: super::live::LiveState = if let Some(value) = value {
        let mut state = serde_json::from_str(&value).map_err(|e| format!("checkpoint: {e}"))?;
        attach(&mut state, &database_path(path), "[]".into(), false);
        hydrate(&mut state)?;
        state
    } else {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let mut state = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        attach(&mut state, &database_path(path), "[]".into(), true);
        state
    };
    state.checkpoint_db = Some(db);
    Ok(state)
}

fn attach(state: &mut super::live::LiveState, path: &Path, key: String, legacy: bool) {
    state.history_store = Some(path.to_owned());
    state.history_key = key.clone();
    state.history_loaded = legacy;
    if legacy {
        state.history_len = state.items.len();
        state.history_dirty = (0..state.items.len()).collect();
        state.projection_dirty = true;
    }
    for (id, task) in &mut state.subtasks {
        let mut parts: Vec<String> = serde_json::from_str(&key).expect("owned history key");
        parts.push(id.clone());
        attach(
            &mut task.state,
            path,
            serde_json::to_string(&parts).unwrap(),
            legacy,
        );
    }
}

pub(super) fn hydrate(state: &mut super::live::LiveState) -> Result<(), String> {
    if state.history_loaded {
        return Ok(());
    }
    let Some(path) = &state.history_store else {
        state.history_loaded = true;
        return Ok(());
    };
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
    let mut query = db
        .prepare("SELECT value FROM items WHERE owner=?1 ORDER BY ordinal")
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map([&state.history_key], |r| {
            r.get::<_, rusqlite::types::Value>(0)
        })
        .map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    for row in rows {
        let value = match row.map_err(|e| e.to_string())? {
            rusqlite::types::Value::Text(text) => serde_json::from_str(&text),
            rusqlite::types::Value::Blob(bytes) => {
                serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice()))
            }
            _ => return Err("Kiro history body has invalid storage type".into()),
        }
        .map_err(|e| e.to_string())?;
        items.push(value);
    }
    if items.len() != state.history_len {
        return Err("Kiro history checkpoint incomplete".into());
    }
    state.items = items;
    state.history_loaded = true;
    Ok(())
}

pub(super) fn save(path: &Path, state: &mut super::live::LiveState) -> Result<(), String> {
    let mut db = match state.checkpoint_db.take() {
        Some(db) => db,
        None => database(path)?,
    };
    let result = (|| -> Result<(), String> {
        let tx = db.transaction().map_err(|e| e.to_string())?;
        persist_items(&tx, state, &database_path(path), "[]".into())?;
        tx.execute("INSERT INTO checkpoint VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET state=excluded.state",
            [serde_json::to_string(state).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        committed(state);
        if path.exists() {
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    state.checkpoint_db = Some(db);
    result.map_err(|e| format!("kiro_sdk_checkpoint: {e}"))
}

fn persist_items(
    tx: &rusqlite::Transaction<'_>,
    state: &mut super::live::LiveState,
    path: &Path,
    key: String,
) -> Result<(), String> {
    if state.history_store.is_none() {
        state.history_store = Some(path.to_owned());
        state.history_key = key.clone();
        state.history_loaded = true;
        state.history_dirty.extend(0..state.items.len());
    }
    if state.history_loaded {
        state.history_len = state.items.len();
        for index in &state.history_dirty {
            if let Some(value) = state.items.get(*index) {
                tx.execute("INSERT INTO items VALUES(?1,?2,?3) ON CONFLICT(owner,ordinal) DO UPDATE SET value=excluded.value",
                    rusqlite::params![key,*index,encode(value)?]).map_err(|e|e.to_string())?;
            }
        }
        tx.execute(
            "DELETE FROM items WHERE owner=?1 AND ordinal>=?2",
            rusqlite::params![key, state.items.len()],
        )
        .map_err(|e| e.to_string())?;
    }
    for (id, task) in &mut state.subtasks {
        let mut parts: Vec<String> = serde_json::from_str(&key).expect("owned history key");
        parts.push(id.clone());
        persist_items(
            tx,
            &mut task.state,
            path,
            serde_json::to_string(&parts).unwrap(),
        )?;
    }
    Ok(())
}
fn committed(state: &mut super::live::LiveState) {
    state.history_dirty.clear();
    for task in state.subtasks.values_mut() {
        committed(&mut task.state);
    }
}

/// Once the read-model has the body, completed children leave the live working set.
pub(super) fn projected(state: &mut super::live::LiveState) {
    state.projection_dirty = false;
    for task in state.subtasks.values_mut() {
        projected(&mut task.state);
        if task.state.history_store.is_some()
            && !matches!(task.status.as_str(), "pending" | "in_progress")
        {
            task.state.items.clear();
            task.state.item_index.clear();
            task.state.history_loaded = false;
        }
    }
}

pub(super) fn prepare_projection(state: &mut super::live::LiveState) -> Result<(), String> {
    if state.projection_dirty {
        hydrate(state)?;
    }
    for task in state.subtasks.values_mut() {
        prepare_projection(&mut task.state)?;
    }
    Ok(())
}

fn encode(value: &Value) -> Result<Vec<u8>, String> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    serde_json::to_writer(&mut encoder, value).map_err(|e| e.to_string())?;
    encoder.finish().map_err(|e| e.to_string())
}

/// Import a closed host without replacing a checkpoint a live adapter has
/// already committed. An interrupted import is retried in the next pass.
pub(super) fn migrate(path: &Path) -> Result<(), String> {
    let mut db = database(path)?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM checkpoint WHERE id=1)",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !exists {
        let mut state: super::live::LiveState = serde_json::from_reader(std::io::BufReader::new(
            std::fs::File::open(path).map_err(|e| e.to_string())?,
        ))
        .map_err(|e| e.to_string())?;
        attach(&mut state, &database_path(path), "[]".into(), true);
        persist_items(&tx, &mut state, &database_path(path), "[]".into())?;
        tx.execute(
            "INSERT INTO checkpoint VALUES(1,?1)",
            [serde_json::to_string(&state).map_err(|e| e.to_string())?],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
