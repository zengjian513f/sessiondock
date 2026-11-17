//! Public mutation admission and private Hub-owned migration fences.
use super::*;
use axum::{body::Body, extract::Request, middleware::Next};
use std::collections::BTreeSet;

#[derive(Deserialize)]
pub struct Freeze {
    operation_id: String,
    #[serde(default)]
    uids: Option<BTreeSet<String>>,
    #[serde(default)]
    release: bool,
    #[serde(default)]
    uid: Option<String>,
    #[serde(default)]
    target_node: Option<String>,
}

pub async fn frozen(State(state): State<AppState>) -> Json<Value> {
    Json(json!({"uids":state.transfer.as_ref().map(|s| s.frozen_uids()).unwrap_or_default()}))
}

pub async fn freeze(State(state): State<AppState>, Json(body): Json<Freeze>) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let id = &body.operation_id;
    if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return failure(TransferError::new("move_plan_stale", "迁移标识无效"));
    }
    let preview = if body.uids.is_none() && !body.release {
        match service.progress_status(id) {
            Ok(value) => Some(value),
            Err(e) => return failure(e),
        }
    } else {
        None
    };
    if let Some(preview) = &preview
        && (body.uid.as_ref().is_some_and(|uid| preview["uid"] != *uid)
            || body.target_node.as_ref().is_some_and(|node| {
                preview["selection"] == true && preview["target_node"] != *node
            }))
    {
        return failure(TransferError::new(
            "move_plan_stale",
            "操作与已确认的迁移选择不符",
        ));
    }
    let uids = body.uids.unwrap_or_else(|| {
        preview
            .as_ref()
            .and_then(|p| p["sessions"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|s| s["uid"].as_str().map(str::to_owned))
            .collect()
    });
    let guard = service.mutation_guard(&uids).await;
    let result = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        service.freeze(&body.operation_id, uids, body.release)
    })
    .await;
    match result {
        Ok(Ok(())) => {
            state.ui_events.publish_sessions();
            Json(preview.unwrap_or(json!({"ok":true}))).into_response()
        }
        Ok(Err(e)) => failure(e),
        Err(e) => failure(TransferError::new("move_io", e.to_string())),
    }
}

// Reads and migration coordination must stay available while a session is
// frozen. All other JSON commands carrying session identities are mutations.
pub(crate) fn readonly(path: &str) -> bool {
    path.starts_with("/api/session/transfer/")
        || matches!(
            path,
            "/api/session/clone"
                | "/api/session/clone/progress"
                | "/api/session/clone/cancel"
                | "/api/sessions/resolve"
                | "/api/sessions/unread"
                | "/api/session/resolve-files"
                | "/api/session/conversation/check"
                | "/api/session/conversation/complete-path"
                | "/api/resources/probe"
                | "/api/session/resources/probe"
                | "/api/resources/compact"
                | "/api/audit/browser"
                | "/api/bug-report"
                | "/api/bug-report/capture"
                | "/api/term/check-dirs"
        )
}

pub(crate) fn identities(value: &Value, uids: &mut BTreeSet<String>) {
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            if matches!(
                key.as_str(),
                "uid"
                    | "resume_uid"
                    | "parent_uid"
                    | "source_uid"
                    | "target_uid"
                    | "predecessor_uid"
                    | "continuation_uid"
            ) {
                if let Some(uid) = value.as_str() {
                    uids.insert(uid.into());
                }
            } else if matches!(key.as_str(), "uids" | "selected_uids") {
                uids.extend(
                    value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned),
                );
            } else if matches!(key.as_str(), "sessions" | "targets") {
                for item in value.as_array().into_iter().flatten() {
                    identities(item, uids);
                }
            }
        }
    }
}

pub async fn middleware(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(service) = &state.transfer else {
        return next.run(request).await;
    };
    if request.method() == axum::http::Method::GET || readonly(request.uri().path()) {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    let mut uids = BTreeSet::new();
    if let Ok(axum::extract::Query(query)) =
        axum::extract::Query::<std::collections::BTreeMap<String, String>>::try_from_uri(&parts.uri)
    {
        identities(&json!(query), &mut uids);
    }
    let body = if parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
    {
        use futures_util::StreamExt;
        let limit = super::super::request_body_limit(parts.uri.path());
        let mut stream = body.into_data_stream();
        let mut raw = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    return super::super::ApiError::new(
                        StatusCode::BAD_REQUEST,
                        "invalid_body",
                        error.to_string(),
                    )
                    .into_response();
                }
            };
            if chunk.len() > limit.saturating_sub(raw.len()) {
                return super::super::ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "body_too_large",
                    "请求体超过大小限制",
                )
                .into_response();
            }
            raw.extend_from_slice(&chunk);
        }
        if let Ok(value) = serde_json::from_slice::<Value>(&raw) {
            identities(&value, &mut uids);
        }
        Body::from(raw)
    } else {
        body
    };
    // Check before and after admission: do not queue a rejected launch until
    // the migration finishes, and do not race a newly accepted freeze.
    let blocked = || !service.frozen_uids().is_disjoint(&uids);
    if blocked() {
        return failure(TransferError::new(
            "move_session_locked",
            "会话正在迁移，已冻结修改操作",
        ));
    }
    let guard = service.mutation_guard(&uids).await;
    if blocked() {
        return failure(TransferError::new(
            "move_session_locked",
            "会话正在迁移，已冻结修改操作",
        ));
    }
    // Keep admission alive even when a browser disconnects mid-mutation.
    match tokio::spawn(async move {
        let _guard = guard;
        next.run(Request::from_parts(parts, body)).await
    })
    .await
    {
        Ok(response) => response,
        Err(e) => failure(TransferError::new("move_io", e.to_string())),
    }
}
