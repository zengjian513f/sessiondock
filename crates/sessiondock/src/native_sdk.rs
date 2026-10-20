//! Shared SDK dispatch and private creation receipts. Each provider owns its
//! native transport; this module never writes native session records.
#[path = "native_sdk_queue.rs"]
mod input_queue;
pub(crate) use input_queue::Input as QueuedInput;

use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

enum Provider {
    Grok(Arc<crate::grok_sdk::Bridge>),
    Agy(Arc<crate::agy_sdk::Bridge>),
    Kiro(Arc<crate::kiro_sdk::Bridge>),
    OpenCode(Arc<crate::opencode_sdk::Bridge>),
    Claude(Arc<crate::claude_sdk::Bridge>),
}
macro_rules! dispatch {
    ($provider:expr, $bridge:ident, $body:expr) => {
        match $provider {
            Provider::Grok($bridge) => $body,
            Provider::Agy($bridge) => $body,
            Provider::Kiro($bridge) => $body,
            Provider::OpenCode($bridge) => $body,
            Provider::Claude($bridge) => $body,
        }
    };
}

pub struct NativeSdks {
    providers: BTreeMap<&'static str, Provider>,
    lifecycle: Option<Arc<crate::lifecycle::service::LifecycleService>>,
    unavailable: BTreeMap<&'static str, String>,
    receipts: Mutex<BTreeMap<String, Value>>,
    submissions: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<Submission>>>>,
    path: Option<PathBuf>,
    bindings: Mutex<BTreeMap<String, Value>>,
    binding_path: Option<PathBuf>,
    input_queue: input_queue::Journal,
    queue_workers: Mutex<BTreeSet<String>>,
    stopped: Mutex<BTreeSet<String>>,
    queue_wake: tokio::sync::Notify,
    shutdown: CancellationToken,
}

#[derive(Default)]
pub(crate) struct Submission;

impl NativeSdks {
    pub fn new(
        config: &crate::config::Config,
        lifecycle: Option<Arc<crate::lifecycle::service::LifecycleService>>,
        shutdown: CancellationToken,
    ) -> io::Result<Arc<Self>> {
        let mut providers = BTreeMap::new();
        let mut unavailable = BTreeMap::new();
        // Unsupported SDK topology must not invalidate an existing node's
        // read model or ordinary CLI launcher configuration.
        macro_rules! register {
            ($source:literal, $module:ident, $variant:ident) => {
                match if config.native_sdk_enabled {
                    crate::$module::new(config, shutdown.clone())
                } else {
                    Ok(None)
                } {
                    Ok(Some(bridge)) => {
                        providers.insert($source, Provider::$variant(bridge));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        unavailable.insert($source, error.to_string());
                    }
                }
            };
        }
        register!("grok", grok_sdk, Grok);
        register!("agy", agy_sdk, Agy);
        register!("kiro", kiro_sdk, Kiro);
        register!("opencode", opencode_sdk, OpenCode);
        register!("claude", claude_sdk, Claude);
        let path = config
            .state_dir
            .as_ref()
            .map(|dir| dir.join("native-sdk-sessions.json"));
        let receipts = match path.as_ref().map(std::fs::read) {
            Some(Ok(bytes)) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Some(Err(error)) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => BTreeMap::new(),
        };
        let binding_path = config
            .state_dir
            .as_ref()
            .map(|dir| dir.join("native-sdk-bindings.json"));
        let bindings = match binding_path.as_ref().map(std::fs::read) {
            Some(Ok(bytes)) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Some(Err(error)) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => BTreeMap::new(),
        };
        let input_queue = input_queue::Journal::open(config.state_dir.as_ref())?;
        let sdks = Arc::new(Self {
            providers,
            lifecycle,
            unavailable,
            receipts: Mutex::new(receipts),
            submissions: Mutex::new(BTreeMap::new()),
            path,
            bindings: Mutex::new(bindings),
            binding_path,
            input_queue,
            queue_workers: Mutex::new(BTreeSet::new()),
            stopped: Mutex::new(BTreeSet::new()),
            queue_wake: tokio::sync::Notify::new(),
            shutdown,
        });
        tokio::spawn(sdks.clone().run_input_queue());
        Ok(sdks)
    }

