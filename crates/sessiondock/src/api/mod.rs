//! Axum transport router nested at `/api`. Handlers stay in sibling modules;
//! this file only mounts routes, `/meta`/`/nodes`, and the 404 fallback.
//! Unconfigured capabilities fail at the handler; unknown paths are
//! `not_found`. `node_router` mounts the same `/api` tree for the node
//! listener without the static page; its gate is `node_auth`.

mod audit;
mod bug_report;
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
        // Batch 31: Claude reliable send on managed instances; 501 until both
        // the delivery ledger and the terminal transport are configured.
        .route(
            "/session/send",
            post(delivery::send).layer(axum::extract::DefaultBodyLimit::max(4 * 1024 * 1024)),
        )
        .route(
            "/session/draft-status",
            post(delivery::draft_status).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/session/outbox/retry",
            post(delivery::retry).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/session/outbox/discard",
            post(delivery::discard).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route("/watch", get(read::watch))
        .route("/search", get(search::get))
        .route(
            "/session/resolve-files",
            post(files::resolve).layer(axum::extract::DefaultBodyLimit::max(1100 * 1024)),
        )
        .route("/session/file", get(files::file))
        .route("/session/files", get(files::directory))
        // Write side: 501 with `files_jobs_disabled` unless write roots exist.
        .route(
            "/session/files/action",
            post(files::action).layer(axum::extract::DefaultBodyLimit::max(512 * 1024)),
        )
        .route(
            "/session/files/upload",
            post(files::upload).layer(axum::extract::DefaultBodyLimit::max(
                files::UPLOAD_BODY_LIMIT,
            )),
        )
        // Batch 41: `uid=bug-report` is Python's raw upload into the
        // repository; every other uid is the JSON upload completion (8 KiB).
        .route(
            "/session/attachment",
            post(bug_report::attachment).layer(axum::extract::DefaultBodyLimit::max(
                bug_report::ATTACHMENT_BODY_LIMIT,
            )),
        )
        .route(
            "/session/star",
            post(metadata::star).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/sessions/fork-visibility",
            post(metadata::visibility).layer(axum::extract::DefaultBodyLimit::max(384 * 1024)),
        )
        // Read-model display pin only; 501 when no metadata directory is configured.
        .route(
            "/session/rewind",
            post(metadata::rewind).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route("/term/list", get(terminal::list))
        .route(
            "/term/create",
            post(lifecycle::create).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route("/term/new-status", get(lifecycle::status))
        .route(
            "/term/takeover",
            post(lifecycle::takeover).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route("/term/complete-dir", get(lifecycle::complete_dir))
        .route(
            "/term/backend",
            post(lifecycle::backend).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/term/bind",
            post(lifecycle::bind).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        // WP-E: drop a finished pending receipt from the sidebar (Python
        // `pending_store.discard`); never kills or deletes anything.
        .route(
            "/term/discard",
            post(lifecycle::discard).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/term/kill",
            post(lifecycle::cancel).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        // Managed instances only; 501 typed refusal for unmanaged/external CLIs.
        .route(
            "/session/stop",
            post(lifecycle::stop).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/term/claim",
            post(terminal::claim).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route("/term/attach", get(terminal::attach))
        // Raw input carries at most 1 MiB of decoded text; JSON escaping can
        // multiply that, so the body bound is Python's 4 MiB request cap.
        .route(
            "/term/send",
            post(terminal::send).layer(axum::extract::DefaultBodyLimit::max(4 * 1024 * 1024)),
        )
        .route(
            "/term/scroll",
            post(terminal::scroll).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route("/live", get(runtime::live))
        // Reads its own bounded body; 501 with the same code as before when unconfigured.
        .route("/audit/browser", post(audit::browser))
        // Recycle bin: every handler answers 501 unless SESSIONDOCK_TRASH_DIR is set.
        .route("/session/{uid}", delete(trash::delete_session))
        .route(
            "/sessions/delete",
            post(trash::delete_batch).layer(axum::extract::DefaultBodyLimit::max(64 * 1024)),
        )
        .route("/trash", get(trash::list))
        .route(
            "/trash/restore",
            post(trash::restore).layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/trash/purge",
            post(trash::purge).layer(axum::extract::DefaultBodyLimit::max(64 * 1024)),
        )
        // Batch 41: 501 `bug_report_disabled` until the bundle directory,
        // repository, audit, terminal, lifecycle and worker profiles exist.
        .route(
            "/bug-report",
            post(bug_report::report)
                .layer(axum::extract::DefaultBodyLimit::max(bug_report::BODY_LIMIT)),
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

/// Python node mode: `protocol: 1` and the persistent `node_id` once the
/// identity is configured; `0`/`null` otherwise so no hub registers this
/// service by accident.
async fn meta(State(state): State<AppState>) -> Json<Value> {
    let (protocol, node_id) = match &state.node {
        Some(node) => (crate::hub::PROTOCOL, json!(node.node_id)),
        None => (0, Value::Null),
    };
    // Batch 44 WP-A: no `migration` disclaimer — this is the replacement
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
