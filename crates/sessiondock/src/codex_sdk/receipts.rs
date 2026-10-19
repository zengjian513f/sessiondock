//! Private identities for threads created before Codex writes their rollout.
//! Completed request results survive restarts; no native history is synthesized.
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::RpcError;

#[derive(Clone, Serialize, Deserialize)]
struct Receipt {
    request_id: Option<String>,
    result: Value,
    row: Value,
    pending: bool,
}

pub(super) struct Receipts {
    path: Option<PathBuf>,
    rows: Mutex<HashMap<String, Receipt>>,
}

impl Receipts {
    pub fn open(directory: Option<PathBuf>) -> io::Result<Self> {
        let path = directory.map(|dir| dir.join("codex-sdk-sessions.json"));
        let rows = match path.as_ref().map(fs::read) {
            Some(Ok(bytes)) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Some(Err(error)) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => HashMap::new(),
        };
        Ok(Self {
            path,
            rows: Mutex::new(rows),
        })
    }

    pub fn completed(&self, request_id: &str) -> Option<Value> {
        self.rows
            .lock()
            .expect("SDK receipts")
            .values()
            .find(|receipt| receipt.request_id.as_deref() == Some(request_id))
            .map(|receipt| receipt.result.clone())
    }

    pub fn insert(
        &self,
        request_id: Option<&str>,
        result: Value,
        row: Value,
    ) -> Result<(), RpcError> {
        let uid = row["uid"]
            .as_str()
            .ok_or(RpcError::Disconnected)?
            .to_owned();
        self.update(|rows| {
            rows.insert(
                uid,
                Receipt {
                    request_id: request_id.map(str::to_owned),
                    result,
                    row,
                    pending: true,
                },
            );
        })
    }

    pub fn pending(&self) -> Result<Vec<Value>, RpcError> {
        let mut rows = self.rows.lock().expect("SDK receipts");
        let mut next = rows.clone();
        let mut changed = false;
        for receipt in next.values_mut().filter(|receipt| receipt.pending) {
            if receipt.row["path"]
                .as_str()
                .is_some_and(|path| std::path::Path::new(path).is_file())
            {
                receipt.pending = false;
                changed = true;
            }
        }
        if changed {
            self.save(&next)?;
            *rows = next;
        }
        Ok(rows
            .values()
            .filter(|receipt| receipt.pending)
            .map(|receipt| receipt.row.clone())
            .collect())
    }

    pub fn thread(&self, uid: &str) -> Option<String> {
        let rows = self.rows.lock().expect("SDK receipts");
        let receipt = rows.get(uid).filter(|receipt| receipt.pending)?;
        receipt.row["sid"].as_str().map(str::to_owned)
    }

    pub fn session(&self, uid: &str) -> Option<Value> {
        let rows = self.rows.lock().expect("SDK receipts");
        let receipt = rows.get(uid).filter(|receipt| receipt.pending)?;
        (!receipt.row["path"]
            .as_str()
            .is_some_and(|path| std::path::Path::new(path).is_file()))
        .then(|| receipt.row.clone())
    }

    pub fn set_name(&self, thread_id: &str, name: &str) -> Result<(), RpcError> {
        self.update(|rows| {
            for receipt in rows.values_mut().filter(|r| r.row["sid"] == thread_id) {
                receipt.row["title"] = Value::String(name.to_owned());
                receipt.result["thread"]["name"] = Value::String(name.to_owned());
            }
        })
    }

    pub fn discard(&self, uid: &str) -> Result<(), RpcError> {
        self.update(|rows| {
            if let Some(receipt) = rows.get_mut(uid) {
                receipt.pending = false;
            }
        })
    }

    pub fn pending_threads(&self) -> Vec<String> {
        self.rows
            .lock()
            .expect("SDK receipts")
            .values()
            .filter(|receipt| receipt.pending)
            .filter_map(|receipt| receipt.row["sid"].as_str().map(str::to_owned))
            .collect()
    }

    fn update(&self, change: impl FnOnce(&mut HashMap<String, Receipt>)) -> Result<(), RpcError> {
        let mut rows = self.rows.lock().expect("SDK receipts");
        let mut next = rows.clone();
        change(&mut next);
        self.save(&next)?;
        *rows = next;
        Ok(())
    }

    fn save(&self, rows: &HashMap<String, Receipt>) -> Result<(), RpcError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let temporary = path.with_extension("tmp");
        let result = (|| -> io::Result<()> {
            let mut options = fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec(rows)?)?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            #[cfg(unix)]
            fs::File::open(path.parent().expect("SDK receipt directory"))?.sync_all()?;
            Ok(())
        })();
        result.map_err(|error| RpcError::Storage(format!("Codex 创建回执保存失败：{error}")))
    }
}