    pub fn capabilities(&self) -> Value {
        json!(
            self.providers
                .keys()
                .map(|source| ((*source).to_owned(), !self.cli_missing(source)))
                .collect::<BTreeMap<_, _>>()
        )
    }

    // SDK bridges are configured lazily, independently of CLI installation.
    // Reuse the launcher's live probe, including wrappers returning 126/127,
    // so repairing a CLI restores SDK availability without a node restart.
    fn cli_missing(&self, source: &str) -> bool {
        let source = match source {
            "claude" => crate::lifecycle::model::Source::Claude,
            "grok" => crate::lifecycle::model::Source::Grok,
            "agy" => crate::lifecycle::model::Source::Agy,
            "kiro" => crate::lifecycle::model::Source::Kiro,
            "opencode" => crate::lifecycle::model::Source::Opencode,
            _ => return false,
        };
        self.lifecycle
            .as_ref()
            .is_some_and(|service| service.cli_missing(source))
    }

    /// Resolve staged attachments once, including when an HTTP response is
    /// lost between publication and native admission. Provider request IDs
    /// still own native input deduplication.
    pub(crate) fn submission(
        &self,
        target: &Value,
        request: &str,
    ) -> Arc<tokio::sync::Mutex<Submission>> {
        let key = json!([target["source"], target["uid"], target["agent"], request]).to_string();
        self.submissions
            .lock()
            .expect("SDK submissions")
            .entry(key)
            .or_default()
            .clone()
    }

    pub fn loaded_uids(&self) -> Vec<String> {
        self.providers
            .values()
            .flat_map(|provider| dispatch!(provider, bridge, bridge.loaded_uids()))
            .collect()
    }

    pub async fn stop(&self, target: &Value) -> Result<bool, String> {
        let source = target["source"].as_str().unwrap_or_default();
        let uid = target["uid"].as_str().ok_or("SDK 缺少 UID")?;
        let key = target["agent"]
            .as_str()
            .filter(|agent| !agent.is_empty())
            .map_or_else(|| uid.to_owned(), |agent| format!("{uid}:{agent}"));
        let Some(provider) = self.providers.get(source) else {
            return Ok(false);
        };
        // Cancel pending admission before releasing the runtime: queued work must
        // not reopen a session the user has explicitly stopped.
        let scope = input_queue::scope(target);
        self.stopped
            .lock()
            .expect("SDK stopped scopes")
            .insert(scope.clone());
        self.input_queue.stop(&scope)?;
        let result = dispatch!(provider, bridge, bridge.stop(&key).await)?;
        self.remember_binding(target, false)?;
        Ok(result)
    }

    pub fn unavailable(&self) -> Value {
        let mut reasons = self.unavailable.clone();
        for source in self
            .providers
            .keys()
            .filter(|source| self.cli_missing(source))
        {
            reasons.insert(source, format!("此机器未安装或无法执行 {source} 客户端"));
        }
        json!(reasons)
    }

    pub fn pending(&self) -> Vec<Value> {
        self.receipts
            .lock()
            .expect("SDK receipts")
            .values()
            .filter(|row| {
                row["sdk_persisted"] != true
                    && row["source"]
                        .as_str()
                        .is_some_and(|source| self.providers.contains_key(source))
            })
            .cloned()
            .collect()
    }

    pub fn receipt(&self, uid: &str) -> Option<Value> {
        self.receipts
            .lock()
            .expect("SDK receipts")
            .get(uid)
            .filter(|row| row["sdk_persisted"] != true)
            .cloned()
    }

