//! The bug-report worker.
//!
//! `launch` creates an ordinary managed instance through the lifecycle service
//! with the source's one configured CLI (what `term/create` would start, on
//! the CLI's default model), records the pending decoration and
//! starts one injection task. The task waits for the CLI's composer to be empty and
//! settled (`delivery::driver` composer models for Claude/Codex, screen
//! stability for Grok), then under its own launch lease persists a step,
//! pastes, verifies the paste on screen, persists, presses Enter, persists,
//! and follows the bounded Enter-resend while the draft visibly stays.
//! Confirmation never comes from the screen: the manifest says `submitted`
//! only when the prompt is found in a native `user` record (Claude: the
//! declared session id; Codex/Grok: a session of that source created under
//! the repository since the launch), otherwise `submitted_unconfirmed`, and
//! `failed` when a step could not be taken. A crash between the persisted
//! paste and Enter leaves `injecting` in the manifest and is never resumed.
//!
//! The paste and Enter are server-originated host input through the
//! launch guard (`request_launch`), exactly like `session/stop`'s EOF keys —
//! they never need the page's console. The
//! worker therefore holds no browser lease: a page that opened the console
//! from the toast keeps it and watches the prompt arrive, and the injection
//! cannot fail because the page got there first.

use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use ptyhost_client::{CaptureKind, ControlOp, ControlReply, HostClient, LaunchTarget};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::{BugReportService, Report, source_label, source_name, update_manifest};
use crate::{
    audit::{AuditService, query::ServerEvent},
    delivery::{
        claude::ComposerState,
        driver::{ComposerKind, ScreenCapture, inspect_for, same_text_ignoring_whitespace},
    },
    lifecycle::{
        model::{LaunchSpec, Record, Source, State},
        service::LifecycleService,
    },
    sessions::MessageQuery,
    state::Reader,
    terminal::TerminalService,
};

/// Origin label recorded in audit rows for the worker's server-originated
/// input (no browser lease is claimed under this name any more).
pub const PAGE: &str = "sessiondock-bug-report";
/// How long injection waits for a ready composer.
pub const READY_TIMEOUT: Duration = Duration::from_secs(90);
/// How long an empty composer or stable frame must last before paste.
pub const SETTLE: Duration = Duration::from_millis(600);
/// Bounded Enter-resend: 4 attempts, 1 s apart.
pub const CONFIRM_ATTEMPTS: usize = 4;
pub const CONFIRM_WAIT: Duration = Duration::from_secs(1);
/// How long the pasted text may take to appear on screen before Enter.
pub const PASTE_TIMEOUT: Duration = Duration::from_secs(4);
/// How long the native `user` record may take to appear after Enter.
pub const NATIVE_TIMEOUT: Duration = Duration::from_secs(20);
/// Bound of one guarded host write (paste / Enter).
const HOST_INPUT_TIMEOUT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(250);
const FAST_POLL: Duration = Duration::from_millis(100);
/// Codex/Grok candidates: sessions of the source under the repository
/// created no earlier than this before the launch.
const CANDIDATE_SLACK: Duration = Duration::from_secs(120);

fn now_text() -> String {
    crate::audit::query::rfc3339(SystemTime::now())
}

/// Services the worker needs; built once by `lib.rs`.
#[derive(Clone)]
pub struct WorkerContext {
    pub service: Arc<BugReportService>,
    pub lifecycle: Arc<LifecycleService>,
    pub terminal: Arc<TerminalService>,
    pub client: Arc<HostClient>,
    pub reader: Reader,
    pub audit: Arc<AuditService>,
    pub shutdown: CancellationToken,
}

impl WorkerContext {
    fn audit(&self, event: &'static str, severity: &'static str, report_id: &str, data: Value) {
        self.audit.record(ServerEvent {
            event,
            category: "bug-report",
            severity,
            uid: "",
            trace_id: report_id,
            page_id: "",
            build: "",
            data,
        });
    }
}

