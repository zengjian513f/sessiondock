//! One conversation send path: drafts are server-owned, successful SEND belongs to the CLI.
pub mod store;
use crate::{
    bridge::LivePrompts,
    delivery::{
        driver::{HostTerminalDriver, LeaseHandle, PageLease, ScreenCapture, TerminalDriver},
        executor::{Failure, ManagedResolver, TargetResolver},
    },
    files::WriteService,
    lifecycle::{
        model::{BindingState, Record},
        service::LifecycleService,
    },
    state::Reader,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use store::Store;
use tokio::sync::Mutex as AsyncMutex;

#[derive(Clone)]
pub struct Identity {
    pub key: String,
    pub uid: String,
    pub source: String,
    pub sid: String,
    pub cwd: PathBuf,
    pub record: Option<Record>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SendInput {
    pub _build: String,
    pub uid: String,
    pub name: String,
    pub text: String,
    pub request_id: String,
    pub draft_revision: Option<u64>,
    pub attachments: Vec<Value>,
    pub quotes: Vec<Value>,
    pub lease: Option<Value>,
}
pub struct Conversations {
    pub store: Arc<Store>,
    pub reader: Reader,
    pub lifecycle: Arc<LifecycleService>,
    pub resolver: Arc<ManagedResolver>,
    pub driver: Arc<HostTerminalDriver>,
    pub writer: Arc<WriteService>,
    pub prompts: Arc<LivePrompts>,
    reports: Option<Arc<crate::bug_report::BugReportService>>,
    locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}
impl Conversations {
    pub fn new(
        store: Arc<Store>,
        reader: Reader,
        lifecycle: Arc<LifecycleService>,
        resolver: Arc<ManagedResolver>,
        driver: Arc<HostTerminalDriver>,
        writer: Arc<WriteService>,
        prompts: Arc<LivePrompts>,
        reports: Option<Arc<crate::bug_report::BugReportService>>,
    ) -> Self {
        Self {
            store,
            reader,
            lifecycle,
            resolver,
            driver,
            writer,
            prompts,
            reports,
            locks: Mutex::new(HashMap::new()),
        }
    }
    pub fn housekeeping(self: &Arc<Self>, shutdown: tokio_util::sync::CancellationToken) {
        let service = self.clone();
        tokio::spawn(async move {
            loop {
                let work = service.clone();
                let result = tokio::task::spawn_blocking(move || work.collect_uploads()).await;
                if let Ok(Err(error)) = result {
                    eprintln!("conversation staging cleanup failed: {}", error.code);
                }
                tokio::select! { _=shutdown.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(3600))=>{} }
            }
        });
    }
    fn collect_uploads(&self) -> Result<(), Failure> {
        let now = std::time::SystemTime::now();
        let cutoff = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(24 * 3600);
        for upload in self.store.retire_unreferenced_uploads(cutoff)? {
            let _ = std::fs::remove_file(self.upload_path(&upload.key, &upload.id));
        }
        let retained: std::collections::HashSet<_> = self
            .store
            .uploads()
            .iter()
            .map(|u| self.upload_path(&u.key, &u.id))
            .collect();
        let directory = self.store.directory().join("conversation-uploads");
        if let Ok(entries) = std::fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if retained.contains(&path) {
                    continue;
                }
                if entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| now.duration_since(t).ok())
                    .is_some_and(|age| age >= Duration::from_secs(24 * 3600))
                {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        Ok(())
    }
    pub async fn identity(&self, uid: &str) -> Result<Identity, Failure> {
        if uid.starts_with("report:") && uid.len() > 7 {
            return Ok(Identity {
                key: self.store.canonical(uid),
                uid: uid.into(),
                source: "codex".into(),
                sid: String::new(),
                cwd: PathBuf::new(),
                record: None,
            });
        }
        let records = self
            .lifecycle
            .list(0, usize::MAX)
            .await
            .map_err(|_| Failure::new(503, "lifecycle_unavailable", "会话身份读取失败"))?;
        if let Some(name) = uid.strip_prefix("tmux:") {
            let record = records
                .into_iter()
                .find(|r| r.host_name() == name)
                .ok_or_else(|| Failure::new(404, "session_error", "会话不存在"))?;
            let launch_key = format!("launch:{}", record.record_id());
            if let Some(native_uid) = record.declared_uid() {
                let native_key = self.store.canonical(native_uid);
                if self.store.canonical(&launch_key) == launch_key {
                    self.store.link(&launch_key, &native_key)?;
                }
            }
            return Ok(Identity {
                key: self
                    .store
                    .canonical(&format!("launch:{}", record.record_id())),
                uid: uid.into(),
                source: crate::bug_report::source_name(record.spec().source()).into(),
                sid: record.declared_sid().unwrap_or("").into(),
                cwd: record.spec().cwd().into(),
                record: Some(record),
            });
        }
        let requested = uid.to_owned();
        let native = self
            .reader
            .run(move |s| s.native_scope(&requested, ""))
            .await
            .map_err(|e| Failure::new(e.status.as_u16(), e.code, e.message))?;
        let resolved = self.resolver.resolve(&native.uid).await.ok();
        let bound_uid = resolved
            .as_ref()
            .map(|t| t.uid.as_str())
            .unwrap_or(&native.uid);
        let record = records
            .iter()
            .find(|r| {
                resolved
                    .as_ref()
                    .is_some_and(|target| r.instance_id() == target.instance_id)
            })
            .or_else(|| {
                records.iter().find(|r| {
                    r.binding().is_some_and(|b| {
                        b.state() == BindingState::Confirmed && b.spec().uid() == bound_uid
                    })
                })
            })
            .cloned();
        let native_key = self.store.canonical(bound_uid);
        let key = if let Some(record) = &record {
            let launch_key = format!("launch:{}", record.record_id());
            let launch_canonical = self.store.canonical(&launch_key);
            if native_key == bound_uid && launch_canonical != bound_uid {
                self.store.link(bound_uid, &launch_canonical)?;
                launch_canonical
            } else {
                if launch_canonical == launch_key && launch_key != native_key {
                    self.store.link(&launch_key, &native_key)?;
                }
                native_key
            }
        } else {
            native_key
        };
        let uid = native.uid.clone();
        let view = self
            .reader
            .run(move |s| s.messages(&uid, &Default::default()))
            .await
            .map_err(|e| Failure::new(e.status.as_u16(), e.code, e.message))?;
        Ok(Identity {
            key,
            uid: native.uid,
            source: native.source,
            sid: native.session_id,
            cwd: PathBuf::from(view["meta"]["cwd"].as_str().unwrap_or("")),
            record,
        })
    }
    fn lock(&self, key: &str) -> Arc<AsyncMutex<()>> {
        self.locks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(key.into())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }
    pub fn upload_lock(&self, key: &str, id: &str) -> Arc<AsyncMutex<()>> {
        self.lock(&format!("upload:{key}\0{id}"))
    }
    async fn lease(
        &self,
        identity: &Identity,
        page: Option<&PageLease>,
    ) -> Result<LeaseHandle, Failure> {
        if identity.uid.starts_with("tmux:") {
            let record = identity
                .record
                .as_ref()
                .ok_or_else(|| Failure::new(409, "session_error", "会话尚未启动"))?;
            let target = self
                .lifecycle
                .target(record.record_id().into())
                .await
                .map_err(|_| Failure::new(409, "terminal_unlinked", "会话未运行，输入已保留"))?;
            self.driver
                .acquire_launch(target, page)
                .await
                .map_err(driver_error)
        } else {
            let target = self.resolver.resolve(&identity.uid).await?;
            self.driver
                .acquire(&target, page)
                .await
                .map_err(driver_error)
        }
    }
    async fn ensure_sendable(
        &self,
        identity: &Identity,
        lease: &LeaseHandle,
    ) -> Result<(), Failure> {
        let capture = self.driver.capture(lease).await.map_err(driver_error)?;
        if crate::delivery::driver::strip_ansi(&capture.text)
            .trim()
            .is_empty()
        {
            return Err(Failure::new(
                409,
                "cli_starting",
                "CLI 正在启动，输入已保留",
            ));
        }
        if screen_question(&capture) {
            return Err(question());
        }
        if !identity.uid.starts_with("tmux:") {
            let uid = identity.uid.clone();
            let view = self
                .reader
                .run(move |s| s.messages(&uid, &Default::default()))
                .await
                .map_err(|e| Failure::new(e.status.as_u16(), e.code, e.message))?;
            if identity.source == "claude"
                && !identity.sid.is_empty()
                && !self
                    .prompts
                    .claude_prompt(&identity.sid, &view["messages"])
                    .is_null()
            {
                return Err(question());
            }
            if identity.source == "codex" && history_question(&view) {
                return Err(question());
            }
        }
        Ok(())
    }
    pub async fn restart(&self, uid: &str, request_id: &str) -> Result<Value, Failure> {
        if request_id.is_empty() {
            return Err(Failure::new(400, "request_id", "缺少启动请求 ID"));
        }
        let identity = self.identity(uid).await?;
        let record = identity
            .record
            .as_ref()
            .filter(|_| uid.starts_with("tmux:"))
            .ok_or_else(|| Failure::new(409, "session_error", "请通过已绑定的原生会话恢复 CLI"))?;
        if !matches!(
            record.state(),
            crate::lifecycle::model::State::Exited | crate::lifecycle::model::State::Failed
        ) {
            return Err(Failure::new(409, "cli_running", "CLI 尚未退出，输入保留"));
        }
        if record
            .binding()
            .is_some_and(|b| b.state() == BindingState::Confirmed)
        {
            return Err(Failure::new(
                409,
                "native_bound",
                "请打开已绑定的原生会话继续",
            ));
        }
        let lock = self.lock(&identity.key);
        let _guard = lock.lock().await;
        let launched = self
            .lifecycle
            .create(
                format!("conversation-restart-{}", {
                    use sha2::{Digest, Sha256};
                    format!(
                        "{:x}",
                        Sha256::digest(format!("{}\0{request_id}", identity.key).as_bytes())
                    )
                }),
                record.spec().clone(),
            )
            .await
            .map_err(|e| Failure::new(503, "conversation_start", format!("CLI 启动失败：{e}")))?;
        self.store
            .link(&format!("launch:{}", launched.record_id()), &identity.key)?;
        let old = self.store.draft(&identity.key);
        let mut value = old.value.clone();
        if !value.is_object() {
            value = json!({"text":"","attachments":[],"quotes":[]});
        }
        value["session"]["uid"] = json!(format!("tmux:{}", launched.host_name()));
        value["session"]["name"] = json!(launched.host_name());
        value["session"]["record_id"] = json!(launched.record_id());
        value["session"]["launch_id"] = json!(launched.launch_id());
        value["session"]["instance_id"] = json!(launched.instance_id());
        value["session"]["sid"] = json!(launched.declared_sid());
        self.store
            .save(&identity.key, old.revision, value.clone())?;
        let mut response = value["session"].clone();
        response["running"] = json!(launched.state() == crate::lifecycle::model::State::Running);
        response["state"] = json!("running");
        response["ok"] = json!(true);
        if let Some(reports) = &self.reports {
            let report_id = reports
                .conversation_restarted(record.record_id(), &response)
                .map_err(|e| {
                    Failure::new(
                        503,
                        "report_status",
                        format!("CLI 已启动，报告关联更新失败：{e}"),
                    )
                })?;
            if let Some(report_id) = report_id {
                self.store
                    .refresh_report_worker(&identity.key, &report_id, &response)?;
            }
        }
        Ok(response)
    }
    pub async fn check(&self, uid: &str, page: Option<&PageLease>) -> Result<(), Failure> {
        let identity = self.identity(uid).await?;
        let lock = self.lock(&identity.key);
        let _guard = lock.lock().await;
        let lease = self.lease(&identity, page).await?;
        let result = self.ensure_sendable(&identity, &lease).await;
        self.driver.release(lease).await;
        result
    }
    pub fn upload_path(&self, key: &str, id: &str) -> PathBuf {
        use sha2::{Digest, Sha256};
        self.store
            .directory()
            .join("conversation-uploads")
            .join(format!(
                "{:x}",
                Sha256::digest(format!("{key}\0{id}").as_bytes())
            ))
    }
    pub async fn publish(
        &self,
        identity: &Identity,
        items: &[Value],
    ) -> Result<Vec<Value>, Failure> {
        let mut results = Vec::new();
        let mut batch = None;
        for item in items {
            let id = item["upload_id"]
                .as_str()
                .ok_or_else(|| Failure::new(400, "attachment_missing", "附件未上传"))?;
            let upload = self.store.upload(&identity.key, id)?;
            let mut document = if let Some(published) = upload.published {
                published
            } else {
                let writer = self.writer.clone();
                let cwd = identity.cwd.clone();
                let path = self.upload_path(&identity.key, id);
                let name = upload.name.clone();
                let mime = upload.mime.clone();
                let batch_id = batch.clone();
                let uid = identity.uid.clone();
                let value = tokio::task::spawn_blocking(move || {
                    writer.session_attachment_publish(
                        &crate::files::FileScope {
                            uid: &uid,
                            agent: None,
                            cwd: &cwd.to_string_lossy(),
                            messages: &[],
                        },
                        batch_id.as_deref(),
                        &name,
                        &mime,
                        &path,
                    )
                })
                .await
                .map_err(|_| Failure::new(503, "attachment_publish", "附件发布任务失败"))?
                .map_err(|e| Failure::new(e.status, e.code, e.message))?;
                self.store.published(&identity.key, id, value.clone())?;
                let _ = std::fs::remove_file(self.upload_path(&identity.key, id));
                value
            };
            batch
                .get_or_insert_with(|| document["attachment_id"].as_str().unwrap_or("").to_owned());
            document["number"] = item["number"].clone();
            results.push(document);
        }
        Ok(results)
    }
    pub async fn send(&self, input: SendInput) -> Result<Value, Failure> {
        self.send_with_prompt(input, None).await
    }
    pub async fn send_with_prompt(
        &self,
        input: SendInput,
        prompt: Option<String>,
    ) -> Result<Value, Failure> {
        if input.request_id.is_empty() {
            return Err(Failure::new(400, "request_id", "缺少稳定提交 ID"));
        }
        let identity = self.identity(&input.uid).await?;
        let lock = self.lock(&identity.key);
        let _guard = lock.lock().await;
        let payload = json!({"text":input.text,"attachments":input.attachments.iter().map(|a|json!({"upload_id":a["upload_id"],"number":a["number"]})).collect::<Vec<_>>(),"quotes":input.quotes});
        if let Some(old) = self.store.request(&identity.key, &input.request_id) {
            if old.payload != store::fingerprint(&payload) {
                return Err(Failure::new(
                    400,
                    "request_conflict",
                    "相同提交 ID 对应不同消息",
                ));
            }
            let mut response = submission_result(&old)?;
            response["draft"] = serde_json::to_value(self.store.draft(&identity.key)).unwrap();
            return Ok(response);
        }
        let page = page_lease(input.lease.as_ref());
        let lease = self.lease(&identity, page.as_ref()).await?;
        if !input.name.is_empty() && input.name != lease.name {
            self.driver.release(lease).await;
            return Err(Failure::new(
                409,
                "terminal_unlinked",
                "请求终端不是该会话绑定的实例",
            ));
        }
        let result = self
            .send_held(&identity, &lease, &input, payload, prompt)
            .await;
        if !result
            .as_ref()
            .err()
            .is_some_and(|e| e.code == "cli_starting")
        {
            if let (Some(reports), Some(record)) = (&self.reports, &identity.record) {
                if let Err(error) = reports.conversation_status(record.record_id(), &result) {
                    // SEND already succeeded: metadata failure cannot authorize another send.
                    eprintln!("conversation report status update failed: {error}");
                }
            }
        }
        self.driver.release(lease).await;
        result
    }
    /// A startup timeout/shutdown may be recorded only while the first task is still unsent.
    pub async fn report_startup_failed(&self, uid: &str, request_id: &str, message: String) {
        let Ok(identity) = self.identity(uid).await else {
            return;
        };
        let lock = self.lock(&identity.key);
        let _guard = lock.lock().await;
        if self
            .store
            .request(&identity.key, request_id)
            .is_some_and(|r| r.phase == "sent")
        {
            return;
        }
        if let (Some(reports), Some(record)) = (&self.reports, &identity.record) {
            let result = Err(Failure::new(503, "cli_starting", message));
            if let Err(error) = reports.conversation_status(record.record_id(), &result) {
                eprintln!("conversation report startup status update failed: {error}");
            }
        }
    }
    async fn send_held(
        &self,
        identity: &Identity,
        lease: &LeaseHandle,
        input: &SendInput,
        payload: Value,
        execution: Option<String>,
    ) -> Result<Value, Failure> {
        self.ensure_sendable(identity, lease).await?;
        let attachments = self.publish(identity, &input.attachments).await?;
        let draft = self.store.draft(&identity.key);
        let report_prompt = draft.value["report_prompt"]
            .as_str()
            .filter(|_| {
                draft.value["requestId"] == input.request_id && draft.value["text"] == input.text
            })
            .map(str::to_owned);
        let prompt = execution
            .or(report_prompt)
            .unwrap_or_else(|| build_prompt(&input.text, &attachments, &input.quotes));
        if prompt.trim().is_empty() {
            return Err(Failure::new(400, "invalid_send", "请输入消息"));
        }
        self.ensure_sendable(identity, lease).await?;
        if let Some(old) = self
            .store
            .begin(&identity.key, &input.request_id, payload)?
        {
            return submission_result(&old);
        }
        let write = async {
            self.driver
                .paste(lease, &prompt)
                .await
                .map_err(driver_error)?;
            tokio::time::sleep(Duration::from_millis(600)).await;
            self.ensure_sendable(identity, lease).await?;
            self.driver
                .keys(lease, &["Enter"])
                .await
                .map_err(driver_error)
        }
        .await;
        if let Err(error) = write {
            let value = json!({"error":error.message,"code":error.code});
            self.store
                .finish(&identity.key, &input.request_id, "error", value, None)?;
            return Err(error);
        }
        let result = json!({"ok":true,"request_id":input.request_id,"state":"sent"});
        self.store.finish(
            &identity.key,
            &input.request_id,
            "sent",
            result.clone(),
            input.draft_revision,
        )?;
        let mut response = result;
        response["draft"] = serde_json::to_value(self.store.draft(&identity.key)).unwrap();
        Ok(response)
    }
}
pub fn page_lease(value: Option<&Value>) -> Option<PageLease> {
    let v = value?;
    let page = v["page"].as_str()?;
    let token = v["token"].as_str()?;
    let instance = v["instance_id"].as_str()?;
    Some(PageLease {
        page: page.into(),
        token: token.into(),
        instance_id: instance.into(),
        launch_id: v["launch_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
    })
}
fn driver_error(e: crate::delivery::driver::DriverError) -> Failure {
    Failure::new(e.status, e.code, e.message)
}
fn question() -> Failure {
    Failure::new(
        409,
        "cli_question",
        "CLI 正在等待选择题回答，请先回答；消息未发送，输入保留",
    )
}
pub fn submission_result(row: &store::Submission) -> Result<Value, Failure> {
    if row.phase == "sent" {
        Ok(row.result.clone())
    } else if row.phase == "error" {
        Err(Failure::new(
            409,
            "send_result_unknown",
            format!(
                "发送发生异常：{}；不会自动重复发送",
                row.result["error"].as_str().unwrap_or("请检查终端")
            ),
        ))
    } else {
        Err(Failure::new(
            409,
            "send_result_unknown",
            "该提交已尝试写入；请核对终端，禁止自动重复 SEND",
        ))
    }
}
pub fn screen_question(capture: &ScreenCapture) -> bool {
    let text = crate::delivery::driver::strip_ansi(&capture.text);
    if crate::bridge::codex::approval_prompt(&text).is_some() {
        return true;
    }
    static MENU: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?m)^\s*[❯›»>]?\s*[12][.)]\s+\S").unwrap());
    let lines: Vec<_> = text.lines().collect();
    let cursor = usize::from(capture.cursor.1);
    let start = cursor.saturating_sub(12);
    let end = (cursor + 8).min(lines.len());
    let active = lines.get(start..end).unwrap_or(&[]).join("\n");
    let selected = active.lines().any(|line| {
        let line = line.trim_start();
        matches!(line.chars().next(), Some('❯' | '›' | '»' | '>')) && MENU.is_match(line)
    });
    let low = active.to_lowercase();
    let footer = low
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            (line.contains("enter")
                && (line.contains("confirm")
                    || line.contains("select")
                    || line.contains("continue")
                    || line.contains("选择")))
            .then_some(index)
        })
        .last();
    let tail = footer.is_some_and(|index| {
        active
            .lines()
            .skip(index + 1)
            .all(|line| line.trim().is_empty())
    });
    selected && tail && MENU.find_iter(&active).count() >= 2
}
fn history_question(view: &Value) -> bool {
    if view["activity"]["state"] != "waiting" {
        return false;
    }
    let Some(messages) = view["messages"].as_array() else {
        return false;
    };
    messages.iter().rev().any(|message| {
        message["role"] == "question"
            && message["call_id"].is_string()
            && !messages.iter().any(|answer| {
                (answer["role"] == "answer" || answer["role"] == "tool_result")
                    && answer["call_id"] == message["call_id"]
            })
    })
}
pub fn build_prompt(text: &str, attachments: &[Value], quotes: &[Value]) -> String {
    let mut prompt = text.to_owned();
    let mut blocks = Vec::new();
    if !attachments.is_empty() {
        blocks.push(
            attachments
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let relative = a["relative_path"]
                        .as_str()
                        .unwrap_or("")
                        .trim_start_matches("./")
                        .trim_start_matches(".\\");
                    let windows = a["path_style"] == "windows"
                        || a["path"].as_str().is_some_and(|p| {
                            p.as_bytes().get(1) == Some(&b':') || p.starts_with("\\\\")
                        });
                    let path = if relative.is_empty() {
                        a["path"].as_str().unwrap_or("").into()
                    } else if windows {
                        format!(".\\{}", relative.replace('/', "\\"))
                    } else {
                        format!("./{relative}")
                    };
                    format!(
                        "附件{}: {path}",
                        a["number"].as_u64().unwrap_or(i as u64 + 1)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    let quoted: Vec<_> = quotes
        .iter()
        .filter_map(|q| q["text"].as_str().or_else(|| q.as_str()))
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .collect();
    if !quoted.is_empty() {
        blocks.push(
            quoted
                .iter()
                .enumerate()
                .map(|(i, q)| format!("引用{}:\n{q}", i + 1))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    for block in blocks {
        if !prompt.is_empty() {
            let trailing = prompt.chars().rev().take_while(|c| *c == '\n').count();
            prompt.push_str(&"\n".repeat(2usize.saturating_sub(trailing)));
        }
        prompt.push_str(&block);
    }
    prompt
}

pub fn random_id() -> Result<String, Failure> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b)
        .map_err(|_| Failure::new(503, "conversation_entropy", "无法分配提交身份"))?;
    Ok(b.iter().map(|v| format!("{v:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn capture(text: &str, cursor: u16) -> ScreenCapture {
        ScreenCapture {
            text: text.into(),
            cursor: (0, cursor),
            lag: None,
            dropped: None,
            resets: None,
            alt: false,
        }
    }
    #[test]
    fn current_choice_blocks_but_old_output_does_not() {
        assert!(screen_question(&capture(
            "Do you trust this directory?\n❯ 1. Yes\n  2. No\nPress Enter to confirm",
            1
        )));
        assert!(screen_question(&capture(
            "Update available\n› 1. Update now\n  2. Skip\nPress Enter to confirm",
            1
        )));
        let old = format!(
            "Do you trust this directory?\n1. Yes\n2. No\nPress Enter to confirm\n{}\n› new editor",
            "output\n".repeat(24)
        );
        assert!(!screen_question(&capture(&old, 29)));
        assert!(!screen_question(&capture(
            "Working · esc to interrupt\n›",
            1
        )));
    }
    #[test]
    fn native_question_requires_waiting_and_no_answer() {
        let mut view =
            json!({"activity":{"state":"waiting"},"messages":[{"role":"question","call_id":"q"}]});
        assert!(history_question(&view));
        view["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"answer","call_id":"q"}));
        assert!(!history_question(&view));
    }
    #[test]
    fn prompt_uses_destination_paths_and_keeps_user_newlines() {
        assert_eq!(
            build_prompt(
                "hello\n\n\n",
                &[
                    json!({"number":3,"relative_path":"sessiondock_attachments/1/x.txt","path_style":"windows"})
                ],
                &[json!({"text":" quote "})]
            ),
            "hello\n\n\n附件3: .\\sessiondock_attachments\\1\\x.txt\n\n引用1:\nquote"
        );
    }
}
