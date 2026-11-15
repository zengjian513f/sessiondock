//! Private identities for threads created before Codex writes their rollout.
//! Completed request results survive restarts; no native history is synthesized.
use std::{collections::HashMap, fs, io, path::PathBuf, sync::Mutex};

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
    ledger: super::ledger::Ledger,
    rows: Mutex<HashMap<String, Receipt>>,
}

impl Receipts {
    pub fn open(directory: Option<PathBuf>) -> io::Result<Self> {
        let ledger = super::ledger::Ledger::open(directory.as_deref(), "codex-sdk-sessions")?;
        if let Some(path) = directory
            .as_ref()
            .map(|d| d.join("codex-sdk-sessions.json"))
            .filter(|p| p.exists())
        {
            let old: HashMap<String, Receipt> = serde_json::from_reader(fs::File::open(&path)?)?;
            ledger.import(
                old.into_iter()
                    .map(|(uid, row)| (String::new(), uid, row.pending, row)),
            )?;
            fs::remove_file(path)?;
        }
        let rows = ledger
            .load::<Receipt>(false)?
            .into_iter()
            .map(|(_, row)| (row.row["uid"].as_str().unwrap_or_default().to_owned(), row))
            .collect();
        Ok(Self {
            ledger,
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
        let receipt = Receipt {
            request_id: request_id.map(str::to_owned),
            result,
            row,
            pending: true,
        };
        let mut rows = self.rows.lock().expect("SDK receipts");
        self.save(&uid, &receipt)?;
        rows.insert(uid, receipt);
        Ok(())
    }

    pub fn pending(&self) -> Result<Vec<Value>, RpcError> {
        let mut rows = self.rows.lock().expect("SDK receipts");
        for (uid, receipt) in rows.iter_mut().filter(|(_, r)| r.pending) {
            if receipt.row["path"]
                .as_str()
                .is_some_and(|path| std::path::Path::new(path).is_file())
            {
                let mut next = receipt.clone();
                next.pending = false;
                self.save(uid, &next)?;
                *receipt = next;
            }
        }
        Ok(rows
            .values()
            .filter(|r| r.pending)
            .map(|r| r.row.clone())
            .collect())
    }

    pub fn live_uids(&self, loaded: &[String]) -> Vec<String> {
        self.rows
            .lock()
            .expect("SDK receipts")
            .iter()
            .filter(|(_, receipt)| {
                receipt.row["sid"]
                    .as_str()
                    .is_some_and(|sid| loaded.iter().any(|id| id == sid))
                    && (receipt.pending
                        || receipt.row["path"]
                            .as_str()
                            .is_some_and(|path| std::path::Path::new(path).is_file()))
            })
            .map(|(uid, _)| uid.clone())
            .collect()
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
        let mut rows = self.rows.lock().expect("SDK receipts");
        for (uid, receipt) in rows
            .iter_mut()
            .filter(|(_, r)| r.row["sid"] == thread_id && r.row["title"] != name)
        {
            let mut next = receipt.clone();
            next.row["title"] = Value::String(name.to_owned());
            next.result["thread"]["name"] = Value::String(name.to_owned());
            self.save(uid, &next)?;
            *receipt = next;
        }
        Ok(())
    }

    pub fn discard(&self, uid: &str) -> Result<(), RpcError> {
        let mut rows = self.rows.lock().expect("SDK receipts");
        if let Some(receipt) = rows.get_mut(uid).filter(|r| r.pending) {
            let mut next = receipt.clone();
            next.pending = false;
            self.save(uid, &next)?;
            *receipt = next;
        }
        Ok(())
    }

    fn save(&self, uid: &str, receipt: &Receipt) -> Result<(), RpcError> {
        self.ledger
            .save("", uid, receipt.pending, receipt)
            .map_err(|e| RpcError::Storage(format!("Codex 创建回执保存失败：{e}")))
    }
}