    /// Native inventory takes over once a creation appears. Keep its request
    /// receipt for idempotency, but never resurrect it after native deletion.
    pub fn observe_inventory(&self, inventory: &[Value]) -> Result<(), String> {
        let mut rows = self.receipts.lock().expect("SDK receipts");
        let mut next = rows.clone();
        let mut changed = false;
        for row in inventory {
            if let Some(receipt) = row["uid"].as_str().and_then(|uid| next.get_mut(uid))
                && receipt["sdk_persisted"] != true
            {
                receipt["sdk_persisted"] = json!(true);
                changed = true;
            }
        }
        if changed {
            self.save(&next)?;
            *rows = next;
        }
        Ok(())
    }

    fn save(&self, rows: &BTreeMap<String, Value>) -> Result<(), String> {
        Self::save_rows(self.path.as_ref(), rows)
    }

    fn save_rows(path: Option<&PathBuf>, rows: &BTreeMap<String, Value>) -> Result<(), String> {
        let Some(path) = path else {
            return Ok(());
        };
        let temporary = path.with_extension("tmp");
        let result = (|| -> io::Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec(rows)?)?;
            file.sync_all()?;
            std::fs::rename(&temporary, path)
        })();
        result.map_err(|error| format!("SDK 创建回执保存失败：{error}"))
    }

    fn remember_binding(&self, target: &Value, active: bool) -> Result<(), String> {
        if !cfg!(unix)
            || !matches!(
                target["source"].as_str(),
                Some("agy" | "claude" | "grok" | "kiro")
            )
        {
            return Ok(());
        }
        let mut rows = self.bindings.lock().expect("SDK bindings");
        let mut next = rows.clone();
        let key = input_queue::scope(target);
        if active {
            next.insert(key, target.clone());
        } else {
            next.remove(&key);
        }
        Self::save_rows(self.binding_path.as_ref(), &next)?;
        *rows = next;
        Ok(())
    }

    async fn reattach_bindings(&self) {
        let targets: Vec<Value> = self
            .bindings
            .lock()
            .expect("SDK bindings")
            .values()
            .cloned()
            .collect();
        for target in targets {
            if self.shutdown.is_cancelled() {
                return;
            }
            let source = target["source"].as_str().unwrap_or_default().to_owned();
            // Never call open here: an absent/exited runtime must not cause a new CLI launch.
            if let Err(error) = self
                .provider_call(&source, "reattach", target, json!({}))
                .await
            {
                eprintln!("SDK {source} startup reattach: {error}");
            }
        }
    }

    pub async fn call(
        self: &Arc<Self>,
        source: &str,
        method: &str,
        target: Value,
        input: Value,
    ) -> Result<Value, String> {
        let scope = input_queue::scope(&target);
        if method == "open" {
            self.stopped
                .lock()
                .expect("SDK stopped scopes")
                .remove(&scope);
        } else if self
            .stopped
            .lock()
            .expect("SDK stopped scopes")
            .contains(&scope)
        {
            return Err("SDK 会话已停止，请先启动会话".into());
        }
        if self.cli_missing(source) {
            return Err(format!("此机器未安装或无法执行 {source} 客户端"));
        }
        let bridge = self
            .providers
            .get(source)
            .ok_or_else(|| format!("此机器未配置 {source} SDK 模式"))?;
        if matches!(method, "create" | "fork") {
            let rows = self.receipts.lock().expect("SDK receipts");
            if let Some(row) = rows.values().find(|row| {
                row["source"] == source
                    && row["sdk_operation"].as_str().unwrap_or("create") == method
                    && (method != "fork" || row["sdk_parent"] == target["uid"])
                    && row["request_id"] == input["request_id"]
                    && row["request_id"].is_string()
            }) {
                if row["cwd"] != target["cwd"] {
                    return Err("此创建请求已用于不同的目录".into());
                }
                return Ok(
                    json!({"uid":row["uid"],"session_id":row["sid"],"cwd":row["cwd"],"session":row}),
                );
            }
        }
        let result = match dispatch!(
            bridge,
            bridge,
            bridge.call(method, target.clone(), input.clone()).await
        ) {
            Ok(result) => result,
            Err(error)
                if method == "open"
                    && self
                        .input_snapshot(&target)
                        .as_array()
                        .is_some_and(|rows| !rows.is_empty()) =>
            {
                return Ok(
                    json!({"uid":target["uid"],"session_id":target["session_id"],"cwd":target["cwd"],
                    "capabilities":{"send":true,"input_queue":true,"queue_cancel":true,"queue_retry":true},
                    "raw":self.input_fallback(&target,&error)["raw"]}),
                );
            }
            Err(error) => return Err(error),
        };
        if matches!(method, "create" | "fork") {
            let uid = result["uid"].as_str().ok_or("SDK 未返回原生会话身份")?;
            let mut row = result
                .get("session")
                .filter(|row| row.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}));
            row["uid"] = json!(uid);
            row["source"] = json!(source);
            row["sid"] = result["session_id"].clone();
            row["cwd"] = target["cwd"].clone();
            if !row["title"].is_string() {
                row["title"] = json!("新会话");
            }
            row["sdk"] = json!(true);
            if !row["size"].is_number() {
                row["size"] = json!(0);
            }
            if !row["agent_items"].is_array() {
                row["agent_items"] = json!([]);
            }
            // Native SDK receipts have a UID/SID already. `pending` belongs
            // to a PTY launch and would route clicks to tmux:undefined.
            row.as_object_mut()
                .expect("SDK session row")
                .remove("pending");
            row["sdk_pending"] = json!(true);
            // A native identity may exist before its first history record.
            // Persist the creation time once so pending rows sort consistently
            // across list refreshes and idempotent create retries.
            let created =
                chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()).to_rfc3339();
            if !row["created"].is_string() {
                row["created"] = json!(created);
            }
            if !row["updated"].is_string() {
                row["updated"] = row["created"].clone();
            }
            row["request_id"] = input["request_id"].clone();
            row["sdk_operation"] = json!(method);
            if method == "fork" {
                row["sdk_parent"] = target["uid"].clone();
            }
            row["sdk_model"] = input["model"].clone();
            row["sdk_effort"] = input["effort"].clone();
            let mut rows = self.receipts.lock().expect("SDK receipts");
            let mut next = rows.clone();
            next.insert(uid.to_owned(), row.clone());
            self.save(&next)?;
            *rows = next;
            let mut result = result;
            result["session"] = row.clone();
            let mut binding = target.clone();
            binding["uid"] = result["uid"].clone();
            binding["session_id"] = result["session_id"].clone();
            binding["source"] = json!(source);
            for field in ["sdk_fork", "sdk_pending", "sdk_model", "sdk_effort"] {
                if let Some(value) = row.get(field) {
                    binding[field] = value.clone();
                }
            }
            self.remember_binding(&binding, true)?;
            return Ok(result);
        }
        if method == "open" {
            self.remember_binding(&target, true)?;
        }
        if method == "discard" {
            let mut rows = self.receipts.lock().expect("SDK receipts");
            let mut next = rows.clone();
            next.remove(target["uid"].as_str().unwrap_or_default());
            self.save(&next)?;
            *rows = next;
        }
        let mut result = result;
        if method == "open" {
            if !result["capabilities"].is_object() {
                result["capabilities"] = json!({});
            }
            result["capabilities"]["input_queue"] = json!(true);
            result["capabilities"]["queue_cancel"] = json!(true);
            result["capabilities"]["queue_retry"] = json!(true);
        }
        Ok(result)
    }

    pub fn subscribe(
        &self,
        source: &str,
        key: &str,
    ) -> Result<(Value, broadcast::Receiver<Arc<str>>), String> {
        let bridge = self
            .providers
            .get(source)
            .ok_or_else(|| format!("此机器未配置 {source} SDK 模式"))?;
        dispatch!(bridge, bridge, bridge.subscribe(key))
    }
}

