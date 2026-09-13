//! In-memory file-operation jobs, bound to the session scope that created them.

use super::{
    FileError, ResolvedTarget,
    write::{Conflict, ScopeKey},
};
use cap_std::fs::Dir;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::File,
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_LISTED_JOBS: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    Uploading,
    Completed,
    Failed,
    Cancelled,
}
impl JobState {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Uploading => "uploading",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Staging state of an active upload: the destination directory handle, the
/// private staging directory and the open staging file.
pub(super) struct Upload {
    pub dest: ResolvedTarget,
    pub staging_dir: Arc<Dir>,
    pub staging_name: String,
    pub file: File,
    pub hasher: sha2::Sha256,
}

pub(super) struct Job {
    pub id: String,
    pub scope: ScopeKey,
    pub action: &'static str,
    pub state: JobState,
    pub created: f64,
    pub updated: f64,
    pub name: String,
    pub destination: String,
    pub conflict: Conflict,
    pub total_bytes: u64,
    pub received: u64,
    /// Start of the last accepted chunk; only that exact chunk may be replayed.
    pub last_offset: u64,
    pub upload_modified: Option<f64>,
    pub declared_sha256: Option<[u8; 32]>,
    pub sha256: Option<String>,
    pub upload: Option<Upload>,
    pub items_total: usize,
    pub items_done: usize,
    pub completed: Vec<Value>,
    pub errors: Vec<Value>,
    pub path: Option<String>,
    pub published_name: Option<String>,
}
impl Job {
    pub fn new(
        scope: ScopeKey,
        action: &'static str,
        name: String,
        destination: &Path,
        conflict: Conflict,
        items_total: usize,
    ) -> Result<Self, FileError> {
        let time = now();
        Ok(Self {
            id: random_id()?,
            scope,
            action,
            state: JobState::Completed,
            created: time,
            updated: time,
            name,
            destination: destination.to_str().unwrap_or("").to_owned(),
            conflict,
            total_bytes: 0,
            received: 0,
            last_offset: 0,
            upload_modified: None,
            declared_sha256: None,
            sha256: None,
            upload: None,
            items_total: items_total.max(1),
            items_done: 0,
            completed: Vec::new(),
            errors: Vec::new(),
            path: None,
            published_name: None,
        })
    }
    /// Legacy job shape (`files.js` reads id/state/bytes/total_bytes/items/
    /// completed/errors/upload_*); scope and handles never leave the process.
    pub fn public(&self) -> Value {
        let mut value = json!({
            "id": self.id, "action": self.action, "state": self.state.wire(),
            "created": self.created, "updated": self.updated, "name": self.name,
            "destination": self.destination, "conflict": self.conflict.wire(),
            "bytes": if self.action == "upload" { self.received } else { 0 },
            "items_done": self.items_done, "items_total": self.items_total,
            "completed": self.completed, "errors": self.errors,
            "error": self.errors.first().and_then(|e| e["error"].as_str()).unwrap_or(""),
        });
        if self.action == "upload" {
            value["total_bytes"] = json!(self.total_bytes);
            value["upload_name"] = json!(self.name);
            value["upload_modified"] = json!(self.upload_modified.unwrap_or(0.0));
            value["path"] = json!(self.path);
            value["sha256"] = json!(self.sha256);
        }
        value
    }
}

pub(super) fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}
pub(super) fn random_id() -> Result<String, FileError> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| {
        FileError::new(
            503,
            "file_random_unavailable",
            "系统随机源不可用，不能分配任务编号",
        )
    })?;
    Ok(super::write::hex(&random))
}
pub(super) fn lock(job: &Arc<Mutex<Job>>) -> Result<MutexGuard<'_, Job>, FileError> {
    job.lock().map_err(|_| {
        FileError::new(
            503,
            "file_job_poisoned",
            "文件任务状态不可用，请刷新任务列表",
        )
    })
}

struct Inner {
    jobs: BTreeMap<String, Arc<Mutex<Job>>>,
}
pub(super) struct Registry {
    inner: Mutex<Inner>,
}
impl Registry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                jobs: BTreeMap::new(),
            }),
        }
    }
    fn inner(&self) -> Result<MutexGuard<'_, Inner>, FileError> {
        self.inner.lock().map_err(|_| {
            FileError::new(503, "file_jobs_poisoned", "文件任务登记不可用，请重启核验")
        })
    }
    pub fn insert_uploading(&self, job: Job) -> Result<Arc<Mutex<Job>>, FileError> {
        let mut inner = self.inner()?;
        let id = job.id.clone();
        let job = Arc::new(Mutex::new(job));
        inner.jobs.insert(id, job.clone());
        Ok(job)
    }
    pub fn insert_finished(&self, job: Job) {
        if let Ok(mut inner) = self.inner() {
            inner.jobs.insert(job.id.clone(), Arc::new(Mutex::new(job)));
        }
    }
    pub fn find(&self, scope: &ScopeKey, id: &str) -> Result<Arc<Mutex<Job>>, FileError> {
        if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(FileError::new(400, "file_job_invalid", "无效的任务编号"));
        }
        let inner = self.inner()?;
        match inner.jobs.get(id) {
            Some(job) if job.lock().is_ok_and(|job| &job.scope == scope) => Ok(job.clone()),
            Some(_) => Err(FileError::new(
                404,
                "file_job_unknown",
                "任务不存在或不属于此会话",
            )),
            None => Err(FileError::new(
                404,
                "file_job_unknown",
                "任务不存在或不属于此会话",
            )),
        }
    }
    pub fn list(&self, scope: &ScopeKey) -> Vec<Value> {
        let Ok(inner) = self.inner() else {
            return Vec::new();
        };
        let mut rows: Vec<(f64, Value)> = inner
            .jobs
            .values()
            .filter_map(|job| job.lock().ok())
            .filter(|job| &job.scope == scope)
            .map(|job| (job.created, job.public()))
            .collect();
        rows.sort_by(|left, right| right.0.total_cmp(&left.0));
        rows.into_iter()
            .take(MAX_LISTED_JOBS)
            .map(|(_, value)| value)
            .collect()
    }
}
