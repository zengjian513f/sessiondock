//! Axum transport router nested at `/api`. Handlers stay in sibling modules;
//! this file only mounts routes, `/meta`/`/nodes`, and the 404 fallback.
//! Unconfigured capabilities fail at the handler; unknown paths are
//! `not_found`. `node_router` mounts the same `/api` tree for the node
//! listener without the static page; its gate is `node_auth`.

mod audit;
mod bug_report;
mod conversation;
mod delivery;
mod files;
mod health;
pub mod hub;
mod lifecycle;
mod media;
mod metadata;
pub(crate) mod node_auth;
mod read;
pub(crate) mod runtime;
mod search;
mod terminal;
mod trash;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{delete, get, post},
};
use serde_json::{Value, json};

use crate::{error::ApiError, state::AppState};

/// Match each handler's request-body policy, including raw uploads.
pub(crate) fn request_body_limit(path: &str) -> usize {
    match path {
        "/api/session/files/upload" => files::UPLOAD_BODY_LIMIT,
        "/api/session/files/action" => crate::files::DEFAULT_UPLOAD_CHUNK_BYTES,
        "/api/session/attachment" => bug_report::ATTACHMENT_BODY_LIMIT,
        "/api/bug-report" => bug_report::REPORT_BODY_LIMIT,
        "/api/session/star"
        | "/api/sessions/fork-visibility"
        | "/api/audit/browser"
        | "/api/bug-report/capture"
        | "/api/trash/restore"
        | "/api/trash/purge"
        | "/api/sessions/delete"
        | "/api/session/resolve-files" => 4 * 1024 * 1024,
        _ => usize::MAX,
    }
}

