//! `POST /api/bug-report`, `POST /api/bug-report/capture` and the
//! `uid=bug-report` branch of `POST /api/session/attachment` (the raw
//! upload special case). Validation, status codes and the 202/500 shapes
//! apply; the bundle and the worker live in `bug_report`.
//!
//! A report may run its worker on a machine other than the one the problem
//! was seen on. The browser then asks the problem's machine for
//! `/api/bug-report/capture` (its session row, delivery ledger, terminal
//! frame and audit window) and hands the answer to the worker's machine as
//! `captured`, together with `origin` naming the problem's machine; that
//! machine writes the bundle without a local capture of its own.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::{
    Extension, Json,
    body::to_bytes,
    extract::{ConnectInfo, FromRequest, Request, State, rejection::JsonRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use ptyhost_client::{CaptureKind, ControlOp, ControlReply};
use serde_json::{Value, json};

use crate::{
    audit::query::QueryFilter,
    bug_report::{
        CreateInput, DEFAULT_SOURCE, EVENT_WINDOW_SECONDS, MAX_DESCRIPTION_CHARS, Origin,
        UPLOAD_UID, parse_source, source_label, source_name, update_manifest, worker,
    },
    error::ApiError,
    files::WriteService,
    lifecycle::model::State as LaunchState,
    state::AppState,
};

/// Raw upload bodies on `/session/attachment?uid=...`.
pub const ATTACHMENT_BODY_LIMIT: usize = WriteService::BUG_REPORT_ATTACHMENT_MAX_BYTES + 1;
/// `POST /api/bug-report` may carry another machine's capture (audit window
/// plus an 8000-row terminal frame), so it takes more than the 4 MiB of the
/// other JSON routes.
pub const REPORT_BODY_LIMIT: usize = 16 * 1024 * 1024;
/// Newest audit rows a remote capture hands over; the local window keeps
/// the `MAX_QUERY_ROWS` bound.
pub const REMOTE_EVENT_ROWS: usize = 20_000;

fn disabled() -> ApiError {
    ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "bug_report_disabled",
        "缺陷报告未启用：需要 SESSIONDOCK_BUG_REPORT_DIR/REPO、审计目录、终端传输和受控创建",
    )
}

fn terminal_off(message: &'static str) -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "terminal_disabled", message)
}

fn text(value: &Value, max: usize) -> String {
    match value {
        Value::String(text) => text.chars().take(max).collect(),
        Value::Null => String::new(),
        Value::Bool(false) => String::new(),
        other => other.to_string().chars().take(max).collect(),
    }
}

/// `max(lo, min(int(value or default), hi))`; a non-numeric value is
/// a 400.
fn dimension(value: &Value, default: u16, low: u16, high: u16) -> Result<u16, ApiError> {
    let number = match value {
        Value::Null => default,
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value as i64))
            .filter(|value| *value >= 0 && *value <= i64::from(u16::MAX))
            .ok_or_else(|| invalid("cols/rows 必须是整数"))?
            as u16,
        Value::String(text) if text.trim().is_empty() => default,
        Value::String(text) => text
            .trim()
            .parse::<u16>()
            .map_err(|_| invalid("cols/rows 必须是整数"))?,
        Value::Bool(false) => default,
        _ => return Err(invalid("cols/rows 必须是整数")),
    };
    let number = if number == 0 { default } else { number };
    Ok(number.clamp(low, high))
}

fn invalid(message: &str) -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "invalid_bug_report",
        message.to_owned(),
    )
}

fn json_body(status: StatusCode, body: Value) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

pub async fn report(
    state: State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    tokio::spawn(report_inner(state, peer, body))
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "bug_report_failed",
                "报告任务异常退出，输入保留",
            )
        })?
}