impl NativeSdks {
    pub(crate) fn begin_input(
        &self,
        target: &Value,
        operation: &str,
        draft: Value,
    ) -> Result<QueuedInput, String> {
        self.input_queue.begin(target, operation, draft)
    }

    pub(crate) fn input_receipt(input: &QueuedInput) -> Value {
        input_queue::Journal::receipt(input)
    }

    pub(crate) fn input_scope(target: &Value) -> String {
        input_queue::scope(target)
    }

    pub(crate) fn input_fallback(&self, target: &Value, error: &str) -> Value {
        json!({"seq":0,"type":"snapshot","raw":{"session_id":target["session_id"],"running":false,"items":[],"requests":[],"error":error,"input_queue":self.input_snapshot(target)}})
    }

    pub(crate) fn input_snapshot(&self, target: &Value) -> Value {
        self.input_queue.visible(&input_queue::scope(target))
    }

    pub(crate) fn input_events(&self, target: &Value) -> (Value, broadcast::Receiver<Arc<str>>) {
        self.input_queue.subscribe(&input_queue::scope(target))
    }

    pub(crate) fn with_inputs(&self, target: &Value, mut snapshot: Value) -> Value {
        if snapshot["type"] == "snapshot" {
            snapshot["raw"]["input_queue"] = self.input_snapshot(target);
        }
        snapshot
    }

