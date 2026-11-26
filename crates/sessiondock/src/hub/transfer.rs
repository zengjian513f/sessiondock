//! Durable cross-node clone orchestration over the authenticated node channel.
//! The Hub forwards tar chunks without buffering the archive in memory.
mod manifest;
mod transport;

use super::{Client, Registry, Target, client, namespace};
use crate::transfer::TransferError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf, sync::Arc, time::Duration};

const IDLE: Duration = Duration::from_secs(300);
const RECENT_TASKS: usize = 20;
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Request {
    pub uid: String,
    pub target_node: String,
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_ids: Option<bool>,
}
impl Request {
    fn same_transfer(&self, other: &Self) -> bool {
        self.uid == other.uid
            && self.target_node == other.target_node
            && self.operation_id == other.operation_id
    }
}
#[derive(Clone, Deserialize, Serialize)]
struct Journal {
    #[serde(default)]
    execution_requested: bool,
    #[serde(default)]
    cancel_requested: bool,
    #[serde(default)]
    frozen_ready: bool,
    #[serde(default)]
    created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    move_seq: Option<u64>,
    request: Request,
    phase: String,
    result: Option<Value>,
    #[serde(default)]
    preview: Option<Value>,
    #[serde(default)]
    bytes_sent: u64,
    #[serde(default)]
    bytes_total: u64,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    transport: Option<transport::Report>,
}

#[derive(Default)]
struct TaskIndex {
    unfinished: BTreeMap<String, Journal>,
    recent: BTreeMap<(u64, String), Value>,
}

impl TaskIndex {
    fn update(&mut self, journal: Journal) {
        let id = journal.request.operation_id.clone();
        if !matches!(journal.phase.as_str(), "complete" | "aborted") {
            self.unfinished.insert(id, journal);
            return;
        }
        self.unfinished.remove(&id);
        if !journal.execution_requested {
            return;
        }
        let members = journal
            .preview
            .as_ref()
            .and_then(|plan| plan["sessions"].as_array());
        let title = members
            .and_then(|members| {
                members
                    .iter()
                    .find(|m| m["uid"] == journal.request.uid)
                    .or_else(|| members.first())
            })
            .and_then(|member| member["title"].as_str());
        let target_uid = journal
            .result
            .as_ref()
            .and_then(|result| result["target_uid"].as_str());
        self.recent.insert(
            (journal.created_ms, id),
            json!({"request":journal.request,"phase":journal.phase,"title":title,
                "created_ms":journal.created_ms,"error":journal.error,"code":journal.error_code,
                "target_uid":target_uid}),
        );
        if self.recent.len() > RECENT_TASKS {
            self.recent.pop_first();
        }
    }
}