async fn report_inner(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    if state.terminal.is_none() {
        return Err(terminal_off("终端未启用，无法启动处理会话"));
    }
    let ctx = state.bug_report.clone().ok_or_else(disabled)?;
    let Json(mut body) = body.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "缺陷报告请求体过大",
            )
        } else {
            ApiError::new(StatusCode::BAD_REQUEST, "bad_body", "bad body")
        }
    })?;
    if !body.is_object() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "bad_body",
            "bad body",
        ));
    }
    let source_text = {
        let raw = text(&body["source"], usize::MAX);
        if raw.is_empty() {
            source_name(DEFAULT_SOURCE).to_owned()
        } else {
            raw
        }
    };
    let Some(source) = parse_source(&source_text) else {
        return Ok(json_body(
            StatusCode::BAD_REQUEST,
            json!({"error": format!("不支持的处理会话类型: {source_text}"), "code": "bug_report_source"}),
        ));
    };
    // The source needs its one
    // configured CLI, the same the picker starts.
    if ctx.lifecycle.entry_for(source, false).is_none() {
        return Ok(json_body(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": format!("本机找不到 {} 命令", source_name(source)), "code": "bug_report_source_unavailable"}),
        ));
    }
    let conversations = state.conversations.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "conversation_disabled",
            "服务端会话草稿未配置，无法转交报告",
        )
    })?;
    let draft_uid = match body["draft_uid"]
        .as_str()
        .filter(|s| s.starts_with("report:") && s.len() > 7)
    {
        Some(uid) => uid.to_owned(),
        None => format!(
            "report:{}",
            crate::conversation::random_id().map_err(|e| ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                e.code,
                e.message
            ))?
        ),
    };
    let request_id = body["request_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| draft_uid.clone());
    let identity = conversations.identity(&draft_uid).await.map_err(|e| {
        ApiError::new(
            StatusCode::from_u16(e.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
            e.code,
            e.message,
        )
    })?;
    let report_lock = conversations.upload_lock(&identity.key, "report-submission");
    let _report_guard = report_lock.lock().await;
    let payload = json!({"description":body["description"],"source":source_text,
        "attachments":body["attachments"],"origin":body["origin"],"uid":body["uid"]});
    if let Some(old) = conversations.store.report(&identity.key, &request_id) {
        if old["payload"] != payload {
            return Err(invalid("相同报告提交 ID 对应了不同内容"));
        }
        if old["result"].is_object() {
            return Ok(json_body(
                StatusCode::from_u16(old["status"].as_u64().unwrap_or(202) as u16)
                    .unwrap_or(StatusCode::ACCEPTED),
                old["result"].clone(),
            ));
        }
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "report_result_unknown",
            "此报告已开始保存，请核对诊断和处理会话；原输入保留，不会重复创建报告",
        ));
    }
    if body["draft_revision"]
        .as_u64()
        .is_some_and(|r| r != conversations.store.draft(&identity.key).revision)
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "draft_revision",
            "另一页面已更新报告草稿，未发布附件和创建诊断",
        ));
    }
    let private = body["attachments"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v["upload_id"].is_string()));
    if private {
        let mut destination = identity.clone();
        destination.cwd = ctx.service.repository().into();
        let published = conversations
            .publish(&destination, body["attachments"].as_array().unwrap())
            .await
            .map_err(|e| {
                ApiError::new(
                    StatusCode::from_u16(e.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                    e.code,
                    e.message,
                )
            })?;
        body["attachments"] = json!(published);
    }
    let cols = dimension(&body["cols"], 120, 40, 300)?;
    let rows = dimension(&body["rows"], 36, 12, 120)?;
    let description = text(&body["description"], MAX_DESCRIPTION_CHARS + 1);
    let attachments = match ctx.service.resolve_attachments(&body["attachments"]) {
        Ok(attachments) => attachments,
        Err(message) => {
            return Ok(json_body(
                StatusCode::BAD_REQUEST,
                json!({"error": message, "code": "bug_report_attachments"}),
            ));
        }
    };
    let uid = text(&body["uid"], 512);
    let snapshot = if body["snapshot"].is_object() {
        body["snapshot"].clone()
    } else {
        json!({})
    };
    let terminal_name = text(&body["terminal_name"], 256);
    // `origin` names the machine the problem was seen on; `captured` is that
    // machine's `/api/bug-report/capture` answer when it is not this one.
    let mut origin = Origin {
        node_id: text(&body["origin"]["node_id"], 64),
        node_name: text(&body["origin"]["node_name"], 128),
        hostname: state.hostname.to_string(),
        uid: text(&body["origin"]["uid"], 512),
        remote: false,
        capture_error: String::new(),
    };
    if origin.uid.is_empty() {
        origin.uid = uid.clone();
    }
    let (context, remote_events) = if body["captured"].is_object() {
        origin.remote = true;
        let captured = &body["captured"];
        origin.capture_error = text(&captured["error"], 2000);
        let hostname = text(&captured["hostname"], 256);
        if !hostname.is_empty() {
            origin.hostname = hostname;
        } else if origin.capture_error.is_empty() {
            // A capture answer without a host is not one this route produced.
            return Err(invalid("captured.hostname 缺失"));
        } else {
            origin.hostname = String::new();
        }
        let events = captured["events"]
            .as_array()
            .map(|rows| {
                let skip = rows.len().saturating_sub(REMOTE_EVENT_ROWS);
                rows.iter().skip(skip).cloned().collect::<Vec<_>>()
            })
            .unwrap_or_default();
        (
            Context {
                session: captured["session"].clone(),
                outbox: captured["outbox"].clone(),
                terminal_capture: text(&captured["terminal_capture"], usize::MAX),
            },
            events,
        )
    } else {
        (
            local_context(&state, &ctx, &uid, &terminal_name).await,
            Vec::new(),
        )
    };
    let Context {
        session,
        outbox,
        terminal_capture,
    } = context;
    let page_id = {
        let page = text(&body["page_id"], 128);
        if page.is_empty() {
            text(&body["_page_id"], 128)
        } else {
            page
        }
    };
    let input = CreateInput {
        description,
        uid: uid.clone(),
        page_id,
        trace_id: text(&body["_trace_id"], 128),
        build: text(&body["_build"], 128),
        hostname: state.hostname.to_string(),
        client_ip: peer
            .map(|Extension(ConnectInfo(address))| address.ip())
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
            .to_string(),
        snapshot,
        terminal_capture,
        session,
        outbox,
        attachments,
        origin,
        remote_events,
    };
    conversations
        .store
        .note_report(
            &identity.key,
            &request_id,
            json!({"payload":payload,"phase":"capturing"}),
        )
        .map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, e.code, e.message))?;
    let audit = ctx.audit.clone();
    let service = ctx.service.clone();
    let created = tokio::task::spawn_blocking(move || service.create(&audit, input))
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "bug_report_failed",
                "报告捕获任务异常退出",
            )
        })?;
    let report = match created {
        Ok(report) => report,
        Err(error) => {
            return Ok(match error.report {
                None => json_body(
                    StatusCode::BAD_REQUEST,
                    json!({"error": error.message, "code": "bug_report_invalid"}),
                ),
                Some((report_id, path)) => {
                    let _ =
                        update_manifest(&path, json!({"status": "failed", "error": error.message}));
                    json_body(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({"error": error.message, "code": "bug_report_capture_failed",
                            "report_id": report_id, "path": path}),
                    )
                }
            });
        }
    };
    let response_body = match worker::launch(
        &ctx,
        &report,
        source,
        cols,
        rows,
        conversations.clone(),
        draft_uid,
        body["draft_revision"].as_u64(),
    )
    .await
    {
        Ok(worker) => (
            StatusCode::ACCEPTED,
            json!({"ok": true, "report_id": report.report_id, "path": report.path,
                "worker": worker}),
        ),
        Err(error) => {
            let message = format!(
                "诊断已保存，但 {} 会话启动失败：{error}",
                source_label(source)
            );
            let _ = update_manifest(&report.path, json!({"status": "failed", "error": message}));
            ctx.audit.record(crate::audit::query::ServerEvent {
                event: "bug_report.worker_launch_failed",
                category: "bug-report",
                severity: "error",
                uid: &uid,
                trace_id: &report.report_id,
                page_id: "",
                build: "",
                data: json!({"report_id": report.report_id, "error": error}),
            });
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"error": message, "code": "bug_report_worker_failed",
                    "report_id": report.report_id, "path": report.path}),
            )
        }
    };
    conversations
        .store
        .note_report(
            &identity.key,
            &request_id,
            json!({"payload":payload,
        "phase":"complete","status":response_body.0.as_u16(),"result":response_body.1}),
        )
        .map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, e.code, e.message))?;
    Ok(json_body(response_body.0, response_body.1))
}