/// The pending instance is running and the injection task
/// is started; the returned object is the route's `worker`.
pub async fn launch(
    ctx: &WorkerContext,
    report: &Report,
    source: Source,
    cols: u16,
    rows: u16,
) -> Result<Value, String> {
    let label = source_label(source);
    let entry = ctx
        .lifecycle
        .entry_for(source, false)
        .ok_or_else(|| format!("本机找不到 {} 命令", source_name(source)))?
        .clone();
    let repository = ctx.service.repository().to_path_buf();
    let adapter_id = entry.id.clone();
    let spec = tokio::task::spawn_blocking(move || {
        if entry.profile {
            LaunchSpec::profile_new(source, adapter_id, &repository)
        } else {
            LaunchSpec::new(source, adapter_id, &repository)
        }
    })
    .await
    .map_err(|_| "启动参数校验任务异常退出".to_owned())?
    .map_err(|error| format!("仓库目录不能作为 {label} 会话的工作目录：{error}"))?;
    let request_id = format!("bug-report-{}", report.report_id);
    let record = ctx
        .lifecycle
        .create(request_id, spec)
        .await
        .map_err(|error| format!("{label} 会话启动失败：{error}"))?;
    if record.state() != State::Running || record.cancel_requested() {
        return Err(format!(
            "{label} 会话未进入运行状态：{:?}{}",
            record.state(),
            record
                .failure()
                .map(|failure| format!("（{failure:?}）"))
                .unwrap_or_default()
        ));
    }
    ctx.service
        .note_worker(record.record_id(), &report.report_id);
    let declared_sid = record.declared_sid().map(str::to_owned);
    let worker = json!({
        "name": record.host_name(), "source": source_name(source), "sid": declared_sid,
        "cwd": record.spec().cwd(), "token": declared_sid.clone().unwrap_or_else(|| record.launch_id().to_owned()),
        "title": format!("处理 {}", report.report_id), "kind": "bug-report",
        "report_id": report.report_id,
        "record_id": record.record_id(), "launch_id": record.launch_id(),
        "instance_id": record.instance_id(), "profile": record.spec().adapter_id(), "cols": cols, "rows": rows,
    });
    let launched_at = SystemTime::now();
    update_manifest(
        &report.path,
        json!({"status": "starting", "tmux": record.host_name(), "worker": worker,
            "worker_source": source_name(source),
            "launched_at": crate::audit::query::rfc3339(launched_at)}),
    )
    .map_err(|error| format!("无法更新 manifest.json：{error}"))?;
    ctx.audit(
        "bug_report.worker_started",
        "info",
        &report.report_id,
        json!({"report_id": report.report_id, "tmux": record.host_name(),
            "record_id": record.record_id(), "cwd": record.spec().cwd(), "source": source_name(source)}),
    );
    let task = Injection {
        ctx: ctx.clone(),
        report_id: report.report_id.clone(),
        report_dir: report.path.clone(),
        prompt: report.prompt.clone(),
        record,
        source,
        launched_at,
    };
    tokio::spawn(task.run());
    Ok(worker)
}

struct Injection {
    ctx: WorkerContext,
    report_id: String,
    report_dir: std::path::PathBuf,
    prompt: String,
    record: Record,
    source: Source,
    launched_at: SystemTime,
}

struct Outcome {
    composer_cleared: bool,
    /// The native session whose `user` record carries the prompt.
    confirmed: Option<Confirmed>,
}

struct Confirmed {
    uid: String,
    text_match: &'static str,
}

