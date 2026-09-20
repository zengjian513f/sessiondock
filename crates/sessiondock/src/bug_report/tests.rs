//! Semantics for the bundle, the
//! attachments and the composer probes. The worker's
//! HTTP flow against a fake CLI is `tests/bug_report_http.rs`.

use std::{fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};

use serde_json::{Value, json};

use super::*;
use crate::{
    audit::{AuditService, Limits},
    delivery::driver::ScreenCapture,
};

fn private(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

struct Fixture {
    _temp: tempfile::TempDir,
    reports: std::path::PathBuf,
    repo: std::path::PathBuf,
    audit_dir: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let reports = root.join("reports");
        let repo = root.join("repo");
        let audit_dir = root.join("audit");
        private(&reports);
        private(&audit_dir);
        fs::create_dir_all(&repo).unwrap();
        Self {
            _temp: temp,
            reports,
            repo,
            audit_dir,
        }
    }
    fn service(&self) -> BugReportService {
        BugReportService::open(
            self.reports.clone(),
            self.repo.clone(),
            self.audit_dir.clone(),
            "rs-test-build".into(),
        )
        .unwrap()
    }
    fn audit(&self) -> (AuditService, tokio_util::sync::CancellationToken) {
        let shutdown = tokio_util::sync::CancellationToken::new();
        let service =
            AuditService::open(self.audit_dir.clone(), Limits::default(), shutdown.clone())
                .unwrap();
        (service, shutdown)
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// Wait for the audit writer thread to land a browser batch on disk.
fn browser_batch(audit: &AuditService, page: &str, uid: &str, event: &str) {
    let body = json!({"page_id": page, "uid": uid, "events": [{"event": event, "ts": "t"}]});
    let admission = audit.admit().unwrap();
    audit
        .submit(
            admission,
            "127.0.0.1".parse().unwrap(),
            body.to_string().as_bytes(),
        )
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while audit.snapshot().written_events < 1 {
        assert!(std::time::Instant::now() < deadline, "audit writer stalled");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[tokio::test]
async fn bundle_filters_related_events_and_captures_context() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let (audit, shutdown) = fixture.audit();
    browser_batch(&audit, "page-1", "codex:one", "dom.snapshot");
    browser_batch(&audit, "page-2", "claude:other", "unrelated");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while audit.snapshot().written_events < 2 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let report = service
        .create(
            &audit,
            CreateInput {
                description: "消息发送后消失".into(),
                uid: "codex:one".into(),
                page_id: "page-1".into(),
                trace_id: "trace-1".into(),
                build: "build".into(),
                hostname: "host".into(),
                client_ip: "192.0.2.1".into(),
                snapshot: json!({"data": {"selected": "codex:one"}, "content": {"composer": "x"}}),
                terminal_capture: "terminal frame".into(),
                session: json!({"path": "/tmp/session.jsonl"}),
                outbox: json!({"outbox": [{"state": "confirming", "secret": "s3cret"}]}),
                attachments: Vec::new(),
                origin: Origin::default(),
                remote_events: Vec::new(),
            },
        )
        .unwrap();
    let directory = report.path.clone();
    assert!(report_id_ok(&report.report_id), "{}", report.report_id);
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let manifest = read_json(&directory.join("manifest.json"));
    assert_eq!(manifest["uid"], "codex:one");
    assert_eq!(manifest["status"], "captured");
    assert_eq!(
        manifest["event_count"], 2,
        "snapshot + report.created: {manifest}"
    );
    assert_eq!(manifest["event_window_seconds"], 900);
    assert_eq!(manifest["terminal_file"], "terminal.txt");
    assert_eq!(manifest["attachments"], json!([]));
    assert_eq!(manifest["repository"], fixture.repo.to_str().unwrap());
    // Secrets are redacted in every bundle document.
    assert_eq!(manifest["outbox"]["outbox"][0]["secret"], "<redacted>");
    assert_eq!(manifest["outbox"]["outbox"][0]["state"], "confirming");
    let rows: Vec<Value> = fs::read_to_string(directory.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let names: std::collections::BTreeSet<&str> = rows
        .iter()
        .map(|row| row["event"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["browser.dom.snapshot", "bug_report.created"]
            .into_iter()
            .collect()
    );
    assert_eq!(
        fs::read_to_string(directory.join("terminal.txt")).unwrap(),
        "terminal frame"
    );
    assert_eq!(
        fs::read_to_string(directory.join("description.md")).unwrap(),
        "消息发送后消失\n"
    );
    let state = read_json(&directory.join("browser-state.json"));
    assert_eq!(state["content"]["composer"], "x");
    let environment = read_json(&directory.join("environment.json"));
    assert_eq!(environment["build"], "rs-test-build");
    assert_eq!(environment["git_head"]["argv"][0], "git");
    // Without an origin the bundle names this machine as the problem's.
    assert_eq!(manifest["origin"]["hostname"], "host");
    assert_eq!(manifest["origin"]["node_name"], "");
    assert_eq!(manifest["origin"]["uid"], "codex:one");
    assert_eq!(manifest["origin"]["remote"], false);
    assert_eq!(manifest["origin"]["capture_error"], Value::Null);
    let prompt = fs::read_to_string(directory.join("worker-prompt.md")).unwrap();
    assert!(prompt.contains(&report.report_id));
    assert!(prompt.contains("问题机器：host\n"));
    assert!(!prompt.contains("另一台机器"));
    assert!(prompt.contains("相关会话：codex:one"));
    assert!(prompt.contains("随后立即 push"));
    assert!(prompt.contains("python3 deploy/deploy.py deploy --all"));
    assert!(prompt.contains("无需再次确认"));
    assert!(!prompt.contains("不要 push"));
    assert!(!prompt.contains("不要部署"));
    assert!(!prompt.contains("同步到中央 Hub"));
    assert!(!prompt.split("诊断包").next().unwrap().contains("附件"));
    // Every bundle file is private.
    for name in ["manifest.json", "events.jsonl", "worker-prompt.md"] {
        assert_eq!(
            fs::metadata(directory.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "{name}"
        );
    }
    shutdown.cancel();
    audit.shutdown().await;
}

/// A worker started away from the problem's machine gets that machine's
/// capture (rows first in `events.jsonl`), and the prompt and manifest name
/// the machine the bundle describes.
#[tokio::test]
async fn bundle_from_a_remote_capture_names_the_origin_machine() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let (audit, shutdown) = fixture.audit();
    let report = service
        .create(
            &audit,
            CreateInput {
                description: "Lyra 上的会话卡住".into(),
                uid: String::new(),
                page_id: "page-9".into(),
                hostname: "cygnus".into(),
                session: json!({"uid": "codex:n1~one", "cwd": "/srv/x"}),
                terminal_capture: "frame from lyra".into(),
                origin: Origin {
                    node_id: "n1".into(),
                    node_name: "Lyra".into(),
                    hostname: "lyra".into(),
                    uid: "codex:n1~one".into(),
                    remote: true,
                    capture_error: String::new(),
                },
                remote_events: vec![
                    json!({"event": "browser.click", "uid": "codex:n1~one", "seq": 7}),
                    json!({"event": "http.request.started", "uid": "codex:n1~one", "seq": 8}),
                ],
                ..Default::default()
            },
        )
        .unwrap();
    let manifest = read_json(&report.path.join("manifest.json"));
    assert_eq!(manifest["hostname"], "cygnus");
    assert_eq!(
        manifest["origin"],
        json!({"node_id": "n1", "node_name": "Lyra", "hostname": "lyra",
            "uid": "codex:n1~one", "remote": true, "capture_error": null})
    );
    assert_eq!(manifest["session"]["cwd"], "/srv/x");
    assert_eq!(manifest["event_count"], 3, "{manifest}");
    let rows: Vec<Value> = fs::read_to_string(report.path.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["event"], "browser.click");
    assert_eq!(rows[1]["event"], "http.request.started");
    assert_eq!(rows[2]["event"], "bug_report.created");
    assert_eq!(
        fs::read_to_string(report.path.join("terminal.txt")).unwrap(),
        "frame from lyra"
    );
    let prompt = fs::read_to_string(report.path.join("worker-prompt.md")).unwrap();
    assert!(prompt.contains("问题机器：Lyra（主机 lyra）\n"), "{prompt}");
    assert!(prompt.contains("相关会话：codex:n1~one\n"), "{prompt}");
    assert!(
        prompt.contains(
            "问题发生在 Lyra（主机 lyra），而这条处理会话运行在另一台机器（本机 cygnus）"
        ),
        "{prompt}"
    );
    assert!(!prompt.contains("抓取服务端上下文失败"));
    shutdown.cancel();
    audit.shutdown().await;
}

/// The problem machine being unreachable does not lose the report; the
/// prompt says what is missing from the bundle.
#[tokio::test]
async fn bundle_records_a_failed_remote_capture() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let (audit, shutdown) = fixture.audit();
    let report = service
        .create(
            &audit,
            CreateInput {
                description: "页面卡死".into(),
                hostname: "cygnus".into(),
                origin: Origin {
                    node_id: "n1".into(),
                    node_name: "Lyra".into(),
                    hostname: String::new(),
                    uid: "claude:n1~two".into(),
                    remote: true,
                    capture_error: "Lyra 离线：中央站未能连接该机器".into(),
                },
                ..Default::default()
            },
        )
        .unwrap();
    let manifest = read_json(&report.path.join("manifest.json"));
    assert_eq!(manifest["origin"]["node_name"], "Lyra");
    assert_eq!(
        manifest["origin"]["capture_error"],
        "Lyra 离线：中央站未能连接该机器"
    );
    assert_eq!(manifest["terminal_file"], "");
    let prompt = fs::read_to_string(report.path.join("worker-prompt.md")).unwrap();
    assert!(prompt.contains("问题机器：Lyra\n"), "{prompt}");
    assert!(
        prompt.contains("从 Lyra 抓取服务端上下文失败：Lyra 离线"),
        "{prompt}"
    );
    assert!(prompt.contains("相关会话：claude:n1~two\n"), "{prompt}");
    shutdown.cancel();
    audit.shutdown().await;
}

#[tokio::test]
async fn bundle_keeps_attachments_and_lists_them_like_the_composer() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let (audit, shutdown) = fixture.audit();
    let uploads = fixture.repo.join(ATTACHMENT_DIR).join("7");
    fs::create_dir_all(&uploads).unwrap();
    let shot = uploads.join("屏幕截图.png");
    fs::write(&shot, [b"\x89PNG".as_slice(), &[0u8; 16]].concat()).unwrap();
    let attachments = service
        .resolve_attachments(&json!([{"path": shot, "number": 3, "name": "屏幕截图.png",
            "kind": "image", "mime": "image/png"}]))
        .unwrap();
    assert_eq!(
        attachments[0].relative_path,
        format!("{ATTACHMENT_DIR}/7/屏幕截图.png")
    );
    assert_eq!(attachments[0].size, 20);
    assert_eq!(attachments[0].number, 3);
    let report = service
        .create(
            &audit,
            CreateInput {
                description: "点了按钮没反应，见 [附件3]".into(),
                uid: "codex:one".into(),
                attachments: attachments.clone(),
                ..Default::default()
            },
        )
        .unwrap();
    let manifest = read_json(&report.path.join("manifest.json"));
    let bundled = report.path.join("attachments").join("03-屏幕截图.png");
    assert_eq!(fs::read(&bundled).unwrap(), fs::read(&shot).unwrap());
    assert_eq!(
        manifest["attachments"][0]["bundle_file"],
        "attachments/03-屏幕截图.png"
    );
    assert_eq!(manifest["attachments"][0]["path"], shot.to_str().unwrap());
    let prompt = fs::read_to_string(report.path.join("worker-prompt.md")).unwrap();
    assert!(prompt.contains(&format!(
        "点了按钮没反应，见 [附件3]\n\n附件3: ./{ATTACHMENT_DIR}/7/屏幕截图.png"
    )));
    assert!(prompt.contains("上传了 1 个附件"));
    assert_eq!(
        fs::read_to_string(report.path.join("description.md")).unwrap(),
        "点了按钮没反应，见 [附件3]\n"
    );
    shutdown.cancel();
    audit.shutdown().await;
}

#[test]
fn resolve_attachments_rejects_paths_outside_the_upload_directory() {
    let fixture = Fixture::new();
    let service = fixture.service();
    fs::create_dir_all(fixture.repo.join(ATTACHMENT_DIR)).unwrap();
    let outside = fixture.repo.parent().unwrap().join("secret.txt");
    fs::write(&outside, "no").unwrap();
    assert_eq!(service.resolve_attachments(&Value::Null).unwrap(), vec![]);
    assert_eq!(service.resolve_attachments(&json!([])).unwrap(), vec![]);
    assert_eq!(service.resolve_attachments(&json!("")).unwrap(), vec![]);
    let outside_error = service
        .resolve_attachments(&json!([{"path": outside}]))
        .unwrap_err();
    assert!(outside_error.contains("不在附件目录中"), "{outside_error}");
    let gone = service
        .resolve_attachments(
            &json!([{"path": fixture.repo.join(ATTACHMENT_DIR).join("1").join("gone.png")}]),
        )
        .unwrap_err();
    assert!(gone.contains("不在附件目录中"), "{gone}");
    assert!(
        service
            .resolve_attachments(&json!([{"name": "x"}]))
            .unwrap_err()
            .contains("缺少路径")
    );
    assert!(
        service
            .resolve_attachments(&json!("x"))
            .unwrap_err()
            .contains("格式无效")
    );
    assert!(
        service
            .resolve_attachments(&json!([{"path": "x"}]))
            .unwrap_err()
            .contains("不在附件目录中")
    );
    let too_many: Vec<Value> = (0..=ATTACHMENT_MAX_COUNT)
        .map(|_| json!({"path": "x"}))
        .collect();
    assert!(
        service
            .resolve_attachments(&Value::Array(too_many))
            .unwrap_err()
            .contains("最多附带")
    );
    let link = fixture.repo.join(ATTACHMENT_DIR).join("link.txt");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let linked = service
        .resolve_attachments(&json!([{"path": link}]))
        .unwrap_err();
    assert!(linked.contains("不在附件目录中"), "{linked}");
    let target = fixture.repo.join(ATTACHMENT_DIR).join("inside.txt");
    fs::write(&target, "ok").unwrap();
    let internal_link = fixture.repo.join(ATTACHMENT_DIR).join("inside-link.txt");
    std::os::unix::fs::symlink(&target, &internal_link).unwrap();
    let internal = service
        .resolve_attachments(&json!([{"path": internal_link}]))
        .unwrap();
    assert_eq!(internal[0].path, target.canonicalize().unwrap());
    // A directory instead of a file.
    let directory = fixture.repo.join(ATTACHMENT_DIR).join("2");
    fs::create_dir_all(&directory).unwrap();
    assert!(
        service
            .resolve_attachments(&json!([{"path": directory}]))
            .unwrap_err()
            .contains("不是文件")
    );
    // Missing upload directory: "尚未上传".
    let other = Fixture::new();
    let missing = other.service();
    assert!(
        missing
            .resolve_attachments(&json!([{"path": "x"}]))
            .unwrap_err()
            .contains("尚未上传")
    );
}

fn capture(text: &str, cursor: (u16, u16)) -> ScreenCapture {
    ScreenCapture {
        text: text.into(),
        cursor,
        lag: Some(0),
        dropped: Some(0),
        resets: Some(0),
        alt: false,
    }
}

#[test]
fn claude_probe_sees_the_placeholder_and_the_cleared_composer() {
    use worker::{Draft, Probe};
    let rule = "─".repeat(40);
    let empty = capture(&format!("welcome\n{rule}\n❯ \n{rule}\n"), (2, 2));
    let mut probe = Probe::new(Source::Claude);
    assert_eq!(probe.state(&empty), Draft::Empty);
    probe.note_paste_frame(&empty);
    let placeholder = capture(
        &format!("welcome\n{rule}\n❯ \x1b[0m[Pasted text #1 +24 lines]\x1b[0m\n{rule}\n"),
        (30, 2),
    );
    assert_eq!(
        probe.paste_visible(
            &placeholder,
            "处理 sessiondock 缺陷报告 BUG-1\n...",
            "BUG-1"
        ),
        Some("placeholder")
    );
    assert_eq!(
        probe.after_enter(
            &placeholder,
            "处理 sessiondock 缺陷报告 BUG-1\n...",
            "BUG-1"
        ),
        Draft::Editing
    );
    // The real Claude 2.1 styled screen encodes the placeholder's spaces as
    // cursor-forward moves and the prompt gap as NBSP (acceptance
    // capture); the ANSI strip must not hide the placeholder.
    let real = capture(
        &format!(
            "welcome\n{rule}\n❯\u{a0}[Pasted\x1b[Ctext\x1b[C#1\x1b[C+21\x1b[Clines]\n\x1b[38;2;136;136;136m{rule}\n"
        ),
        (28, 2),
    );
    assert_eq!(
        probe.paste_visible(&real, "处理 sessiondock 缺陷报告 BUG-1\n...", "BUG-1"),
        Some("placeholder")
    );
    assert!(worker::paste_placeholder("[Pastedtext#1+21lines]"));
    assert!(!worker::paste_placeholder("[Pasting"));
    let cleared = capture(
        &format!("welcome\n{rule}\n❯ \n{rule}\n✻ Thinking… (esc to interrupt)\n"),
        (2, 2),
    );
    assert_eq!(probe.after_enter(&cleared, "x", "BUG-1"), Draft::Empty);
    // A screen too small for the block: the changed frame carrying the report
    // id is accepted as paste evidence, and a frame without it is not.
    let mut probe = Probe::new(Source::Claude);
    probe.note_paste_frame(&empty);
    assert_eq!(
        probe.paste_visible(
            &capture("处理 sessiondock 缺陷报告 BUG-1\n用户描述", (0, 0)),
            "x",
            "BUG-1"
        ),
        Some("screen")
    );
    assert_eq!(
        probe.paste_visible(&capture("something else", (0, 0)), "x", "BUG-1"),
        None
    );
    // An editor scrolled to its end shows the prompt's last line instead.
    let prompt = "处理 sessiondock 缺陷报告 BUG-1\n\n5. 完成后在会话中说明根因、修改的文件、验证结果和仍存风险。\n";
    assert_eq!(
        probe.paste_visible(
            &capture(
                "…\n  5. 完成后在会话中说明根因、修改的文件、\n验证结果和仍存风险。",
                (0, 0)
            ),
            prompt,
            "BUG-1"
        ),
        Some("screen")
    );
    // A lagging host is never a known composer.
    let mut lagging = capture(&format!("welcome\n{rule}\n❯ \n{rule}\n"), (2, 2));
    lagging.lag = Some(3);
    assert_eq!(Probe::new(Source::Claude).state(&lagging), Draft::Unknown);
}

#[test]
fn grok_probe_uses_screen_stability_like_python() {
    use worker::{Draft, Probe};
    let mut probe = Probe::new(Source::Grok);
    let frame = capture("grok> _", (0, 0));
    assert_eq!(probe.state(&frame), Draft::Unknown, "not settled yet");
    assert_eq!(
        probe.state(&capture("", (0, 0))),
        Draft::Unknown,
        "blank frame"
    );
    std::thread::sleep(worker::SETTLE + Duration::from_millis(50));
    assert_eq!(
        probe.state(&capture("", (0, 0))),
        Draft::Unknown,
        "blank frame stays unknown"
    );
    let mut probe = Probe::new(Source::Grok);
    probe.state(&frame);
    std::thread::sleep(worker::SETTLE + Duration::from_millis(50));
    assert_eq!(probe.state(&frame), Draft::Empty);
    probe.note_paste_frame(&frame);
    let pasted = capture("grok> 处理 sessiondock 缺陷报告 BUG-grok", (0, 0));
    assert_eq!(
        probe.paste_visible(&pasted, "x", "BUG-grok"),
        Some("screen")
    );
    assert_eq!(probe.after_enter(&pasted, "x", "BUG-grok"), Draft::Editing);
    assert_eq!(
        probe.after_enter(&capture("thinking…", (0, 0)), "x", "BUG-grok"),
        Draft::Empty
    );
    assert_eq!(
        probe.after_enter(&capture("", (0, 0)), "x", "BUG-grok"),
        Draft::Unknown
    );
}

#[test]
fn manifest_updates_merge_and_redact() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    fs::write(
        dir.join("manifest.json"),
        "{\"status\":\"starting\",\"keep\":1}\n",
    )
    .unwrap();
    update_manifest(
        dir,
        json!({"status": "submitted", "cookie": "abc", "worker": {"token": "sid-1", "api_key": "k"}}),
    )
    .unwrap();
    let manifest = read_json(&dir.join("manifest.json"));
    assert_eq!(manifest["status"], "submitted");
    assert_eq!(manifest["keep"], 1);
    assert_eq!(manifest["cookie"], "<redacted>");
    // The list: `token` (the worker's launch identity) is kept, keys are not.
    assert_eq!(manifest["worker"]["token"], "sid-1");
    assert_eq!(manifest["worker"]["api_key"], "<redacted>");
    // Corrupt manifest: rewritten from the changes alone.
    fs::write(dir.join("manifest.json"), "not json").unwrap();
    update_manifest(dir, json!({"status": "failed"})).unwrap();
    assert_eq!(read_json(&dir.join("manifest.json"))["status"], "failed");
    assert!(report_id_ok("BUG-20260912-213000-0a1b2c"));
    assert!(!report_id_ok("BUG-2026091-213000-0a1b2c"));
    assert!(!report_id_ok("BUG-20260912-213000-0A1B2C"));
    assert!(!report_id_ok("../BUG-20260912-213000-0a1b2c"));
}

#[test]
fn service_remembers_worker_decorations_from_manifests() {
    let fixture = Fixture::new();
    let bundle = fixture.reports.join("BUG-20260912-000000-abcdef");
    private(&bundle);
    fs::write(
        bundle.join("manifest.json"),
        json!({"status": "submitted", "worker": {"record_id": "r".repeat(32)}}).to_string(),
    )
    .unwrap();
    let service = fixture.service();
    let decoration = service.pending_decoration(&"r".repeat(32)).unwrap();
    assert_eq!(decoration["kind"], "bug-report");
    assert_eq!(decoration["report_id"], "BUG-20260912-000000-abcdef");
    assert_eq!(decoration["title"], "处理 BUG-20260912-000000-abcdef");
    assert!(service.pending_decoration("other").is_none());
}

#[test]
fn attachment_names_are_kept_readable_but_never_paths() {
    use crate::files::WriteService;
    assert_eq!(
        WriteService::attachment_name("屏幕截图.png"),
        "屏幕截图.png"
    );
    assert_eq!(WriteService::attachment_name("../../etc/passwd"), "passwd");
    assert_eq!(
        WriteService::attachment_name("dir\\evil\x00name.txt"),
        "evil_name.txt"
    );
    assert_eq!(WriteService::attachment_name("  . "), "attachment");
    assert_eq!(WriteService::attachment_name(".."), "attachment");
    assert_eq!(WriteService::attachment_name("a  b   c.png"), "a b c.png");
    let long = format!("{}.png", "x".repeat(400));
    let cleaned = WriteService::attachment_name(&long);
    assert!(cleaned.ends_with(".png") && cleaned.len() <= 154);
}