/// The server-side context of a report: the session's list row, its
/// delivery ledger and the managed terminal's frame, all read on this
/// machine.
struct Context {
    session: Value,
    outbox: Value,
    terminal_capture: String,
}

async fn local_context(
    state: &AppState,
    ctx: &worker::WorkerContext,
    uid: &str,
    terminal_name: &str,
) -> Context {
    let session = if !uid.is_empty() && !uid.starts_with("tmux:") {
        session_row(state, uid).await
    } else {
        json!({})
    };
    let terminal_capture = if terminal_name.is_empty() {
        String::new()
    } else {
        terminal_capture(state, ctx, terminal_name).await
    };
    let outbox = if uid.is_empty() {
        json!({})
    } else {
        outbox_snapshot(state, uid).await
    };
    Context {
        session,
        outbox,
        terminal_capture,
    }
}

/// `POST /api/bug-report/capture`: the server-side context of a report on
/// this machine, for a worker that starts elsewhere. Body `{uid,
/// terminal_name, page_id|_page_id, _trace_id}`; answer `{ok, hostname,
/// captured_at, session, outbox, terminal_capture, events}` where `events`
/// is the same 900 s audit window `create` would have bundled here (newest
/// `REMOTE_EVENT_ROWS` rows). Gated exactly like the report route.
pub async fn capture(
    State(state): State<AppState>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    if state.terminal.is_none() {
        return Err(terminal_off("终端未启用，无法抓取会话上下文"));
    }
    let ctx = state.bug_report.clone().ok_or_else(disabled)?;
    let Json(body) =
        body.map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "bad_body", "bad body"))?;
    if !body.is_object() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "bad_body",
            "bad body",
        ));
    }
    let uid = text(&body["uid"], 512);
    let terminal_name = text(&body["terminal_name"], 256);
    let page_id = {
        let page = text(&body["page_id"], 128);
        if page.is_empty() {
            text(&body["_page_id"], 128)
        } else {
            page
        }
    };
    let trace_id = text(&body["_trace_id"], 128);
    let context = local_context(&state, &ctx, &uid, &terminal_name).await;
    let now = std::time::SystemTime::now();
    let audit_dir = ctx.service.audit_dir().to_path_buf();
    let filter = QueryFilter {
        uid: uid.clone(),
        page_id,
        trace_id,
        report_id: String::new(),
    };
    let events = tokio::task::spawn_blocking(move || {
        let since = now
            .checked_sub(std::time::Duration::from_secs(EVENT_WINDOW_SECONDS))
            .unwrap_or(std::time::UNIX_EPOCH);
        let until = now + std::time::Duration::from_secs(5);
        let rows = crate::audit::query::query(
            &audit_dir,
            since,
            until,
            &filter,
            crate::audit::query::MAX_QUERY_ROWS,
        );
        let skip = rows.len().saturating_sub(REMOTE_EVENT_ROWS);
        rows.into_iter().skip(skip).collect::<Vec<_>>()
    })
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "bug_report_failed",
            "审计查询任务异常退出",
        )
    })?;
    Ok(json_body(
        StatusCode::OK,
        json!({
            "ok": true, "hostname": state.hostname.to_string(),
            "captured_at": crate::audit::query::rfc3339(now),
            "uid": uid, "terminal_name": terminal_name,
            "session": context.session, "outbox": context.outbox,
            "terminal_capture": context.terminal_capture, "events": events,
        }),
    ))
}