impl Injection {
    async fn run(self) {
        let name = self.record.host_name().to_owned();
        let label = source_label(self.source);
        let result = if self.ctx.shutdown.is_cancelled() {
            Err("服务正在关闭，未注入缺陷报告".to_owned())
        } else {
            self.inject().await
        };
        let submitted = crate::audit::query::rfc3339(SystemTime::now());
        match result {
            Ok(Outcome {
                composer_cleared,
                confirmed: Some(confirmed),
            }) => {
                let _ = update_manifest(
                    &self.report_dir,
                    json!({"status": "submitted", "submitted_at": submitted, "tmux": name,
                        "composer_cleared": composer_cleared,
                        "confirmed_from": {"uid": confirmed.uid, "method": "native_user_record",
                            "text_match": confirmed.text_match}}),
                );
                self.ctx
                    .service
                    .note_status(self.record.record_id(), "submitted", None);
                self.ctx.audit(
                    "bug_report.worker_submitted",
                    "info",
                    &self.report_id,
                    json!({"report_id": self.report_id, "tmux": name, "uid": confirmed.uid}),
                );
            }
            Ok(Outcome {
                composer_cleared,
                confirmed: None,
            }) => {
                let message = format!("提示词已粘贴到 {label}，但未能确认已提交；请在终端里检查");
                let _ = update_manifest(
                    &self.report_dir,
                    json!({"status": "submitted_unconfirmed", "submitted_at": submitted,
                        "tmux": name, "error": message, "composer_cleared": composer_cleared}),
                );
                self.ctx.service.note_status(
                    self.record.record_id(),
                    "submitted_unconfirmed",
                    Some(&message),
                );
                self.ctx.audit(
                    "bug_report.worker_unconfirmed",
                    "warning",
                    &self.report_id,
                    json!({"report_id": self.report_id, "tmux": name,
                        "composer_cleared": composer_cleared}),
                );
            }
            Err(message) => {
                let _ = update_manifest(
                    &self.report_dir,
                    json!({"status": "failed", "error": message, "tmux": name}),
                );
                self.ctx
                    .service
                    .note_status(self.record.record_id(), "failed", Some(&message));
                self.ctx.audit(
                    "bug_report.worker_failed",
                    "error",
                    &self.report_id,
                    json!({"report_id": self.report_id, "tmux": name, "error": message}),
                );
            }
        }
    }

    /// One durable manifest step: the whole `injection` object is rewritten
    /// so a reader sees every step taken so far.
    fn persist(&self, status: &str, injection: &Map<String, Value>) -> Result<(), String> {
        update_manifest(
            &self.report_dir,
            json!({"status": status, "injection": Value::Object(injection.clone())}),
        )
        .map_err(|error| format!("无法更新 manifest.json：{error}"))?;
        self.ctx
            .service
            .note_status(self.record.record_id(), status, None);
        Ok(())
    }

