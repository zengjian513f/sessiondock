//! In-memory job registry for write-side file operations. Jobs are bound to
//! the session scope that created them, expire after inactivity, and finished
//! jobs are retained briefly so the legacy task panel can observe completion.
//! Nothing here touches the filesystem except dropping expired staging files.

use super::{
    FileError, ResolvedTarget,
    write::{Conflict, ScopeKey, WriteLimits},
};
use cap_std::fs::Dir;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::File,
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const MAX_FINISHED_JOBS: usize = 64;
const MAX_EXPIRED_IDS: usize = 256;
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
    fn finished(self) -> bool {
        !matches!(self, Self::Uploading)
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
    pub activity: Instant,
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
            activity: Instant::now(),
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
    pub fn touch(&mut self) {
        self.activity = Instant::now();
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
    expired: VecDeque<String>,
}
pub(super) struct Registry {
    limits: WriteLimits,
    inner: Mutex<Inner>,
}
impl Registry {
    pub fn new(limits: WriteLimits) -> Self {
        Self {
            limits,
            inner: Mutex::new(Inner {
                jobs: BTreeMap::new(),
                expired: VecDeque::new(),
            }),
        }
    }
    fn inner(&self) -> Result<MutexGuard<'_, Inner>, FileError> {
        self.inner.lock().map_err(|_| {
            FileError::new(503, "file_jobs_poisoned", "文件任务登记不可用，请重启核验")
        })
    }
    /// Drop jobs idle longer than the expiry and surplus finished jobs.
    /// Expired uploads lose their staging file; their ids answer 410.
    fn sweep(inner: &mut Inner, limits: &WriteLimits) {
        let now = Instant::now();
        let mut stale = Vec::new();
        let mut finished = Vec::new();
        for (id, job) in &inner.jobs {
            let Ok(job) = job.lock() else {
                stale.push(id.clone());
                continue;
            };
            if now.duration_since(job.activity) > limits.expiry {
                stale.push(id.clone());
            } else if job.state.finished() {
                finished.push((job.activity, id.clone()));
            }
        }
        if finished.len() > MAX_FINISHED_JOBS {
            let overflow = finished.len() - MAX_FINISHED_JOBS;
            finished.sort();
            stale.extend(finished.into_iter().take(overflow).map(|(_, id)| id));
        }
        for id in stale {
            if let Some(job) = inner.jobs.remove(&id) {
                if let Ok(mut job) = job.lock()
                    && let Some(upload) = job.upload.take()
                {
                    drop(upload.file);
                    let _ = upload.staging_dir.remove_file(&upload.staging_name);
                }
                inner.expired.push_back(id);
                while inner.expired.len() > MAX_EXPIRED_IDS {
                    inner.expired.pop_front();
                }
            }
        }
    }
    pub fn insert_uploading(&self, job: Job) -> Result<Arc<Mutex<Job>>, FileError> {
        let mut inner = self.inner()?;
        Self::sweep(&mut inner, &self.limits);
        let active = inner
            .jobs
            .values()
            .filter(|job| job.lock().is_ok_and(|job| !job.state.finished()))
            .count();
        if active >= self.limits.max_jobs {
            if let Some(upload) = job.upload {
                drop(upload.file);
                let _ = upload.staging_dir.remove_file(&upload.staging_name);
            }
            return Err(FileError::new(
                429,
                "file_jobs_limit",
                format!(
                    "最多同时进行 {} 个上传任务，请稍后重试",
                    self.limits.max_jobs
                ),
            )
            .with_details(json!({"limit": self.limits.max_jobs, "active": active})));
        }
        let id = job.id.clone();
        let job = Arc::new(Mutex::new(job));
        inner.jobs.insert(id, job.clone());
        Ok(job)
    }
    pub fn insert_finished(&self, job: Job) {
        if let Ok(mut inner) = self.inner() {
            Self::sweep(&mut inner, &self.limits);
            inner.jobs.insert(job.id.clone(), Arc::new(Mutex::new(job)));
        }
    }
    pub fn find(&self, scope: &ScopeKey, id: &str) -> Result<Arc<Mutex<Job>>, FileError> {
        if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(FileError::new(400, "file_job_invalid", "无效的任务编号"));
        }
        let mut inner = self.inner()?;
        Self::sweep(&mut inner, &self.limits);
        match inner.jobs.get(id) {
            Some(job) if job.lock().is_ok_and(|job| &job.scope == scope) => Ok(job.clone()),
            Some(_) => Err(FileError::new(
                404,
                "file_job_unknown",
                "任务不存在或不属于此会话",
            )),
            None if inner.expired.iter().any(|expired| expired == id) => Err(FileError::new(
                410,
                "file_job_expired",
                format!(
                    "任务已因 {} 秒无活动而过期，暂存数据已丢弃，请重新开始",
                    self.limits.expiry.as_secs()
                ),
            )
            .with_details(json!({"expiry_seconds": self.limits.expiry.as_secs()}))),
            None => Err(FileError::new(
                404,
                "file_job_unknown",
                "任务不存在或不属于此会话",
            )),
        }
    }
    pub fn list(&self, scope: &ScopeKey) -> Vec<Value> {
        let Ok(mut inner) = self.inner() else {
            return Vec::new();
        };
        Self::sweep(&mut inner, &self.limits);
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
    #[cfg(test)]
    pub fn expire_all(&self) {
        if let Ok(mut inner) = self.inner() {
            for job in inner.jobs.values() {
                if let Ok(mut job) = job.lock() {
                    job.activity =
                        Instant::now() - self.limits.expiry - std::time::Duration::from_secs(1);
                }
            }
            Self::sweep(&mut inner, &self.limits);
        }
    }
}