    pub(crate) fn input_preparation_failed(
        &self,
        target: &Value,
        id: &str,
        error: String,
    ) -> Result<(), String> {
        self.input_queue
            .change(&input_queue::scope(target), id, |row| {
                if row.state == "preparing" {
                    row.state = "failed".into();
                    row.error = Some(error);
                }
                Ok(())
            })?;
        Ok(())
    }

    pub(crate) fn prepare_input(
        self: &Arc<Self>,
        target: &Value,
        id: &str,
        input: Value,
    ) -> Result<Value, String> {
        let row = self
            .input_queue
            .change(&input_queue::scope(target), id, |row| {
                if row.state == "preparing" {
                    row.input = Some(input);
                    row.state = "queued".into();
                    row.error = None;
                }
                Ok(())
            })?;
        if row.operation == "turn" {
            self.queue_wake.notify_one();
        }
        Ok(Self::input_receipt(&row))
    }

    pub(crate) async fn input_action(
        self: &Arc<Self>,
        target: &Value,
        id: &str,
        action: &str,
        confirmed: bool,
        request: &str,
    ) -> Result<(Value, Option<Value>), String> {
        let key = input_queue::scope(target);
        if request.is_empty() {
            return Err("队列操作缺少稳定 request_id".into());
        }
        let mut duplicate = false;
        let row=self.input_queue.change(&key,id,|row| {
            if row.actions.contains_key(request) {duplicate=true;return Ok(())}
            if row.state=="cancelled" {
                row.actions.insert(request.into(),json!({"action":action,"state":"cancelled","attempts":row.attempts}));
                return Ok(())
            }
            if !matches!(row.state.as_str(),"queued"|"failed"|"unknown"|"preparing") {return Err("此输入已经交给原生运行，不能撤销或重新发送；停止任务须使用中断。".into())}
            if row.state=="unknown"&&!confirmed {return Err("请先核对原生会话并明确确认未知投递的重复风险。".into())}
            match action {
                "cancel"=>{row.state="cancelled".into();row.error=None;},
                "retry"=>{
                    if row.operation!="turn" {return Err("原生引导操作不能转成普通输入重试；请核对后移除记录，再明确发起新的引导。".into())}
                    row.delivery_id=crate::conversation::random_id().map_err(|error|error.message)?;
                    row.attempts+=1;row.state=if row.input.is_some(){"queued"}else{"preparing"}.into();row.error=None;row.result=None;row.consumption="unverified".into();row.stop_requested=false;
                },
                _=>return Err("未知队列操作".into()),
            }
            row.actions.insert(request.into(),json!({"action":action,"state":row.state,"attempts":row.attempts}));
            Ok(())
        })?;
        self.queue_wake.notify_one();
        let draft =
            (!duplicate && action == "retry" && row.input.is_none() && row.state == "preparing")
                .then(|| row.draft.clone());
        Ok((
            json!({"input_queue":self.input_snapshot(target),"receipt":Self::input_receipt(&row),"native_preserved":true,"duplicate":duplicate}),
            draft,
        ))
    }