    async fn inject(&self) -> Result<Outcome, String> {
        let label = source_label(self.source);
        let record_id = self.record.record_id().to_owned();
        let target = self
            .ctx
            .lifecycle
            .target(record_id)
            .await
            .map_err(|error| format!("{label} 实例不可控制：{error}"))?;
        let mut probe = Probe::new(self.source);
        // Readiness without a lease: reading the screen model needs no
        // ownership, so a page that opens the console meanwhile is not
        // locked out for the whole CLI start-up.
        let deadline = Instant::now() + READY_TIMEOUT;
        let mut empty_since: Option<Instant> = None;
        let mut last_state: Option<Draft> = None;
        loop {
            if self.ctx.shutdown.is_cancelled() {
                return Err("服务正在关闭，未注入缺陷报告".into());
            }
            if Instant::now() >= deadline {
                return Err(format!("等待 {label} 输入框就绪超时"));
            }
            let capture = self
                .capture(&target)
                .await
                .map_err(|error| format!("{label} 进程在接收缺陷报告前退出或不可读：{error}"))?;
            let state = probe.state(&capture);
            if last_state != Some(state) {
                last_state = Some(state);
                self.ctx.audit(
                    "bug_report.worker_probe",
                    "info",
                    &self.report_id,
                    json!({"report_id": self.report_id, "tmux": self.record.host_name(),
                        "draft_state": state.wire()}),
                );
            }
            match state {
                Draft::Empty => {
                    let since = *empty_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= SETTLE {
                        break;
                    }
                    tokio::time::sleep(FAST_POLL).await;
                }
                Draft::Editing => {
                    return Err(format!("新建 {label} 会话出现了意外草稿，未覆盖"));
                }
                Draft::Unknown => {
                    empty_since = None;
                    tokio::time::sleep(POLL).await;
                }
            }
        }
        let before = self
            .capture(&target)
            .await
            .map_err(|error| format!("无法读取终端画面：{error}"))?;
        if probe.state(&before) != Draft::Empty {
            return Err(format!("写入前 {label} 编辑器画面已变化，未粘贴"));
        }
        // Step one, persisted before the terminal changes.
        let mut injection = Map::new();
        injection.insert("paste_started_at".into(), json!(now_text()));
        injection.insert("origin".into(), json!(PAGE));
        self.persist("injecting", &injection)?;
        probe.note_paste_frame(&before);
        self.host_input(
            &target,
            ControlOp::Paste {
                text: self.prompt.clone(),
                bracketed: true,
            },
        )
        .await
        .map_err(|error| match error {
            HostInputError::Ambiguous(detail) => format!("粘贴结果不明：{detail}"),
            HostInputError::Failed(detail) => format!("粘贴失败：{detail}"),
        })?;
        let paste_deadline = Instant::now() + PASTE_TIMEOUT;
        let verified = loop {
            tokio::time::sleep(Duration::from_millis(60)).await;
            let capture = self
                .capture(&target)
                .await
                .map_err(|error| format!("无法读取终端画面：{error}"))?;
            if let Some(kind) = probe.paste_visible(&capture, &self.prompt, &self.report_id) {
                break kind;
            }
            if Instant::now() >= paste_deadline {
                return Err(format!("未观察到正文进入 {label} 编辑器；未发送 Enter"));
            }
        };
        injection.insert("pasted_at".into(), json!(now_text()));
        injection.insert("paste_verified".into(), json!(verified));
        self.persist("injecting", &injection)?;
        // Step two: Enter is its own persisted step; an unacknowledged write
        // is recorded as such and never repeated blindly.
        injection.insert("enter_started_at".into(), json!(now_text()));
        self.persist("injecting", &injection)?;
        let acknowledged = match self.enter(&target).await {
            Ok(()) => true,
            Err(HostInputError::Ambiguous(_)) => false,
            Err(HostInputError::Failed(detail)) => {
                return Err(format!("发送 Enter 失败：{detail}"));
            }
        };
        injection.insert("entered_at".into(), json!(now_text()));
        injection.insert("enter_acknowledged".into(), json!(acknowledged));
        self.persist("injecting", &injection)?;
        let composer_cleared = self.confirm_cleared(&target, &mut probe).await?;
        let confirmed = self.confirm_native().await;
        Ok(Outcome {
            composer_cleared,
            confirmed,
        })
    }