pub struct Transfers {
    policy: crate::transfer::transport::Policy,
    directory: PathBuf,
    work: crate::transfer::progress::Registry,
    links: Arc<std::sync::Mutex<std::collections::BTreeMap<String, crate::session_links::Record>>>,
    // Allocate and publish successful move positions in the same order, even
    // when independent transfers finish on different nodes at the same time.
    completed_moves: Arc<std::sync::Mutex<u64>>,
    gates: crate::transfer::coordination::Locks,
    tasks: Arc<std::sync::Mutex<TaskIndex>>,
    // Move the guard into the blocking write: cancelling its async waiter must
    // not let a later save overtake it or reuse its temporary file.
    saves: crate::transfer::coordination::Locks,
    executions: std::sync::Mutex<std::collections::BTreeMap<String, std::sync::Weak<()>>>,
    cancellations:
        std::sync::Mutex<std::collections::BTreeMap<String, tokio_util::sync::CancellationToken>>,
}
impl Transfers {
    pub fn open(directory: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let mut links = std::collections::BTreeMap::new();
        let mut tasks = TaskIndex::default();
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let mut journal: Journal = match fs::read(&path).and_then(|raw| {
                serde_json::from_slice(&raw)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
            }) {
                Ok(journal) => journal,
                Err(error) => {
                    // A damaged transfer must not prevent the Hub from serving
                    // other sessions. Preserve its journal for repair.
                    crate::log::warn(
                        "transfer.recovery_unreadable",
                        serde_json::json!({"path": path.display().to_string(), "error": error.to_string()}),
                    );
                    continue;
                }
            };
            if journal.created_ms == 0 {
                journal.created_ms = fs::metadata(&path)?
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
            }
            let record = Self::link_record(&journal);
            links.insert(record.id.clone(), record);
            tasks.update(journal);
        }
        let move_seq = links
            .values()
            .filter_map(|record| record.move_seq)
            .max()
            .unwrap_or_default();
        Ok(Self {
            completed_moves: Arc::new(std::sync::Mutex::new(move_seq)),
            policy: Default::default(),
            directory,
            work: Default::default(),
            links: Arc::new(std::sync::Mutex::new(links)),
            tasks: Arc::new(std::sync::Mutex::new(tasks)),
            saves: Default::default(),
            gates: Default::default(),
            cancellations: Default::default(),
            executions: Default::default(),
        })
    }
    pub fn with_policy(mut self, policy: crate::transfer::transport::Policy) -> Self {
        self.policy = policy;
        self
    }
    /// No browser is responsible for finishing compensation or a committed move.
    /// On restart all unfinished journals are reconciled, never blindly replayed.
    pub fn spawn(
        self: Arc<Self>,
        registry: Arc<Registry>,
        client: Arc<Client>,
        shutdown: tokio_util::sync::CancellationToken,
    ) {
        let _ = registry.transfer_links.set(self.links.clone());
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            let mut workers = tokio::task::JoinSet::new();
            let mut recovering = std::collections::BTreeSet::new();
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    Some(done) = workers.join_next(), if !workers.is_empty() => {
                        if let Ok(id) = done { recovering.remove(&id); }
                    }
                    _ = interval.tick() => {
                        let requests: Vec<_> = self.tasks.lock().unwrap_or_else(|e| e.into_inner())
                            .unfinished.values().map(|journal| (journal.request.clone(), journal.execution_requested && !journal.cancel_requested)).collect();
                        for (request, execute) in requests {
                            let id = request.operation_id.clone();
                            let active = self.executions.lock().unwrap_or_else(|e| e.into_inner())
                                .get(&id).is_some_and(|worker| worker.strong_count() > 0);
                            if active || !recovering.insert(id.clone()) { continue; }
                            let transfers = self.clone(); let registry = registry.clone(); let client = client.clone();
                            workers.spawn(async move {
                                if execute {
                                    let _ = transfers.execute(registry, client, request).await;
                                } else {
                                    // Legacy abandoned operations and explicit cancellation retain
                                    // their source-first compensation/committed-move recovery.
                                    let _ = transfers.cancel_inner(registry, client, request, true).await;
                                }
                                id
                            });
                        }
                    }
                }
            }
        });
    }
    fn path(&self, id: &str) -> Result<PathBuf, TransferError> {
        if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            return Err(TransferError::new("move_plan_stale", "迁移标识无效"));
        }
        Ok(self.directory.join(format!("{id}.json")))
    }
    async fn load(&self, id: &str) -> Result<Journal, TransferError> {
        // A cancelled save may still be running in the blocking pool. Mutating
        // callers must observe it before deriving their next journal state.
        let _guard = self.saves.acquire(vec![id.to_owned()]).await;
        Ok(serde_json::from_slice(&fs::read(self.path(id)?)?)?)
    }
    async fn save(&self, journal: &Journal) -> Result<(), TransferError> {
        let path = self.path(&journal.request.operation_id)?;
        let mut journal = journal.clone();
        let tasks = self.tasks.clone();
        let links = self.links.clone();
        let completed_moves = self.completed_moves.clone();
        let save_guard = self
            .saves
            .acquire(vec![journal.request.operation_id.clone()])
            .await;
        // Cancellation intent is monotonic. A worker holding an older journal
        // must not erase an accepted cancellation on its next phase save.
        if let Some(previous) = self
            .tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unfinished
            .get(&journal.request.operation_id)
        {
            journal.cancel_requested |= previous.cancel_requested;
        }
        tokio::task::spawn_blocking(move || {
            let _save_guard = save_guard;
            let mut completion = None;
            if journal.phase == "complete"
                && journal.result.as_ref().is_some_and(|v| v["mode"] == "move")
            {
                let mut position = completed_moves.lock().unwrap_or_else(|e| e.into_inner());
                let links = links.lock().unwrap_or_else(|e| e.into_inner());
                journal.move_seq = match links
                    .get(&journal.request.operation_id)
                    .filter(|record| record.phase == "complete")
                {
                    // Keep a legacy receipt in its original order when hydrated.
                    Some(record) => record.move_seq,
                    None => {
                        *position += 1;
                        Some(*position)
                    }
                };
                completion = Some(position);
            }
            let raw = serde_json::to_vec(&journal)?;
            let temp = path.with_extension("tmp");
            let mut options = fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut out = options.open(&temp)?;
            out.write_all(&raw)?;
            out.sync_all()?;
            {
                // Publish disk and memory together, even if the async caller was
                // cancelled. After rename, update memory even if directory fsync
                // fails: readers already see the new journal on disk.
                let mut tasks = tasks.lock().unwrap_or_else(|e| e.into_inner());
                fs::rename(temp, &path)?;
                let record = Self::link_record(&journal);
                links
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(record.id.clone(), record);
                tasks.update(journal);
            }
            fs::File::open(path.parent().unwrap())?.sync_all()?;
            drop(completion);
            Ok::<_, TransferError>(())
        })
        .await
        .map_err(|e| TransferError::new("move_io", e.to_string()))?
    }
    fn link_record(journal: &Journal) -> crate::session_links::Record {
        let (source_node, local_uid) =
            namespace::split(&journal.request.uid, true).unwrap_or_default();
        let result = journal.result.as_ref().unwrap_or(&Value::Null);
        let preview = journal.preview.as_ref().unwrap_or(&Value::Null);
        let mappings = serde_json::from_value(result["link_map"].clone()).unwrap_or_default();
        let mut record = crate::session_links::Record {
            id: journal.request.operation_id.clone(),
            created_ms: journal.created_ms,
            move_seq: journal.move_seq,
            mode: result["mode"]
                .as_str()
                .or(preview["mode"].as_str())
                .unwrap_or("clone")
                .into(),
            phase: journal.phase.clone(),
            source_node,
            target_node: journal.request.target_node.clone(),
            mappings,
            legacy: !result["link_map"].is_object(),
            ..Default::default()
        };
        for member in preview["sessions"].as_array().into_iter().flatten() {
            record.legacy_members.insert(format!(
                "{}:{}",
                member["source"].as_str().unwrap_or(""),
                member["sid"].as_str().unwrap_or("")
            ));
            if let Some(uid) = member["uid"].as_str() {
                record.legacy_members.insert(
                    namespace::split(uid, true)
                        .map(|(_, u)| u)
                        .unwrap_or_else(|_| uid.into()),
                );
            }
        }
        if record.legacy
            && let Some(target) = result["target_uid"].as_str()
        {
            let target = namespace::split(target, true)
                .map(|(_, u)| u)
                .unwrap_or_else(|_| target.into());
            record.mappings.insert(local_uid, target.clone());
            for member in preview["sessions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["uid"] == journal.request.uid)
            {
                record.mappings.insert(
                    format!(
                        "{}:{}",
                        member["source"].as_str().unwrap_or(""),
                        member["sid"].as_str().unwrap_or("")
                    ),
                    target.clone(),
                );
            }
        }
        record
    }
    pub fn link_records(&self) -> Vec<crate::session_links::Record> {
        self.links
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }
    pub async fn hydrate_links(
        &self,
        registry: &Registry,
        client: &Client,
        id: &str,
    ) -> Option<crate::session_links::Record> {
        let _guard = self.gates.acquire(vec![id.to_owned()]).await;
        let mut journal = self.load(id).await.ok()?;
        if journal.phase != "complete" {
            return None;
        }
        if journal.created_ms == 0 {
            journal.created_ms = self
                .links
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(id)
                .map(|r| r.created_ms)
                .unwrap_or_default();
        }
        let record = Self::link_record(&journal);
        if !record.legacy {
            return Some(record);
        }
        let mut queried = std::collections::BTreeSet::new();
        for nid in [&record.target_node, &record.source_node] {
            if !queried.insert(nid) {
                continue;
            }
            let Some(node) = registry.get(nid) else {
                continue;
            };
            let Ok((200, value)) = registry
                .request(
                    client,
                    &node,
                    "/api/session/transfer/status",
                    "POST",
                    Some(&json!({"operation_id":id})),
                    Duration::from_secs(5),
                )
                .await
            else {
                continue;
            };
            if value["operation_id"] == id && value["link_map"].is_object() {
                journal.result.as_mut()?["link_map"] = value["link_map"].clone();
                if journal.created_ms == 0 {
                    journal.created_ms = value["created_ms"].as_u64().unwrap_or(record.created_ms);
                }
                self.save(&journal).await.ok()?;
                return Some(Self::link_record(&journal));
            }
        }
        None
    }
    fn public(journal: &Journal) -> Value {
        json!({"request":journal.request,"phase":journal.phase,"plan":journal.preview,"created_ms":journal.created_ms,
            "background":journal.execution_requested,"cancel_requested":journal.cancel_requested,
            "bytes_sent":journal.bytes_sent,"bytes_total":journal.bytes_total,
            "error":journal.error,"code":journal.error_code,"result":journal.result,"transport":journal.transport})
    }
    /// Startup recovery and successful saves keep this view current without
    /// reopening completed journals or waiting for the operation's mutation gate.
    pub fn tasks(&self) -> Value {
        let tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        let operations: Vec<_> = tasks.unfinished.values().map(Self::public).collect();
        let recent: Vec<_> = tasks.recent.values().rev().collect();
        json!({"operations":operations,"recent":recent})
    }
    pub fn frozen(&self) -> Value {
        let tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        let mut uids = std::collections::BTreeSet::new();
        for journal in tasks.unfinished.values().filter(|j| j.execution_requested) {
            for member in journal
                .preview
                .as_ref()
                .and_then(|p| p["sessions"].as_array())
                .into_iter()
                .flatten()
            {
                if let Some(uid) = member["uid"].as_str() {
                    uids.insert(uid.to_owned());
                    if let Ok((_, local)) = namespace::split(uid, true)
                        && let Some((kind, id)) = local.split_once(':')
                    {
                        uids.insert(format!("{kind}:{}~{id}", journal.request.target_node));
                    }
                }
            }
        }
        json!({"uids":uids})
    }
    pub async fn progress(
        &self,
        registry: &Registry,
        client: &Client,
        request: &Request,
    ) -> Result<Value, TransferError> {
        let cached = self
            .tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unfinished
            .get(&request.operation_id)
            .cloned();
        let mut journal: Journal = match cached {
            Some(journal) => journal,
            None => serde_json::from_slice(&fs::read(self.path(&request.operation_id)?)?)?,
        };
        if !journal.request.same_transfer(request) {
            return Err(TransferError::new(
                "move_conflict",
                "操作已绑定其他迁移目标",
            ));
        }
        // Journals written by older versions have no preview. Fetch only this
        // selected operation; listing tasks must work even when a node is offline.
        if journal.preview.is_none() {
            let (source_id, _) = namespace::split(&request.uid, true)
                .map_err(|e| TransferError::new("move_plan_stale", e.to_string()))?;
            let source = registry
                .get(&source_id)
                .ok_or_else(|| TransferError::new("move_node_unavailable", "源机器不可用"))?;
            let address = registry
                .target(&source)
                .map_err(|_| TransferError::new("move_node_unavailable", "源机器不可用"))?;
            let state = call(
                client,
                &address,
                "/api/session/transfer/status",
                &json!({"operation_id":request.operation_id}),
                true,
            )
            .await?
            .1;
            journal.preview = Some(namespace::public_payload(
                state,
                &source,
                "/api/session/clone",
            ));
        }
        let (source_id, local_uid) = namespace::split(&request.uid, true)
            .map_err(|e| TransferError::new("move_plan_stale", e.to_string()))?;
        let mut work = self.work.snapshot(&request.operation_id);
        if !matches!(journal.phase.as_str(), "complete" | "aborted")
            && (source_id == request.target_node || work.as_array().is_none_or(Vec::is_empty))
        {
            // One small, lock-free-of-execution probe; never reread the native journal.
            // Bound only the optional telemetry wait, not the transfer itself.
            let node_id = if matches!(
                journal.phase.as_str(),
                "checking" | "publishing" | "verifying"
            ) || journal.phase == "transferring"
                && (journal.bytes_total > 0
                    && journal
                        .transport
                        .as_ref()
                        .is_some_and(|r| r.method == "scp")
                    || journal.bytes_total > 0 && journal.bytes_sent >= journal.bytes_total)
            {
                &request.target_node
            } else {
                &source_id
            };
            if let Some(node) = registry.get(node_id)
                && let Ok(address) = registry.target(&node)
                && let Ok(Ok((_, status))) = tokio::time::timeout(
                    Duration::from_millis(750),
                    call(
                        client,
                        &address,
                        "/api/session/transfer/work",
                        &json!({"operation_id":request.operation_id}),
                        true,
                    ),
                )
                .await
            {
                if work.as_array().is_none_or(Vec::is_empty) {
                    work = status["work"].clone();
                }
                if source_id == request.target_node && status["uid"] == local_uid {
                    if let Some(phase) = status["phase"].as_str() {
                        journal.phase = phase.into();
                    }
                    if let Some(error) = status["error"].as_str() {
                        journal.error = Some(error.into());
                    }
                }
            }
        }
        let mut public = Self::public(&journal);
        public["work"] = work;
        Ok(public)
    }
    async fn record_error(&self, request: &Request, error: &TransferError) {
        if let Ok(mut journal) = self.load(&request.operation_id).await
            && journal.request.same_transfer(request)
        {
            journal.error = Some(error.message.clone());
            journal.error_code = Some(error.code.clone());
            let _ = self.save(&journal).await;
        }
    }
    pub async fn cancel(
        &self,
        registry: Arc<Registry>,
        client: Arc<Client>,
        request: Request,
    ) -> Result<Value, TransferError> {
        self.cancel_inner(registry, client, request, false).await
    }
    async fn cancel_inner(
        &self,
        registry: Arc<Registry>,
        client: Arc<Client>,
        request: Request,
        recovery: bool,
    ) -> Result<Value, TransferError> {
        let mut journal = self.load(&request.operation_id).await?;
        if !journal.request.same_transfer(&request) {
            return Err(TransferError::new(
                "move_conflict",
                "操作已绑定其他迁移目标",
            ));
        }
        if matches!(journal.phase.as_str(), "complete" | "aborted") {
            return Ok(journal.result.unwrap_or_else(|| json!({"phase":"aborted"})));
        }
        if !recovery {
            journal.cancel_requested = true;
            self.save(&journal).await?;
        }
        // Validate binding above, then interrupt preparation before taking the
        // operation lock. Never wait behind an unrelated transfer.
        {
            // Recheck ownership after loading the journal. Only abandoned workers
            // are recovered; browser polling is not an execution lease.
            let executions = self.executions.lock().unwrap_or_else(|e| e.into_inner());
            if recovery
                && executions
                    .get(&request.operation_id)
                    .is_some_and(|worker| worker.strong_count() > 0)
            {
                return Ok(Self::public(&journal));
            }
            if let Some(token) = self
                .cancellations
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&request.operation_id)
            {
                token.cancel();
            }
        }
        let (source_id, _) = namespace::split(&request.uid, true)
            .map_err(|e| TransferError::new("move_plan_stale", e.to_string()))?;
        for id in [&source_id, &request.target_node] {
            if let Some(node) = registry.get(id)
                && let Ok(target) = registry.target(&node)
            {
                let _ = tokio::time::timeout(
                    Duration::from_secs(3),
                    call(
                        &client,
                        &target,
                        "/api/session/transfer/interrupt",
                        &json!({"operation_id":request.operation_id}),
                        false,
                    ),
                )
                .await;
            }
        }
        let _guard = self.gates.acquire(vec![request.operation_id.clone()]).await;
        journal = self.load(&request.operation_id).await?;
        let result = self.reconcile(registry, client, &mut journal).await;
        if let Err(error) = &result {
            self.record_error(&request, error).await;
        }
        result
    }
    async fn reconcile(
        &self,
        registry: Arc<Registry>,
        client: Arc<Client>,
        journal: &mut Journal,
    ) -> Result<Value, TransferError> {
        if matches!(journal.phase.as_str(), "complete" | "aborted") {
            return Ok(journal
                .result
                .clone()
                .unwrap_or_else(|| json!({"phase":"aborted"})));
        }
        let (source_id, _) = namespace::split(&journal.request.uid, true)
            .map_err(|e| TransferError::new("move_plan_stale", e.to_string()))?;
        let source = registry
            .get(&source_id)
            .ok_or_else(|| TransferError::new("move_node_unavailable", "源机器不可用"))?;
        let address = registry
            .target(&source)
            .map_err(|_| TransferError::new("move_node_unavailable", "源机器不可用"))?;
        let state = call(
            &client,
            &address,
            "/api/session/transfer/status",
            &json!({"operation_id":journal.request.operation_id}),
            true,
        )
        .await?
        .1;
        if matches!(
            state["phase"].as_str(),
            Some("moved" | "retiring" | "retired" | "exported" | "complete")
        ) {
            return self.run(registry, client, journal.request.clone()).await;
        }
        self.abort(&registry, &client, journal).await
    }
    async fn abort(
        &self,
        registry: &Registry,
        client: &Client,
        journal: &mut Journal,
    ) -> Result<Value, TransferError> {
        if journal.phase == "aborted" {
            return Ok(json!({"phase":"aborted"}));
        }
        let (source_id, local_uid) = namespace::split(&journal.request.uid, true)
            .map_err(|e| TransferError::new("move_plan_stale", e.to_string()))?;
        let address = |id: &str| {
            let node = registry
                .get(id)
                .ok_or_else(|| TransferError::new("move_node_unavailable", "迁移节点不可用"))?;
            registry
                .target(&node)
                .map_err(|_| TransferError::new("move_node_unavailable", "迁移节点不可用"))
        };
        let source = address(&source_id)?;
        let target = address(&journal.request.target_node)?;
        let id = journal.request.operation_id.clone();
        let source_state = call(
            client,
            &source,
            "/api/session/transfer/status",
            &json!({"operation_id":id}),
            true,
        )
        .await?
        .1;
        if source_state["uid"] != local_uid {
            return Err(TransferError::new("move_plan_stale", "操作与源会话不符"));
        }
        if source_id == journal.request.target_node {
            let result = call(
                client,
                &source,
                "/api/session/transfer/abort",
                &json!({"operation_id":id,"step":"local"}),
                true,
            )
            .await?
            .1;
            journal.phase = result["phase"].as_str().unwrap_or("aborted").into();
            if journal.phase == "complete" {
                journal.result = Some(namespace::public_payload(
                    result.clone(),
                    &registry.get(&source_id).unwrap(),
                    "/api/session/clone",
                ));
            }
            self.save(journal).await?;
            return Ok(journal.result.clone().unwrap_or(result));
        }
        // A clone publication that won the race with cancellation is committed.
        // Never delete a complete target which may already be in use.
        if source_state["mode"] == "clone" {
            let mut received = call(
                client,
                &target,
                "/api/session/transfer/status",
                &json!({"operation_id":id}),
                false,
            )
            .await?;
            if received.0 != 404 {
                received = call(
                    client,
                    &target,
                    "/api/session/transfer/abort",
                    &json!({"operation_id":id,"step":"target"}),
                    true,
                )
                .await?;
            }
            if received.0 != 404 && received.1["phase"] == "complete" {
                let mut result = namespace::public_payload(
                    received.1,
                    &registry.get(&journal.request.target_node).unwrap(),
                    "/api/session/clone",
                );
                result["uid"] = journal.request.uid.clone().into();
                call(
                    client,
                    &source,
                    "/api/session/transfer/release",
                    &json!({"operation_id":id,"completed":true}),
                    true,
                )
                .await?;
                journal.phase = "complete".into();
                journal.result = Some(result.clone());
                self.save(journal).await?;
                return Ok(result);
            }
        }
        // Source-first is essential: a delayed switch must be durably rejected
        // before removing any verified target file. A lost reply is retryable.
        call(
            client,
            &source,
            "/api/session/transfer/abort",
            &json!({"operation_id":id,"step":"source"}),
            true,
        )
        .await?;
        journal.phase = "aborting".into();
        self.save(journal).await?;
        let received = call(
            client,
            &target,
            "/api/session/transfer/status",
            &json!({"operation_id":id}),
            false,
        )
        .await?;
        if received.0 != 404 {
            call(
                client,
                &target,
                "/api/session/transfer/abort",
                &json!({"operation_id":id,"step":"target"}),
                true,
            )
            .await?;
        }
        call(
            client,
            &source,
            "/api/session/transfer/abort",
            &json!({"operation_id":id,"step":"finish"}),
            true,
        )
        .await?;
        journal.phase = "aborted".into();
        self.freeze(registry, client, journal, true).await?;
        self.save(journal).await?;
        Ok(json!({"phase":"aborted"}))
    }
    /// Persist acceptance before replying. The scheduler owns all later work;
    /// neither this request nor status readers are an execution lease.
    pub async fn start(
        &self,
        registry: Arc<Registry>,
        client: Arc<Client>,
        mut request: Request,
    ) -> Result<Value, TransferError> {
        if let Some(journal) = self
            .tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unfinished
            .get(&request.operation_id)
            .cloned()
        {
            if request.mode.is_none() {
                request.mode = journal.request.mode.clone();
            }
            if request.new_ids.is_none() {
                request.new_ids = journal.request.new_ids;
            }
            if journal.request != request {
                return Err(TransferError::new(
                    "move_conflict",
                    "操作已绑定其他迁移选项",
                ));
            }
            if journal.execution_requested && journal.frozen_ready {
                return Ok(Self::public(&journal));
            }
        }
        let _guard = self.gates.acquire(vec![request.operation_id.clone()]).await;
        let path = self.path(&request.operation_id)?;
        namespace::split(&request.uid, true)
            .map_err(|e| TransferError::new("move_plan_stale", e.to_string()))?;
        let mut journal = if path.exists() {
            let previous = self.load(&request.operation_id).await?;
            if request.mode.is_none() {
                request.mode = previous.request.mode.clone();
            }
            if request.new_ids.is_none() {
                request.new_ids = previous.request.new_ids;
            }
            if previous.request != request {
                return Err(TransferError::new(
                    "move_conflict",
                    "操作已绑定其他迁移选项",
                ));
            }
            previous
        } else {
            Journal {
                execution_requested: true,
                cancel_requested: false,
                frozen_ready: false,
                created_ms: crate::session_links::now_ms(),
                move_seq: None,
                request,
                phase: "planned".into(),
                result: None,
                preview: None,
                bytes_sent: 0,
                bytes_total: 0,
                error: None,
                error_code: None,
                transport: None,
            }
        };
        journal.execution_requested = true;
        self.save(&journal).await?;
        if !matches!(journal.phase.as_str(), "complete" | "aborted") && !journal.cancel_requested {
            self.freeze(&registry, &client, &mut journal, false).await?;
        }
        Ok(Self::public(&journal))
    }
    async fn freeze(
        &self,
        registry: &Registry,
        client: &Client,
        journal: &mut Journal,
        release: bool,
    ) -> Result<(), TransferError> {
        let (source_id, _) = namespace::split(&journal.request.uid, true)
            .map_err(|e| TransferError::new("move_plan_stale", e.to_string()))?;
        let mut uids = Value::Null;
        for (index, id) in [&source_id, &journal.request.target_node]
            .into_iter()
            .enumerate()
        {
            if index == 1 && id == &source_id {
                continue;
            }
            let node = registry
                .get(id)
                .ok_or_else(|| TransferError::new("move_node_unavailable", "迁移节点不可用"))?;
            let target = registry
                .target(&node)
                .map_err(|_| TransferError::new("move_node_unavailable", "迁移节点不可用"))?;
            let mut body = json!({"operation_id":journal.request.operation_id,"release":release});
            if index == 0 && !release {
                body["uid"] = json!(namespace::split(&journal.request.uid, true).unwrap().1);
                body["target_node"] = json!(journal.request.target_node);
            }
            if !uids.is_null() {
                body["uids"] = uids.clone();
            }
            let (_, preview) =
                call(client, &target, "/api/session/transfer/freeze", &body, true).await?;
            if id == &source_id && !release {
                if preview["uid"] != namespace::split(&journal.request.uid, true).unwrap().1 {
                    return Err(TransferError::new("move_plan_stale", "操作与源会话不符"));
                }
                uids = json!(
                    preview["sessions"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|m| m["uid"].as_str())
                        .collect::<Vec<_>>()
                );
                journal.preview = Some(namespace::public_payload(
                    preview,
                    &node,
                    "/api/session/clone",
                ));
                self.save(journal).await?;
            }
        }
        journal.frozen_ready = !release;
        self.save(journal).await?;
        Ok(())
    }
    pub async fn execute(
        &self,
        registry: Arc<Registry>,
        client: Arc<Client>,
        mut request: Request,
    ) -> Result<Value, TransferError> {
        let _guard = self.gates.acquire(vec![request.operation_id.clone()]).await;
        // Older task drawers omit confirmation options. Reuse the durable
        // choice; a conflicting retry must not cancel the original operation.
        if self.path(&request.operation_id)?.exists() {
            let previous = self.load(&request.operation_id).await?;
            if request.mode.is_none() {
                request.mode = previous.request.mode.clone();
            }
            if request.new_ids.is_none() {
                request.new_ids = previous.request.new_ids;
            }
            if previous.request != request {
                return Err(TransferError::new(
                    "move_conflict",
                    "操作已绑定其他迁移选项",
                ));
            }
            if previous.cancel_requested {
                let mut previous = previous;
                return self.reconcile(registry, client, &mut previous).await;
            }
        }
        let cancellation = tokio_util::sync::CancellationToken::new();
        let execution = Arc::new(());
        {
            let mut executions = self.executions.lock().unwrap_or_else(|e| e.into_inner());
            executions.insert(request.operation_id.clone(), Arc::downgrade(&execution));
            self.cancellations
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(request.operation_id.clone(), cancellation.clone());
        }
        // Cancellation may have been persisted between the earlier journal
        // read and registering this token. Close that admission race.
        if let Ok(journal) = self.load(&request.operation_id).await
            && journal.cancel_requested
        {
            cancellation.cancel();
        }
        let result = tokio::select! {
            result = self.run(registry.clone(), client.clone(), request.clone()) => result,
            _ = cancellation.cancelled() => Err(TransferError::new("move_cancelled", "正在取消操作")),
        };
        self.cancellations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&request.operation_id);
        self.executions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&request.operation_id);
        if let Err(error) = &result {
            self.record_error(&request, error).await;
            // Lost connections and post-handoff cleanup are retried by the
            // scheduler with the same operation, without browser resubmission.
            if matches!(
                error.code.as_str(),
                "move_node_unavailable" | "move_cleanup_pending"
            ) {
                return result;
            }
            // Release the operation gate before the shared cancellation path.
            // It interrupts node workers before compensating their publications.
            drop(_guard);
            if let Ok(value) = self.cancel(registry, client, request.clone()).await
                && value["phase"] == "complete"
            {
                return Ok(value);
            }
        }
        result
    }
    async fn run(
        &self,
        registry: Arc<Registry>,
        client: Arc<Client>,
        request: Request,
    ) -> Result<Value, TransferError> {
        let live = self.work.start(&request.operation_id);
        let path = self.path(&request.operation_id)?;
        let mut journal = if path.exists() {
            let previous = self.load(&request.operation_id).await?;
            if previous.request != request {
                return Err(TransferError::new(
                    "move_conflict",
                    "操作已绑定其他迁移目标",
                ));
            }
            previous
        } else {
            Journal {
                execution_requested: true,
                cancel_requested: false,
                frozen_ready: false,
                created_ms: crate::session_links::now_ms(),
                move_seq: None,
                request: request.clone(),
                phase: "planned".into(),
                result: None,
                preview: None,
                bytes_sent: 0,
                bytes_total: 0,
                error: None,
                error_code: None,
                transport: None,
            }
        };
        if journal.phase == "complete" {
            return journal
                .result
                .ok_or_else(|| TransferError::new("move_recovery_required", "迁移结果缺失"));
        }
        let (source_id, local_uid) = namespace::split(&request.uid, true)
            .map_err(|e| TransferError::new("move_plan_stale", e.to_string()))?;
        let source = registry
            .get(&source_id)
            .ok_or_else(|| TransferError::new("move_node_unavailable", "源机器不可用"))?;
        let target = registry
            .get(&request.target_node)
            .ok_or_else(|| TransferError::new("move_node_unavailable", "目标机器不可用"))?;
        let source_address = registry
            .target(&source)
            .map_err(|_| TransferError::new("move_node_unavailable", "源机器不可用"))?;
        let target_address = registry
            .target(&target)
            .map_err(|_| TransferError::new("move_node_unavailable", "目标机器不可用"))?;
        journal.error = None;
        journal.error_code = None;
        if source_id != request.target_node {
            self.save(&journal).await?;
        }
        // Reset only when explicitly starting/retrying an operation, before
        // any preparatory work is dispatched to either node.
        let reset = json!({"operation_id":request.operation_id,"reset":true});
        // Both resets finish before preparation; neither depends on the other.
        let mut source_reset = reset.clone();
        if let Some(mode) = &request.mode {
            source_reset["mode"] = mode.clone().into();
            source_reset["uid"] = local_uid.clone().into();
            source_reset["new_ids"] = request.new_ids.into();
        }
        tokio::try_join!(
            call(
                &client,
                &source_address,
                "/api/session/transfer/interrupt",
                &source_reset,
                false
            ),
            call(
                &client,
                &target_address,
                "/api/session/transfer/interrupt",
                &reset,
                false
            ),
        )?;
        let operation = json!({"operation_id":request.operation_id});
        let source_state = call(
            &client,
            &source_address,
            "/api/session/transfer/status",
            &operation,
            true,
        )
        .await?
        .1;
        if matches!(journal.phase.as_str(), "aborting" | "aborted")
            || matches!(source_state["phase"].as_str(), Some("aborting" | "aborted"))
        {
            self.abort(&registry, &client, &mut journal).await?;
            return Err(TransferError::new(
                "move_cancelled",
                "本次移动已撤回，请重新查看清单",
            ));
        }
        if source_state["uid"] != local_uid {
            return Err(TransferError::new("move_plan_stale", "操作与源会话不符"));
        }
        if source_state["selection"] == true && source_state["target_node"] != request.target_node {
            return Err(TransferError::new(
                "move_plan_stale",
                "目标机器与已确认的迁移选择不一致",
            ));
        }
        if request
            .mode
            .as_ref()
            .is_some_and(|mode| source_state["mode"] != *mode)
            || request
                .new_ids
                .is_some_and(|new_ids| source_state["new_ids"] != new_ids)
        {
            return Err(TransferError::new(
                "move_plan_stale",
                "源节点未接受确认选项，请重新查看清单",
            ));
        }
        journal.preview = Some(namespace::public_payload(
            source_state.clone(),
            &source,
            "/api/session/clone",
        ));
        self.save(&journal).await?;
        if source_id == request.target_node {
            if source_state["mode"] != "clone"
                || source_state["new_ids"] != true
                || source_state["incoming"] == true
            {
                return Err(TransferError::new(
                    "move_conflict",
                    "同机操作只支持生成新身份的复制",
                ));
            }
            journal.phase = "publishing".into();
            self.save(&journal).await?;
            let result = call(
                &client,
                &source_address,
                "/api/session/clone",
                &json!({"uid":local_uid,"operation_id":request.operation_id}),
                true,
            )
            .await?
            .1;
            if result["phase"] != "complete" || !result["target_uid"].is_string() {
                return Err(TransferError::new(
                    "move_recovery_required",
                    "本机复制结果尚未确认",
                ));
            }
            let result = namespace::public_payload(result, &source, "/api/session/clone");
            journal.phase = "complete".into();
            journal.result = Some(result.clone());
            self.save(&journal).await?;
            return Ok(result);
        }
        let moving = source_state["mode"] == "move";
        if moving
            && matches!(
                source_state["phase"].as_str(),
                Some("planned" | "exporting")
            )
        {
            self.freeze(&registry, &client, &mut journal, false).await?;
        }
        let mut current = call(
            &client,
            &target_address,
            "/api/session/transfer/status",
            &operation,
            false,
        )
        .await?;
        if current.0 != 404 && current.1["incoming"] != true {
            return Err(TransferError::new(
                "move_conflict",
                "目标操作并非迁移接收记录",
            ));
        }
        if current.1["phase"] != "complete" && current.1["phase"] != "ready" {
            let reserve_manifest = current.0 == 404 && source_state["reserve_manifest"] == true;
            if !reserve_manifest {
                let reserved = call(
                    &client,
                    &source_address,
                    "/api/session/transfer/reserve",
                    &operation,
                    true,
                )
                .await?
                .1;
                if reserved["uid"] != local_uid {
                    return Err(TransferError::new(
                        "move_plan_stale",
                        "迁移操作与所选会话不符",
                    ));
                }
            }
            if current.0 == 404 {
                journal.phase = "preparing".into();
                self.save(&journal).await?;
                let checked = self
                    .check_manifest(
                        &client,
                        &source_address,
                        &target_address,
                        &json!({"operation_id":request.operation_id,"reserve":reserve_manifest}),
                        &mut journal,
                        &live,
                    )
                    .await?;
                if source_state["selection"] == true && checked.1["selected_move"] != true {
                    return Err(TransferError::new(
                        "move_group_unsupported",
                        "目标版本尚不支持选择迁移，请更新节点",
                    ));
                }
                if moving && checked.1["move_handoff"] != true {
                    return Err(TransferError::new(
                        "move_group_unsupported",
                        "目标版本不支持迁移归属交接",
                    ));
                }
                journal.phase = "transferring".into();
                self.save(&journal).await?;
                self.transport(
                    &client,
                    &source_address,
                    &target_address,
                    source_state["scp_transfer"] == true && checked.1["scp_transfer"] == true,
                    &operation,
                    &mut journal,
                )
                .await?;
            }
            // Recheck stopped state and the source snapshot immediately before
            // target publication. Source lease fences managed launches throughout.
            journal.phase = "rechecking".into();
            self.save(&journal).await?;
            call(
                &client,
                &source_address,
                "/api/session/transfer/reserve",
                &operation,
                true,
            )
            .await?;
            journal.phase = "publishing".into();
            self.save(&journal).await?;
            current = call(
                &client,
                &target_address,
                "/api/session/clone",
                &json!({"uid":local_uid,"operation_id":request.operation_id}),
                true,
            )
            .await?;
        }
        let mut retired = false;
        if moving && current.1["phase"] == "ready" {
            journal.phase = "switching".into();
            self.save(&journal).await?;
            if let Err(error) = call(
                &client,
                &source_address,
                "/api/session/transfer/switch",
                &operation,
                true,
            )
            .await
            {
                if error.code == "move_plan_stale" {
                    self.abort(&registry, &client, &mut journal).await?;
                    return Err(TransferError::new(
                        "move_cancelled",
                        "源会话已变化，本次移动已撤回，请重新查看清单",
                    ));
                }
                return Err(error);
            }
            journal.phase = "retiring".into();
            self.save(&journal).await?;
            if let Err(error) = call(
                &client,
                &source_address,
                "/api/session/transfer/retire",
                &operation,
                true,
            )
            .await
            {
                journal.phase = "cleanup_pending".into();
                self.save(&journal).await?;
                return Err(TransferError::new(
                    "move_cleanup_pending",
                    format!("移动已提交，服务端正在重试源端清理：{}", error.message),
                ));
            }
            retired = true;
            current = call(
                &client,
                &target_address,
                "/api/session/transfer/activate",
                &operation,
                true,
            )
            .await?;
        }
        if current.1["phase"] != "complete" {
            return Err(TransferError::new(
                "move_recovery_required",
                "目标迁移尚未完成",
            ));
        }
        let mut result = namespace::public_payload(current.1, &target, "/api/session/clone");
        result["uid"] = request.uid.clone().into();
        journal.phase = "releasing".into();
        journal.result = Some(result.clone());
        self.save(&journal).await?;
        if moving && !retired {
            journal.phase = "retiring".into();
            self.save(&journal).await?;
            if let Err(error) = call(
                &client,
                &source_address,
                "/api/session/transfer/retire",
                &operation,
                true,
            )
            .await
            {
                journal.phase = "cleanup_pending".into();
                self.save(&journal).await?;
                return Err(TransferError::new(
                    "move_cleanup_pending",
                    format!("目标已可继续；源端清理待重试：{}", error.message),
                ));
            }
        } else if !moving {
            call(
                &client,
                &source_address,
                "/api/session/transfer/release",
                &json!({"operation_id":request.operation_id,"completed":true}),
                true,
            )
            .await?;
        }
        journal.phase = "complete".into();
        self.freeze(&registry, &client, &mut journal, true).await?;
        self.save(&journal).await?;
        Ok(result)
    }
    async fn stream(
        &self,
        client: &Client,
        source: &Target,
        target: &Target,
        operation: &Value,
        journal: &mut Journal,
    ) -> Result<(), TransferError> {
        let encoded = serde_json::to_vec(operation)?;
        let response = client
            .open(
                source,
                client::Request {
                    method: "POST",
                    target: "/api/session/transfer/export",
                    headers: &[("Content-Type", "application/json")],
                    body: Some(&encoded),
                    connect: Duration::from_secs(10),
                    idle: IDLE,
                },
            )
            .await
            .map_err(network)?;
        if response.status != 200 {
            let raw = response
                .into_body()
                .read_to_end(super::JSON_LIMIT)
                .await
                .map_err(network)?;
            return Err(remote_error(&serde_json::from_slice(&raw)?));
        }
        let length = response
            .header("content-length")
            .ok_or_else(|| TransferError::new("move_format", "迁移包缺少长度"))?
            .to_owned();
        journal.bytes_total = length
            .parse()
            .map_err(|_| TransferError::new("move_format", "迁移包长度无效"))?;
        journal.bytes_sent = 0;
        self.save(journal).await?;
        let headers = [
            ("Content-Type", "application/x-tar"),
            ("Content-Length", length.as_str()),
        ];
        let mut upload = client
            .connect(
                target,
                &client::Request {
                    method: "POST",
                    target: "/api/session/transfer/receive",
                    headers: &headers,
                    body: None,
                    connect: Duration::from_secs(10),
                    idle: IDLE,
                },
            )
            .await
            .map_err(network)?;
        let live = self.work.start(&journal.request.operation_id);
        let progress = live.task("传输会话包", "bytes", Some(journal.bytes_total));
        let mut body = response.into_body();
        let mut last_save = std::time::Instant::now();
        while let Some(bytes) = body.read().await.map_err(network)? {
            upload.send(&bytes).await.map_err(network)?;
            journal.bytes_sent += bytes.len() as u64;
            progress.set(journal.bytes_sent);
            if last_save.elapsed() >= Duration::from_secs(1) {
                self.save(journal).await?;
                last_save = std::time::Instant::now();
            }
        }
        self.save(journal).await?;
        drop(progress);
        let received = upload.response().await.map_err(network)?;
        let status = received.status;
        let raw = received
            .into_body()
            .read_to_end(super::JSON_LIMIT)
            .await
            .map_err(network)?;
        let result: Value = serde_json::from_slice(&raw)?;
        if status != 200 {
            return Err(remote_error(&result));
        }
        Ok(())
    }
}
fn network(error: super::ClientError) -> TransferError {
    TransferError::new("move_node_unavailable", error.message(IDLE))
}
fn remote_error(value: &Value) -> TransferError {
    TransferError::new(
        value["code"].as_str().unwrap_or("move_io"),
        value["error"].as_str().unwrap_or("节点迁移操作失败"),
    )
}
async fn call(
    client: &Client,
    target: &Target,
    path: &str,
    body: &Value,
    success: bool,
) -> Result<(u16, Value), TransferError> {
    call_work(client, target, path, body, success, None).await
}
async fn call_work(
    client: &Client,
    target: &Target,
    path: &str,
    body: &Value,
    success: bool,
    work: Option<&Arc<crate::transfer::progress::Work>>,
) -> Result<(u16, Value), TransferError> {
    // Transfer manifests carry complete native database before/after images.
    // They are not ordinary catalog replies and can exceed the client's 64 MiB
    // JSON cap. Keep HTTP framing, authentication and idle timeouts unchanged.
    let failed = |error: super::ClientError| {
        crate::log::warn(
            "transfer.request_failed",
            serde_json::json!({"path": path, "error": error.to_string()}),
        );
        network(error)
    };
    let encoded = if let Some(work) = work {
        let mut writer = std::io::BufWriter::with_capacity(
            65536,
            crate::transfer::progress::Io(Vec::new(), work.task("编码迁移清单", "bytes", None)),
        );
        serde_json::to_writer(&mut writer, body)?;
        writer.flush()?;
        writer
            .into_inner()
            .map_err(|e| TransferError::new("move_io", e.to_string()))?
            .0
    } else {
        serde_json::to_vec(body)?
    };
    let length = encoded.len().to_string();
    let headers = [
        ("Content-Type", "application/json"),
        ("Content-Length", length.as_str()),
    ];
    let mut upload = client
        .connect(
            target,
            &client::Request {
                method: "POST",
                target: path,
                headers: &headers,
                body: None,
                connect: IDLE,
                idle: IDLE,
            },
        )
        .await
        .map_err(failed)?;
    {
        let sent = work.map(|work| work.task("发送迁移清单", "bytes", Some(encoded.len() as u64)));
        for chunk in encoded.chunks(65536) {
            upload.send(chunk).await.map_err(failed)?;
            if let Some(sent) = &sent {
                sent.add(chunk.len() as u64);
            }
        }
    }
    let response = upload.response().await.map_err(failed)?;
    let received = work.map(|work| {
        work.task(
            "接收迁移清单",
            "bytes",
            response
                .header("content-length")
                .and_then(|s| s.parse().ok()),
        )
    });
    read_reply(response, success, received).await
}

async fn read_reply(
    response: client::Response,
    success: bool,
    received: Option<crate::transfer::progress::Task>,
) -> Result<(u16, Value), TransferError> {
    let failed = network;
    let status = response.status;
    let mut response_body = response.into_body();
    let mut raw = Vec::new();
    while let Some(chunk) = response_body.read().await.map_err(failed)? {
        raw.extend_from_slice(&chunk);
        if let Some(received) = &received {
            received.add(chunk.len() as u64);
        }
    }
    drop(received);
    let value = serde_json::from_slice(&raw)
        .map_err(|_| failed(super::ClientError::Invalid("body is not JSON")))?;
    if status != 200 && (success || status != 404) {
        return Err(remote_error(&value));
    }
    Ok((status, value))
}