    /// Controls such as steer have their own operation identity and never enter
    /// the ordinary turn worker or become automatic FIFO input on recovery.
    pub(crate) async fn dispatch_input_control(
        self: &Arc<Self>,
        target: &Value,
        id: &str,
    ) -> Result<Value, String> {
        let key = input_queue::scope(target);
        let row = self.input_queue.change(&key, id, |row| {
            if row.state != "queued" {
                return Err("此原生操作已经处理或结果未确认，不会自动重发。".into());
            }
            row.state = "sending".into();
            Ok(())
        })?;
        let mut input = row.input.clone().ok_or("原生操作缺少已准备输入")?;
        input["request_id"] = json!(row.delivery_id);
        let result = self
            .provider_call(
                row.target["source"].as_str().unwrap_or_default(),
                &row.operation,
                row.target.clone(),
                input,
            )
            .await;
        let saved = self.input_queue.change(&key, id, |row| {
            match result {
                Ok(result) => {
                    row.result = Some(result);
                    row.state = "completed".into();
                }
                Err(error) => {
                    let native_reject =
                        error.contains("code -32601") || error.contains("code -32602");
                    row.state = if native_reject
                        || (!uncertain(&error) && !error.contains("grok_sdk_native_error"))
                    {
                        "failed"
                    } else {
                        "unknown"
                    }
                    .into();
                    row.error = Some(error);
                }
            }
            Ok(())
        })?;
        Ok(Self::input_receipt(&saved))
    }

    pub(crate) fn input_stop_requested(
        &self,
        target: &Value,
        requested: bool,
    ) -> Result<(), String> {
        let key = input_queue::scope(target);
        if let Some(current) = self.input_queue.first(&key)
            && matches!(current.state.as_str(), "sending" | "accepted")
        {
            self.input_queue.change(&key, &current.id, |row| {
                row.stop_requested = requested;
                Ok(())
            })?;
        }
        Ok(())
    }

    async fn provider_call(
        &self,
        source: &str,
        method: &str,
        target: Value,
        input: Value,
    ) -> Result<Value, String> {
        if self
            .stopped
            .lock()
            .expect("SDK stopped scopes")
            .contains(&input_queue::scope(&target))
        {
            return Err("SDK 会话已停止".into());
        }
        let provider = self
            .providers
            .get(source)
            .ok_or_else(|| format!("此机器未配置 {source} SDK 模式"))?;
        dispatch!(provider, bridge, bridge.call(method, target, input).await)
    }