pub fn router() -> Router<AppState> {
    let router = Router::new()
        .route("/health", get(health::get_health))
        .route("/meta", get(meta))
        .route("/nodes", get(nodes))
        .route("/sessions", get(read::list))
        .route("/messages/{uid}", get(read::messages))
        .route("/messages/{uid}/page", get(read::history_page))
        .route("/messages/{uid}/media-page", get(read::media_page))
        .route("/media/{token}", get(media::get))
        .route("/session/input-history", get(read::input_history))
        .route("/session/outbox", get(delivery::outbox))
        .route(
            "/session/conversation",
            get(conversation::get).post(conversation::save).layer(
                axum::extract::DefaultBodyLimit::max(request_body_limit(
                    "/api/session/conversation",
                )),
            ),
        )
        .route("/session/conversation/restart", post(conversation::restart))
        .route(
            "/session/conversation/send",
            post(conversation::send).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/session/conversation/send"),
            )),
        )
        .route("/session/conversation/check", post(conversation::check))
        .route(
            "/session/conversation/attachment",
            post(conversation::upload).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route(
            "/session/conversation/attachment/discard",
            post(conversation::discard),
        )
        .route(
            "/session/conversation/import",
            post(conversation::import).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/session/conversation/import"),
            )),
        )
        .route("/session/conversation/drafts", get(conversation::drafts))
        // Claude reliable send on managed instances; 501 until both
        // the delivery ledger and the terminal transport are configured.
        .route(
            "/session/send",
            post(delivery::send).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/session/send",
            ))),
        )
        .route(
            "/session/draft-status",
            post(delivery::draft_status).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/session/draft-status"),
            )),
        )
        .route(
            "/session/outbox/retry",
            post(delivery::retry).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/session/outbox/retry",
            ))),
        )
        .route(
            "/session/outbox/discard",
            post(delivery::discard).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/session/outbox/discard"),
            )),
        )
        .route("/watch", get(read::watch))
        .route("/search", get(search::get))
        .route(
            "/session/resolve-files",
            post(files::resolve).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/session/resolve-files",
            ))),
        )
        .route("/session/file", get(files::file))
        .route("/session/files", get(files::directory))
        // Write routes require the file-writing service.
        .route(
            "/session/files/action",
            post(files::action).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/session/files/action",
            ))),
        )
        .route(
            "/session/files/upload",
            post(files::upload).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/session/files/upload",
            ))),
        )
        // Raw attachments target a native session cwd or the bug-report repo.
        // Without a query uid, retain JSON upload completion.
        .route(
            "/session/attachment",
            post(bug_report::attachment).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/session/attachment"),
            )),
        )
        .route(
            "/session/star",
            post(metadata::star).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/session/star",
            ))),
        )
        .route(
            "/sessions/fork-visibility",
            post(metadata::visibility).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/sessions/fork-visibility"),
            )),
        )
        // Read-model display pin only; 501 when no metadata directory is configured.
        .route(
            "/session/rewind",
            post(metadata::rewind).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/session/rewind",
            ))),
        )
        .route("/term/list", get(terminal::list))
        .route(
            "/term/create",
            post(lifecycle::create).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/term/create"),
            )),
        )
        .route("/term/new-status", get(lifecycle::status))
        .route(
            "/term/takeover",
            post(lifecycle::takeover).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/term/takeover"),
            )),
        )
        .route("/term/complete-dir", get(lifecycle::complete_dir))
        .route(
            "/term/backend",
            post(lifecycle::backend).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/term/backend"),
            )),
        )
        .route(
            "/term/bind",
            post(lifecycle::bind).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/term/bind",
            ))),
        )
        // Drop a finished pending receipt from the sidebar;
        // never kills or deletes anything.
        .route(
            "/term/discard",
            post(lifecycle::discard).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/term/discard"),
            )),
        )
        .route(
            "/term/kill",
            post(lifecycle::cancel).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/term/kill"),
            )),
        )
        // Stop the selected managed or precisely observed native CLI.
        .route(
            "/session/stop",
            post(lifecycle::stop).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/session/stop",
            ))),
        )
        .route(
            "/term/claim",
            post(terminal::claim).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/term/claim",
            ))),
        )
        .route("/term/attach", get(terminal::attach))
        // The host applies its decoded-input protocol limit after JSON parsing.
        .route(
            "/term/send",
            post(terminal::send).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/term/send",
            ))),
        )
        .route(
            "/term/scroll",
            post(terminal::scroll).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/term/scroll",
            ))),
        )
        .route("/live", get(runtime::live))
        // Reads its own bounded body; 501 with the same code as before when unconfigured.
        .route(
            "/audit/browser",
            post(audit::browser).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/audit/browser",
            ))),
        )
        // Recycle bin: every handler answers 501 unless SESSIONDOCK_TRASH_DIR is set.
        .route("/session/{uid}", delete(trash::delete_session))
        .route(
            "/sessions/delete",
            post(trash::delete_batch).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/sessions/delete"),
            )),
        )
        .route("/trash", get(trash::list))
        .route(
            "/trash/restore",
            post(trash::restore).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/trash/restore",
            ))),
        )
        .route(
            "/trash/purge",
            post(trash::purge).layer(axum::extract::DefaultBodyLimit::max(request_body_limit(
                "/api/trash/purge",
            ))),
        )
        // 501 `bug_report_disabled` until the bundle directory,
        // repository, audit, terminal, lifecycle and worker profiles exist.
        .route(
            "/bug-report",
            post(bug_report::report).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/bug-report"),
            )),
        )
        // The problem machine's side of a report whose worker starts elsewhere.
        .route(
            "/bug-report/capture",
            post(bug_report::capture).layer(axum::extract::DefaultBodyLimit::max(
                request_body_limit("/api/bug-report/capture"),
            )),
        );
    router.fallback(not_found)
}

/// The node listener's tree: `/api` only, so the hub can proxy every session,
/// message, media, file, terminal, SSE and WS route; the static page is 404.
pub fn node_router() -> Router<AppState> {
    Router::new()
        .nest("/api", router())
        .fallback(node_not_found)
}

/// Node mode: `protocol: 1` and the persistent `node_id` once the
/// identity is configured; `0`/`null` otherwise so no hub registers this
/// service by accident.
async fn meta(State(state): State<AppState>) -> Json<Value> {
    let (protocol, node_id) = match &state.node {
        Some(node) => (crate::hub::PROTOCOL, json!(node.node_id)),
        None => (0, Value::Null),
    };
    // No `migration` disclaimer — this is the replacement
    // service, and `capabilities.stage` says so.
    Json(json!({
        "build": state.assets.build, "hostname": &*state.hostname, "mode": "local",
        "protocol": protocol, "node_id": node_id, "capabilities": &*state.capabilities,
    }))
}

async fn nodes(State(state): State<AppState>) -> Json<Value> {
    match &state.node {
        Some(node) => Json(json!({"mode": "local", "nodes": [{"id": node.node_id,
            "name": &*state.hostname, "online": true}]})),
        None => Json(
            json!({"mode": "local", "nodes": [{"id": "rs-local", "name": &*state.hostname,
            "online": true}], "federation": false}),
        ),
    }
}

async fn node_not_found() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "node listener serves /api only",
    )
}

async fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "API route not found")
}