/// The published list row, `{}` when absent.
async fn session_row(state: &AppState, uid: &str) -> Value {
    let wanted = uid.to_owned();
    state
        .reader
        .run(move |store| {
            let document = store.list(false)?;
            Ok(document["sessions"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|row| row["uid"] == wanted.as_str())
                .cloned()
                .unwrap_or_else(|| json!({})))
        })
        .await
        .unwrap_or_else(|_| json!({}))
}

/// The delivery ledger's view
/// of the session, `{}` when there is no ledger or the session is unknown.
async fn outbox_snapshot(state: &AppState, uid: &str) -> Value {
    let Some(delivery) = &state.delivery else {
        return json!({});
    };
    let wanted = uid.to_owned();
    let Ok(scope) = state
        .reader
        .run(move |store| store.native_scope(&wanted, ""))
        .await
    else {
        return json!({});
    };
    let encoded = match scope.source.as_str() {
        "codex" => delivery.codex_outbox(scope.uid, scope.agent_id).await,
        "claude" => {
            delivery
                .claude_outbox(crate::delivery::claude::Scope {
                    uid: scope.uid,
                    session_id: scope.session_id,
                    agent_id: scope.agent_id,
                })
                .await
        }
        _ => return json!({}),
    };
    match encoded {
        Ok(encoded) => serde_json::from_slice(encoded.as_bytes()).unwrap_or_else(|_| json!({})),
        Err(_) => json!({}),
    }
}