    /// Server-originated input through the launch guard: the host validates
    /// the immutable launch/instance identity, no browser lease is involved.
    /// A timeout or transport failure after submission is ambiguous — the
    /// bytes may have reached the PTY — and is never retried blindly.
    async fn host_input(
        &self,
        target: &LaunchTarget,
        operation: ControlOp,
    ) -> Result<(), HostInputError> {
        match tokio::time::timeout(
            HOST_INPUT_TIMEOUT,
            self.ctx.client.request_launch(target, operation),
        )
        .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(ptyhost_client::Error::Timeout)) | Err(_) => {
                Err(HostInputError::Ambiguous("宿主未在限时内确认写入".into()))
            }
            Ok(Err(ptyhost_client::Error::Io(_))) => {
                Err(HostInputError::Ambiguous("宿主连接在写入后中断".into()))
            }
            Ok(Err(error)) => Err(HostInputError::Failed(format!("{error:?}"))),
        }
    }

    async fn capture(&self, target: &LaunchTarget) -> Result<ScreenCapture, String> {
        let reply = self
            .ctx
            .client
            .request_launch(
                target,
                ControlOp::Capture {
                    kind: CaptureKind::Screen,
                    styled: true,
                    join: false,
                    lines: 0,
                },
            )
            .await
            .map_err(|error| format!("{error:?}"))?;
        match reply {
            ControlReply::Capture(capture) => Ok(ScreenCapture {
                text: capture.text,
                cursor: (capture.cursor[0], capture.cursor[1]),
                lag: capture.lag,
                dropped: capture.dropped,
                resets: capture.resets,
                alt: capture.alt,
            }),
            _ => Err("终端 host 未返回屏幕捕获".into()),
        }
    }

    async fn enter(&self, target: &LaunchTarget) -> Result<(), HostInputError> {
        self.host_input(
            target,
            ControlOp::Keys {
                keys: vec!["Enter".into()],
            },
        )
        .await
    }

    /// The composer must be empty again; while
    /// the pasted draft visibly stays after a second, Enter is resent a
    /// bounded number of times. Any other frame is watched, never typed into.
    async fn confirm_cleared(
        &self,
        target: &LaunchTarget,
        probe: &mut Probe,
    ) -> Result<bool, String> {
        for attempt in 1..=CONFIRM_ATTEMPTS {
            let waited = Instant::now() + CONFIRM_WAIT;
            let mut state = Draft::Unknown;
            while Instant::now() < waited {
                tokio::time::sleep(FAST_POLL).await;
                let capture = self
                    .capture(target)
                    .await
                    .map_err(|error| format!("处理会话在提交缺陷报告后不可读：{error}"))?;
                state = probe.after_enter(&capture, &self.prompt, &self.report_id);
                if state == Draft::Empty {
                    return Ok(true);
                }
                if state != Draft::Editing {
                    break;
                }
            }
            if state != Draft::Editing {
                continue;
            }
            self.ctx.audit(
                "bug_report.worker_enter_retry",
                "info",
                &self.report_id,
                json!({"report_id": self.report_id, "tmux": target.name(), "attempt": attempt}),
            );
            if let Err(HostInputError::Failed(detail)) = self.enter(target).await {
                return Err(format!("重发 Enter 失败：{detail}"));
            }
        }
        let capture = self
            .capture(target)
            .await
            .map_err(|error| format!("处理会话在提交缺陷报告后不可读：{error}"))?;
        Ok(probe.after_enter(&capture, &self.prompt, &self.report_id) == Draft::Empty)
    }

    /// The native `user` record that carries the prompt. Claude: the declared
    /// session. Codex/Grok: the newest sessions of the source under the
    /// repository created since the launch, opened one by one.
    async fn confirm_native(&self) -> Option<Confirmed> {
        let deadline = Instant::now() + NATIVE_TIMEOUT;
        // Every poll rescans the roots for the new file (a fresh session is
        // not in any cached index), so it runs once a second at most.
        let interval = Duration::from_secs(1);
        loop {
            if let Some(found) = self.native_once().await {
                return Some(found);
            }
            if Instant::now() >= deadline || self.ctx.shutdown.is_cancelled() {
                return None;
            }
            tokio::time::sleep(interval).await;
        }
    }

    async fn native_once(&self) -> Option<Confirmed> {
        let candidates: Vec<String> = self.candidates().await;
        for uid in candidates {
            let read = {
                let uid = uid.clone();
                self.ctx
                    .reader
                    .run(move |store| store.messages(&uid, &MessageQuery::default()))
                    .await
            };
            let Ok(view) = read else { continue };
            let users: Vec<&str> = view["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|message| message["role"] == "user")
                .filter_map(|message| message["text"].as_str())
                .collect();
            if users.iter().any(|text| text.contains(&self.report_id)) {
                return Some(Confirmed {
                    uid,
                    text_match: "report_id",
                });
            }
            // A declared session is ours alone: its first human input after
            // our Enter can only be the collapsed paste the TUI expanded
            // elsewhere (Claude shows `[Pasted text #N +M lines]`).
            if self.record.declared_sid().is_some()
                && users
                    .first()
                    .is_some_and(|text| text.trim_start().starts_with("[Pasted text"))
            {
                return Some(Confirmed {
                    uid,
                    text_match: "paste_placeholder",
                });
            }
        }
        None
    }

    /// The published rows are the only sid → uid map (a Claude uid is the
    /// index's own identity, not the sid): a declared session is looked up by
    /// its sid; a pending one by source, repository cwd and creation time.
    async fn candidates(&self) -> Vec<String> {
        let source = source_name(self.source).to_owned();
        let repository = self.ctx.service.repository().to_string_lossy().into_owned();
        let since = crate::audit::query::rfc3339(
            self.launched_at
                .checked_sub(CANDIDATE_SLACK)
                .unwrap_or(self.launched_at),
        );
        let listed = self.ctx.reader.run(|store| store.list(true)).await;
        let Ok(document) = listed else {
            return Vec::new();
        };
        if let Some(sid) = self.record.declared_sid() {
            return document["sessions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|row| row["source"] == source.as_str() && row["sid"] == sid)
                .filter_map(|row| row["uid"].as_str().map(str::to_owned))
                .collect();
        }
        let mut rows: Vec<(String, String)> = document["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| row["source"] == source.as_str() && row["cwd"] == repository.as_str())
            .filter_map(|row| {
                let created = row["created"].as_str()?;
                let uid = row["uid"].as_str()?;
                (created >= since.as_str()).then(|| (created.to_owned(), uid.to_owned()))
            })
            .collect();
        rows.sort_by(|left, right| right.0.cmp(&left.0));
        rows.into_iter().map(|(_, uid)| uid).collect()
    }
}

/// Outcome of one server-originated host write.
enum HostInputError {
    /// Submitted but unacknowledged: the bytes may have arrived.
    Ambiguous(String),
    /// Refused before submission (guard mismatch, exited, protocol).
    Failed(String),
}

/// Composer draft: empty, editing, or unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Draft {
    Empty,
    Editing,
    Unknown,
}

impl Draft {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Editing => "editing",
            Self::Unknown => "unknown",
        }
    }
}