    async fn run_input_queue(self: Arc<Self>) {
        self.reattach_bindings().await;
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            tokio::select! {_=self.shutdown.cancelled()=>return,_=tick.tick()=>{},_=self.queue_wake.notified()=>{}}
            for (key, target) in self.input_queue.runnable() {
                if !self
                    .queue_workers
                    .lock()
                    .expect("SDK queue workers")
                    .insert(key.clone())
                {
                    continue;
                }
                let sdks = self.clone();
                tokio::spawn(async move {
                    sdks.clone().work_inputs(&key, target).await;
                    sdks.queue_workers
                        .lock()
                        .expect("SDK queue workers")
                        .remove(&key);
                });
            }
        }
    }

    async fn work_inputs(self: Arc<Self>, key: &str, target: Value) {
        let source = target["source"].as_str().unwrap_or_default();
        let native_key = if target["agent"].as_str().unwrap_or_default().is_empty() {
            target["uid"].as_str().unwrap_or_default().to_owned()
        } else {
            format!(
                "{}:{}",
                target["uid"].as_str().unwrap_or_default(),
                target["agent"].as_str().unwrap_or_default()
            )
        };
        let Some(first) = self.input_queue.first(key) else {
            return;
        };
        if first.state == "unknown" {
            // Reattaching Agy can resolve an old erroneous result. Observe the
            // existing actor only: missing evidence must never launch or resend.
            if source == "agy"
                && let Ok((snapshot, _)) = self.subscribe(source, &native_key)
            {
                let raw = &snapshot["raw"];
                if raw["session_id"] == first.target["session_id"]
                    && observed_input(source, &first, raw)
                    && raw["items"].as_array().into_iter().flatten().any(|item| {
                        item["role"] == "user"
                            && item["request_id"] == first.delivery_id
                            // Set by the actor only after execute succeeds, not
                            // by a native user_input ACK or assistant prose.
                            && item["status"] == "completed"
                    })
                {
                    let _ = self.input_queue.change(key, &first.id, |entry| {
                        if entry.state == "unknown" && entry.delivery_id == first.delivery_id {
                            entry.state = "completed".into();
                            entry.error = None;
                        }
                        Ok(())
                    });
                }
            }
            return;
        }
        if first.state == "recovering" {
            let mut input = first.input.clone().unwrap_or_else(|| json!({}));
            input["request_id"] = json!(first.delivery_id);
            let recovered = self
                .provider_call(source, "recover", target.clone(), input)
                .await;
            let ready = recovered.is_ok();
            let _ = self.input_queue.change(key, &first.id, |row| {
                match recovered {
                    Ok(result) => {
                        row.state = "accepted".into();
                        row.result = Some(result);
                        row.error = None;
                    }
                    Err(error) => {
                        row.state = "unknown".into();
                        row.error = Some(format!("SDK 宿主恢复失败：{error}；不会自动重发输入。"));
                    }
                }
                Ok(())
            });
            if !ready {
                return;
            }
        }
        if let Err(error) = self
            .provider_call(source, "open", target.clone(), json!({}))
            .await
        {
            let _ = self.input_queue.change(key, &first.id, |row| {
                if row.state == "queued" {
                    row.state = "failed".into();
                    row.error = Some(error);
                }
                Ok(())
            });
            return;
        }
        // This receiver is also the provider's active-work lease. It is retained
        // while waiting behind another turn, sending and observing the result.
        let Ok((mut snapshot, mut events)) = self.subscribe(source, &native_key) else {
            return;
        };
        loop {
            if self.shutdown.is_cancelled() {
                return;
            }
            let Some(mut row) = self.input_queue.first(key) else {
                return;
            };
            if !matches!(row.state.as_str(), "queued" | "accepted") {
                return;
            }
            let raw = &snapshot["raw"];
            if row.state == "queued" {
                let ready = source == "opencode"
                    || (raw["running"] != true
                        && raw["requests"].as_array().is_none_or(Vec::is_empty));
                if ready {
                    let sending = self.input_queue.change(key, &row.id, |entry| {
                        if entry.state != "queued" {
                            return Err("SDK queue changed".into());
                        }
                        entry.state = "sending".into();
                        entry.error = None;
                        Ok(())
                    });
                    let Ok(sending) = sending else { continue };
                    row = sending;
                    let Some(mut input) = row.input.clone() else {
                        let _ = self.input_queue.change(key, &row.id, |entry| {
                            entry.state = "failed".into();
                            entry.error = Some("输入尚未完成准备，请明确重试。".into());
                            Ok(())
                        });
                        return;
                    };
                    input["request_id"] = json!(row.delivery_id);
                    let result = self
                        .provider_call(source, "turn", target.clone(), input)
                        .await;
                    let saved = self.input_queue.change(key, &row.id, |entry| {
                        if entry.state == "cancelled" {
                            return Ok(());
                        }
                        match result {
                            Ok(result) => {
                                entry.result = Some(result);
                                entry.state = "accepted".into();
                            }
                            Err(error) => {
                                entry.state = if uncertain(&error) {
                                    "unknown"
                                } else {
                                    "failed"
                                }
                                .into();
                                entry.error = Some(error);
                            }
                        }
                        Ok(())
                    });
                    if saved.is_err() {
                        return;
                    }
                    match self.subscribe(source, &native_key) {
                        Ok((current, receiver)) => {
                            snapshot = current;
                            events = receiver;
                        }
                        Err(_) => return,
                    }
                    continue;
                }
            } else {
                let observed = observed_input(source, &row, raw);
                if observed
                    && row.consumption != "observed"
                    && let Ok(current) = self.input_queue.change(key, &row.id, |entry| {
                        entry.consumption = "observed".into();
                        Ok(())
                    })
                {
                    row = current;
                }
                let native_queued = source == "opencode"
                    && raw["queue"].as_array().into_iter().flatten().any(|entry| {
                        entry["id"]
                            == row
                                .result
                                .as_ref()
                                .map(|r| r["message_id"].clone())
                                .unwrap_or(Value::Null)
                    });
                let native_admitted = source == "opencode"
                    && raw["items"].as_array().into_iter().flatten().any(|item| {
                        item["native_admitted"] == true
                            && row.result.as_ref().is_some_and(|result| {
                                result["message_id"].is_string()
                                    && item["native_id"] == result["message_id"]
                            })
                    });
                let done = if source == "opencode" {
                    observed
                        || native_queued
                        || native_admitted
                        || (raw["running"] != true
                            && raw.get("error").is_some_and(|error| {
                                !error.is_null() && error.as_str() != Some("")
                            }))
                } else {
                    raw["running"] != true
                };
                if done {
                    let error = raw
                        .get("error")
                        .filter(|error| !error.is_null() && error.as_str() != Some(""));
                    let _ = self.input_queue.change(key, &row.id, |entry| {
                        if let Some(error) = error {
                            entry.state = if raw["error_kind"] == "interrupted"
                                || (entry.stop_requested && raw["error_kind"] == "turn")
                            {
                                "completed"
                            } else if raw["error_kind"] == "turn" && entry.consumption != "observed"
                            {
                                "failed"
                            } else {
                                "unknown"
                            }
                            .into();
                            entry.error = Some(
                                error
                                    .as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| error.to_string()),
                            );
                        } else {
                            entry.state = "completed".into();
                            if !observed {
                                if native_queued {
                                    entry.consumption = "native_queued".into();
                                } else if native_admitted {
                                    entry.consumption = "native_accepted".into();
                                }
                            }
                        }
                        Ok(())
                    });
                    continue;
                }
            }
            tokio::select! {
                _=self.shutdown.cancelled()=>return,
                _=tokio::time::sleep(std::time::Duration::from_millis(350))=>{},
                event=events.recv()=>match event {
                    Ok(event)=>{if let Ok(value)=serde_json::from_str::<Value>(&event) && value["type"]=="snapshot"{snapshot=value;}},
                    Err(broadcast::error::RecvError::Lagged(_))=>{match self.subscribe(source,&native_key){Ok((current,receiver))=>{snapshot=current;events=receiver;},Err(_)=>return}},
                    Err(broadcast::error::RecvError::Closed)=>{
                        let _=self.input_queue.change(key,&row.id,|entry|{if entry.state=="accepted"{entry.state="unknown".into();entry.error=Some("原生事件连接结束，投递结果未确认；不会自动重发。".into());}Ok(())});return;
                    }
                }
            }
        }
    }
}

fn uncertain(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "disconnect",
        "transport",
        "timeout",
        "shutdown",
        "no_result",
        "worker",
        "broken pipe",
        "unexpected eof",
        "unknown",
    ]
    .iter()
    .any(|term| error.contains(term))
}

fn observed_input(source: &str, row: &QueuedInput, raw: &Value) -> bool {
    raw["items"].as_array().into_iter().flatten().any(|item| {
        if item["role"] != "user" {
            return false;
        }
        match source {
            "agy" => {
                item["request_id"] == row.delivery_id
                    && item["native"]["step_type"] == "user_input"
                    && item["native_id"].is_string()
            }
            "claude" => {
                item["native_echo"] == true
                    && row.result.as_ref().is_some_and(|result| {
                        result["message_id"].is_string()
                            && item["native_id"] == result["message_id"]
                    })
            }
            "opencode" => {
                item["delivery_state"] == "consumed"
                    && row.result.as_ref().is_some_and(|result| {
                        result["message_id"].is_string()
                            && item["native_id"] == result["message_id"]
                    })
            }
            _ => false,
        }
    })
}