/// Capture history (`name`, 8000) with the screen as fallback:
/// only a managed instance (native-associated or pending) is readable, through
/// its guard envelope; anything else yields no capture.
async fn terminal_capture(state: &AppState, ctx: &worker::WorkerContext, name: &str) -> String {
    let scrollback = || ControlOp::Capture {
        kind: CaptureKind::Scrollback,
        styled: false,
        join: false,
        lines: 8000,
    };
    let screen = || ControlOp::Capture {
        kind: CaptureKind::Screen,
        styled: false,
        join: false,
        lines: 0,
    };
    let extract = |reply: Result<ControlReply, ptyhost_client::Error>| match reply {
        Ok(ControlReply::Capture(capture)) => Some(capture.text),
        _ => None,
    };
    if let Ok(Some(observed)) = super::runtime::observe(state).await
        && let Some(target) = observed
            .hosts
            .iter()
            .filter(|host| host.summary.name == name)
            .find_map(|host| host.bound_target())
    {
        if let Some(text) = extract(ctx.client.request_bound(target, scrollback()).await) {
            return text;
        }
        return extract(ctx.client.request_bound(target, screen()).await).unwrap_or_default();
    }
    let Ok(records) = ctx.lifecycle.list(0, 128).await else {
        return String::new();
    };
    let Some(record) = records.iter().find(|record| {
        record.host_name() == name
            && record.state() == LaunchState::Running
            && !record.cancel_requested()
    }) else {
        return String::new();
    };
    let Ok(target) = ctx.lifecycle.target(record.record_id().to_owned()).await else {
        return String::new();
    };
    if let Some(text) = extract(ctx.client.request_launch(&target, scrollback()).await) {
        return text;
    }
    extract(ctx.client.request_launch(&target, screen()).await).unwrap_or_default()
}