/// Composer probe: the delivery driver's composer model for Claude/Codex,
/// Frame-stability probe for Grok.
pub enum Probe {
    Composer {
        kind: ComposerKind,
        pre_paste: Option<String>,
    },
    Screen(ScreenProbe),
}

impl Probe {
    pub fn new(source: Source) -> Self {
        match source {
            Source::Claude => Self::Composer {
                kind: ComposerKind::Claude,
                pre_paste: None,
            },
            Source::Codex => Self::Composer {
                kind: ComposerKind::Codex,
                pre_paste: None,
            },
            Source::Grok => Self::Screen(ScreenProbe::default()),
        }
    }

    /// Readiness state before the paste.
    pub fn state(&mut self, capture: &ScreenCapture) -> Draft {
        match self {
            Self::Composer { kind, .. } => match inspect_for(*kind, capture).state {
                ComposerState::Empty => Draft::Empty,
                ComposerState::Editing => Draft::Editing,
                ComposerState::Unknown => Draft::Unknown,
            },
            Self::Screen(probe) => probe.state(capture),
        }
    }

    pub fn note_paste_frame(&mut self, capture: &ScreenCapture) {
        match self {
            Self::Composer { pre_paste, .. } => *pre_paste = Some(capture.text.clone()),
            Self::Screen(probe) => probe.pre_paste = Some(capture.text.clone()),
        }
    }

    /// Evidence that the paste reached the editor: the composer shows the
    /// exact text (soft wraps ignored) or the TUI's collapsed-paste
    /// placeholder; when the block cannot be located (a screen too small for
    /// the prompt), a changed frame that now carries the report id.
    pub fn paste_visible(
        &mut self,
        capture: &ScreenCapture,
        prompt: &str,
        report_id: &str,
    ) -> Option<&'static str> {
        if capture.lag.is_some_and(|lag| lag > 0) {
            return None;
        }
        match self {
            Self::Composer { kind, pre_paste } => {
                let view = inspect_for(*kind, capture);
                if view.state == ComposerState::Editing
                    && let Some(text) = &view.text
                {
                    if same_text_ignoring_whitespace(text, prompt) {
                        return Some("text");
                    }
                    if paste_placeholder(text) {
                        return Some("placeholder");
                    }
                }
                (pre_paste.as_deref() != Some(capture.text.as_str())
                    && screen_shows(&capture.text, prompt, report_id))
                .then_some("screen")
            }
            Self::Screen(probe) => probe.paste_visible(capture, prompt, report_id),
        }
    }

    /// Post-paste probe: `editing` while the draft is still there,
    /// `empty` once the composer cleared, `unknown` for any other frame.
    pub fn after_enter(&mut self, capture: &ScreenCapture, prompt: &str, report_id: &str) -> Draft {
        if capture.lag.is_some_and(|lag| lag > 0) {
            return Draft::Unknown;
        }
        match self {
            Self::Composer { kind, .. } => {
                let view = inspect_for(*kind, capture);
                match view.state {
                    ComposerState::Empty => Draft::Empty,
                    ComposerState::Editing => {
                        let still = view.text.as_deref().is_some_and(|text| {
                            same_text_ignoring_whitespace(text, prompt)
                                || paste_placeholder(text)
                                || text.contains(report_id)
                        });
                        if still {
                            Draft::Editing
                        } else {
                            Draft::Unknown
                        }
                    }
                    ComposerState::Unknown => Draft::Unknown,
                }
            }
            Self::Screen(probe) => probe.after_enter(capture),
        }
    }
}

