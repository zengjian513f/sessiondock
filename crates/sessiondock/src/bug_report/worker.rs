//! The bug-report worker.
//!
//! The processing instance owns an ordinary server draft and uses the common
//! one-shot conversation sender. SEND success is the submission boundary.
//! Native history is independent and never causes an Enter resend.

use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use ptyhost_client::HostClient;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{BugReportService, Report, source_label, source_name, update_manifest};
use crate::{
    audit::{AuditService, query::ServerEvent},
    delivery::{
        claude::ComposerState,
        driver::{ComposerKind, ScreenCapture, inspect_for, same_text_ignoring_whitespace},
    },
    lifecycle::{
        model::{LaunchSpec, Source, State},
        service::LifecycleService,
    },
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
const POLL: Duration = Duration::from_millis(250);
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
    conversations: Arc<crate::conversation::Conversations>,
    draft_uid: String,
    draft_revision: Option<u64>,
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
    let uid = format!("tmux:{}", record.host_name());
    let identity = conversations
        .identity(&draft_uid)
        .await
        .map_err(|e| e.message)?;
    conversations
        .store
        .link(&format!("launch:{}", record.record_id()), &identity.key)
        .map_err(|e| e.message)?;
    let previous = conversations.store.draft(&identity.key);
    if draft_revision.is_some_and(|revision| revision != previous.revision) {
        return Err("草稿已被另一页面修改；诊断已保存，未发送".into());
    }
    let request_id = format!("report-send:{}", report.report_id);
    let original = std::fs::read_to_string(report.path.join("description.md"))
        .map_err(|e| format!("报告正文读取失败：{e}"))?;
    let mut value = previous.value.clone();
    if !value.is_object() {
        value = json!({"text":original.trim_end(),"attachments":[],"quotes":[]});
    }
    value["session"] = worker.clone();
    value["session"]["uid"] = json!(uid);
    value["report_prompt"] = json!(report.prompt);
    value["requestId"] = json!(request_id);
    value["report_text"] = value["text"].clone();
    value["requestText"] = json!(serde_json::to_string(&json!({
        "text":value["text"],
        "attachments":value["attachments"].as_array().into_iter().flatten()
            .filter(|a| a["uploaded"]["upload_id"].is_string())
            .map(|a| json!({"upload_id":a["uploaded"]["upload_id"],"number":a["number"]})).collect::<Vec<_>>(),
        "quotes":value["quotes"].as_array().cloned().unwrap_or_default()
    })).map_err(|e| e.to_string())?);
    let draft = conversations
        .store
        .save(&identity.key, previous.revision, value)
        .map_err(|e| e.message)?;
    let input = crate::conversation::SendInput {
        uid,
        name: record.host_name().into(),
        request_id,
        text: draft.value["text"].as_str().unwrap_or("").into(),
        draft_revision: Some(draft.revision),
        // Report attachments were already published when freezing diagnostics.
        attachments: draft.value["attachments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["uploaded"]["upload_id"].is_string())
            .map(|a| json!({"upload_id":a["uploaded"]["upload_id"],"number":a["number"]}))
            .collect(),
        quotes: draft.value["quotes"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        lease: None,
        _build: String::new(),
    };
    let report_id = report.report_id.clone();
    let prompt = report.prompt.clone();
    let context = ctx.clone();
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        let result = loop {
            if context.shutdown.is_cancelled() {
                break Err("服务正在关闭，输入保留".to_owned());
            }
            match conversations
                .send_with_prompt(input.clone(), Some(prompt.clone()))
                .await
            {
                Ok(value) => break Ok(value),
                Err(error)
                    if error.code == "cli_starting" && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(POLL).await;
                }
                Err(error) => break Err(error.message),
            }
        };
        if let Err(message) = &result {
            conversations
                .report_startup_failed(&input.uid, &input.request_id, message.clone())
                .await;
        }
        let status = match result {
            Ok(value) => json!({"status":"submitted", "submission":value,
                "injection":{"origin":PAGE,"submitted_at":now_text(),"basis":"SEND"}}),
            Err(message) => json!({"status":"failed", "error":message,
                "injection":{"origin":PAGE,"basis":"SEND","draft_retained":true}}),
        };
        context.audit("bug_report.worker_submission", "info", &report_id, status);
    });
    Ok(worker)
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
            Source::Grok | Source::Shell => Self::Screen(ScreenProbe::default()),
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