/// `POST /api/session/attachment`: the raw upload for `uid=bug-report`
/// (query `name`, optional `id`, the file as the body) writes into the
/// repository's attachment directory through the write service; every other
/// native uid uses the conversation upload route of `api/files.rs`.
/// Requests without a query uid retain the JSON upload-completion contract.
pub async fn attachment(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    let query: std::collections::HashMap<String, String> =
        request.uri().query().map(url_form).unwrap_or_default();
    if query.get("uid").map(String::as_str) != Some(UPLOAD_UID) {
        if query.contains_key("uid") {
            return super::files::upload_attachment(State(state), request).await;
        }
        let body = Json::<super::files::AttachmentRequest>::from_request(request, &state).await;
        return super::files::attachment(State(state), body).await;
    }
    if state.terminal.is_none() {
        return Err(terminal_off("终端未启用，无法上传报告附件"));
    }
    let ctx = state.bug_report.clone().ok_or_else(disabled)?;
    let writer = state.files_write.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "files_jobs_disabled",
            "文件写入服务未启用",
        )
    })?;
    let name = query
        .get("name")
        .cloned()
        .unwrap_or_else(|| "attachment".into());
    let requested_id = query.get("id").cloned().filter(|id| !id.trim().is_empty());
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > WriteService::BUG_REPORT_ATTACHMENT_MAX_BYTES) {
        return Ok(json_body(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({"error": format!("单个附件不能超过 {} MB", WriteService::BUG_REPORT_ATTACHMENT_MAX_BYTES / (1024 * 1024)),
                "code": "file_upload_too_large"}),
        ));
    }
    let bytes = match to_bytes(
        request.into_body(),
        WriteService::BUG_REPORT_ATTACHMENT_MAX_BYTES,
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(_) => {
            return Ok(json_body(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({"error": format!("单个附件不能超过 {} MB", WriteService::BUG_REPORT_ATTACHMENT_MAX_BYTES / (1024 * 1024)),
                    "code": "file_upload_too_large"}),
            ));
        }
    };
    let repository = ctx.service.repository().to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        writer.bug_report_upload(
            &repository,
            requested_id.as_deref().map(str::trim),
            &name,
            &content_type,
            &bytes,
        )
    })
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "file_worker_failed",
            "附件写入任务异常退出",
        )
    })?;
    Ok(match result {
        Ok(document) => json_body(StatusCode::OK, document),
        Err(error) => {
            let mut body = json!({"error": error.message, "code": error.code});
            if let (Some(target), Some(details)) = (body.as_object_mut(), error.details.as_object())
            {
                for (key, value) in details {
                    target.insert(key.clone(), value.clone());
                }
            }
            json_body(
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                body,
            )
        }
    })
}

/// Minimal `application/x-www-form-urlencoded` query decoding (first value wins).
fn url_form(query: &str) -> std::collections::HashMap<String, String> {
    fn decode(text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'+' => out.push(b' '),
                b'%' if index + 2 < bytes.len() => {
                    let hex = &text[index + 1..index + 3];
                    match u8::from_str_radix(hex, 16) {
                        Ok(byte) => {
                            out.push(byte);
                            index += 2;
                        }
                        Err(_) => out.push(b'%'),
                    }
                }
                byte => out.push(byte),
            }
            index += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    let mut map = std::collections::HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        map.entry(decode(key)).or_insert_with(|| decode(value));
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_clamp_like_python() {
        assert_eq!(dimension(&json!(100), 120, 40, 300).unwrap(), 100);
        assert_eq!(dimension(&json!(10), 120, 40, 300).unwrap(), 40);
        assert_eq!(dimension(&json!(999), 120, 40, 300).unwrap(), 300);
        assert_eq!(dimension(&Value::Null, 36, 12, 120).unwrap(), 36);
        assert_eq!(dimension(&json!(0), 36, 12, 120).unwrap(), 36);
        assert_eq!(dimension(&json!("50"), 36, 12, 120).unwrap(), 50);
        assert!(dimension(&json!("wide"), 36, 12, 120).is_err());
    }

    #[test]
    fn query_decoding_handles_percent_and_plus() {
        let map = url_form("uid=bug-report&name=%E5%B1%8F%E5%B9%95+shot.png&id=7");
        assert_eq!(map["uid"], "bug-report");
        assert_eq!(map["name"], "屏幕 shot.png");
        assert_eq!(map["id"], "7");
    }
}