/// Claude Code collapses a multi-line paste into `[Pasted text #1 +N lines]`;
/// Codex shows `[Pasted Content N chars]`.
pub fn paste_placeholder(text: &str) -> bool {
    // The styled screen encodes the placeholder's inner spaces as cursor
    // moves (`\x1b[C`), which the ANSI strip removes: compare without
    // whitespace (`[Pastedtext#1+21lines]` on the real Claude 2.1 screen).
    let text: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    text.starts_with("[Pasted") && text.contains(']')
}

/// Paste evidence when the composer block cannot be read as a whole (a
/// screen or composer too small for the prompt): the frame shows the report
/// id or the prompt's last line. An editor scrolled to its end shows the
/// tail; one showing its head shows the id. Whitespace is ignored because
/// the rows may soft-wrap anywhere.
pub fn screen_shows(screen: &str, prompt: &str, report_id: &str) -> bool {
    let plain: String = crate::delivery::driver::strip_ansi(screen)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    if plain.contains(report_id) {
        return true;
    }
    let tail: String = prompt
        .lines()
        .rev()
        .map(|line| {
            line.chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>()
        })
        .find(|line| line.chars().count() >= 8)
        .unwrap_or_default();
    let count = tail.chars().count();
    let tail: String = tail.chars().skip(count.saturating_sub(24)).collect();
    !tail.is_empty() && plain.contains(&tail)
}

/// A non-blank frame that stopped changing is
/// "empty"; after the paste the frame equal to the pasted one is "editing"
/// and any other non-blank frame counts as moved on.
#[derive(Default)]
pub struct ScreenProbe {
    last: Option<String>,
    last_change: Option<Instant>,
    pre_paste: Option<String>,
    pasted_frame: Option<String>,
}

impl ScreenProbe {
    fn touch(&mut self, capture: &ScreenCapture) {
        if self.last.as_deref() != Some(capture.text.as_str()) {
            self.last = Some(capture.text.clone());
            self.last_change = Some(Instant::now());
        }
    }

    pub fn state(&mut self, capture: &ScreenCapture) -> Draft {
        self.touch(capture);
        if capture.lag.is_some_and(|lag| lag > 0) {
            return Draft::Unknown;
        }
        let blank = crate::delivery::driver::strip_ansi(&capture.text)
            .trim()
            .is_empty();
        let settled = self
            .last_change
            .is_some_and(|changed| changed.elapsed() >= SETTLE);
        if blank || !settled {
            Draft::Unknown
        } else {
            Draft::Empty
        }
    }

    pub fn paste_visible(
        &mut self,
        capture: &ScreenCapture,
        prompt: &str,
        report_id: &str,
    ) -> Option<&'static str> {
        self.touch(capture);
        if self.pre_paste.as_deref() != Some(capture.text.as_str())
            && screen_shows(&capture.text, prompt, report_id)
        {
            self.pasted_frame = Some(capture.text.clone());
            Some("screen")
        } else {
            None
        }
    }

    pub fn after_enter(&mut self, capture: &ScreenCapture) -> Draft {
        self.touch(capture);
        if self.pasted_frame.as_deref() == Some(capture.text.as_str()) {
            return Draft::Editing;
        }
        if crate::delivery::driver::strip_ansi(&capture.text)
            .trim()
            .is_empty()
        {
            Draft::Unknown
        } else {
            Draft::Empty
        }
    }
}
